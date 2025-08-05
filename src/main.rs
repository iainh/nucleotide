#![recursion_limit = "512"]

use std::time::Duration;
use std::panic;

use anyhow::{Context, Result};
use helix_core::diagnostic::Severity;
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;

use gpui::{
    App, AppContext, Menu, MenuItem, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, px,
};

pub use application::Input;
use application::{Application, InputEvent};

mod actions;
mod application;
mod core;
mod completion;
mod config;
mod document;
mod error_boundary;
mod info_box;
mod key_hint_view;
mod line_cache;
mod lsp_status;
mod notification;
mod overlay;
mod picker;
mod picker_delegate;
mod picker_element;
mod picker_view;
mod preview_tracker;
mod prompt;
mod prompt_view;
mod statusline;
mod theme_manager;
mod ui;
mod utils;
mod workspace;

pub type Core = Application;

fn setup_logging(verbosity: u64) -> Result<()> {
    let mut base_config = fern::Dispatch::new();

    base_config = match verbosity {
        0 => base_config.level(log::LevelFilter::Warn),
        1 => base_config.level(log::LevelFilter::Info),
        2 => base_config.level(log::LevelFilter::Debug),
        _3_or_more => base_config.level(log::LevelFilter::Trace),
    };

    // Separate file config so we can include year, month and day in file logs
    let file_config = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {} [{}] {}",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"),
                record.target(),
                record.level(),
                message
            ))
        })
        .chain(std::io::stdout())
        .chain(fern::log_file(helix_loader::log_file())?);

    base_config.chain(file_config).apply()?;

    Ok(())
}

fn install_panic_handler() {
    panic::set_hook(Box::new(|info| {
        log::error!("Application panic: {info}");
        
        // Log backtrace if enabled
        if let Ok(backtrace) = std::env::var("RUST_BACKTRACE") {
            if backtrace == "1" || backtrace == "full" {
                eprintln!("Backtrace:\n{:?}", std::backtrace::Backtrace::capture());
            }
        }
        
        // Try to save any unsaved work would go here if we had access to the app state
        // For now, just log and exit gracefully
        eprintln!("Fatal error: {info}");
        
        // Exit gracefully
        std::process::exit(1);
    }));
}

fn main() -> Result<()> {
    // Install panic handler to prevent data loss
    install_panic_handler();
    
    let rt = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Failed to initialize Tokio runtime: {e}");
            return Err(anyhow::anyhow!("Runtime initialization failed: {}", e));
        }
    };
    let handle = rt.handle();
    let _guard = handle.enter();
    let (app, config) = match init_editor() {
        Ok(Some((app, config))) => (app, config),
        Ok(None) => {
            eprintln!("Editor initialization returned None");
            return Err(anyhow::anyhow!("Editor initialization failed"));
        }
        Err(e) => {
            eprintln!("Failed to initialize editor: {e}");
            return Err(e);
        }
    };
    drop(_guard);
    gui_main(app, config, handle.clone());
    Ok(())
}

fn window_options(_cx: &mut App) -> gpui::WindowOptions {
    WindowOptions {
        app_id: Some("helix-gpui".to_string()),
        titlebar: Some(TitlebarOptions {
            title: Some("Helix".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
            origin: gpui::point(px(100.0), px(100.0)),
            size: gpui::size(px(1200.0), px(800.0)),
        })),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        display_id: None,
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(gpui::WindowDecorations::Server),
        window_min_size: Some(gpui::size(px(400.0), px(300.0))),
    }
}

// Import actions from our centralized definitions
use crate::actions::{
    completion::*, editor::*, help::*, picker::*, test::*, window::*,
};

fn app_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Helix".into(),
            items: vec![
                MenuItem::action("About", About),
                MenuItem::separator(),
                // MenuItem::action("Settings", OpenSettings),
                // MenuItem::separator(),
                MenuItem::action("Hide Helix", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open...", OpenFile),
                // MenuItem::action("Open Directory", OpenDirectory),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Tutorial", OpenTutorial),
                MenuItem::action("Test Prompt", TestPrompt),
                MenuItem::action("Test Completion", TestCompletion),
            ],
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}

