// ABOUTME: This file implements the GUI-specific configuration system for nucleotide
// ABOUTME: It loads nucleotide.toml and falls back to config.toml for unspecified values

use helix_loader::config_dir;
use helix_term::config::Config as HelixConfig;
use nucleotide_types::{FontConfig, FontWeight, ProjectMarkersConfig};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Default theme for light mode
pub const DEFAULT_LIGHT_THEME: &str = "nucleotide-outdoors";

/// Default theme for dark mode
pub const DEFAULT_DARK_THEME: &str = "nucleotide-teal";

/// UI-specific configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    /// Font used for UI elements (menus, dialogs, etc.)
    #[serde(default)]
    pub font: Option<FontConfig>,
}

/// Editor-specific GUI configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorGuiConfig {
    /// Font used in the editor
    #[serde(default)]
    pub font: Option<FontConfig>,
}

/// Theme mode selection
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow system appearance
    #[default]
    System,
    /// Always use light theme
    Light,
    /// Always use dark theme
    Dark,
}

/// Theme configuration for automatic switching
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Theme mode selection
    #[serde(default)]
    pub mode: ThemeMode,

    /// Theme to use in light mode (defaults to "nucleotide-outdoors" if not specified)
    #[serde(default)]
    pub light_theme: Option<String>,

    /// Theme to use in dark mode (defaults to "nucleotide-teal" if not specified)
    #[serde(default)]
    pub dark_theme: Option<String>,
}

impl ThemeConfig {
    /// Get the light theme name with default fallback
    pub fn get_light_theme(&self) -> String {
        self.light_theme
            .clone()
            .unwrap_or_else(|| DEFAULT_LIGHT_THEME.to_string())
    }

    /// Get the dark theme name with default fallback
    pub fn get_dark_theme(&self) -> String {
        self.dark_theme
            .clone()
            .unwrap_or_else(|| DEFAULT_DARK_THEME.to_string())
    }
}

/// Window appearance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Enable blur for dark themes
    #[serde(default)]
    pub blur_dark_themes: bool,

    /// Automatically adjust window appearance based on theme
    #[serde(default = "default_true")]
    pub appearance_follows_theme: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            blur_dark_themes: false,
            appearance_follows_theme: true,
        }
    }
}

/// LSP feature flags configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspConfig {
    /// Enable project-based LSP startup (vs file-based)
    #[serde(default)]
    pub project_lsp_startup: bool,

    /// Timeout for LSP startup in milliseconds
    #[serde(default = "default_lsp_startup_timeout")]
    pub startup_timeout_ms: u64,

    /// Enable graceful fallback to file-based startup on project detection failures
    #[serde(default = "default_true")]
    pub enable_fallback: bool,
}

fn default_lsp_startup_timeout() -> u64 {
    5000 // 5 seconds default timeout
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            project_lsp_startup: false,
            startup_timeout_ms: default_lsp_startup_timeout(),
            enable_fallback: true,
        }
    }
}

impl LspConfig {
    /// Validate the LSP configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate timeout is reasonable
        if self.startup_timeout_ms == 0 {
            return Err("LSP startup timeout must be greater than 0".to_string());
        }

        if self.startup_timeout_ms > 60000 {
            return Err("LSP startup timeout should not exceed 60 seconds".to_string());
        }

        // Log warnings for potentially problematic configurations
        if self.startup_timeout_ms < 1000 {
            nucleotide_logging::warn!(
                timeout_ms = self.startup_timeout_ms,
                "LSP startup timeout is very low - may cause frequent failures"
            );
        }

        if self.project_lsp_startup && !self.enable_fallback {
            nucleotide_logging::warn!(
                "Project LSP startup enabled without fallback - may cause LSP failures in non-project contexts"
            );
        }

