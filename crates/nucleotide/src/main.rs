#![recursion_limit = "512"]

use std::panic;
use std::time::Duration;

use anyhow::{Context, Result};
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;
use nucleotide_logging::{error, info, instrument, warn};

use gpui::{
    App, AppContext, Menu, MenuItem, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, px,
};

// Import from the library crate instead of re-declaring modules
use nucleotide::application::Application;
use nucleotide::input_coordinator::InputCoordinator;
use nucleotide::{
    ThemeManager, application, config, info_box, notification, overlay, types, workspace,
};
use std::path::PathBuf;
use std::sync::Arc;

// Import nucleotide-ui enhanced components
// Note: These traits will be used in the workspace and component integration

// Only declare modules that are not in lib.rs (binary-specific modules)
mod test_utils;

pub type Core = Application;

// Re-export shared types
pub use types::{EditorStatus, Update};

fn setup_logging(verbosity: u64) -> Result<()> {
    use nucleotide_logging::{LoggingConfig, init_logging_with_reload};

    // Create configuration based on verbosity level
    let mut config =
        LoggingConfig::from_env().context("Failed to create logging config from environment")?;

    // Override log level based on command line verbosity
    let level = match verbosity {
        0 => nucleotide_logging::Level::WARN,
        1 => nucleotide_logging::Level::INFO,
        2 => nucleotide_logging::Level::DEBUG,
        _3_or_more => nucleotide_logging::Level::TRACE,
    };
    config.level = level.into();

    // Initialize the new logging system with hot-reload support
    init_logging_with_reload(config).context("Failed to initialize nucleotide logging")?;

    // Log startup message
    nucleotide_logging::info!("Nucleotide logging system initialized");

    Ok(())
}

#[instrument]
fn install_panic_handler() {
    panic::set_hook(Box::new(|info| {
        // Extract structured panic information
        let payload = info.payload();
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()));

        let panic_message = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };

        nucleotide_logging::error!(
            panic_message = %panic_message,
            location = ?location,
            thread = ?std::thread::current().name(),
            "Application panic occurred"
        );

        // Log backtrace if enabled
        if let Ok(backtrace) = std::env::var("RUST_BACKTRACE") {
            if backtrace == "1" || backtrace == "full" {
                let bt = std::backtrace::Backtrace::capture();
                nucleotide_logging::error!(
                    backtrace = %format!("{:?}", bt),
                    "Panic backtrace"
                );
                eprintln!("Backtrace:\n{bt:?}");
            }
        }

        // Log system information for debugging
        nucleotide_logging::error!(
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            version = env!("CARGO_PKG_VERSION"),
            "System information at panic time"
        );

        // Try to save any unsaved work would go here if we had access to the app state
        // For now, just log and exit gracefully
        eprintln!("Fatal error: {panic_message}");
        if let Some(loc) = &location {
            eprintln!("Location: {loc}");
        }

        // Exit gracefully
        std::process::exit(1);
    }));
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
        if let Some(rt) = nucleotide::utils::detect_bundle_runtime() {
            // SAFETY: Setting HELIX_RUNTIME environment variable during startup
            // before any threads are spawned is safe.
            unsafe {
                std::env::set_var("HELIX_RUNTIME", &rt);
            }
        }
    }

    // Set RUST_LOG to info level for bundled app to show our debugging messages
    // Only set if not already configured by user
    if std::env::var("RUST_LOG").is_err() {
        // SAFETY: Setting RUST_LOG environment variable during startup
        // before any threads are spawned is safe.
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
}

/// Determine the optimal workspace root directory for LSP servers
/// Priority: explicit working directory > first file parent > git repo root > current dir
#[instrument(skip(args))]
fn determine_workspace_root(args: &Args) -> Result<Option<PathBuf>> {
    // Priority 1: Explicit working directory
    if let Some(dir) = &args.working_directory {
        info!(directory = ?dir, "Using explicit working directory as workspace root");
        return Ok(Some(dir.clone()));
    }

    // Priority 2: If first file is a directory, use it
    if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        info!(directory = ?path, "Using directory argument as workspace root");
        return Ok(Some(path.clone()));
    }

    // Priority 3: For file arguments, find the workspace root of the first file's parent
    if let Some((first_file, _)) = args.files.first() {
        if let Some(parent) = first_file.parent() {
            if parent.exists() {
                let workspace_root = nucleotide::application::find_workspace_root_from(parent);
                info!(
                    file = ?first_file,
                    parent = ?parent,
                    workspace_root = ?workspace_root,
                    "Found workspace root from file parent"
                );
                return Ok(Some(workspace_root));
            }
        }
    }

    // Priority 4: Try to find workspace root from current directory
    if let Ok(current_dir) = std::env::current_dir() {
        let workspace_root = nucleotide::application::find_workspace_root_from(&current_dir);
        if workspace_root != current_dir {
            info!(
                current_dir = ?current_dir,
                workspace_root = ?workspace_root,
                "Found workspace root from current directory"
            );
            return Ok(Some(workspace_root));
        }
    }

    // No specific workspace root found
    info!("No specific workspace root detected, using default working directory logic");
    Ok(None)
}