#[derive(Debug)]
pub enum Update {
    Redraw,
    Prompt(prompt::Prompt),
    Picker(picker::Picker),
    Completion(gpui::Entity<completion::CompletionView>),
    Info(helix_view::info::Info),
    EditorEvent(helix_view::editor::EditorEvent),
    EditorStatus(EditorStatus),
    OpenFile(std::path::PathBuf),
    ShouldQuit,
    CommandSubmitted(String),
}

impl gpui::EventEmitter<Update> for Application {}

struct FontSettings {
    fixed_font: gpui::Font,
    var_font: gpui::Font,
}

impl gpui::Global for FontSettings {}

#[derive(Clone)]
pub struct EditorFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: gpui::FontWeight,
}

impl gpui::Global for EditorFontConfig {}

#[derive(Clone)]
pub struct UiFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: gpui::FontWeight,
}

impl gpui::Global for UiFontConfig {}

fn gui_main(mut app: Application, config: crate::config::Config, handle: tokio::runtime::Handle) {
    gpui::Application::new().run(move |cx| {
        // Set up theme manager with Helix theme
        let helix_theme = app.editor.theme.clone();
        let theme_manager = theme_manager::ThemeManager::new(helix_theme);
        let ui_theme = theme_manager.ui_theme().clone();
        cx.set_global(theme_manager);
        cx.set_global(ui_theme);
        
        // Set up fonts from configuration
        let editor_font_config = config.editor_font();
        let ui_font_config = config.ui_font();
        
        let font_settings = FontSettings {
            fixed_font: gpui::font(&editor_font_config.family),
            var_font: gpui::font(&ui_font_config.family),
        };
        cx.set_global(font_settings);
        
        // Store editor font config for document views
        cx.set_global(EditorFontConfig {
            family: editor_font_config.family,
            size: editor_font_config.size,
            weight: editor_font_config.weight.into(),
        });
        
        // Store UI font config for UI components
        cx.set_global(UiFontConfig {
            family: ui_font_config.family,
            size: ui_font_config.size,
            weight: ui_font_config.weight.into(),
        });
        
        // Initialize preview tracker
        cx.set_global(crate::preview_tracker::PreviewTracker::new());
        
        let options = window_options(cx);

        let _ = cx.open_window(options, |_window, cx| {
            let input = cx.new(|_| crate::application::Input);
            let crank = cx.new(|mc| {
                mc.spawn(async move |crank, cx| {
                    loop {
                        // Add error handling around the timer operation
                        match cx.background_executor()
                            .timer(Duration::from_millis(200)) // 5fps instead of 20fps
                            .await {
                            _ => {
                                // Timer completed, emit update event
                                if let Err(e) = crank.update(cx, |_crank, cx| {
                                    cx.emit(());
                                }) {
                                    log::warn!("Failed to emit crank event: {e:?}");
                                    // Continue the loop even if update fails
                                }
                            }
                        }
                    }
                })
                .detach();
                crate::application::Crank
            });
            let crank_1 = crank.clone();
            std::mem::forget(crank_1);

            let input_1 = input.clone();
            let handle_1 = handle.clone();
            // Create LSP state entity
            let lsp_state = cx.new(|_| crate::core::lsp_state::LspState::new());
            
            // Create a separate timer for LSP spinner updates
            struct SpinnerTimer;
            impl gpui::EventEmitter<()> for SpinnerTimer {}
            
            let lsp_state_clone = lsp_state.clone();
            let _spinner_timer = cx.new(|mc| {
                mc.spawn(async move |_timer, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(80)) // Match helix spinner interval
                            .await;
                        
                        if let Err(e) = cx.update(|cx| {
                            lsp_state_clone.update(cx, |state, cx| {
                                // Only update spinner if there's LSP activity
                                println!("[SPINNER] Timer tick");
                                if state.should_show_spinner() {
                                    let frame = state.get_spinner_frame();
                                    state.status_message = Some(frame.to_string());
                                } else {
                                    // Clear status message when no activity
                                    state.status_message = None;
                                }
                                // Always notify to ensure UI updates
                                cx.notify();
                            });
                        }) {
                            log::warn!("Failed to update LSP spinner: {e:?}");
                        }
                    }
                })
                .detach();
                SpinnerTimer
            });
            
            let app = cx.new(move |mc| {
                let handle_1 = handle_1.clone();
                let handle_2 = handle_1.clone();
                mc.subscribe(
                    &input_1.clone(),
                    move |this: &mut Application, _, ev, cx| {
                        this.handle_input_event(ev.clone(), cx, handle_1.clone());
                    },
                )
                .detach();
                mc.subscribe(&crank, move |this: &mut Application, _, ev, cx| {
                    *ev;
                    this.handle_crank_event((), cx, handle_2.clone());
                })
                .detach();
                
                // Set the LSP state
                app.lsp_state = Some(lsp_state.clone());
                app
            });

            cx.activate(true);
            cx.set_menus(app_menus());
            
            // Set up keybindings with proper key contexts
            
            // General editor actions
            cx.bind_keys([
                gpui::KeyBinding::new("up", MoveUp, Some("Editor")),
                gpui::KeyBinding::new("down", MoveDown, Some("Editor")),
                gpui::KeyBinding::new("left", MoveLeft, Some("Editor")),
                gpui::KeyBinding::new("right", MoveRight, Some("Editor")),
                gpui::KeyBinding::new("enter", Confirm, Some("Editor")),
                gpui::KeyBinding::new("escape", Cancel, Some("Editor")),
            ]);
            
            // Picker-specific keybindings
            cx.bind_keys([
                gpui::KeyBinding::new("up", SelectPrev, Some("Picker")),
                gpui::KeyBinding::new("down", SelectNext, Some("Picker")),
                gpui::KeyBinding::new("enter", ConfirmSelection, Some("Picker")),
                gpui::KeyBinding::new("escape", DismissPicker, Some("Picker")),
                gpui::KeyBinding::new("cmd-p", TogglePreview, Some("Picker")),
                gpui::KeyBinding::new("home", SelectFirst, Some("Picker")),
                gpui::KeyBinding::new("end", SelectLast, Some("Picker")),
            ]);
            
            // Completion-specific keybindings
            cx.bind_keys([
                gpui::KeyBinding::new("up", CompletionSelectPrev, Some("Completion")),
                gpui::KeyBinding::new("down", CompletionSelectNext, Some("Completion")),
                gpui::KeyBinding::new("ctrl-p", CompletionSelectPrev, Some("Completion")),
                gpui::KeyBinding::new("ctrl-n", CompletionSelectNext, Some("Completion")),
                gpui::KeyBinding::new("enter", CompletionConfirm, Some("Completion")),
                gpui::KeyBinding::new("tab", CompletionConfirm, Some("Completion")),
                gpui::KeyBinding::new("escape", CompletionDismiss, Some("Completion")),
                gpui::KeyBinding::new("ctrl-g", CompletionDismiss, Some("Completion")),
                gpui::KeyBinding::new("home", CompletionSelectFirst, Some("Completion")),
                gpui::KeyBinding::new("end", CompletionSelectLast, Some("Completion")),
            ]);

            let input_1 = input.clone();
            // Create overlay view
            let overlay = cx.new(|cx| {
                let view = overlay::OverlayView::new(&cx.focus_handle(), &app);
                view.subscribe(&app, cx);
                view
            });
            
            // Create notifications view with theme colors
            let notifications = cx.new(|cx| {
                let ui_theme = cx.global::<ui::Theme>();
                notification::NotificationView::new(ui_theme.background, ui_theme.text)
            });
            
            // Create info box view with default style
            let info = cx.new(|_cx| {
                info_box::InfoBoxView::new(gpui::Style::default())
            });
            
            // Create workspace
            
            
            cx.new(|cx| {
                cx.subscribe(&app, |w: &mut workspace::Workspace, _, ev, cx| {
                    w.handle_event(ev, cx);
                })
                .detach();
                workspace::Workspace::with_views(app, input_1.clone(), handle, overlay, notifications, info, cx)
            })
        });
    })
}