        Ok(())
    }

    /// Get a sanitized version of the config with valid values
    pub fn sanitized(&self) -> Self {
        let mut config = self.clone();

        // Ensure timeout is within reasonable bounds
        if config.startup_timeout_ms == 0 {
            nucleotide_logging::warn!("Invalid LSP timeout 0, using default 5000ms");
            config.startup_timeout_ms = 5000;
        } else if config.startup_timeout_ms > 60000 {
            nucleotide_logging::warn!(
                original_timeout = config.startup_timeout_ms,
                "LSP timeout too high, capping at 60 seconds"
            );
            config.startup_timeout_ms = 60000;
        }

        config
    }
}

fn default_true() -> bool {
    true
}

/// GUI-specific configuration that extends Helix configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    /// UI-specific settings
    #[serde(default)]
    pub ui: UiConfig,

    /// Editor GUI settings
    #[serde(default)]
    pub editor: EditorGuiConfig,

    /// Theme configuration
    #[serde(default)]
    pub theme: ThemeConfig,

    /// Window appearance configuration
    #[serde(default)]
    pub window: WindowConfig,

    /// LSP feature flags and configuration
    #[serde(default)]
    pub lsp: LspConfig,

    /// Project markers configuration for custom project detection
    #[serde(default)]
    pub project_markers: ProjectMarkersConfig,

    /// File operations behavior
    #[serde(default)]
    pub file_ops: FileOpsConfig,
}

/// Delete behavior preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DeleteBehavior {
    #[default]
    Trash,
    Permanent,
}

/// File operations configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfig {
    /// How delete should behave: move to trash or delete permanently
    #[serde(default)]
    pub delete_behavior: DeleteBehavior,
}

impl Default for FileOpsConfig {
    fn default() -> Self {
        Self {
            delete_behavior: DeleteBehavior::Trash,
        }
    }
}

/// Combined configuration merging GUI and Helix configs
#[derive(Debug, Clone)]
pub struct Config {
    /// Current Helix configuration (includes both file config and runtime changes)
    pub helix: HelixConfig,

    /// GUI-specific configuration
    pub gui: GuiConfig,
}

impl Config {
    /// Load configuration from the standard locations
    pub fn load() -> anyhow::Result<Self> {
        let config_dir = config_dir();
        Self::load_from_dir(&config_dir)
    }

    /// Load configuration from a specific directory
    pub fn load_from_dir(dir: &Path) -> anyhow::Result<Self> {
        // First, load the base Helix configuration
        let mut helix_config = load_helix_config(dir)?;

        // Then, load GUI-specific configuration if it exists
        let gui_config = load_gui_config(dir).unwrap_or_default();

        // Enable recommended diagnostics rendering by default when user has not configured it.
        // Matches Helix book guidance: end-of-line diagnostics = "hint" and inline cursor-line = "warning".
        {
            use helix_core::diagnostic::Severity;
            use helix_view::annotations::diagnostics::DiagnosticFilter;

            let editor_cfg = &mut helix_config.editor;
            let inline = &mut editor_cfg.inline_diagnostics;

            let no_inline_configured = matches!(inline.cursor_line, DiagnosticFilter::Disable)
                && matches!(inline.other_lines, DiagnosticFilter::Disable);
            let no_eol_configured = matches!(
                editor_cfg.end_of_line_diagnostics,
                DiagnosticFilter::Disable
            );

            if no_inline_configured && no_eol_configured {
                editor_cfg.end_of_line_diagnostics = DiagnosticFilter::Enable(Severity::Hint);
                inline.cursor_line = DiagnosticFilter::Enable(Severity::Warning);
                // Keep other_lines disabled unless user opts in via helix config
            }
        }

        Ok(Self {
            helix: helix_config,
            gui: gui_config,
        })
    }

    /// Apply a config update from Helix (e.g., from toggle command)
    /// We don't need to know what the config keys mean - we just store the new config
    pub fn apply_helix_config_update(&mut self, new_editor_config: &helix_view::editor::Config) {
        self.helix.editor = new_editor_config.clone();
    }