#[instrument]
fn main() -> Result<()> {
    // Set HELIX_RUNTIME for macOS bundles before any Helix code runs (backup)
    #[cfg(target_os = "macos")]
    if std::env::var("HELIX_RUNTIME").is_err() {
        if let Some(rt) = nucleotide::utils::detect_bundle_runtime() {
            // SAFETY: Setting HELIX_RUNTIME environment variable during startup
            // before any threads are spawned is safe.
            unsafe {
                std::env::set_var("HELIX_RUNTIME", &rt);
            }
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
    let (app, config, workspace_root) = match init_editor() {
        Ok(Some((app, config, workspace_root))) => {
            info!("Editor initialized successfully");
            (app, config, workspace_root)
        }
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
    info!("Starting GUI main loop");
    gui_main(app, config, handle.clone(), workspace_root);
    info!("Application shutting down");
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

fn window_options(cx: &mut App, config: &nucleotide::config::Config) -> gpui::WindowOptions {
    let window_decorations = match std::env::var("HELIX_WINDOW_DECORATIONS") {
        Ok(val) if val == "server" => gpui::WindowDecorations::Server,
        Ok(val) if val == "client" => gpui::WindowDecorations::Client,
        _ => gpui::WindowDecorations::Client, // Default to client decorations
    };

    // Determine window background appearance based on theme and configuration
    let theme_manager = cx.global::<crate::ThemeManager>();
    let is_dark = theme_manager.is_dark_theme();
    let window_background = if is_dark && config.gui.window.blur_dark_themes {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
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
        window_background,
        window_decorations: Some(window_decorations),
        window_min_size: Some(gpui::size(px(400.0), px(300.0))),
    }
}

// Import actions from our centralized definitions
use nucleotide::actions::{
    common::{Cancel, Confirm, MoveDown, MoveLeft, MoveRight, MoveUp},
    completion::{
        CompletionConfirm, CompletionDismiss, CompletionSelectFirst, CompletionSelectLast,
        CompletionSelectNext, CompletionSelectPrev, TriggerCompletion,
    },
    editor::{
        CloseFile, Copy, DecreaseFontSize, IncreaseFontSize, OpenDirectory, OpenFile, Paste, Quit,
        Redo, Save, SaveAs, Undo,
    },
    help::{About, OpenTutorial},
    picker::{ConfirmSelection, DismissPicker, SelectFirst, SelectLast, TogglePreview},
    test::{TestCompletion, TestPrompt},
    window::{Hide, HideOthers, Minimize, ShowAll, Zoom},
    workspace::ToggleFileTree,
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

// Update and EditorStatus are now in types module

// This section previously contained Update enum which is now in types.rs
// Keeping this comment to maintain line numbers for now

/*
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
        trigger: nucleotide::event_bridge::CompletionTrigger,
    },
    // File tree events
    FileTreeEvent(nucleotide::file_tree::FileTreeEvent),
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
*/

// Font types are now exported from nucleotide::types
use nucleotide::{EditorFontConfig, FontSettings, UiFontConfig};

#[instrument(skip(app, config, handle))]
fn gui_main(
    mut app: Application,
    config: nucleotide::config::Config,
    handle: tokio::runtime::Handle,
    workspace_root: Option<std::path::PathBuf>,
) {
    // Store a channel for sending file open requests from macOS
    let (file_open_tx, mut file_open_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();

    let gpui_app = gpui::Application::new().with_assets(nucleotide_ui::Assets);

    // Register handler for macOS file open events (dock drops and Finder "Open With")
    gpui_app.on_open_urls({
        let file_open_tx = file_open_tx.clone();
        move |urls| {
            info!(urls = ?urls, "Received open URLs request");

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
                    error!(error = %e, "Failed to send file open request");
                }
            }
        }
    });

    gpui_app.run({
        let workspace_root_for_closure = workspace_root.clone();
        move |cx| {
            let workspace_root = workspace_root_for_closure;
        // Initialize the enhanced UI system
        use nucleotide_ui::providers::init_provider_system;

        // Initialize nucleotide-ui first (sets up UIConfig and component registry)
        nucleotide_ui::init(cx, None);

        // Initialize SystemAppearance global state from current window appearance
        nucleotide_ui::theme_manager::SystemAppearance::init(cx);

        // Initialize the provider system
        init_provider_system();

        // Set up theme manager with Helix theme
        let helix_theme = app.editor.theme.clone();
        let mut theme_manager = crate::ThemeManager::new(helix_theme);

        // Detect initial system appearance
        #[cfg(target_os = "macos")]
        {
            // Get current system appearance from window
            // This will be properly detected when we create the window
            // For now, we'll use a default based on theme darkness
            if theme_manager.is_dark_theme() {
                theme_manager
                    .set_system_appearance(nucleotide_ui::theme_manager::SystemAppearance::Dark);
            }
        }

        let ui_theme = theme_manager.ui_theme().clone();
        let is_dark_theme = theme_manager.is_dark_theme(); // Store before moving
        cx.set_global(theme_manager);
        cx.set_global(ui_theme);

        // Initialize the design token system based on the current theme
        use nucleotide_ui::Theme as EnhancedTheme;
        let enhanced_theme = if is_dark_theme {
            EnhancedTheme::dark()
        } else {
            EnhancedTheme::light()
        };
        cx.set_global(enhanced_theme);

        // Set up the enhanced provider system
        let ui_theme = cx.global::<nucleotide_ui::Theme>();

        // Create theme provider from existing theme manager
        let theme_provider = nucleotide_ui::providers::ThemeProvider::new(ui_theme.clone());

        // Create configuration provider for UI settings
        let config_provider = nucleotide_ui::providers::ConfigurationProvider::new();

        // Register global providers
        nucleotide_ui::providers::update_provider_context(|context| {
            context.register_global_provider(theme_provider);
            context.register_global_provider(config_provider);
        });

        nucleotide_logging::info!(
            "Provider system initialized with theme and configuration providers"
        );

        // Setup provider lifecycle management
        setup_provider_lifecycle(cx);

        // Initialize VCS service
        let vcs_config = nucleotide::vcs_service::VcsConfig::default();
        let vcs_service = nucleotide::vcs_service::VcsServiceHandle::new(vcs_config, cx);
        cx.set_global(vcs_service);

        // Set up fonts from configuration
        let editor_font_config = config.editor_font();
        let ui_font_config = config.ui_font();

        let font_settings = FontSettings {
            fixed_font: nucleotide_types::Font {
                family: editor_font_config.family.clone(),
                weight: editor_font_config.weight,
                style: nucleotide_types::FontStyle::Normal,
            },
            var_font: nucleotide_types::Font {
                family: ui_font_config.family.clone(),
                weight: ui_font_config.weight,
                style: nucleotide_types::FontStyle::Normal,
            },
        };
        cx.set_global(font_settings);

        // Store editor font config for document views
        cx.set_global(EditorFontConfig {
            family: editor_font_config.family,
            size: editor_font_config.size,
            weight: editor_font_config.weight,
            line_height: 1.4, // Default line height
        });

        // Store UI font config for UI components
        cx.set_global(UiFontConfig {
            family: ui_font_config.family,
            size: ui_font_config.size,
            weight: ui_font_config.weight,
        });

        // Initialize preview tracker
        cx.set_global(nucleotide_core::preview_tracker::PreviewTracker::new());

        let options = window_options(cx, &config);

        let _ = cx.open_window(options, |_window, cx| {
            // Set up window event handlers to send events to Helix
            info!("Setting up window event handlers");

            // Example: Send window resize events to Helix
            // Note: This is a conceptual example - actual GPUI window resize events
            // would be handled differently depending on the GPUI version
            cx.spawn(async move |_cx| {
                // This would be triggered by actual GPUI window events
                nucleotide_core::gpui_to_helix_bridge::send_gpui_event_to_helix(
                    nucleotide_core::gpui_to_helix_bridge::GpuiToHelixEvent::WindowResized {
                        width: 120,
                        height: 40,
                    },
                );
            })
            .detach();

            let input = cx.new(|_| nucleotide::application::Input);
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
                            warn!(error = ?e, "Failed to emit crank event");
                            // Continue the loop even if update fails
                        }
                    }
                })
                .detach();
                nucleotide::application::Crank
            });
            let crank_1 = crank.clone();
            std::mem::forget(crank_1);

            let input_1 = input.clone();
            let handle_1 = handle.clone();
            // Create LSP state entity
            let lsp_state = cx.new(|_| nucleotide_lsp::LspState::new());

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
                                let old_message = state.status_message.clone();
                                state.status_message = state.get_lsp_indicator();

                                // Update project status service with current LSP state
                                if let Some(project_status) = cx.try_global::<nucleotide::project_status_service::ProjectStatusHandle>() {
                                    let project_status = project_status.clone();
                                    project_status.update_lsp_state(state, cx);
                                }

                                // Only notify if there's actually a change
                                if state.status_message != old_message {
                                    cx.notify();
                                }
                            });
                        }) {
                            warn!(error = ?e, "Failed to update LSP indicator");
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
            use nucleotide::actions::workspace::{
                NewFile, NewWindow, ShowBufferPicker, ShowCommandPalette, ShowFileFinder,
                ToggleFileTree,
            };

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
                // Completion trigger
                gpui::KeyBinding::new("ctrl-space", TriggerCompletion, None),
                // File tree toggle
                gpui::KeyBinding::new("ctrl-b", ToggleFileTree, None),
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
            use nucleotide::actions::file_tree::{OpenFile, SelectNext, SelectPrev};
            cx.bind_keys([
                gpui::KeyBinding::new(
                    "up",
                    nucleotide::actions::file_tree::SelectPrev,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "down",
                    nucleotide::actions::file_tree::SelectNext,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "k",
                    nucleotide::actions::file_tree::SelectPrev,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "j",
                    nucleotide::actions::file_tree::SelectNext,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "left",
                    nucleotide::actions::file_tree::ToggleExpanded,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "right",
                    nucleotide::actions::file_tree::ToggleExpanded,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "h",
                    nucleotide::actions::file_tree::ToggleExpanded,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "l",
                    nucleotide::actions::file_tree::ToggleExpanded,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "space",
                    nucleotide::actions::file_tree::ToggleExpanded,
                    Some("FileTree"),
                ),
                gpui::KeyBinding::new(
                    "enter",
                    nucleotide::actions::file_tree::OpenFile,
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
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                notification::NotificationView::new(
                    ui_theme.tokens.colors.background,
                    ui_theme.tokens.colors.text_primary,
                )
            });

            // Create info box view with default style
            let info = cx.new(|_cx| info_box::InfoBoxView::new(gpui::Style::default()));

            // Create InputCoordinator for centralized input handling
            let input_coordinator = Arc::new(InputCoordinator::new());

            // Project LSP command processor will be started automatically when the Application runs

            // Create workspace

            let workspace = cx.new(|cx| {
                let mut workspace = workspace::Workspace::with_views(
                    app,
                    input_1.clone(),
                    handle,
                    overlay,
                    notifications,
                    info,
                    input_coordinator,
                    cx,
                );

                // Set the current project root explicitly using the workspace root that was successfully determined
                if let Some(root) = workspace_root.as_ref() {
                    workspace.set_current_project_root(Some(root.clone()));
                }

                // Subscribe to self to handle Update events
                cx.subscribe(&cx.entity(), |w: &mut workspace::Workspace, _, ev, cx| {
                    w.handle_event(ev, cx);
                })
                .detach();

                workspace
            });

            // Initialize ProjectLspManager for project detection and proactive LSP startup
            // This must be done after workspace creation to ensure proper initialization
            // NOTE: We cannot do this asynchronously here due to GPUI context limitations,
            // so we'll trigger it in the Application itself during startup
            nucleotide_logging::info!("Workspace created - ProjectLspManager will be initialized automatically");

            // Spawn a task to handle file open requests from macOS
            let workspace_clone = workspace.clone();
            cx.spawn(async move |cx| {
                while let Some(paths) = file_open_rx.recv().await {
                    info!(paths = ?paths, "Processing file open request");

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
                                    info!(directory = ?parent, "Will change working directory");
                                }
                            }
                        }
                    }

                    // Change working directory if needed
                    if should_change_dir {
                        if let Some(dir) = new_working_dir.clone() {
                            if let Err(e) = helix_stdx::env::set_current_working_dir(&dir) {
                                error!(
                                    directory = ?dir,
                                    error = %e,
                                    "Failed to change working directory"
                                );
                            } else {
                                info!(directory = ?dir, "Changed working directory");

                                // Update the core's project directory and emit OpenDirectory event
                                if let Err(e) = cx.update(|cx| {
                                    workspace_clone.update(cx, |workspace, cx| {
                                        workspace.set_project_directory(dir.clone(), cx);
                                        info!(directory = ?dir, "Updated project directory");
                                        // Emit OpenDirectory event to update file tree
                                        cx.emit(Update::Event(
                                            nucleotide::types::AppEvent::Workspace(
                                                nucleotide::types::WorkspaceEvent::OpenDirectory {
                                                    path: dir.clone(),
                                                },
                                            ),
                                        ));
                                    })
                                }) {
                                    error!(error = %e, "Failed to update project directory");
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
                                    cx.emit(Update::Event(nucleotide::types::AppEvent::Workspace(
                                        nucleotide::types::WorkspaceEvent::OpenFile {
                                            path: path.clone(),
                                        },
                                    )));
                                })
                            }) {
                                error!(file = %path.display(), error = %e, "Failed to open file");
                            }
                        } else {
                            warn!(file = %path.display(), "File does not exist");
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
                let titlebar = cx.new(|cx| nucleotide_ui::titlebar::TitleBar::new("titlebar", cx));

                workspace.update(cx, |workspace, cx| {
                    workspace.set_titlebar(titlebar.into());
                    cx.notify();
                });
            }

            workspace
        });
        }
    })
}