fn init_editor() -> Result<Option<(Application, crate::config::Config)>> {
    let help = format!(
        "\
{} {}
{}
{}

USAGE:
    hx [FLAGS] [files]...

ARGS:
    <files>...    Sets the input file to use, position can also be specified via file[:row[:col]]

FLAGS:
    -h, --help                     Prints help information
    --tutor                        Loads the tutorial
    --health [CATEGORY]            Checks for potential errors in editor setup
                                   CATEGORY can be a language or one of 'clipboard', 'languages'
                                   or 'all'. 'all' is the default if not specified.
    -g, --grammar {{fetch|build}}    Fetches or builds tree-sitter grammars listed in languages.toml
    -c, --config <file>            Specifies a file to use for configuration
    -v                             Increases logging verbosity each use for up to 3 times
    --log <file>                   Specifies a file to use for logging
                                   (default file: {})
    -V, --version                  Prints version information
    --vsplit                       Splits all given files vertically into different windows
    --hsplit                       Splits all given files horizontally into different windows
    -w, --working-dir <path>       Specify an initial working directory
    +N                             Open the first given file at line number N
",
        env!("CARGO_PKG_NAME"),
        VERSION_AND_GIT_HASH,
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_DESCRIPTION"),
        helix_loader::default_log_file().display(),
    );

    let mut args = Args::parse_args().context("could not parse arguments")?;

    helix_loader::initialize_config_file(args.config_file.clone());
    helix_loader::initialize_log_file(args.log_file.clone());

    // Help has a higher priority and should be handled separately.
    if args.display_help {
        print!("{help}");
        std::process::exit(0);
    }

    if args.display_version {
        println!("helix {VERSION_AND_GIT_HASH}");
        std::process::exit(0);
    }

    if args.health {
        if let Err(err) = helix_term::health::print_health(args.health_arg) {
            // Piping to for example `head -10` requires special handling:
            // https://stackoverflow.com/a/65760807/7115678
            if err.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(err.into());
            }
        }

        std::process::exit(0);
    }

    if args.fetch_grammars {
        helix_loader::grammar::fetch_grammars()?;
        return Ok(None);
    }

    if args.build_grammars {
        helix_loader::grammar::build_grammars(None)?;
        return Ok(None);
    }

    setup_logging(args.verbosity).context("failed to initialize logging")?;

    // Before setting the working directory, resolve all the paths in args.files
    args.files = args
        .files
        .into_iter()
        .map(|(path, pos)| (helix_stdx::path::canonicalize(&path), pos))
        .collect();

    // NOTE: Set the working directory early so the correct configuration is loaded. Be aware that
    // Application::new() depends on this logic so it must be updated if this changes.
    if let Some(path) = &args.working_directory {
        helix_stdx::env::set_current_working_dir(path)?;
    } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        // If the first file is a directory, it will be the working directory unless -w was specified
        helix_stdx::env::set_current_working_dir(path)?;
    }

    // Load our combined configuration (helix + gui)
    let config = match crate::config::Config::load() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Failed to load configuration: {err}");
            eprintln!("Using default configuration");
            crate::config::Config {
                helix: helix_term::config::Config::default(),
                gui: crate::config::GuiConfig::default(),
            }
        }
    };

    let lang_loader = helix_core::config::user_lang_loader().unwrap_or_else(|err| {
        eprintln!("{err}");
        eprintln!("Press <ENTER> to continue with default language config");
        use std::io::Read;
        // This waits for an enter press.
        let _ = std::io::stdin().read(&mut []);
        helix_core::config::default_lang_loader()
    });

    // TODO: use the thread local executor to spawn the application task separately from the work pool
    let app = application::init_editor(args, config.helix.clone(), lang_loader)
        .context("unable to create new application")?;

    Ok(Some((app, config)))
}
