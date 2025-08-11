#![recursion_limit = "512"]

use std::panic;
use std::time::Duration;

use anyhow::{Context, Result};
use helix_core::diagnostic::Severity;
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;

use gpui::{
    px, App, AppContext, Menu, MenuItem, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};

pub use application::Input;
use application::{Application, InputEvent};

mod actions;
mod application;
mod assets;
mod command_system;
mod completion;
mod config;
mod core;
mod document;
mod event_bridge;
mod file_tree;
mod gpui_to_helix_bridge;
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
mod scroll_manager;
mod statusline;
mod test_utils;
mod theme_manager;
mod titlebar;
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

#[cfg(target_os = "macos")]
pub fn detect_bundle_runtime() -> Option<std::path::PathBuf> {
    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop(); // nucl or nucleotide-bin
        exe.pop(); // MacOS
        exe.push("Resources");
        exe.push("runtime");
        if exe.is_dir() {
            return Some(exe);
        }
    }
    None
}

// Use constructor to set environment variable before any static initialization
#[cfg(target_os = "macos")]
#[ctor::ctor]
fn _early_runtime_init() {
    let needs_override = match std::env::var("HELIX_RUNTIME") {
        Ok(p) => p.contains("$(") || !std::path::Path::new(&p).join("themes").is_dir(),
        Err(_) => true,
    };

    if needs_override {
        if let Some(rt) = detect_bundle_runtime() {
            std::env::set_var("HELIX_RUNTIME", &rt);
        }
    }
}