    /// Get the current Helix config
    pub fn to_helix_config(&self) -> HelixConfig {
        self.helix.clone()
    }

    /// Get the editor font configuration
    pub fn editor_font(&self) -> FontConfig {
        self.gui.editor.font.clone().unwrap_or_else(|| {
            // Fall back to UI font if specified
            self.gui.ui.font.clone().unwrap_or_default()
        })
    }

    /// Get the UI font configuration
    pub fn ui_font(&self) -> FontConfig {
        self.gui.ui.font.clone().unwrap_or_else(|| {
            // Default UI font
            FontConfig {
                family: "SF Pro Display".to_string(),
                weight: FontWeight::Normal,
                size: 13.0,
                line_height: 1.5,
            }
        })
    }

    /// Check if project-based LSP startup is enabled
    pub fn is_project_lsp_startup_enabled(&self) -> bool {
        self.gui.lsp.project_lsp_startup
    }

    /// Get LSP startup timeout in milliseconds
    pub fn lsp_startup_timeout_ms(&self) -> u64 {
        self.gui.lsp.startup_timeout_ms
    }

    /// Check if fallback to file-based LSP startup is enabled
    pub fn is_lsp_fallback_enabled(&self) -> bool {
        self.gui.lsp.enable_fallback
    }

    /// Check if project markers are enabled
    pub fn is_project_markers_enabled(&self) -> bool {
        self.gui.project_markers.enable_project_markers
    }

    /// Get project detection timeout in milliseconds
    pub fn project_detection_timeout_ms(&self) -> u64 {
        self.gui.project_markers.detection_timeout_ms
    }

    /// Check if builtin fallback for project detection is enabled
    pub fn is_project_builtin_fallback_enabled(&self) -> bool {
        self.gui.project_markers.enable_builtin_fallback
    }

    /// Get the project markers configuration
    pub fn project_markers(&self) -> &ProjectMarkersConfig {
        &self.gui.project_markers
    }
}

/// Load Helix configuration from config.toml
fn load_helix_config(_dir: &Path) -> anyhow::Result<HelixConfig> {
    use helix_term::config::{Config, ConfigLoadError};

    match Config::load_default() {
        Ok(config) => Ok(config),
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(Config::default())
        }
        Err(ConfigLoadError::Error(err)) => Err(err.into()),
        Err(ConfigLoadError::BadConfig(err)) => Err(err.into()),
    }
}

/// Load GUI configuration from nucleotide.toml
fn load_gui_config(dir: &Path) -> anyhow::Result<GuiConfig> {
    let gui_config_path = dir.join("nucleotide.toml");

    nucleotide_logging::info!(
        config_dir = %dir.display(),
        config_path = %gui_config_path.display(),
        config_exists = gui_config_path.exists(),
        "Loading GUI configuration"
    );

    let mut config = if gui_config_path.exists() {
        let config_str = std::fs::read_to_string(&gui_config_path)?;
        let config: GuiConfig = toml::from_str(&config_str)?;

        nucleotide_logging::info!(
            theme_mode = ?config.theme.mode,
            light_theme = %config.theme.get_light_theme(),
            dark_theme = %config.theme.get_dark_theme(),
            "Loaded base GUI configuration"
        );
        config
    } else {
        nucleotide_logging::info!("No GUI configuration file found, using defaults");
        GuiConfig::default()
    };

    // Load project markers configuration from project_markers.toml
    match load_project_markers_config(dir) {
        Ok(project_markers_config) => {
            config.project_markers = project_markers_config;
        }
        Err(err) => {
            nucleotide_logging::warn!(
                error = %err,
                "Failed to load project markers configuration, using defaults"
            );
            config.project_markers = ProjectMarkersConfig::default();
        }
    }

    // Validate and sanitize LSP configuration
    if let Err(validation_error) = config.lsp.validate() {
        nucleotide_logging::error!(
            config_path = %gui_config_path.display(),
            error = %validation_error,
            "Invalid LSP configuration - using sanitized values"
        );
        config.lsp = config.lsp.sanitized();
    } else {
        nucleotide_logging::info!("LSP configuration validation passed");
    }

    // Validate and sanitize project markers configuration
    if let Err(validation_error) = config.project_markers.validate() {
        nucleotide_logging::error!(
            error = %validation_error,
            "Invalid project markers configuration - using sanitized values"
        );
        config.project_markers = config.project_markers.sanitized();
    } else {
        nucleotide_logging::info!("Project markers configuration validation passed");
    }

    nucleotide_logging::info!(
        project_lsp_startup = config.lsp.project_lsp_startup,
        lsp_startup_timeout_ms = config.lsp.startup_timeout_ms,
        lsp_fallback_enabled = config.lsp.enable_fallback,
        project_markers_enabled = config.project_markers.enable_project_markers,
        project_detection_timeout_ms = config.project_markers.detection_timeout_ms,
        builtin_fallback_enabled = config.project_markers.enable_builtin_fallback,
        project_markers_count = config.project_markers.markers.len(),
        "Loaded and validated complete GUI configuration"
    );

    Ok(config)
}