fn init_editor() -> Result<
    Option<(
        Application,
        crate::config::Config,
        Option<std::path::PathBuf>,
    )>,
> {
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

    // Load our combined configuration (helix + gui)
    let config = match crate::config::Config::load() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Failed to load configuration: {err}");
            eprintln!("Using default configuration");
            nucleotide::config::Config {
                helix: helix_term::config::Config::default(),
                gui: nucleotide::config::GuiConfig::default(),
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
    // Determine workspace root before creating application
    let workspace_root = determine_workspace_root(&args)?;

    // Set the working directory based on workspace root determination
    if let Some(root) = &workspace_root {
        info!(workspace_root = ?root, "Setting workspace root before Editor/LSP initialization");
        helix_stdx::env::set_current_working_dir(root)?;
    } else {
        // NOTE: Set the working directory early so the correct configuration is loaded. Be aware that
        // Application::new() depends on this logic so it must be updated if this changes.
        if let Some(path) = &args.working_directory {
            helix_stdx::env::set_current_working_dir(path)?;
        } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
            // If the first file is a directory, it will be the working directory unless -w was specified
            helix_stdx::env::set_current_working_dir(path)?;
        }
    }

    let app = application::init_editor(args, config.helix.clone(), config.clone(), lang_loader)
        .context("unable to create new application")?;

    Ok(Some((app, config, workspace_root)))
}

