#![recursion_limit = "512"]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::panic;
use std::time::Duration;

use anyhow::{Context, Result};
use helix_term::args::Args;
use nucleotide_logging::{error, info, instrument, warn};

use gpui::{
    AppContext, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowKind, WindowOptions, px,
};

// Import from the library crate instead of re-declaring modules
use nucleotide::application::{Application, MaintenanceWake};
use nucleotide::input_coordinator::InputCoordinator;
use nucleotide::{self, ThemeManager, config, info_box, notification, overlay, types, workspace};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

// Import nucleotide-ui enhanced components
// Note: These traits will be used in the workspace and component integration

// Only declare modules that are not in lib.rs (binary-specific modules)
mod test_utils;
#[cfg(target_os = "windows")]
mod windows_single_instance;

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
        if let Ok(backtrace) = std::env::var("RUST_BACKTRACE")
            && (backtrace == "1" || backtrace == "full")
        {
            let bt = std::backtrace::Backtrace::capture();
            nucleotide_logging::error!(
                backtrace = %format!("{:?}", bt),
                "Panic backtrace"
            );
            eprintln!("Backtrace:\n{:?}", bt);
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
        error!("Fatal error: {panic_message}");
        if let Some(loc) = &location {
            error!("Location: {loc}");
        }

        // Exit gracefully
        std::process::exit(1);
    }));
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn configure_bundle_runtime_environment() {
    let Some(rt) = nucleotide::utils::detect_bundle_runtime() else {
        return;
    };

    if let Some(manifest_dir) = nucleotide::utils::manifest_dir_for_runtime(&rt) {
        unsafe { std::env::set_var("CARGO_MANIFEST_DIR", manifest_dir) };
    }

    let needs_override = match std::env::var("HELIX_RUNTIME") {
        Ok(p) => p.contains("$(") || !std::path::Path::new(&p).join("themes").is_dir(),
        Err(_) => true,
    };

    if needs_override {
        unsafe { std::env::set_var("HELIX_RUNTIME", &rt) };
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn configure_bundle_runtime_environment() {}

// Use constructor to set environment variable before any static initialization
#[cfg(target_os = "macos")]
#[ctor::ctor(unsafe)]
fn _early_runtime_init() {
    configure_bundle_runtime_environment();

    // Set RUST_LOG to info level for bundled app to show our debugging messages
    // Only set if not already configured by user
    if std::env::var("RUST_LOG").is_err() {
        unsafe { std::env::set_var("RUST_LOG", "info") };
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

    // Priority 2: Remote startup arguments must not be probed through the host filesystem.
    if let Some((path, _)) = args.files.first()
        && let Some(remote_root) = nucleotide_workspace::remote_startup_workspace_root(path)
    {
        info!(
            file = ?path,
            workspace_root = ?remote_root,
            "Using remote startup workspace root without host filesystem probes"
        );
        return Ok(Some(remote_root));
    }

    // Priority 3: If first file is a directory, use it
    if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        info!(directory = ?path, "Using directory argument as workspace root");
        return Ok(Some(path.clone()));
    }

    // Priority 4: For file arguments, find the workspace root of the first file's parent
    if let Some((first_file, _)) = args.files.first()
        && let Some(parent) = first_file.parent()
        && parent.exists()
    {
        let workspace_root = nucleotide::application::find_workspace_root_from(parent);
        info!(
            file = ?first_file,
            parent = ?parent,
            workspace_root = ?workspace_root,
            "Found workspace root from file parent"
        );
        return Ok(Some(workspace_root));
    }

    // Priority 5: Try to find workspace root from current directory
    if let Some(workspace_root) =
        nucleotide::application::implicit_workspace_root_from_current_dir()
    {
        info!(
            workspace_root = ?workspace_root,
            "Found workspace root from current directory"
        );
        return Ok(Some(workspace_root));
    }

    // No specific workspace root found
    info!("No specific workspace root detected, using default working directory logic");
    Ok(None)
}

fn normalize_startup_file_path(path: &Path) -> PathBuf {
    if nucleotide_workspace::classify_workspace_location(path).is_remote() {
        path.to_path_buf()
    } else {
        helix_stdx::path::canonicalize(path)
    }
}

fn startup_host_working_directory(args: &Args, workspace_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(root) = workspace_root {
        if nucleotide_workspace::classify_workspace_location(root).is_remote() {
            info!(
                workspace_root = ?root,
                "Keeping host working directory unchanged for remote workspace root"
            );
            return None;
        }

        return Some(root.to_path_buf());
    }

    if let Some(path) = &args.working_directory {
        if nucleotide_workspace::classify_workspace_location(path).is_remote() {
            info!(
                working_directory = ?path,
                "Keeping host working directory unchanged for remote working directory"
            );
            return None;
        }

        return Some(path.clone());
    }

    args.files
        .first()
        .filter(|(path, _)| {
            !nucleotide_workspace::classify_workspace_location(path).is_remote() && path.is_dir()
        })
        .map(|(path, _)| path.clone())
}

#[cfg(any(target_os = "windows", test))]
fn parse_startup_dock_action<I, S>(args: I) -> Result<Option<usize>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut argv = args.into_iter();
    let _program = argv.next();

    let Some(flag) = argv.next() else {
        return Ok(None);
    };

    if flag.as_ref() != "--dock-action" {
        return Ok(None);
    }

    let Some(index) = argv.next() else {
        anyhow::bail!("--dock-action must specify an action index");
    };

    if argv.next().is_some() {
        anyhow::bail!("--dock-action cannot be combined with files or other flags");
    }

    index
        .as_ref()
        .parse::<usize>()
        .map(Some)
        .context("--dock-action must specify a numeric action index")
}

#[cfg(target_os = "windows")]
fn startup_dock_action() -> Result<Option<usize>> {
    parse_startup_dock_action(std::env::args())
}

#[cfg(any(target_os = "windows", test))]
fn is_nucleotide_url_arg(value: &str) -> bool {
    value
        .get(..NUCLEOTIDE_URL_SCHEME.len() + 1)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("nucleotide:"))
}