/// Load project markers configuration from project_markers.toml
fn load_project_markers_config(dir: &Path) -> anyhow::Result<ProjectMarkersConfig> {
    let markers_config_path = dir.join("project_markers.toml");

    nucleotide_logging::info!(
        config_dir = %dir.display(),
        config_path = %markers_config_path.display(),
        config_exists = markers_config_path.exists(),
        "Loading project markers configuration"
    );

    if markers_config_path.exists() {
        let config_str = std::fs::read_to_string(&markers_config_path)?;
        let config: ProjectMarkersConfig = toml::from_str(&config_str)?;

        nucleotide_logging::info!(
            markers_count = config.markers.len(),
            enabled = config.enable_project_markers,
            language_servers = ?config.get_language_servers(),
            "Loaded project markers configuration"
        );

        Ok(config)
    } else {
        nucleotide_logging::info!("No project markers configuration file found, using defaults");
        // Return default project markers configuration if file doesn't exist
        Ok(ProjectMarkersConfig::default())
    }
}

/// Example nucleotide.toml configuration:
/// ```toml
/// [ui]
/// [ui.font]
/// family = "SF Pro Display"
/// weight = "normal"
/// size = 13.0
///
/// [editor]
/// [editor.font]
/// family = "SF Mono"
/// weight = "medium"
/// size = 14.0
///
/// [lsp]
/// project_lsp_startup = true
/// startup_timeout_ms = 5000
/// enable_fallback = true
///
/// [project_markers]
/// enable_project_markers = true
/// detection_timeout_ms = 1000
/// enable_builtin_fallback = true
/// ```
///
/// Example project_markers.toml configuration:
/// ```toml
/// enable_project_markers = true
/// detection_timeout_ms = 1000
/// enable_builtin_fallback = true
///
/// [markers.rust_web]
/// markers = ["Cargo.toml", "leptos.toml"]
/// language_server = "rust-analyzer"
/// root_strategy = "closest"
/// priority = 80
///
/// [markers.node_typescript]
/// markers = ["package.json", "tsconfig.json"]
/// language_server = "typescript-language-server"
/// root_strategy = "first"
/// priority = 70
///
/// [markers.python_django]
/// markers = ["manage.py", "pyproject.toml"]
/// language_server = "pyright"
/// root_strategy = "furthest"
/// priority = 60
/// ```
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use nucleotide_types::{ProjectMarker, RootStrategy};

    #[test]
    fn test_font_weight_serialization() {
        // Test deserialization from JSON (since TOML doesn't support bare enum values)
        let deserialized: FontWeight =
            serde_json::from_str("\"semibold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::SemiBold);

        let deserialized: FontWeight =
            serde_json::from_str("\"bold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::Bold);

        // Test that FontWeight converts correctly to gpui::FontWeight
        assert_eq!(
            gpui::FontWeight::from(FontWeight::Bold),
            gpui::FontWeight::BOLD
        );
        assert_eq!(
            gpui::FontWeight::from(FontWeight::Normal),
            gpui::FontWeight::NORMAL
        );
    }

    #[test]
    fn test_gui_config_parsing() {
        let config_str = r#"
[ui.font]
family = "Inter"
weight = "medium"
size = 13.0

[editor.font]
family = "JetBrains Mono"
weight = "normal"
size = 14.5
"#;

        let config: GuiConfig = toml::from_str(config_str).expect("Failed to parse GuiConfig");

        let ui_font = config.ui.font.as_ref().expect("UI font should be set");
        assert_eq!(ui_font.family, "Inter");
        assert_eq!(ui_font.weight, FontWeight::Medium);
        assert_eq!(ui_font.size, 13.0);

        let editor_font = config
            .editor
            .font
            .as_ref()
            .expect("Editor font should be set");
        assert_eq!(editor_font.family, "JetBrains Mono");
        assert_eq!(editor_font.weight, FontWeight::Normal);
        assert_eq!(editor_font.size, 14.5);
    }

    #[test]
    fn test_lsp_config_parsing() {
        let config_str = r#"
[lsp]
project_lsp_startup = true
startup_timeout_ms = 3000
enable_fallback = false
"#;

        let config: GuiConfig = toml::from_str(config_str).expect("Failed to parse LSP config");

        assert_eq!(config.lsp.project_lsp_startup, true);
        assert_eq!(config.lsp.startup_timeout_ms, 3000);
        assert_eq!(config.lsp.enable_fallback, false);
    }

    #[test]
    fn test_lsp_config_defaults() {
        let config = GuiConfig::default();

        // Test default values
        assert_eq!(config.lsp.project_lsp_startup, false);
        assert_eq!(config.lsp.startup_timeout_ms, 5000);
        assert_eq!(config.lsp.enable_fallback, true);
    }

    #[test]
    fn test_config_convenience_methods() {
        let mut gui_config = GuiConfig::default();
        gui_config.lsp.project_lsp_startup = true;
        gui_config.lsp.startup_timeout_ms = 2000;
        gui_config.lsp.enable_fallback = false;

        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        assert_eq!(config.is_project_lsp_startup_enabled(), true);
        assert_eq!(config.lsp_startup_timeout_ms(), 2000);
        assert_eq!(config.is_lsp_fallback_enabled(), false);
    }

    #[test]
    fn test_full_feature_flag_integration() {
        // Test complete feature flag configuration example
        let config_str = r#"
[ui.font]
family = "Inter"
size = 13.0

[editor.font]
family = "JetBrains Mono"
size = 14.0

[lsp]
project_lsp_startup = true
startup_timeout_ms = 3000
enable_fallback = true

[theme]
mode = "dark"
dark_theme = "monokai"
"#;

        let gui_config: GuiConfig =
            toml::from_str(config_str).expect("Failed to parse feature flag config");

        // Validate LSP configuration
        assert!(gui_config.lsp.validate().is_ok());

        // Test all LSP feature flag values
        assert_eq!(gui_config.lsp.project_lsp_startup, true);
        assert_eq!(gui_config.lsp.startup_timeout_ms, 3000);
        assert_eq!(gui_config.lsp.enable_fallback, true);

        // Test convenience methods via full config
        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        assert_eq!(config.is_project_lsp_startup_enabled(), true);
        assert_eq!(config.lsp_startup_timeout_ms(), 3000);
        assert_eq!(config.is_lsp_fallback_enabled(), true);
    }

    #[test]
    fn test_invalid_lsp_configuration_validation() {
        // Test invalid timeout (0)
        let mut config = LspConfig::default();
        config.startup_timeout_ms = 0;
        assert!(config.validate().is_err());

        // Test invalid timeout (too high)
        config.startup_timeout_ms = 100000;
        assert!(config.validate().is_err());

        // Test valid configuration
        config.startup_timeout_ms = 5000;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_lsp_config_sanitization() {
        // Test sanitizing invalid timeout (0)
        let mut config = LspConfig::default();
        config.startup_timeout_ms = 0;
        let sanitized = config.sanitized();
        assert_eq!(sanitized.startup_timeout_ms, 5000);

        // Test sanitizing too high timeout
        config.startup_timeout_ms = 100000;
        let sanitized = config.sanitized();
        assert_eq!(sanitized.startup_timeout_ms, 60000);

        // Test valid configuration remains unchanged
        config.startup_timeout_ms = 3000;
        let sanitized = config.sanitized();
        assert_eq!(sanitized.startup_timeout_ms, 3000);
    }

    #[test]
    fn test_project_marker_parsing() {
        let config_str = r#"
markers = ["Cargo.toml", "leptos.toml"]
language_server = "rust-analyzer"
root_strategy = "closest"
priority = 80
"#;

        let marker: ProjectMarker =
            toml::from_str(config_str).expect("Failed to parse ProjectMarker");

        assert_eq!(marker.markers, vec!["Cargo.toml", "leptos.toml"]);
        assert_eq!(marker.language_server, "rust-analyzer");
        assert_eq!(marker.root_strategy, RootStrategy::Closest);
        assert_eq!(marker.priority, 80);
    }

    #[test]
    fn test_project_marker_defaults() {
        let config_str = r#"
markers = ["package.json"]
language_server = "typescript-language-server"
"#;

        let marker: ProjectMarker =
            toml::from_str(config_str).expect("Failed to parse ProjectMarker");

        assert_eq!(marker.markers, vec!["package.json"]);
        assert_eq!(marker.language_server, "typescript-language-server");
        assert_eq!(marker.root_strategy, RootStrategy::Closest); // default
        assert_eq!(marker.priority, 50); // default
    }

    #[test]
    fn test_root_strategy_serialization() {
        assert_eq!(RootStrategy::default(), RootStrategy::Closest);

        // Test deserialization from TOML within a structure
        #[derive(Deserialize)]
        struct TestStruct {
            strategy: RootStrategy,
        }

        let first: TestStruct = toml::from_str("strategy = \"first\"")
            .expect("Failed to deserialize RootStrategy::First");
        assert_eq!(first.strategy, RootStrategy::First);

        let closest: TestStruct = toml::from_str("strategy = \"closest\"")
            .expect("Failed to deserialize RootStrategy::Closest");
        assert_eq!(closest.strategy, RootStrategy::Closest);

        let furthest: TestStruct = toml::from_str("strategy = \"furthest\"")
            .expect("Failed to deserialize RootStrategy::Furthest");
        assert_eq!(furthest.strategy, RootStrategy::Furthest);
    }

    #[test]
    fn test_project_markers_config_parsing() {
        let config_str = r#"
enable_project_markers = true
detection_timeout_ms = 2000
enable_builtin_fallback = false

[markers.rust_web]
markers = ["Cargo.toml", "leptos.toml"]
language_server = "rust-analyzer"
root_strategy = "closest"
priority = 80

[markers.node_typescript]
markers = ["package.json", "tsconfig.json"]
language_server = "typescript-language-server"
root_strategy = "first"
priority = 70
"#;

        let config: ProjectMarkersConfig =
            toml::from_str(config_str).expect("Failed to parse ProjectMarkersConfig");

        assert_eq!(config.enable_project_markers, true);
        assert_eq!(config.detection_timeout_ms, 2000);
        assert_eq!(config.enable_builtin_fallback, false);
        assert_eq!(config.markers.len(), 2);

        let rust_web = config
            .markers
            .get("rust_web")
            .expect("rust_web marker should exist");
        assert_eq!(rust_web.markers, vec!["Cargo.toml", "leptos.toml"]);
        assert_eq!(rust_web.language_server, "rust-analyzer");
        assert_eq!(rust_web.priority, 80);

        let node_ts = config
            .markers
            .get("node_typescript")
            .expect("node_typescript marker should exist");
        assert_eq!(node_ts.markers, vec!["package.json", "tsconfig.json"]);
        assert_eq!(node_ts.language_server, "typescript-language-server");
        assert_eq!(node_ts.priority, 70);
    }

    #[test]
    fn test_project_markers_config_defaults() {
        let config = ProjectMarkersConfig::default();

        assert_eq!(config.enable_project_markers, false);
        assert_eq!(config.detection_timeout_ms, 1000);
        assert_eq!(config.enable_builtin_fallback, true);
        assert!(config.markers.is_empty());
    }

    #[test]
    fn test_project_marker_validation() {
        // Valid marker
        let valid_marker = ProjectMarker {
            markers: vec!["Cargo.toml".to_string()],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(valid_marker.validate().is_ok());

        // Invalid - empty markers
        let invalid_marker = ProjectMarker {
            markers: vec![],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());

        // Invalid - empty marker pattern
        let invalid_marker = ProjectMarker {
            markers: vec!["".to_string()],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());

        // Invalid - empty language server
        let invalid_marker = ProjectMarker {
            markers: vec!["Cargo.toml".to_string()],
            language_server: "".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());

        // Invalid - marker with path separator
        let invalid_marker = ProjectMarker {
            markers: vec!["src/main.rs".to_string()],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());
    }

    #[test]
    fn test_project_markers_config_validation() {
        // Valid config
        let mut valid_config = ProjectMarkersConfig::default();
        valid_config.enable_project_markers = true;
        valid_config.detection_timeout_ms = 1000;
        valid_config.markers.insert(
            "rust".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 50,
            },
        );
        assert!(valid_config.validate().is_ok());

        // Invalid - zero timeout
        let mut invalid_config = ProjectMarkersConfig::default();
        invalid_config.detection_timeout_ms = 0;
        assert!(invalid_config.validate().is_err());

        // Invalid - timeout too high
        let mut invalid_config = ProjectMarkersConfig::default();
        invalid_config.detection_timeout_ms = 50000;
        assert!(invalid_config.validate().is_err());

        // Invalid - empty project name
        let mut invalid_config = ProjectMarkersConfig::default();
        invalid_config.markers.insert(
            "".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 50,
            },
        );
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_project_marker_sanitization() {
        // Test sanitizing empty markers
        let mut marker = ProjectMarker {
            markers: vec!["".to_string(), "Cargo.toml".to_string(), "  ".to_string()],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        let sanitized = marker.sanitized();
        assert_eq!(sanitized.markers, vec!["Cargo.toml"]);

        // Test sanitizing completely empty markers list
        marker.markers = vec![];
        let sanitized = marker.sanitized();
        assert_eq!(sanitized.markers, vec![".project"]);

        // Test sanitizing empty language server
        marker.language_server = "  ".to_string();
        let sanitized = marker.sanitized();
        assert_eq!(sanitized.language_server, "unknown");

        // Test sanitizing high priority
        marker.priority = 2000;
        let sanitized = marker.sanitized();
        assert_eq!(sanitized.priority, 1000);
    }

    #[test]
    fn test_project_markers_config_sanitization() {
        // Test sanitizing invalid timeout
        let mut config = ProjectMarkersConfig::default();
        config.detection_timeout_ms = 0;
        let sanitized = config.sanitized();
        assert_eq!(sanitized.detection_timeout_ms, 1000);

        // Test sanitizing timeout too high
        config.detection_timeout_ms = 50000;
        let sanitized = config.sanitized();
        assert_eq!(sanitized.detection_timeout_ms, 30000);

        // Test sanitizing empty project name
        config.markers.insert(
            "".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 50,
            },
        );
        config.markers.insert(
            "valid_name".to_string(),
            ProjectMarker {
                markers: vec!["package.json".to_string()],
                language_server: "typescript-language-server".to_string(),
                root_strategy: RootStrategy::First,
                priority: 60,
            },
        );
        let sanitized = config.sanitized();
        assert!(!sanitized.markers.contains_key(""));
        assert!(sanitized.markers.contains_key("valid_name"));
        assert_eq!(sanitized.markers.len(), 1);
    }

    #[test]
    fn test_project_markers_config_utility_methods() {
        let mut config = ProjectMarkersConfig::default();
        config.markers.insert(
            "rust".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 80,
            },
        );
        config.markers.insert(
            "typescript".to_string(),
            ProjectMarker {
                markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
                language_server: "typescript-language-server".to_string(),
                root_strategy: RootStrategy::First,
                priority: 70,
            },
        );
        config.markers.insert(
            "python".to_string(),
            ProjectMarker {
                markers: vec!["pyproject.toml".to_string()],
                language_server: "pyright".to_string(),
                root_strategy: RootStrategy::Furthest,
                priority: 60,
            },
        );

        // Test get_language_servers
        let servers = config.get_language_servers();
        assert_eq!(servers.len(), 3);
        assert!(servers.contains(&"rust-analyzer".to_string()));
        assert!(servers.contains(&"typescript-language-server".to_string()));
        assert!(servers.contains(&"pyright".to_string()));

        // Test get_markers_for_server
        let rust_markers = config.get_markers_for_server("rust-analyzer");
        assert_eq!(rust_markers.len(), 1);
        assert_eq!(rust_markers[0].0, "rust");

        // Test find_project_by_marker
        let cargo_projects = config.find_project_by_marker("Cargo.toml");
        assert_eq!(cargo_projects.len(), 1);
        assert_eq!(cargo_projects[0].0, "rust");

        let package_projects = config.find_project_by_marker("package.json");
        assert_eq!(package_projects.len(), 1);
        assert_eq!(package_projects[0].0, "typescript");
    }

    #[test]
    fn test_config_project_markers_convenience_methods() {
        let mut gui_config = GuiConfig::default();
        gui_config.project_markers.enable_project_markers = true;
        gui_config.project_markers.detection_timeout_ms = 2000;
        gui_config.project_markers.enable_builtin_fallback = false;

        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        assert_eq!(config.is_project_markers_enabled(), true);
        assert_eq!(config.project_detection_timeout_ms(), 2000);
        assert_eq!(config.is_project_builtin_fallback_enabled(), false);
        assert!(config.project_markers().markers.is_empty());
    }

    #[test]
    fn test_project_markers_integration_with_gui_config() {
        let config_str = r#"
[ui.font]
family = "Inter"
size = 13.0

[project_markers]
enable_project_markers = true
detection_timeout_ms = 1500
enable_builtin_fallback = true

[lsp]
project_lsp_startup = true
startup_timeout_ms = 3000
"#;

        let config: GuiConfig =
            toml::from_str(config_str).expect("Failed to parse GUI config with project markers");

        // Test that project markers are properly integrated
        assert_eq!(config.project_markers.enable_project_markers, true);
        assert_eq!(config.project_markers.detection_timeout_ms, 1500);
        assert_eq!(config.project_markers.enable_builtin_fallback, true);

        // Test that other configs still work
        assert_eq!(config.lsp.project_lsp_startup, true);
        assert_eq!(config.lsp.startup_timeout_ms, 3000);

        let ui_font = config.ui.font.as_ref().expect("UI font should be set");
        assert_eq!(ui_font.family, "Inter");
        assert_eq!(ui_font.size, 13.0);
    }
}