/// Setup provider lifecycle management for proper cleanup and state management
fn setup_provider_lifecycle(_cx: &mut App) {
    // Setup cleanup handlers for provider system when the app shuts down
    // This ensures proper resource cleanup when the application exits

    // Test provider composition patterns
    let _composition_result = demonstrate_provider_composition();

    nucleotide_logging::debug!("Provider lifecycle management configured");
}

/// Demonstrate provider composition patterns for nested contexts
fn demonstrate_provider_composition() -> Result<(), String> {
    // Example of how provider composition would work for nested contexts
    // This demonstrates the pattern without actually creating UI elements

    use nucleotide_ui::providers::with_provider_context;

    // Test that we can access the provider context
    let theme_available = with_provider_context(|context| {
        context
            .get_provider::<nucleotide_ui::providers::ThemeProvider>()
            .is_some()
    })
    .unwrap_or(false);

    let config_available = with_provider_context(|context| {
        context
            .get_provider::<nucleotide_ui::providers::ConfigurationProvider>()
            .is_some()
    })
    .unwrap_or(false);

    if theme_available && config_available {
        nucleotide_logging::info!(
            "Provider composition working correctly - theme and config providers accessible"
        );
        Ok(())
    } else {
        let error_msg = format!(
            "Provider composition validation failed - theme: {}, config: {}",
            theme_available, config_available
        );
        nucleotide_logging::warn!(error_msg);
        Err(error_msg)
    }
}