#[cfg(any(target_os = "windows", test))]
fn parse_startup_protocol_request<I, S>(args: I) -> Result<Option<ProtocolOpenRequest>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut argv = args.into_iter();
    let _program = argv.next();

    let Some(url) = argv.next() else {
        return Ok(None);
    };

    let url = url.as_ref();
    if !is_nucleotide_url_arg(url) {
        return Ok(None);
    }

    if argv.next().is_some() {
        anyhow::bail!("nucleotide:// URL launches cannot be combined with files or other flags");
    }

    parse_nucleotide_url(url)
        .with_context(|| format!("unsupported Nucleotide URL: {url}"))
        .map(Some)
}

#[cfg(target_os = "windows")]
fn startup_protocol_request() -> Result<Option<ProtocolOpenRequest>> {
    parse_startup_protocol_request(std::env::args())
}

#[cfg(any(target_os = "windows", test))]
fn apply_protocol_request_to_args(args: &mut Args, request: ProtocolOpenRequest) {
    if let Some(working_directory) = request.working_directory {
        args.working_directory = Some(working_directory);
    }

    for file in request.files {
        args.files
            .entry(file.path)
            .and_modify(|positions| positions.push(file.position))
            .or_insert_with(|| vec![file.position]);
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_APP_USER_MODEL_ID: &str = "org.spiralpoint.nucleotide";

#[cfg(target_os = "windows")]
fn windows_wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn configure_windows_app_user_model_id() {
    let app_id = windows_wide_nul(WINDOWS_APP_USER_MODEL_ID);
    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr())
    };

    if result < 0 {
        warn!(
            hresult = format!("{result:#010x}"),
            app_user_model_id = WINDOWS_APP_USER_MODEL_ID,
            "Failed to set Windows AppUserModelID"
        );
    } else {
        info!(
            app_user_model_id = WINDOWS_APP_USER_MODEL_ID,
            "Configured Windows AppUserModelID"
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn configure_windows_app_user_model_id() {}

#[cfg(not(target_os = "windows"))]
fn startup_dock_action() -> Result<Option<usize>> {
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
fn startup_protocol_request() -> Result<Option<ProtocolOpenRequest>> {
    Ok(None)
}

#[cfg(not(any(target_os = "windows", test)))]
fn apply_protocol_request_to_args(_args: &mut Args, _request: ProtocolOpenRequest) {}

#[instrument]
fn main() -> Result<()> {
    // Set HELIX_RUNTIME for packaged apps before any Helix runtime lookup occurs.
    configure_bundle_runtime_environment();

    // Install panic handler to prevent data loss
    install_panic_handler();

    let initial_dock_action = startup_dock_action()?;
    let initial_protocol_request = startup_protocol_request()?;
    let mut args = if initial_dock_action.is_some() || initial_protocol_request.is_some() {
        Args::default()
    } else {
        nucleotide::cli::parse_args()?
    };
    if let Some(request) = initial_protocol_request {
        apply_protocol_request_to_args(&mut args, request);
    }

    helix_loader::initialize_config_file(args.config_file.clone());
    helix_loader::initialize_log_file(args.log_file.clone());

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
        return Ok(());
    }

    if args.build_grammars {
        helix_loader::grammar::build_grammars(None)?;
        return Ok(());
    }

    setup_logging(args.verbosity).context("failed to initialize logging")?;
    configure_wsl_graphics();
    configure_windows_app_user_model_id();

    // Before setting the working directory, resolve all the paths in args.files
    args.files = args
        .files
        .into_iter()
        .map(|(path, pos)| (normalize_startup_file_path(&path), pos))
        .collect();

    #[cfg(target_os = "windows")]
    let _windows_single_instance_guard =
        match windows_single_instance::claim_or_forward(&args, initial_dock_action)? {
            windows_single_instance::ClaimResult::Primary(guard) => guard,
            windows_single_instance::ClaimResult::Forwarded => return Ok(()),
        };

    let (platform_open_tx, platform_open_rx) =
        tokio::sync::mpsc::unbounded_channel::<ExternalOpenRequest>();

    #[cfg(target_os = "windows")]
    windows_single_instance::start_listener(platform_open_tx.clone());

    // Load our combined configuration (helix + gui)
    let config = match crate::config::Config::load() {
        Ok(config) => config,
        Err(err) => {
            error!("Failed to load configuration: {err}");
            error!("Using default configuration");
            nucleotide::config::Config {
                helix: helix_term::config::Config::default(),
                gui: nucleotide::config::GuiConfig::default(),
            }
        }
    };

    let lang_loader = helix_core::config::user_lang_loader().unwrap_or_else(|err| {
        error!("{err}");
        error!("Press <ENTER> to continue with default language config");
        use std::io::Read;
        // This waits for an enter press.
        let _ = std::io::stdin().read(&mut []);
        helix_core::config::default_lang_loader()
    });

    // TODO: use the thread local executor to spawn the application task separately from the work pool
    // Determine workspace root before creating application
    let workspace_root = determine_workspace_root(&args)?;

    // NOTE: Set the host working directory early so the correct configuration is loaded. Remote
    // workspace roots are project identifiers for the backend, not valid host process directories.
    if let Some(host_working_directory) =
        startup_host_working_directory(&args, workspace_root.as_deref())
    {
        info!(
            directory = ?host_working_directory,
            "Setting host working directory before Editor/LSP initialization"
        );
        helix_stdx::env::set_current_working_dir(&host_working_directory)?;
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("Failed to initialize Tokio runtime: {e}");
            return Err(anyhow::anyhow!("Runtime initialization failed: {}", e));
        }
    };
    let handle = rt.handle();
    let _guard = handle.enter();

    // Initialize the editor AFTER the Tokio runtime is created and entered
    // This is critical because Helix LSP components need an active Tokio runtime
    let app = nucleotide::application::init_editor(
        args,
        config.helix.clone(),
        config.clone(),
        lang_loader,
    )
    .context("unable to create new application")?;

    info!("Starting GUI main loop");
    gui_main(
        app,
        config,
        handle.clone(),
        workspace_root,
        initial_dock_action,
        platform_open_tx,
        platform_open_rx,
    );
    info!("Application shutting down");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct ExternalOpenPosition {
    row: usize,
    col: usize,
}

impl From<helix_core::Position> for ExternalOpenPosition {
    fn from(position: helix_core::Position) -> Self {
        Self {
            row: position.row,
            col: position.col,
        }
    }
}

impl From<ExternalOpenPosition> for helix_core::Position {
    fn from(position: ExternalOpenPosition) -> Self {
        Self::new(position.row, position.col)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct ExternalOpenFile {
    path: PathBuf,
    position: ExternalOpenPosition,
}

impl ExternalOpenFile {
    fn new(path: PathBuf, position: helix_core::Position) -> Self {
        Self {
            path,
            position: position.into(),
        }
    }

    fn path(path: PathBuf) -> Self {
        Self::new(path, helix_core::Position::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct ExternalOpenRequest {
    files: Vec<ExternalOpenFile>,
    working_directory: Option<PathBuf>,
    dock_action: Option<usize>,
}

impl ExternalOpenRequest {
    fn paths(paths: Vec<PathBuf>) -> Self {
        Self {
            files: paths.into_iter().map(ExternalOpenFile::path).collect(),
            working_directory: None,
            dock_action: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolOpenFile {
    path: PathBuf,
    position: helix_core::Position,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolOpenRequest {
    files: Vec<ProtocolOpenFile>,
    working_directory: Option<PathBuf>,
}

#[cfg(any(target_os = "windows", test))]
const NUCLEOTIDE_URL_SCHEME: &str = "nucleotide";

fn parse_file_url(url_str: &str) -> Option<PathBuf> {
    if let Ok(url) = Url::parse(url_str)
        && url.scheme() == "file"
    {
        return url.to_file_path().ok();
    }
    None
}

#[cfg(any(target_os = "windows", test))]
fn path_from_url_query_value(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        None
    } else if let Some(path) = parse_file_url(value) {
        Some(path)
    } else {
        Some(PathBuf::from(value))
    }
}

#[cfg(any(target_os = "windows", test))]
fn one_based_query_position(line: Option<&str>, column: Option<&str>) -> helix_core::Position {
    let row = line
        .and_then(|line| line.parse::<usize>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let col = column
        .and_then(|column| column.parse::<usize>().ok())
        .unwrap_or(1)
        .saturating_sub(1);

    helix_core::Position::new(row, col)
}

#[cfg(any(target_os = "windows", test))]
fn parse_nucleotide_url(url_str: &str) -> Option<ProtocolOpenRequest> {
    let url = Url::parse(url_str).ok()?;
    if url.scheme() != NUCLEOTIDE_URL_SCHEME {
        return None;
    }

    let action = url.host_str().unwrap_or_default();
    if !action.is_empty() && !action.eq_ignore_ascii_case("open") {
        return None;
    }

    let mut paths = Vec::new();
    let mut working_directory = None;
    let mut line = None;
    let mut column = None;

    for (key, value) in url.query_pairs() {
        let value = value.into_owned();
        match key.as_ref() {
            "path" | "file" | "url" => {
                if let Some(path) = path_from_url_query_value(&value) {
                    paths.push(path);
                }
            }
            "cwd" | "dir" | "directory" | "working_dir" | "working-directory" => {
                working_directory = path_from_url_query_value(&value);
            }
            "line" | "row" => {
                line = Some(value);
            }
            "column" | "col" | "character" => {
                column = Some(value);
            }
            _ => {}
        }
    }

    let position = one_based_query_position(line.as_deref(), column.as_deref());
    Some(ProtocolOpenRequest {
        files: paths
            .into_iter()
            .map(|path| ProtocolOpenFile { path, position })
            .collect(),
        working_directory,
    })
}

fn open_request_workspace_dir(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

fn window_options(
    _cx: &mut impl gpui::AppContext,
    config: &nucleotide::config::Config,
    is_dark_chrome: bool,
) -> gpui::WindowOptions {
    let window_decorations = match std::env::var("HELIX_WINDOW_DECORATIONS") {
        Ok(val) if val == "server" => gpui::WindowDecorations::Server,
        Ok(val) if val == "client" => gpui::WindowDecorations::Client,
        _ => gpui::WindowDecorations::Client, // Default to client decorations
    };

    let window_background = config.window_background_appearance(is_dark_chrome);

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
        is_resizable: true,
        is_minimizable: true,
        display_id: None,
        window_background,
        window_decorations: Some(window_decorations),
        window_min_size: Some(gpui::size(px(400.0), px(300.0))),
        icon: None,
        tabbing_identifier: None,
    }
}

fn should_create_custom_titlebar(decorations: gpui::Decorations) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let _ = decorations;
        true
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        matches!(decorations, gpui::Decorations::Client { .. })
    }
}

#[cfg(target_os = "linux")]
fn configure_wsl_graphics() {
    // WSLg currently ships a pared-down Wayland compositor that lacks the newer
    // surface APIs gpui (via Zed) requires. When both WAYLAND_DISPLAY and DISPLAY
    // exist, gpui assumes Wayland, hits UnsupportedVersion, and panics. Force the
    // compositor detection to fall back to X11 so Nucleotide can launch on WSL.
    if !is_running_in_wsl() {
        return;
    }

    let wayland_display_present = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11_display_present = std::env::var_os("DISPLAY").is_some();

    if wayland_display_present && x11_display_present {
        warn!(
            "WSL Wayland backend is unstable for gpui; forcing X11 fallback by clearing WAYLAND_DISPLAY"
        );
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }

        if std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
        {
            unsafe {
                std::env::set_var("XDG_SESSION_TYPE", "x11");
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_wsl_graphics() {}

#[cfg(target_os = "linux")]
fn is_running_in_wsl() -> bool {
    if std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
        || std::env::var_os("WSLENV").is_some()
    {
        return true;
    }

    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| {
            let release = release.to_ascii_lowercase();
            release.contains("microsoft") || release.contains("wsl")
        })
        .unwrap_or(false)
}

// Import actions from our centralized definitions
#[cfg(not(target_os = "windows"))]
use nucleotide::actions::window::{Hide, HideOthers, ShowAll};
#[cfg(target_os = "windows")]
use nucleotide::actions::workspace::{
    NewFile, NewWindow, ShowBufferPicker, ShowCodeActions, ShowCommandPrompt, ShowFileFinder,
};
use nucleotide::actions::{
    common::{Cancel, Confirm, MoveDown, MoveLeft, MoveRight, MoveUp},
    completion::{
        CompletionConfirm, CompletionDismiss, CompletionSelectFirst, CompletionSelectLast,
        CompletionSelectNext, CompletionSelectPrev, TriggerCompletion,
    },
    editor::{
        CloseFile, Copy, DecreaseFontSize, IncreaseFontSize, OpenDirectory, OpenFile, OpenSettings,
        Paste, Quit, Redo, ReloadConfiguration, RevertCurrentChange, Save, SaveAs, Undo,
    },
    help::{About, OpenTutorial, ThemeDebug},
    picker::{ConfirmSelection, DismissPicker, SelectFirst, SelectLast, TogglePreview},
    test::{TestCompletion, TestPrompt},
    window::{Minimize, Zoom},
    workspace::{
        RunFileTests, RunLast, RunNearest, ShowRunnables, SplitPaneDown, SplitPaneLeft,
        SplitPaneRight, SplitPaneUp, ToggleDocumentation, ToggleFileTree, TogglePreviewTab,
        ToggleTerminal, UnpinAllTabs,
    },
};

fn app_menus() -> Vec<Menu> {
    #[cfg(target_os = "windows")]
    {
        windows_app_menus()
    }

    #[cfg(not(target_os = "windows"))]
    {
        default_app_menus()
    }
}

#[cfg(not(target_os = "windows"))]
fn default_app_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Nucleotide".into(),
            disabled: false,
            items: vec![
                MenuItem::action("About", About),
                MenuItem::action("Settings...", OpenSettings),
                MenuItem::action("Reload Configuration", ReloadConfiguration),
                MenuItem::separator(),
                MenuItem::action("Hide Nucleotide", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Open...", OpenFile),
                MenuItem::action("Open Directory", OpenDirectory),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Revert Current Change", RevertCurrentChange),
                MenuItem::separator(),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Toggle File Tree", ToggleFileTree),
                MenuItem::action("Toggle Documentation", ToggleDocumentation),
                MenuItem::action("Toggle Terminal", ToggleTerminal),
                MenuItem::separator(),
                MenuItem::action("Split Right", SplitPaneRight),
                MenuItem::action("Split Left", SplitPaneLeft),
                MenuItem::action("Split Up", SplitPaneUp),
                MenuItem::action("Split Down", SplitPaneDown),
                MenuItem::separator(),
                MenuItem::action("Toggle Preview Tab", TogglePreviewTab),
                MenuItem::action("Unpin All Tabs", UnpinAllTabs),
            ],
        },
        Menu {
            name: "Run".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Run...", ShowRunnables),
                MenuItem::action("Run Nearest", RunNearest),
                MenuItem::action("Run File Tests", RunFileTests),
                MenuItem::separator(),
                MenuItem::action("Run Last", RunLast),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
        Menu {
            name: "Help".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Tutorial", OpenTutorial),
                MenuItem::action("Test Prompt", TestPrompt),
                MenuItem::action("Test Completion", TestCompletion),
                MenuItem::separator(),
                MenuItem::action("Theme Debug", ThemeDebug),
            ],
        },
    ]
}

#[cfg(target_os = "windows")]
fn windows_app_menus() -> Vec<Menu> {
    vec![
        Menu::new("File").items([
            MenuItem::action("New File", NewFile),
            MenuItem::action("New Window", NewWindow),
            MenuItem::separator(),
            MenuItem::action("Open File...", OpenFile),
            MenuItem::action("Open Folder...", OpenDirectory),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As...", SaveAs),
            MenuItem::action("Close File", CloseFile),
            MenuItem::separator(),
            MenuItem::action("Settings...", OpenSettings),
            MenuItem::action("Reload Configuration", ReloadConfiguration),
            MenuItem::separator(),
            MenuItem::action("Exit", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", Undo),
            MenuItem::action("Redo", Redo),
            MenuItem::separator(),
            MenuItem::action("Revert Current Change", RevertCurrentChange),
            MenuItem::separator(),
            MenuItem::action("Copy", Copy),
            MenuItem::action("Paste", Paste),
            MenuItem::separator(),
            MenuItem::action("Trigger Completion", TriggerCompletion),
            MenuItem::action("Code Actions", ShowCodeActions),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette...", ShowCommandPrompt),
            MenuItem::action("Go to File...", ShowFileFinder),
            MenuItem::action("Open Buffer...", ShowBufferPicker),
            MenuItem::separator(),
            MenuItem::action("File Tree", ToggleFileTree),
            MenuItem::action("Documentation", ToggleDocumentation),
            MenuItem::action("Terminal", ToggleTerminal),
            MenuItem::action("Preview Tab", TogglePreviewTab),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Split").items([
                MenuItem::action("Split Right", SplitPaneRight),
                MenuItem::action("Split Left", SplitPaneLeft),
                MenuItem::action("Split Up", SplitPaneUp),
                MenuItem::action("Split Down", SplitPaneDown),
            ])),
            MenuItem::separator(),
            MenuItem::action("Increase Font Size", IncreaseFontSize),
            MenuItem::action("Decrease Font Size", DecreaseFontSize),
            MenuItem::separator(),
            MenuItem::action("Unpin All Tabs", UnpinAllTabs),
        ]),
        Menu::new("Run").items([
            MenuItem::action("Run...", ShowRunnables),
            MenuItem::action("Run Nearest", RunNearest),
            MenuItem::action("Run File Tests", RunFileTests),
            MenuItem::separator(),
            MenuItem::action("Run Last", RunLast),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Maximize/Restore", Zoom),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Tutorial", OpenTutorial),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Developer").items([
                MenuItem::action("Theme Debug", ThemeDebug),
                MenuItem::separator(),
                MenuItem::action("Test Prompt", TestPrompt),
                MenuItem::action("Test Completion", TestCompletion),
            ])),
            MenuItem::separator(),
            MenuItem::action("About Nucleotide", About),
        ]),
    ]
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dock_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::action("Open...", OpenFile),
        MenuItem::action("Open Directory...", OpenDirectory),
    ]
}

// Font types are now exported from nucleotide::types
use nucleotide::{EditorFontConfig, FontSettings, UiFontConfig};

#[instrument(skip(app, config, handle))]
fn gui_main(
    mut app: Application,
    config: nucleotide::config::Config,
    handle: tokio::runtime::Handle,
    workspace_root: Option<std::path::PathBuf>,
    initial_dock_action: Option<usize>,
    platform_open_tx: tokio::sync::mpsc::UnboundedSender<ExternalOpenRequest>,
    mut platform_open_rx: tokio::sync::mpsc::UnboundedReceiver<ExternalOpenRequest>,
) {
    let gpui_app = gpui_platform::application().with_assets(nucleotide_ui::Assets);

    // Register handler for macOS file open events (dock drops and Finder "Open With")
    gpui_app.on_open_urls({
        let platform_open_tx = platform_open_tx.clone();
        move |urls| {
            info!(urls = ?urls, "Received open URLs request");

            // Parse URLs and send file paths to the main app
            let mut paths = Vec::new();
            for url in urls {
                if let Some(file_path) = parse_file_url(&url) {
                    paths.push(file_path);
                } else if let Ok(path) = PathBuf::from(&url).canonicalize()
                    && path.exists()
                {
                    // Handle direct file paths (not URLs)
                    paths.push(path);
                }
            }

            if !paths.is_empty()
                && let Err(e) = platform_open_tx.send(ExternalOpenRequest::paths(paths))
            {
                error!(error = %e, "Failed to send file open request");
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

            // Initialize Linux platform detection if on Linux
            #[cfg(target_os = "linux")]
            {
                // Force platform detection to run early and log results
                let platform_info = nucleotide_ui::titlebar::get_platform_info();
                info!(
                    "Linux platform detection completed - DE: {:?}, WM: {:?}, Layout: {:?}, CSD: {:?}",
                    platform_info.desktop_environment,
                    platform_info.window_manager,
                    platform_info.button_layout,
                    platform_info.compositor_capability
                );
            }

            // Initialize SystemAppearance global state from current window appearance
            nucleotide_appearance::SystemAppearance::init(cx);

            // Initialize the provider system
            init_provider_system();

            // Set up fonts from configuration
            let editor_font_config = config.editor_font();
            let ui_font_config = config.ui_font();

            // Set up theme manager with Helix theme
            let helix_theme = app.editor.theme.clone();
            #[allow(unused_mut)]
            let mut theme_manager =
                crate::ThemeManager::new_with_chrome_style(helix_theme, config.ui_chrome_style());
            theme_manager.set_ui_font_size(px(ui_font_config.size));

            theme_manager
                .set_system_appearance(nucleotide_appearance::SystemAppearance::global(cx));

            // Derive and install the UI theme from the ThemeManager (Helix → tokens bridge)
            let ui_theme_derived = theme_manager.ui_theme().clone();
            let is_dark_chrome = theme_manager.is_dark_chrome(); // Store before moving
            cx.set_global(theme_manager);
            cx.set_global(ui_theme_derived.clone());
            cx.set_global(nucleotide_ui::markdown::MarkdownSyntaxLoader::new(
                app.editor.syn_loader.load_full(),
            ));

            // Set up the enhanced provider system using the derived theme
            let ui_theme = ui_theme_derived;

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

            // Initialize centralized focus coordinator for input/focus management
            cx.set_global(nucleotide_ui::FocusCoordinator::default());

            // Initialize VCS service
            let vcs_config = nucleotide_vcs::VcsConfig::default();
            let vcs_service = nucleotide_vcs::VcsServiceHandle::new(vcs_config, cx);
            cx.set_global(vcs_service);

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
                line_height: editor_font_config.line_height,
            });

            // Store UI font config for UI components
            cx.set_global(UiFontConfig {
                family: ui_font_config.family,
                size: ui_font_config.size,
                weight: ui_font_config.weight,
            });

            // Initialize preview tracker
            cx.set_global(nucleotide_core::preview_tracker::PreviewTracker::new());

            if let Some(directwrite) = &config.gui.window.directwrite
                && let Err(error) =
                    cx.set_direct_write_text_rendering_params(Some(directwrite.to_gpui_params()))
            {
                warn!(error = %error, "Failed to apply DirectWrite text rendering settings");
            }

            let options = window_options(cx, &config, is_dark_chrome);

            let _ = cx.open_window(options, |#[allow(unused)] window, cx| {
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

                            cx.update(|cx| {
                                lsp_state_clone.update(cx, |state, cx| {
                                    // Update LSP indicator - shows static when idle, animated when busy
                                    let old_message = state.status_message.clone();
                                    state.status_message = state.get_lsp_indicator();

                                    // Update project status service with current LSP state
                                    if let Some(project_status) =
                                        cx.try_global::<nucleotide_project::ProjectStatusHandle>()
                                    {
                                        let project_status = project_status.clone();
                                        project_status.update_lsp_state(state);
                                    }

                                    // Only notify if there's actually a change
                                    if state.status_message != old_message {
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    })
                    .detach();
                    SpinnerTimer
                });

                let app = cx.new(move |mc| {
                    let handle_1 = handle_1.clone();
                    let _handle_2 = handle_1.clone();
                    mc.subscribe(
                        &input_1.clone(),
                        move |this: &mut Application, _, ev, cx| {
                            this.handle_input_event(ev.clone(), cx, handle_1.clone());
                        },
                    )
                    .detach();

                    // Set the LSP state
                    app.lsp_state = Some(lsp_state.clone());
                    app
                });

                // Initialize the application with its entity handle for LSP completion
                app.update(cx, |app, cx| {
                    app.post_init(cx);
                });

                // Start event/LSP maintenance after the root workspace has been
                // returned so first-window construction does less scheduling work.
                let app_weak = app.downgrade();
                let handle_for_maintenance = handle.clone();
                let (maintenance_wake, mut maintenance_wake_rx) = MaintenanceWake::channel();
                app.update(cx, |app, _cx| {
                    app.set_maintenance_wake(maintenance_wake.clone());
                });
                cx.defer(move |cx| {
                    maintenance_wake.notify();

                    let app_weak = app_weak.clone();
                    let handle_for_maintenance = handle_for_maintenance.clone();
                    let maintenance_wake = maintenance_wake.clone();
                    cx.spawn(async move |cx| {
                        while maintenance_wake_rx.recv().await.is_some() {
                            if let Some(app_entity) = app_weak.upgrade() {
                                let should_continue = app_entity.update(cx, |app, cx| {
                                    app.drive_event_driven_maintenance(
                                        cx,
                                        handle_for_maintenance.clone(),
                                        &maintenance_wake,
                                    )
                                });

                                if !should_continue {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    })
                    .detach();
                });

                // Helix/bridge maintenance is driven by source wakeups, including
                // LSP messages such as $/progress.

                nucleotide_logging::info!("Application initialized with continuous event processing");

                // Completion is handled directly through Helix's completion events.
                nucleotide_logging::info!("Using direct Helix completion integration");

                cx.activate(true);
                cx.set_menus(app_menus());

                // Set up keybindings with proper key contexts

                // Import workspace actions for global bindings
                use nucleotide::actions::workspace::{
                    NewFile, NewWindow, RunFileTests, RunLast, RunNearest, ShowBufferPicker,
                    ShowCodeActions, ShowCommandPrompt, ShowFileFinder, ShowRunnables,
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
                    gpui::KeyBinding::new("cmd-shift-p", ShowCommandPrompt, None),
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
                    // Temporary keybinding for Code Actions (Ctrl-.)
                    gpui::KeyBinding::new("ctrl-.", ShowCodeActions, None),
                    gpui::KeyBinding::new("ctrl-r", ShowRunnables, None),
                    gpui::KeyBinding::new("ctrl-shift-r", RunNearest, None),
                    gpui::KeyBinding::new("ctrl-alt-r", RunLast, None),
                    gpui::KeyBinding::new("ctrl-alt-t", RunFileTests, None),
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
                        "/",
                        nucleotide::actions::file_tree::StartSearch,
                        Some("FileTree"),
                    ),
                    gpui::KeyBinding::new(
                        "escape",
                        nucleotide::actions::file_tree::ClearSearch,
                        Some("FileTree"),
                    ),
                    gpui::KeyBinding::new(
                        "n",
                        nucleotide::actions::file_tree::SelectNextSearchMatch,
                        Some("FileTree"),
                    ),
                    gpui::KeyBinding::new(
                        "shift-n",
                        nucleotide::actions::file_tree::SelectPrevSearchMatch,
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

                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    cx.set_dock_menu(dock_menu_items());
                    info!("Configured platform dock/taskbar menu");
                }

                let input_1 = input.clone();
                // Create overlay view
                let overlay = cx.new(|cx| {
                    let view = overlay::OverlayView::new(&cx.focus_handle(), &app);
                    view.subscribe(&app, cx);
                    view
                });

                // Create notifications view with hybrid color system
                let notifications = cx.new(|_cx| notification::NotificationView::new());

                // Create info box view with hybrid color system
                let info = cx.new(|_cx| info_box::InfoBoxView::new());

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
                        workspace.set_current_project_root(Some(root.clone()), cx);
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
                nucleotide_logging::info!(
                    "Workspace created - ProjectLspManager will be initialized automatically"
                );

                // Spawn a task to handle file open requests from platform shell integrations.
                let workspace_clone = workspace.clone();
                let window_handle = window.window_handle();
                cx.spawn(async move |cx| {
                    while let Some(request) = platform_open_rx.recv().await {
                        info!(request = ?request, "Processing platform open request");

                        if let Err(error) = window_handle.update(cx, |_, window, _cx| {
                            window.activate_window();
                        }) {
                            warn!(error = %error, "Failed to activate window for platform open request");
                        }

                        if let Some(action_index) = request.dock_action {
                            cx.update(|cx| {
                                info!(action_index, "Performing forwarded dock/taskbar action");
                                cx.perform_dock_menu_action(action_index);
                            });
                        }

                        // If we have files to open, change working directory from the request
                        // or from the first file/directory.
                        let mut new_working_dir = request.working_directory.clone();

                        if new_working_dir.is_none() {
                            for file in &request.files {
                                if file.path.exists()
                                    && let Some(dir) = open_request_workspace_dir(&file.path)
                                {
                                    new_working_dir = Some(dir.clone());
                                    info!(directory = ?dir, "Will change working directory");
                                    break;
                                }
                            }
                        }

                        // Change working directory if needed
                        if let Some(dir) = new_working_dir.clone() {
                            if dir.exists() {
                                if let Err(e) = helix_stdx::env::set_current_working_dir(&dir) {
                                    error!(
                                        directory = ?dir,
                                        error = %e,
                                        "Failed to change working directory"
                                    );
                                } else {
                                    info!(directory = ?dir, "Changed working directory");

                                    // Update the core's project directory and emit OpenDirectory event
                                    cx.update(|cx| {
                                        workspace_clone.update(cx, |workspace, cx| {
                                            workspace.set_project_directory(dir.clone(), cx);
                                            info!(directory = ?dir, "Updated project directory");
                                            // Emit OpenDirectory event to update file tree
                                            cx.emit(Update::Event(
                                                nucleotide::types::AppEvent::Workspace(
                                                    nucleotide::types::WorkspaceEvent::FileSelected {
                                                        path: dir.clone(),
                                                        source:
                                                            nucleotide_events::v2::workspace::SelectionSource::Command,
                                                    },
                                                ),
                                            ));
                                        });
                                    });
                                }
                            } else {
                                warn!(directory = %dir.display(), "Forwarded working directory does not exist");
                            }
                        }

                        // Now open all files/folders in the request.
                        for file in request.files {
                            let path = file.path;
                            if path.exists() {
                                if path.is_file() {
                                    let position = file.position.into();
                                    cx.update(|cx| {
                                        workspace_clone.update(cx, |workspace, cx| {
                                            workspace.open_file_at(&path, position, cx);
                                        });
                                    });
                                } else {
                                    // Send folder selections through the workspace event path.
                                    cx.update(|cx| {
                                        workspace_clone.update(cx, |_workspace, cx| {
                                            cx.emit(Update::Event(
                                                nucleotide::types::AppEvent::Workspace(
                                                    nucleotide::types::WorkspaceEvent::FileSelected {
                                                        path: path.clone(),
                                                        source:
                                                            nucleotide_events::v2::workspace::SelectionSource::Command,
                                                    },
                                                ),
                                            ));
                                        });
                                    });
                                }
                            } else {
                                warn!(file = %path.display(), "File does not exist");
                            }
                        }
                    }
                })
                .detach();

                // Create and set titlebar after workspace is created.
                let should_create_titlebar =
                    should_create_custom_titlebar(window.window_decorations());

                if should_create_titlebar {
                    let titlebar =
                        cx.new(|cx| nucleotide_ui::titlebar::TitleBar::new("titlebar", cx));

                    workspace.update(cx, |workspace, cx| {
                        workspace.set_titlebar(titlebar);
                        cx.notify();
                    });
                }

                if let Some(action_index) = initial_dock_action {
                    cx.defer(move |cx| {
                        info!(action_index, "Performing startup dock/taskbar action");
                        cx.perform_dock_menu_action(action_index);
                    });
                }

                workspace
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_request_workspace_dir_uses_directory_itself() {
        let temp_dir = tempfile::tempdir().unwrap();

        assert_eq!(
            open_request_workspace_dir(temp_dir.path()),
            Some(temp_dir.path().to_path_buf())
        );
    }

    #[test]
    fn open_request_workspace_dir_uses_file_parent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("file.txt");
        std::fs::write(&file_path, "").unwrap();

        assert_eq!(
            open_request_workspace_dir(&file_path),
            Some(temp_dir.path().to_path_buf())
        );
    }

    #[test]
    fn determine_workspace_root_uses_remote_file_parent_without_host_probe() {
        let mut args = Args::default();
        let path = PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs");
        args.files
            .insert(path, vec![helix_core::Position::default()]);

        assert_eq!(
            determine_workspace_root(&args).unwrap(),
            Some(PathBuf::from("ssh://me@example.com/home/me/project/src"))
        );
    }

    #[test]
    fn normalize_startup_file_path_preserves_ssh_uri() {
        let path = PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs");

        assert_eq!(normalize_startup_file_path(&path), path);
    }

    #[test]
    fn normalize_startup_file_path_preserves_wsl_unc_path() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs");

        assert_eq!(normalize_startup_file_path(&path), path);
    }

    #[test]
    fn startup_host_working_directory_uses_local_workspace_root() {
        let mut args = Args::default();
        args.working_directory = Some(PathBuf::from("/ignored"));
        let workspace_root = PathBuf::from("/tmp/project");

        assert_eq!(
            startup_host_working_directory(&args, Some(&workspace_root)),
            Some(workspace_root)
        );
    }

    #[test]
    fn startup_host_working_directory_skips_ssh_workspace_root() {
        let args = Args::default();
        let workspace_root = PathBuf::from("ssh://me@example.com/home/me/project");

        assert_eq!(
            startup_host_working_directory(&args, Some(&workspace_root)),
            None
        );
    }

    #[test]
    fn startup_host_working_directory_skips_wsl_workspace_root() {
        let args = Args::default();
        let workspace_root = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project");

        assert_eq!(
            startup_host_working_directory(&args, Some(&workspace_root)),
            None
        );
    }

    #[test]
    fn startup_host_working_directory_skips_remote_explicit_working_directory() {
        let mut args = Args::default();
        args.working_directory = Some(PathBuf::from("ssh://me@example.com/home/me/project"));

        assert_eq!(startup_host_working_directory(&args, None), None);
    }

    #[test]
    fn startup_host_working_directory_uses_local_explicit_working_directory() {
        let mut args = Args::default();
        args.working_directory = Some(PathBuf::from("/tmp/project"));

        assert_eq!(
            startup_host_working_directory(&args, None),
            Some(PathBuf::from("/tmp/project"))
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn dock_menu_items_dispatch_existing_open_actions() {
        let items = dock_menu_items();
        assert_eq!(items.len(), 2);

        match &items[0] {
            MenuItem::Action { name, action, .. } => {
                assert_eq!(name.as_ref(), "Open...");
                assert!(action.partial_eq(&nucleotide::actions::editor::OpenFile));
            }
            _ => panic!("expected open file action"),
        }

        match &items[1] {
            MenuItem::Action { name, action, .. } => {
                assert_eq!(name.as_ref(), "Open Directory...");
                assert!(action.partial_eq(&nucleotide::actions::editor::OpenDirectory));
            }
            _ => panic!("expected open directory action"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_app_menus_use_windows_editor_conventions() {
        let menus = app_menus();
        let names = menus
            .iter()
            .map(|menu| menu.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(names, ["File", "Edit", "View", "Run", "Window", "Help"]);
        assert!(menus.iter().all(|menu| menu.name.as_ref() != "Nucleotide"));

        let file_menu = &menus[0];
        assert!(matches!(
            file_menu.items.last(),
            Some(MenuItem::Action { name, action, .. })
                if name.as_ref() == "Exit"
                    && action.partial_eq(&nucleotide::actions::editor::Quit)
        ));

        let help_menu = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Help")
            .expect("Help menu should exist");
        assert!(help_menu.items.iter().any(|item| matches!(
            item,
            MenuItem::Action { name, action, .. }
                if name.as_ref() == "About Nucleotide"
                    && action.partial_eq(&nucleotide::actions::help::About)
        )));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_view_menu_groups_split_commands_under_submenu() {
        let menus = app_menus();
        let view_menu = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "View")
            .expect("View menu should exist");

        let split_menu = view_menu
            .items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu(menu) if menu.name.as_ref() == "Split" => Some(menu),
                _ => None,
            })
            .expect("Split submenu should exist");

        assert_eq!(split_menu.items.len(), 4);
    }

    #[test]
    fn startup_dock_action_parser_ignores_normal_cli_args() {
        assert_eq!(
            parse_startup_dock_action(["nucl", "src/main.rs"]).unwrap(),
            None
        );
    }

    #[test]
    fn startup_dock_action_parser_accepts_jump_list_action() {
        assert_eq!(
            parse_startup_dock_action(["nucl", "--dock-action", "1"]).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn startup_dock_action_parser_rejects_malformed_action() {
        assert!(parse_startup_dock_action(["nucl", "--dock-action"]).is_err());
        assert!(parse_startup_dock_action(["nucl", "--dock-action", "abc"]).is_err());
        assert!(parse_startup_dock_action(["nucl", "--dock-action", "0", "extra"]).is_err());
    }

    #[test]
    fn nucleotide_url_parser_accepts_focus_only_open() {
        let request = parse_nucleotide_url("nucleotide://open").unwrap();

        assert!(request.files.is_empty());
        assert_eq!(request.working_directory, None);
    }

    #[test]
    fn nucleotide_url_parser_rejects_other_schemes_and_actions() {
        assert!(parse_nucleotide_url("https://example.com").is_none());
        assert!(parse_nucleotide_url("nucleotide://settings").is_none());
    }

    #[test]
    fn nucleotide_url_parser_decodes_windows_paths() {
        let request = parse_nucleotide_url(
            "nucleotide://open?path=C%3A%5CUsers%5CIain%5Cproject&dir=C%3A%5CUsers%5CIain",
        )
        .unwrap();

        assert_eq!(
            request.files,
            vec![ProtocolOpenFile {
                path: PathBuf::from(r"C:\Users\Iain\project"),
                position: helix_core::Position::default(),
            }]
        );
        assert_eq!(
            request.working_directory,
            Some(PathBuf::from(r"C:\Users\Iain"))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn nucleotide_url_parser_accepts_file_urls() {
        let request = parse_nucleotide_url(
            "nucleotide://open?url=file%3A%2F%2F%2FC%3A%2FUsers%2FIain%2Fproject%2Fmain.rs",
        )
        .unwrap();

        assert_eq!(
            request.files,
            vec![ProtocolOpenFile {
                path: PathBuf::from(r"C:\Users\Iain\project\main.rs"),
                position: helix_core::Position::default(),
            }]
        );
    }

    #[test]
    fn nucleotide_url_parser_accepts_one_based_line_and_column() {
        let request = parse_nucleotide_url(
            "nucleotide://open?path=C%3A%5CUsers%5CIain%5Cproject%5Cmain.rs&line=42&column=7",
        )
        .unwrap();

        assert_eq!(
            request.files,
            vec![ProtocolOpenFile {
                path: PathBuf::from(r"C:\Users\Iain\project\main.rs"),
                position: helix_core::Position::new(41, 6),
            }]
        );
    }

    #[test]
    fn startup_protocol_parser_accepts_single_nucleotide_url() {
        let request = parse_startup_protocol_request(["nucl", "nucleotide://open"]).unwrap();

        assert_eq!(
            request,
            Some(ProtocolOpenRequest {
                files: Vec::new(),
                working_directory: None,
            })
        );
    }

    #[test]
    fn startup_protocol_request_applies_positions_to_args() {
        let request = parse_startup_protocol_request([
            "nucl",
            "nucleotide://open?path=C%3A%5CUsers%5CIain%5Cproject%5Cmain.rs&line=3&column=9",
        ])
        .unwrap()
        .unwrap();
        let mut args = Args::default();

        apply_protocol_request_to_args(&mut args, request);

        assert_eq!(
            args.files
                .get(&PathBuf::from(r"C:\Users\Iain\project\main.rs"))
                .cloned(),
            Some(vec![helix_core::Position::new(2, 8)])
        );
    }

    #[test]
    fn startup_protocol_parser_ignores_normal_cli_args() {
        assert_eq!(
            parse_startup_protocol_request(["nucl", "src/main.rs"]).unwrap(),
            None
        );
    }

    #[test]
    fn startup_protocol_parser_rejects_combined_args() {
        assert!(
            parse_startup_protocol_request(["nucl", "nucleotide://open", "src/main.rs"]).is_err()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_app_user_model_id_uses_bundle_identifier() {
        assert_eq!(WINDOWS_APP_USER_MODEL_ID, "org.spiralpoint.nucleotide");
        assert!(WINDOWS_APP_USER_MODEL_ID.contains('.'));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_wide_nul_is_nul_terminated() {
        let value = windows_wide_nul(WINDOWS_APP_USER_MODEL_ID);

        assert_eq!(value.last().copied(), Some(0));
        assert_eq!(value.iter().filter(|&&ch| ch == 0).count(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn custom_titlebar_is_created_on_windows_even_with_server_decorations() {
        assert!(should_create_custom_titlebar(gpui::Decorations::Server));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn custom_titlebar_requires_client_decorations_on_other_platforms() {
        assert!(!should_create_custom_titlebar(gpui::Decorations::Server));
        assert!(should_create_custom_titlebar(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        }));
    }

    #[test]
    fn view_menu_exposes_documentation_and_terminal_toggles() {
        let menus = app_menus();
        let view_menu = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "View")
            .expect("view menu should exist");

        let actions = view_menu
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, action, .. } => Some((name.as_ref(), action)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            actions.iter().any(|(name, action)| {
                *name == "Toggle Documentation"
                    && action.partial_eq(&nucleotide::actions::workspace::ToggleDocumentation)
            }),
            "View menu should expose Toggle Documentation"
        );
        assert!(
            actions.iter().any(|(name, action)| {
                *name == "Toggle Terminal"
                    && action.partial_eq(&nucleotide::actions::workspace::ToggleTerminal)
            }),
            "View menu should expose Toggle Terminal"
        );
    }

    #[test]
    fn edit_menu_exposes_revert_current_change() {
        let menus = app_menus();
        let edit_menu = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Edit")
            .expect("edit menu should exist");

        let has_revert = edit_menu.items.iter().any(|item| match item {
            MenuItem::Action { name, action, .. } => {
                name.as_ref() == "Revert Current Change"
                    && action.partial_eq(&nucleotide::actions::editor::RevertCurrentChange)
            }
            _ => false,
        });

        assert!(has_revert, "Edit menu should expose Revert Current Change");
    }
}