fn main() -> Result<()> {
    // Set HELIX_RUNTIME for macOS bundles before any Helix code runs (backup)
    #[cfg(target_os = "macos")]
    if std::env::var("HELIX_RUNTIME").is_err() {
        if let Some(rt) = detect_bundle_runtime() {
            std::env::set_var("HELIX_RUNTIME", &rt);
        }
    }

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

fn parse_file_url(url: &str) -> Option<String> {
    // Handle file:// URLs
    if let Some(file_path) = url.strip_prefix("file://") {
        // Decode URL-encoded characters (spaces, special chars, etc.)
        let decoded = file_path
            .replace("%20", " ")
            .replace("%2F", "/")
            .replace("%3A", ":")
            .replace("%40", "@")
            .replace("%21", "!")
            .replace("%24", "$")
            .replace("%26", "&")
            .replace("%27", "'")
            .replace("%28", "(")
            .replace("%29", ")")
            .replace("%2A", "*")
            .replace("%2B", "+")
            .replace("%2C", ",")
            .replace("%3B", ";")
            .replace("%3D", "=");
        return Some(decoded);
    }

    // Handle file: URLs without //
    if let Some(file_path) = url.strip_prefix("file:") {
        let decoded = file_path
            .replace("%20", " ")
            .replace("%2F", "/")
            .replace("%3A", ":")
            .replace("%40", "@");
        return Some(decoded);
    }

    None
}

fn window_options(_cx: &mut App) -> gpui::WindowOptions {
    let window_decorations = match std::env::var("HELIX_WINDOW_DECORATIONS") {
        Ok(val) if val == "server" => gpui::WindowDecorations::Server,
        Ok(val) if val == "client" => gpui::WindowDecorations::Client,
        _ => gpui::WindowDecorations::Client, // Default to client decorations
    };

    WindowOptions {
        app_id: Some("nucleotide".to_string()),
        titlebar: Some(TitlebarOptions {
            title: None,                                                 // We'll render our own title
            appears_transparent: true,                                   // Key for custom titlebar
            traffic_light_position: Some(gpui::point(px(9.0), px(9.0))), // Required for macOS client decorations
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
        window_decorations: Some(window_decorations),
        window_min_size: Some(gpui::size(px(400.0), px(300.0))),
    }
}

// Import actions from our centralized definitions
use crate::actions::{
    completion::*, editor::*, help::*, picker::*, test::*, window::*, workspace::*,
};

fn app_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Nucleotide".into(),
            items: vec![
                MenuItem::action("About", About),
                MenuItem::separator(),
                MenuItem::action("Hide Nucleotide", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open...", OpenFile),
                MenuItem::action("Open Directory", OpenDirectory),
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
            name: "View".into(),
            items: vec![MenuItem::action("Toggle File Tree", ToggleFileTree)],
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

pub enum Update {
    Redraw,
    Prompt(prompt::Prompt),
    Picker(picker::Picker),
    DirectoryPicker(picker::Picker),
    Completion(gpui::Entity<completion::CompletionView>),
    Info(helix_view::info::Info),
    EditorEvent(helix_view::editor::EditorEvent),
    EditorStatus(EditorStatus),
    OpenFile(std::path::PathBuf),
    OpenDirectory(std::path::PathBuf),
    ShouldQuit,
    CommandSubmitted(String),
    SearchSubmitted(String),
    // Helix event bridge - these allow UI to respond to Helix events
    DocumentChanged {
        doc_id: helix_view::DocumentId,
    },
    SelectionChanged {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    ModeChanged {
        old_mode: helix_view::document::Mode,
        new_mode: helix_view::document::Mode,
    },
    DiagnosticsChanged {
        doc_id: helix_view::DocumentId,
    },
    // Additional granular events for better UI responsiveness
    DocumentOpened {
        doc_id: helix_view::DocumentId,
    },
    DocumentClosed {
        doc_id: helix_view::DocumentId,
    },
    ViewFocused {
        view_id: helix_view::ViewId,
    },
    LanguageServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },
    LanguageServerExited {
        server_id: helix_lsp::LanguageServerId,
    },
    CompletionRequested {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: crate::event_bridge::CompletionTrigger,
    },
    // File tree events
    FileTreeEvent(crate::file_tree::FileTreeEvent),
    // Picker request events - emitted when helix wants to show a picker
    ShowFilePicker,
    ShowBufferPicker,
}

// Manual Debug implementation to avoid proc macro issues with Entity<T>
impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Redraw => write!(f, "Update::Redraw"),
            Update::Prompt(p) => f.debug_tuple("Update::Prompt").field(p).finish(),
            Update::Picker(p) => f.debug_tuple("Update::Picker").field(p).finish(),
            Update::DirectoryPicker(p) => {
                f.debug_tuple("Update::DirectoryPicker").field(p).finish()
            }
            Update::Completion(_) => write!(f, "Update::Completion(<Entity>)"),
            Update::Info(i) => f.debug_tuple("Update::Info").field(i).finish(),
            Update::EditorEvent(e) => f.debug_tuple("Update::EditorEvent").field(e).finish(),
            Update::EditorStatus(s) => f.debug_tuple("Update::EditorStatus").field(s).finish(),
            Update::OpenFile(p) => f.debug_tuple("Update::OpenFile").field(p).finish(),
            Update::OpenDirectory(p) => f.debug_tuple("Update::OpenDirectory").field(p).finish(),
            Update::ShouldQuit => write!(f, "Update::ShouldQuit"),
            Update::CommandSubmitted(c) => {
                f.debug_tuple("Update::CommandSubmitted").field(c).finish()
            }
            Update::DocumentChanged { doc_id } => f
                .debug_struct("Update::DocumentChanged")
                .field("doc_id", doc_id)
                .finish(),
            Update::SelectionChanged { doc_id, view_id } => f
                .debug_struct("Update::SelectionChanged")
                .field("doc_id", doc_id)
                .field("view_id", view_id)
                .finish(),
            Update::ModeChanged { old_mode, new_mode } => f
                .debug_struct("Update::ModeChanged")
                .field("old_mode", old_mode)
                .field("new_mode", new_mode)
                .finish(),
            Update::DiagnosticsChanged { doc_id } => f
                .debug_struct("Update::DiagnosticsChanged")
                .field("doc_id", doc_id)
                .finish(),
            Update::DocumentOpened { doc_id } => f
                .debug_struct("Update::DocumentOpened")
                .field("doc_id", doc_id)
                .finish(),
            Update::DocumentClosed { doc_id } => f
                .debug_struct("Update::DocumentClosed")
                .field("doc_id", doc_id)
                .finish(),
            Update::ViewFocused { view_id } => f
                .debug_struct("Update::ViewFocused")
                .field("view_id", view_id)
                .finish(),
            Update::LanguageServerInitialized { server_id } => f
                .debug_struct("Update::LanguageServerInitialized")
                .field("server_id", server_id)
                .finish(),
            Update::LanguageServerExited { server_id } => f
                .debug_struct("Update::LanguageServerExited")
                .field("server_id", server_id)
                .finish(),
            Update::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => f
                .debug_struct("Update::CompletionRequested")
                .field("doc_id", doc_id)
                .field("view_id", view_id)
                .field("trigger", trigger)
                .finish(),
            Update::FileTreeEvent(e) => f.debug_tuple("Update::FileTreeEvent").field(e).finish(),
            Update::ShowFilePicker => write!(f, "Update::ShowFilePicker"),
            Update::ShowBufferPicker => write!(f, "Update::ShowBufferPicker"),
            Update::SearchSubmitted(_) => write!(f, "Update::SearchSubmitted"),
        }
    }
}

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
    // Store a channel for sending file open requests from macOS
    let (file_open_tx, mut file_open_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();

    let gpui_app = gpui::Application::new().with_assets(crate::assets::Assets);

    // Register handler for macOS file open events (dock drops and Finder "Open With")
    gpui_app.on_open_urls({
        let file_open_tx = file_open_tx.clone();
        move |urls| {
            log::info!("Received open URLs request: {:?}", urls);

            // Parse URLs and send file paths to the main app
            let mut paths = Vec::new();
            for url in urls {
                if let Some(file_path) = parse_file_url(&url) {
                    paths.push(file_path);
                } else if std::path::Path::new(&url).exists() {
                    // Handle direct file paths (not URLs)
                    paths.push(url.to_string());
                }
            }

            if !paths.is_empty() {
                if let Err(e) = file_open_tx.send(paths) {
                    log::error!("Failed to send file open request: {}", e);
                }
            }
        }
    });

    gpui_app.run(move |cx| {
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

        let _ = cx.open_window(options, |window, cx| {
            // Set up window event handlers to send events to Helix
            log::info!("Setting up window event handlers");

            // Example: Send window resize events to Helix
            // Note: This is a conceptual example - actual GPUI window resize events
            // would be handled differently depending on the GPUI version
            cx.spawn(async move |_cx| {
                // This would be triggered by actual GPUI window events
                crate::gpui_to_helix_bridge::send_gpui_event_to_helix(
                    crate::gpui_to_helix_bridge::GpuiToHelixEvent::WindowResized {
                        width: 120,
                        height: 40,
                    },
                );
            })
            .detach();

            let input = cx.new(|_| crate::application::Input);
            let crank = cx.new(|mc| {
                mc.spawn(async move |crank, cx| {
                    loop {
                        // Wait for timer
                        cx.background_executor()
                            .timer(Duration::from_millis(200)) // 5fps instead of 20fps
                            .await;

                        // Timer completed, emit update event
                        if let Err(e) = crank.update(cx, |_crank, cx| {
                            cx.emit(());
                        }) {
                            log::warn!("Failed to emit crank event: {e:?}");
                            // Continue the loop even if update fails
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
                                // Update LSP indicator - shows static when idle, animated when busy
                                state.status_message = state.get_lsp_indicator();

                                // Only notify if there's a change
                                cx.notify();
                            });
                        }) {
                            log::warn!("Failed to update LSP indicator: {e:?}");
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
                mc.subscribe(&crank, move |this: &mut Application, _, _ev, cx| {
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

            // Import workspace actions for global bindings
            use crate::actions::workspace::*;

            // Global actions - work regardless of focus (no context specified)
            cx.bind_keys([
                gpui::KeyBinding::new("cmd-q", Quit, None),
                gpui::KeyBinding::new("cmd-o", OpenFile, None),
                gpui::KeyBinding::new("cmd-shift-o", OpenDirectory, None),
                gpui::KeyBinding::new("cmd-s", Save, None),
                gpui::KeyBinding::new("cmd-shift-s", SaveAs, None),
                gpui::KeyBinding::new("cmd-w", CloseFile, None),
                gpui::KeyBinding::new("cmd-n", NewFile, None),
                gpui::KeyBinding::new("cmd-shift-n", NewWindow, None),
                gpui::KeyBinding::new("cmd-p", ShowFileFinder, None),
                gpui::KeyBinding::new("cmd-shift-p", ShowCommandPalette, None),
                gpui::KeyBinding::new("cmd-b", ShowBufferPicker, None),
                gpui::KeyBinding::new("cmd-z", Undo, None),
                gpui::KeyBinding::new("cmd-shift-z", Redo, None),
                gpui::KeyBinding::new("cmd-c", Copy, None),
                gpui::KeyBinding::new("cmd-v", Paste, None),
                gpui::KeyBinding::new("cmd-+", IncreaseFontSize, None),
                gpui::KeyBinding::new("cmd-=", IncreaseFontSize, None), // Also bind = key since + requires shift
                gpui::KeyBinding::new("cmd--", DecreaseFontSize, None),
            ]);

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
                gpui::KeyBinding::new("ctrl-p", SelectPrev, Some("Picker")),
                gpui::KeyBinding::new("ctrl-n", SelectNext, Some("Picker")),
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

            // FileTree-specific keybindings
            use crate::actions::file_tree::*;
            cx.bind_keys([
                gpui::KeyBinding::new(
                    "up",
                    crate::actions::file_tree::SelectPrev,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "down",
                    crate::actions::file_tree::SelectNext,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new("k", crate::actions::file_tree::SelectPrev, Some("FileTree")),
                gpui::KeyBinding::new("j", crate::actions::file_tree::SelectNext, Some("FileTree")),
                gpui::KeyBinding::new(
                    "left",
                    crate::actions::file_tree::ExpandCollapse,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "right",
                    crate::actions::file_tree::ExpandCollapse,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "h",
                    crate::actions::file_tree::ExpandCollapse,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "l",
                    crate::actions::file_tree::ExpandCollapse,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "space",
                    crate::actions::file_tree::ExpandCollapse,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "enter",
                    crate::actions::file_tree::OpenFile,
                    Some("FileTree"),
                ),
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
            let info = cx.new(|_cx| info_box::InfoBoxView::new(gpui::Style::default()));

            // Create workspace

            let workspace = cx.new(|cx| {
                let workspace = workspace::Workspace::with_views(
                    app,
                    input_1.clone(),
                    handle,
                    overlay,
                    notifications,
                    info,
                    cx,
                );

                // Subscribe to self to handle Update events
                cx.subscribe(&cx.entity(), |w: &mut workspace::Workspace, _, ev, cx| {
                    w.handle_event(ev, cx);
                })
                .detach();

                workspace
            });

            // Spawn a task to handle file open requests from macOS
            let workspace_clone = workspace.clone();
            cx.spawn(async move |cx| {
                while let Some(paths) = file_open_rx.recv().await {
                    log::info!("Processing file open request for paths: {:?}", paths);

                    // If we have files to open, change working directory to the parent of the first file
                    let mut should_change_dir = false;
                    let mut new_working_dir = None;

                    for (index, path_str) in paths.iter().enumerate() {
                        let path = std::path::PathBuf::from(path_str);
                        if path.exists() {
                            // For the first valid file, set its parent as the working directory
                            if index == 0 && !should_change_dir {
                                if let Some(parent) = path.parent() {
                                    new_working_dir = Some(parent.to_path_buf());
                                    should_change_dir = true;
                                    log::info!("Will change working directory to: {:?}", parent);
                                }
                            }
                        }
                    }

                    // Change working directory if needed
                    if should_change_dir {
                        if let Some(dir) = new_working_dir.clone() {
                            if let Err(e) = helix_stdx::env::set_current_working_dir(&dir) {
                                log::error!(
                                    "Failed to change working directory to {:?}: {}",
                                    dir,
                                    e
                                );
                            } else {
                                log::info!("Changed working directory to: {:?}", dir);

                                // Update the core's project directory and emit OpenDirectory event
                                if let Err(e) = cx.update(|cx| {
                                    workspace_clone.update(cx, |workspace, cx| {
                                        workspace.set_project_directory(dir.clone(), cx);
                                        log::info!("Updated project directory to: {:?}", dir);
                                        // Emit OpenDirectory event to update file tree
                                        cx.emit(Update::OpenDirectory(dir.clone()));
                                    })
                                }) {
                                    log::error!("Failed to update project directory: {}", e);
                                }
                            }
                        }
                    }

                    // Now open all the files
                    for path_str in paths {
                        let path = std::path::PathBuf::from(path_str);
                        if path.exists() {
                            // Send OpenFile update to the workspace
                            if let Err(e) = cx.update(|cx| {
                                workspace_clone.update(cx, |_workspace, cx| {
                                    cx.emit(Update::OpenFile(path.clone()));
                                })
                            }) {
                                log::error!("Failed to open file {}: {}", path.display(), e);
                            }
                        } else {
                            log::warn!("File does not exist: {}", path.display());
                        }
                    }
                }
            })
            .detach();

            // Create and set titlebar after workspace is created - on macOS we always want custom titlebar
            // regardless of what decorations are reported

            // Always create titlebar on macOS (and when client decorations on other platforms)
            #[cfg(target_os = "macos")]
            let should_create_titlebar = true;
            #[cfg(not(target_os = "macos"))]
            let should_create_titlebar = {
                let decorations = window.window_decorations();
                matches!(decorations, gpui::Decorations::Client { .. })
            };

            if should_create_titlebar {
                let titlebar =
                    cx.new(|cx| crate::titlebar::TitleBar::new("titlebar", &workspace, cx));

                workspace.update(cx, |workspace, cx| {
                    workspace.set_titlebar(titlebar.into());
                    cx.notify();
                });
            }

            workspace
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
    nucl [FLAGS] [files]...

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
        eprintln!("helix {VERSION_AND_GIT_HASH}");
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
