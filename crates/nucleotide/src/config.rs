// ABOUTME: This file implements the GUI-specific configuration system for nucleotide
// ABOUTME: It loads nucleotide.toml and falls back to config.toml for unspecified values

use crate::file_tree::FileTreeDisplayDensity;
use helix_loader::config_dir;
use helix_term::config::Config as HelixConfig;
use nucleotide_appearance::UiChromeStyle;
use nucleotide_types::{FontConfig, FontWeight, ProjectMarkersConfig};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

/// Default theme for light mode
pub const DEFAULT_LIGHT_THEME: &str = "nucleotide-cyan-light";

/// Default theme for dark mode
pub const DEFAULT_DARK_THEME: &str = "nucleotide-teal";

const GPUI_SYSTEM_UI_FONT: &str = ".SystemUIFont";
const DEFAULT_UI_FONT_LINE_HEIGHT: f32 = 1.5;

#[cfg(target_os = "windows")]
const FALLBACK_PLATFORM_UI_FONT_SIZE: f32 = 13.0;

#[cfg(not(target_os = "windows"))]
const FALLBACK_PLATFORM_UI_FONT_SIZE: f32 = 13.0;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_MINIMUM_DEFAULT_UI_FONT_SIZE: f32 = 14.0;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_DEFAULT_DPI: f32 = 96.0;

/// Complete example configuration used for new `nucleotide.toml` files.
pub const NUCLEOTIDE_EXAMPLE_CONFIG: &str = include_str!("../nucleotide.example.toml");

fn normalize_ui_font(mut font: FontConfig) -> FontConfig {
    if matches!(
        font.family.as_str(),
        "SF Pro Display" | "SF Pro" | "system-ui"
    ) {
        font.family = GPUI_SYSTEM_UI_FONT.to_string();
    }
    font
}

fn default_ui_font() -> FontConfig {
    FontConfig {
        family: GPUI_SYSTEM_UI_FONT.to_string(),
        weight: FontWeight::Normal,
        size: default_ui_font_size(),
        line_height: DEFAULT_UI_FONT_LINE_HEIGHT,
    }
}

fn default_ui_font_size() -> f32 {
    default_ui_font_size_from_platform_size(platform_ui_font_size())
}

fn default_ui_font_size_from_platform_size(platform_size: Option<f32>) -> f32 {
    let size = platform_size.unwrap_or(FALLBACK_PLATFORM_UI_FONT_SIZE);

    #[cfg(target_os = "windows")]
    {
        windows_default_ui_font_size(size)
    }

    #[cfg(not(target_os = "windows"))]
    {
        size
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_default_ui_font_size(size: f32) -> f32 {
    size.max(WINDOWS_MINIMUM_DEFAULT_UI_FONT_SIZE)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialFontConfig {
    family: Option<String>,
    weight: Option<FontWeight>,
    size: Option<f32>,
    line_height: Option<f32>,
}

fn merge_font_config(mut base: FontConfig, partial: PartialFontConfig) -> FontConfig {
    if let Some(family) = partial.family {
        base.family = family;
    }
    if let Some(weight) = partial.weight {
        base.weight = weight;
    }
    if let Some(size) = partial.size {
        base.size = size;
    }
    if let Some(line_height) = partial.line_height {
        base.line_height = line_height;
    }
    base
}

fn deserialize_ui_font<'de, D>(deserializer: D) -> Result<Option<FontConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<PartialFontConfig>::deserialize(deserializer)
        .map(|font| font.map(|font| merge_font_config(default_ui_font(), font)))
}

fn deserialize_editor_font<'de, D>(deserializer: D) -> Result<Option<FontConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<PartialFontConfig>::deserialize(deserializer)
        .map(|font| font.map(|font| merge_font_config(FontConfig::default(), font)))
}

#[cfg(any(target_os = "windows", test))]
fn logical_font_size_from_windows_logfont_height(lf_height: i32, dpi: u32) -> Option<f32> {
    let dpi = dpi as f32;
    if dpi <= 0.0 {
        return None;
    }

    let physical_size = lf_height.checked_abs()? as f32;
    physical_size
        .is_finite()
        .then_some(physical_size * WINDOWS_DEFAULT_DPI / dpi)
        .filter(|size| *size > 0.0)
}

#[cfg(target_os = "macos")]
fn platform_ui_font_size() -> Option<f32> {
    use objc2_app_kit::NSFont;

    let size = NSFont::systemFontSize() as f32;
    size.is_finite().then_some(size).filter(|size| *size > 0.0)
}

#[cfg(target_os = "windows")]
fn platform_ui_font_size() -> Option<f32> {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Graphics::Gdi::LOGFONTW,
        UI::{
            HiDpi::GetDpiForSystem,
            WindowsAndMessaging::{
                NONCLIENTMETRICSW, SPI_GETICONTITLELOGFONT, SPI_GETNONCLIENTMETRICS,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
                USER_DEFAULT_SCREEN_DPI,
            },
        },
    };

    fn logfont_size(font: &LOGFONTW, dpi: u32) -> Option<f32> {
        logical_font_size_from_windows_logfont_height(font.lfHeight, dpi)
    }

    // SAFETY: SystemParametersInfoW writes into stack-allocated Windows structs
    // whose sizes are passed explicitly. The pointers remain valid for each call.
    unsafe {
        let dpi = match GetDpiForSystem() {
            0 => USER_DEFAULT_SCREEN_DPI,
            dpi => dpi,
        };

        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let metrics_result = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&mut metrics as *mut NONCLIENTMETRICSW).cast::<c_void>(),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        );
        if metrics_result != 0 {
            if let Some(size) = logfont_size(&metrics.lfMessageFont, dpi) {
                return Some(size);
            }
        }

        let mut icon_title_font = LOGFONTW::default();
        let icon_result = SystemParametersInfoW(
            SPI_GETICONTITLELOGFONT,
            std::mem::size_of::<LOGFONTW>() as u32,
            (&mut icon_title_font as *mut LOGFONTW).cast::<c_void>(),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        );
        if icon_result != 0 {
            return logfont_size(&icon_title_font, dpi);
        }
    }

    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_ui_font_size() -> Option<f32> {
    None
}

/// Controls where Nucleotide derives non-editor UI chrome styling from.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UiLook {
    /// Derive UI chrome from the active Helix theme.
    #[default]
    Theme,
    /// Use platform-specific UI chrome while preserving editor theme colors.
    System,
}

impl UiLook {
    pub fn to_ui_chrome_style(self) -> UiChromeStyle {
        match self {
            Self::Theme => UiChromeStyle::Theme,
            Self::System => UiChromeStyle::System,
        }
    }
}

/// UI-specific configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    /// Source for UI chrome styling.
    #[serde(default)]
    pub look: UiLook,

    /// Font used for UI elements (menus, dialogs, etc.)
    #[serde(default, deserialize_with = "deserialize_ui_font")]
    pub font: Option<FontConfig>,
}

/// Editor-specific GUI configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorGuiConfig {
    /// Font used in the editor
    #[serde(default, deserialize_with = "deserialize_editor_font")]
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

    /// Theme to use in light mode (defaults to "nucleotide-cyan-light" if not specified)
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Enable blur for dark themes
    #[serde(default)]
    pub blur_dark_themes: bool,

    /// Automatically adjust window appearance based on theme
    #[serde(default = "default_true")]
    pub appearance_follows_theme: bool,

    /// Windows DirectWrite text rendering overrides.
    #[serde(default = "default_window_directwrite_config")]
    pub directwrite: Option<DirectWriteConfig>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            blur_dark_themes: false,
            appearance_follows_theme: true,
            directwrite: default_window_directwrite_config(),
        }
    }
}

#[cfg(target_os = "windows")]
fn default_window_directwrite_config() -> Option<DirectWriteConfig> {
    Some(DirectWriteConfig::windows_fluent_default())
}

#[cfg(not(target_os = "windows"))]
fn default_window_directwrite_config() -> Option<DirectWriteConfig> {
    None
}

/// Windows DirectWrite text rendering configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DirectWriteConfig {
    /// Gamma correction value passed to DirectWrite.
    #[serde(default)]
    pub gamma: Option<f32>,

    /// Enhanced contrast value passed to DirectWrite.
    #[serde(default)]
    pub enhanced_contrast: Option<f32>,

    /// ClearType level passed to DirectWrite.
    #[serde(default)]
    pub clear_type_level: Option<f32>,

    /// Pixel geometry passed to DirectWrite.
    #[serde(default)]
    pub pixel_geometry: Option<DirectWritePixelGeometry>,

    /// Rendering mode passed to DirectWrite.
    #[serde(default)]
    pub rendering_mode: Option<DirectWriteRenderingMode>,
}

impl DirectWriteConfig {
    /// DirectWrite defaults tuned for crisp Windows UI text.
    pub fn windows_fluent_default() -> Self {
        Self {
            gamma: Some(1.8),
            enhanced_contrast: Some(0.75),
            clear_type_level: Some(1.0),
            pixel_geometry: Some(DirectWritePixelGeometry::Rgb),
            rendering_mode: Some(DirectWriteRenderingMode::NaturalSymmetric),
        }
    }

    /// Convert this configuration into GPUI's DirectWrite rendering parameters.
    pub fn to_gpui_params(&self) -> gpui::DirectWriteTextRenderingParams {
        gpui::DirectWriteTextRenderingParams {
            gamma: self.gamma,
            enhanced_contrast: self.enhanced_contrast,
            clear_type_level: self.clear_type_level,
            pixel_geometry: self.pixel_geometry.map(Into::into),
            rendering_mode: self.rendering_mode.map(Into::into),
        }
    }

    /// Validate DirectWrite numeric ranges before passing them to GPUI.
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();

        if let Some(gamma) = self.gamma
            && !valid_directwrite_gamma(gamma)
        {
            errors.push(format!(
                "DirectWrite gamma must be finite and between 1.0 and 2.2 inclusive; got {gamma}"
            ));
        }

        if let Some(enhanced_contrast) = self.enhanced_contrast
            && !valid_directwrite_enhanced_contrast(enhanced_contrast)
        {
            errors.push(format!(
                "DirectWrite enhanced_contrast must be finite and greater than or equal to 0.0; got {enhanced_contrast}"
            ));
        }

        if let Some(clear_type_level) = self.clear_type_level
            && !valid_directwrite_clear_type_level(clear_type_level)
        {
            errors.push(format!(
                "DirectWrite clear_type_level must be finite and between 0.0 and 1.0 inclusive; got {clear_type_level}"
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// Return a copy with invalid DirectWrite numeric fields unset.
    pub fn sanitized(&self) -> Self {
        let mut config = self.clone();

        if config
            .gamma
            .is_some_and(|gamma| !valid_directwrite_gamma(gamma))
        {
            nucleotide_logging::warn!(
                gamma = ?config.gamma,
                "Invalid DirectWrite gamma; using DirectWrite default"
            );
            config.gamma = None;
        }

        if config.enhanced_contrast.is_some_and(|enhanced_contrast| {
            !valid_directwrite_enhanced_contrast(enhanced_contrast)
        }) {
            nucleotide_logging::warn!(
                enhanced_contrast = ?config.enhanced_contrast,
                "Invalid DirectWrite enhanced_contrast; using DirectWrite default"
            );
            config.enhanced_contrast = None;
        }

        if config
            .clear_type_level
            .is_some_and(|clear_type_level| !valid_directwrite_clear_type_level(clear_type_level))
        {
            nucleotide_logging::warn!(
                clear_type_level = ?config.clear_type_level,
                "Invalid DirectWrite clear_type_level; using DirectWrite default"
            );
            config.clear_type_level = None;
        }

        config
    }
}

// IDWriteFactory3::CreateCustomRenderingParams documents gamma as greater
// than zero and no more than 256, enhanced contrast as zero or greater, and
// clearTypeLevel as 0.0 through 1.0:
// https://learn.microsoft.com/windows/win32/api/dwrite_3/nf-dwrite_3-idwritefactory3-createcustomrenderingparams
//
// GPUI currently applies gamma in its shader with a correction-ratio table
// defined only for 1.0 through 2.2, so Nucleotide validates to that narrower
// renderer-supported range instead of accepting DirectWrite values that would
// be silently clamped by GPUI.
fn valid_directwrite_gamma(value: f32) -> bool {
    value.is_finite() && (1.0..=2.2).contains(&value)
}

fn valid_directwrite_enhanced_contrast(value: f32) -> bool {
    value.is_finite() && value >= 0.0
}

fn valid_directwrite_clear_type_level(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// DirectWrite pixel geometry used for subpixel text rendering on Windows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectWritePixelGeometry {
    /// Disable subpixel colour layout.
    Flat,
    /// Red-green-blue subpixel order.
    Rgb,
    /// Blue-green-red subpixel order.
    Bgr,
}

impl From<DirectWritePixelGeometry> for gpui::DirectWritePixelGeometry {
    fn from(value: DirectWritePixelGeometry) -> Self {
        match value {
            DirectWritePixelGeometry::Flat => gpui::DirectWritePixelGeometry::Flat,
            DirectWritePixelGeometry::Rgb => gpui::DirectWritePixelGeometry::Rgb,
            DirectWritePixelGeometry::Bgr => gpui::DirectWritePixelGeometry::Bgr,
        }
    }
}

/// DirectWrite rendering mode used for glyph rasterization on Windows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectWriteRenderingMode {
    /// Let DirectWrite choose the rendering mode.
    Default,
    /// Aliased text rendering.
    Aliased,
    /// GDI-compatible classic ClearType rendering.
    GdiClassic,
    /// GDI-compatible natural ClearType rendering.
    GdiNatural,
    /// Natural ClearType rendering.
    Natural,
    /// Natural symmetric ClearType rendering.
    NaturalSymmetric,
}

impl From<DirectWriteRenderingMode> for gpui::DirectWriteRenderingMode {
    fn from(value: DirectWriteRenderingMode) -> Self {
        match value {
            DirectWriteRenderingMode::Default => gpui::DirectWriteRenderingMode::Default,
            DirectWriteRenderingMode::Aliased => gpui::DirectWriteRenderingMode::Aliased,
            DirectWriteRenderingMode::GdiClassic => gpui::DirectWriteRenderingMode::GdiClassic,
            DirectWriteRenderingMode::GdiNatural => gpui::DirectWriteRenderingMode::GdiNatural,
            DirectWriteRenderingMode::Natural => gpui::DirectWriteRenderingMode::Natural,
            DirectWriteRenderingMode::NaturalSymmetric => {
                gpui::DirectWriteRenderingMode::NaturalSymmetric
            }
        }
    }
}

/// Visibility modes for close buttons on unpinned tabs.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TabCloseButtonVisibility {
    /// Always render close buttons on unpinned tabs.
    Always,
    /// Render close buttons on unpinned tabs only while hovering over the tab.
    #[default]
    Hover,
    /// Do not render close buttons on unpinned tabs.
    Hidden,
}

/// Position of the tab close or pin button.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TabClosePosition {
    /// Render the close or pin button on the left side of the tab.
    Left,
    /// Render the close or pin button on the right side of the tab.
    #[default]
    Right,
}

/// Diagnostic marker visibility for tabs.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TabDiagnosticsVisibility {
    /// Do not show diagnostics on tabs.
    #[default]
    Off,
    /// Show only error diagnostics on tabs.
    Errors,
    /// Show error and warning diagnostics on tabs.
    All,
}

/// Tab activation policy after closing the active tab.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabActivateOnClose {
    /// Activate the previously active tab.
    #[default]
    History,
    /// Activate the right neighbour when present, otherwise the left neighbour.
    Neighbour,
    /// Activate the left neighbour when present, otherwise the right neighbour.
    LeftNeighbour,
}

/// Tab bar behaviour configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabBarConfig {
    /// Show the tab bar.
    #[serde(default = "default_true")]
    pub show: bool,

    /// Show tab-bar navigation history buttons.
    #[serde(default = "default_true")]
    pub show_nav_history_buttons: bool,

    /// Show tab-bar action buttons such as new file and split controls.
    #[serde(default = "default_true")]
    pub show_tab_bar_buttons: bool,

    /// Render pinned tabs in a separate row when both pinned and unpinned tabs are open.
    #[serde(default)]
    pub show_pinned_tabs_in_separate_row: bool,
}

impl Default for TabBarConfig {
    fn default() -> Self {
        Self {
            show: true,
            show_nav_history_buttons: true,
            show_tab_bar_buttons: true,
            show_pinned_tabs_in_separate_row: false,
        }
    }
}

/// Per-tab behaviour and decoration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabsConfig {
    /// Show git status decorations in tabs.
    #[serde(default)]
    pub git_status: bool,

    /// Position of the close or pin button in each tab.
    #[serde(default)]
    pub close_position: TabClosePosition,

    /// What to activate after closing the current tab.
    #[serde(default)]
    pub activate_on_close: TabActivateOnClose,

    /// Show file icons in tabs.
    #[serde(default = "default_true")]
    pub file_icons: bool,

    /// Show diagnostic decorations in tabs.
    #[serde(default)]
    pub show_diagnostics: TabDiagnosticsVisibility,

    /// Controls close button visibility for unpinned tabs.
    #[serde(default)]
    pub show_close_button: TabCloseButtonVisibility,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            git_status: false,
            close_position: TabClosePosition::Right,
            activate_on_close: TabActivateOnClose::History,
            file_icons: true,
            show_diagnostics: TabDiagnosticsVisibility::Off,
            show_close_button: TabCloseButtonVisibility::Hover,
        }
    }
}

/// Preview tab behaviour configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewTabsConfig {
    /// Enable preview tabs.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Open project-panel single-clicked files in preview mode.
    #[serde(default = "default_true")]
    pub enable_preview_from_project_panel: bool,

    /// Open file-finder selections in preview mode.
    #[serde(default)]
    pub enable_preview_from_file_finder: bool,

    /// Open multibuffer files in preview mode.
    #[serde(default = "default_true")]
    pub enable_preview_from_multibuffer: bool,

    /// Open code-navigation multibuffers in preview mode.
    #[serde(default)]
    pub enable_preview_multibuffer_from_code_navigation: bool,

    /// Open code-navigation files in preview mode.
    #[serde(default = "default_true")]
    pub enable_preview_file_from_code_navigation: bool,

    /// Keep preview tabs in preview mode when navigating away from them.
    #[serde(default)]
    pub enable_keep_preview_on_code_navigation: bool,
}

impl Default for PreviewTabsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_preview_from_project_panel: true,
            enable_preview_from_file_finder: false,
            enable_preview_from_multibuffer: true,
            enable_preview_multibuffer_from_code_navigation: false,
            enable_preview_file_from_code_navigation: true,
            enable_keep_preview_on_code_navigation: false,
        }
    }
}

/// Project tree display configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTreeUiConfig {
    /// Density preset for project-tree rows.
    #[serde(default)]
    pub density: FileTreeDisplayDensity,
    /// Collapse single-child directory chains into one visible row.
    #[serde(default = "default_true")]
    pub flatten_empty_directories: bool,
}

impl Default for FileTreeUiConfig {
    fn default() -> Self {
        Self {
            density: FileTreeDisplayDensity::Default,
            flatten_empty_directories: true,
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

    /// Maximum number of tabs to keep open. Unset for unlimited.
    #[serde(default)]
    pub max_tabs: Option<std::num::NonZeroUsize>,

    /// Tab bar behaviour
    #[serde(default)]
    pub tab_bar: TabBarConfig,

    /// Per-tab behaviour and decoration settings
    #[serde(default)]
    pub tabs: TabsConfig,

    /// Preview tab behaviour
    #[serde(default)]
    pub preview_tabs: PreviewTabsConfig,

    /// File tree display settings
    #[serde(default)]
    pub file_tree: FileTreeUiConfig,

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
        // Resolve the editor font, falling back to UI font if not set
        if let Some(font) = self.gui.editor.font.clone() {
            nucleotide_logging::info!(
                source = "editor",
                family = %font.family,
                weight = ?font.weight,
                size = font.size,
                line_height = font.line_height,
                "Resolved editor font"
            );
            font
        } else {
            let fallback = self.gui.ui.font.clone().unwrap_or_default();
            nucleotide_logging::info!(
                source = "ui_fallback",
                family = %fallback.family,
                weight = ?fallback.weight,
                size = fallback.size,
                line_height = fallback.line_height,
                "Editor font not set; using UI font"
            );
            fallback
        }
    }

    /// Get the UI font configuration
    pub fn ui_font(&self) -> FontConfig {
        normalize_ui_font(self.gui.ui.font.clone().unwrap_or_else(default_ui_font))
    }

    /// Get the UI look configuration.
    pub fn ui_look(&self) -> UiLook {
        self.gui.ui.look
    }

    /// Get the UI chrome style used by nucleotide-ui.
    pub fn ui_chrome_style(&self) -> UiChromeStyle {
        self.ui_look().to_ui_chrome_style()
    }

    /// Get the native window background material for the current UI look.
    pub fn window_background_appearance(
        &self,
        is_dark_chrome: bool,
    ) -> gpui::WindowBackgroundAppearance {
        nucleotide_appearance::window_background_appearance(
            self.ui_chrome_style(),
            is_dark_chrome,
            self.gui.window.blur_dark_themes,
        )
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

    let mut config = match Config::load_default() {
        Ok(config) => config,
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Config::default()
        }
        Err(ConfigLoadError::Error(err)) => return Err(err.into()),
        Err(ConfigLoadError::BadConfig(err)) => return Err(err.into()),
    };

    merge_nucleotide_helix_keybindings(&mut config)?;

    Ok(config)
}

fn merge_nucleotide_helix_keybindings(config: &mut HelixConfig) -> anyhow::Result<()> {
    use helix_term::{
        commands::MappableCommand,
        keymap::{KeyTrie, KeyTrieNode, merge_keys},
    };
    use helix_view::{document::Mode, input::KeyEvent};
    use std::{collections::HashMap, str::FromStr};

    let space = KeyEvent::from_str("space")?;
    let v = KeyEvent::from_str("v")?;
    let r = KeyEvent::from_str("r")?;
    let reset_diff_change = MappableCommand::from_str(":reset-diff-change")?;
    if let Some(normal_keymap) = config.keys.get(&Mode::Normal) {
        if matches!(
            normal_keymap.search(&[space, v]),
            Some(KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_))
        ) || normal_keymap.search(&[space, v, r]).is_some()
        {
            return Ok(());
        }
    }

    let mut vcs_node = HashMap::new();
    vcs_node.insert(r, KeyTrie::MappableCommand(reset_diff_change));

    let mut space_node = HashMap::new();
    space_node.insert(v, KeyTrie::Node(KeyTrieNode::new("VCS", vcs_node, vec![r])));

    let mut normal_node = HashMap::new();
    normal_node.insert(
        space,
        KeyTrie::Node(KeyTrieNode::new("Space", space_node, vec![v])),
    );

    merge_keys(
        &mut config.keys,
        HashMap::from([(
            Mode::Normal,
            KeyTrie::Node(KeyTrieNode::new("Normal mode", normal_node, vec![space])),
        )]),
    );

    Ok(())
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

    // Load project markers configuration from project_markers.toml when present.
    match load_project_markers_config(dir) {
        Ok(Some(project_markers_config)) => {
            config.project_markers = project_markers_config;
        }
        Ok(None) => {
            nucleotide_logging::info!(
                "No project markers configuration file found, using nucleotide.toml settings"
            );
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

    // Validate and sanitize DirectWrite configuration
    if let Some(directwrite) = config.window.directwrite.as_ref() {
        if let Err(validation_error) = directwrite.validate() {
            nucleotide_logging::error!(
                config_path = %gui_config_path.display(),
                error = %validation_error,
                "Invalid DirectWrite configuration - using sanitized values"
            );
            config.window.directwrite = Some(directwrite.sanitized());
        } else {
            nucleotide_logging::info!("DirectWrite configuration validation passed");
        }
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

/// Load project markers configuration from project_markers.toml when present.
fn load_project_markers_config(dir: &Path) -> anyhow::Result<Option<ProjectMarkersConfig>> {
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

        Ok(Some(config))
    } else {
        Ok(None)
    }
}

/// The complete example `nucleotide.toml` content is embedded in
/// [`NUCLEOTIDE_EXAMPLE_CONFIG`].
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
    fn nucleotide_helix_keybindings_include_revert_current_change() {
        use helix_term::{
            commands::MappableCommand,
            keymap::{KeymapResult, Keymaps},
        };
        use helix_view::{document::Mode, input::KeyEvent};
        use std::str::FromStr;

        let mut config = HelixConfig::default();
        merge_nucleotide_helix_keybindings(&mut config).unwrap();

        let mut keymaps = Keymaps::new(Box::new(arc_swap::access::Constant(config.keys)));
        assert!(matches!(
            keymaps.get(Mode::Normal, KeyEvent::from_str("space").unwrap()),
            KeymapResult::Pending(_)
        ));
        assert!(matches!(
            keymaps.get(Mode::Normal, KeyEvent::from_str("v").unwrap()),
            KeymapResult::Pending(_)
        ));

        let command = match keymaps.get(Mode::Normal, KeyEvent::from_str("r").unwrap()) {
            KeymapResult::Matched(command) => command,
            other => panic!("expected <space> v r to match reset-diff-change, got {other:?}"),
        };

        assert_eq!(
            command,
            MappableCommand::from_str(":reset-diff-change").unwrap()
        );
    }

    #[test]
    fn nucleotide_helix_keybindings_do_not_override_existing_binding() {
        use helix_term::{
            commands::MappableCommand,
            keymap::{KeyTrie, KeyTrieNode, KeymapResult, Keymaps, merge_keys},
        };
        use helix_view::{document::Mode, input::KeyEvent};
        use std::{collections::HashMap, str::FromStr};

        let space = KeyEvent::from_str("space").unwrap();
        let v = KeyEvent::from_str("v").unwrap();
        let r = KeyEvent::from_str("r").unwrap();

        let mut vcs_node = HashMap::new();
        vcs_node.insert(
            r,
            KeyTrie::MappableCommand(MappableCommand::command_palette),
        );

        let mut space_node = HashMap::new();
        space_node.insert(v, KeyTrie::Node(KeyTrieNode::new("VCS", vcs_node, vec![r])));

        let mut normal_node = HashMap::new();
        normal_node.insert(
            space,
            KeyTrie::Node(KeyTrieNode::new("Space", space_node, vec![v])),
        );

        let mut config = HelixConfig::default();
        merge_keys(
            &mut config.keys,
            HashMap::from([(
                Mode::Normal,
                KeyTrie::Node(KeyTrieNode::new("Normal mode", normal_node, vec![space])),
            )]),
        );

        merge_nucleotide_helix_keybindings(&mut config).unwrap();

        let mut keymaps = Keymaps::new(Box::new(arc_swap::access::Constant(config.keys)));
        assert!(matches!(
            keymaps.get(Mode::Normal, KeyEvent::from_str("space").unwrap()),
            KeymapResult::Pending(_)
        ));
        assert!(matches!(
            keymaps.get(Mode::Normal, KeyEvent::from_str("v").unwrap()),
            KeymapResult::Pending(_)
        ));
        let command = match keymaps.get(Mode::Normal, KeyEvent::from_str("r").unwrap()) {
            KeymapResult::Matched(command) => command,
            other => panic!("expected existing <space> v r binding to remain, got {other:?}"),
        };
        assert_eq!(command, MappableCommand::command_palette);
    }

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
max_tabs = 5

[ui]
look = "system"

[ui.font]
family = "Inter"
weight = "medium"
size = 13.0

[editor.font]
family = "JetBrains Mono"
weight = "normal"
size = 14.5

[window.directwrite]
gamma = 1.8
enhanced_contrast = 0.75
clear_type_level = 0.6
pixel_geometry = "bgr"
rendering_mode = "gdi_classic"

[tab_bar]
show = false
show_nav_history_buttons = false
show_tab_bar_buttons = false
show_pinned_tabs_in_separate_row = true

[tabs]
show_close_button = "hover"
close_position = "left"
activate_on_close = "left_neighbour"
file_icons = true
git_status = true
show_diagnostics = "all"

[preview_tabs]
enabled = false
enable_preview_from_project_panel = false
enable_preview_from_file_finder = true
enable_preview_from_multibuffer = false
enable_preview_multibuffer_from_code_navigation = true
enable_preview_file_from_code_navigation = false
enable_keep_preview_on_code_navigation = true

[file_tree]
density = "compact"
flatten_empty_directories = false
"#;

        let config: GuiConfig = toml::from_str(config_str).expect("Failed to parse GuiConfig");

        assert_eq!(config.ui.look, UiLook::System);

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
        let directwrite = config
            .window
            .directwrite
            .as_ref()
            .expect("DirectWrite config should be set");
        assert_eq!(directwrite.gamma, Some(1.8));
        assert_eq!(directwrite.enhanced_contrast, Some(0.75));
        assert_eq!(directwrite.clear_type_level, Some(0.6));
        assert_eq!(
            directwrite.pixel_geometry,
            Some(DirectWritePixelGeometry::Bgr)
        );
        assert_eq!(
            directwrite.rendering_mode,
            Some(DirectWriteRenderingMode::GdiClassic)
        );
        let gpui_params = directwrite.to_gpui_params();
        assert_eq!(
            gpui_params.pixel_geometry,
            Some(gpui::DirectWritePixelGeometry::Bgr)
        );
        assert_eq!(
            gpui_params.rendering_mode,
            Some(gpui::DirectWriteRenderingMode::GdiClassic)
        );
        assert_eq!(config.max_tabs.map(std::num::NonZeroUsize::get), Some(5));
        assert!(!config.tab_bar.show);
        assert!(!config.tab_bar.show_nav_history_buttons);
        assert!(!config.tab_bar.show_tab_bar_buttons);
        assert!(config.tab_bar.show_pinned_tabs_in_separate_row);
        assert_eq!(
            config.tabs.show_close_button,
            TabCloseButtonVisibility::Hover
        );
        assert_eq!(config.tabs.close_position, TabClosePosition::Left);
        assert_eq!(
            config.tabs.activate_on_close,
            TabActivateOnClose::LeftNeighbour
        );
        assert!(config.tabs.file_icons);
        assert!(config.tabs.git_status);
        assert_eq!(config.tabs.show_diagnostics, TabDiagnosticsVisibility::All);
        assert!(!config.preview_tabs.enabled);
        assert!(!config.preview_tabs.enable_preview_from_project_panel);
        assert!(config.preview_tabs.enable_preview_from_file_finder);
        assert!(!config.preview_tabs.enable_preview_from_multibuffer);
        assert!(
            config
                .preview_tabs
                .enable_preview_multibuffer_from_code_navigation
        );
        assert!(!config.preview_tabs.enable_preview_file_from_code_navigation);
        assert!(config.preview_tabs.enable_keep_preview_on_code_navigation);
        assert_eq!(config.file_tree.density, FileTreeDisplayDensity::Compact);
        assert!(!config.file_tree.flatten_empty_directories);
    }

    #[test]
    fn directwrite_config_validates_numeric_ranges() {
        let valid = DirectWriteConfig {
            gamma: Some(1.8),
            enhanced_contrast: Some(0.75),
            clear_type_level: Some(0.6),
            pixel_geometry: Some(DirectWritePixelGeometry::Rgb),
            rendering_mode: Some(DirectWriteRenderingMode::Natural),
        };
        assert!(valid.validate().is_ok());

        let invalid = DirectWriteConfig {
            gamma: Some(0.9),
            enhanced_contrast: Some(-0.1),
            clear_type_level: Some(1.1),
            pixel_geometry: Some(DirectWritePixelGeometry::Bgr),
            rendering_mode: Some(DirectWriteRenderingMode::GdiClassic),
        };
        let error = invalid
            .validate()
            .expect_err("invalid DirectWrite values should fail validation");
        assert!(error.contains("gamma"));
        assert!(error.contains("enhanced_contrast"));
        assert!(error.contains("clear_type_level"));
    }

    #[test]
    fn directwrite_config_sanitized_drops_invalid_numeric_values() {
        let invalid = DirectWriteConfig {
            gamma: Some(f32::NAN),
            enhanced_contrast: Some(f32::NEG_INFINITY),
            clear_type_level: Some(-0.1),
            pixel_geometry: Some(DirectWritePixelGeometry::Flat),
            rendering_mode: Some(DirectWriteRenderingMode::Aliased),
        };

        let sanitized = invalid.sanitized();
        assert_eq!(sanitized.gamma, None);
        assert_eq!(sanitized.enhanced_contrast, None);
        assert_eq!(sanitized.clear_type_level, None);
        assert_eq!(
            sanitized.pixel_geometry,
            Some(DirectWritePixelGeometry::Flat)
        );
        assert_eq!(
            sanitized.rendering_mode,
            Some(DirectWriteRenderingMode::Aliased)
        );
    }

    #[test]
    fn window_config_uses_tuned_windows_directwrite_defaults() {
        let window = WindowConfig::default();

        if cfg!(target_os = "windows") {
            let directwrite = window
                .directwrite
                .expect("Windows should use tuned DirectWrite defaults");

            assert_eq!(directwrite.gamma, Some(1.8));
            assert_eq!(directwrite.enhanced_contrast, Some(0.75));
            assert_eq!(directwrite.clear_type_level, Some(1.0));
            assert_eq!(
                directwrite.pixel_geometry,
                Some(DirectWritePixelGeometry::Rgb)
            );
            assert_eq!(
                directwrite.rendering_mode,
                Some(DirectWriteRenderingMode::NaturalSymmetric)
            );
        } else {
            assert_eq!(window.directwrite, None);
        }
    }

    #[test]
    fn nucleotide_example_config_parses_and_documents_supported_fields() {
        let config: GuiConfig =
            toml::from_str(NUCLEOTIDE_EXAMPLE_CONFIG).expect("example config should parse");

        assert_eq!(config.theme.mode, ThemeMode::System);
        assert_eq!(config.theme.get_light_theme(), DEFAULT_LIGHT_THEME);
        assert_eq!(config.theme.get_dark_theme(), DEFAULT_DARK_THEME);
        assert_eq!(config.ui.look, UiLook::Theme);
        assert!(config.ui.font.is_none());
        assert!(config.editor.font.is_none());
        assert!(config.window.appearance_follows_theme);
        assert!(config.tab_bar.show);
        assert_eq!(
            config.tabs.show_close_button,
            TabCloseButtonVisibility::Hover
        );
        assert!(config.preview_tabs.enabled);
        assert_eq!(config.file_tree.density, FileTreeDisplayDensity::Default);
        assert_eq!(config.file_ops.delete_behavior, DeleteBehavior::Trash);
        assert!(!config.lsp.project_lsp_startup);
        assert!(!config.project_markers.enable_project_markers);

        for setting in [
            "max_tabs",
            "[theme]",
            "mode",
            "light_theme",
            "dark_theme",
            "[window]",
            "blur_dark_themes",
            "appearance_follows_theme",
            "[window.directwrite]",
            "gamma",
            "enhanced_contrast",
            "clear_type_level",
            "pixel_geometry",
            "rendering_mode",
            "[ui]",
            "look",
            "[ui.font]",
            "[editor.font]",
            "family",
            "weight",
            "size",
            "line_height",
            "[tab_bar]",
            "show_nav_history_buttons",
            "show_tab_bar_buttons",
            "show_pinned_tabs_in_separate_row",
            "[tabs]",
            "git_status",
            "file_icons",
            "show_diagnostics",
            "show_close_button",
            "close_position",
            "activate_on_close",
            "[preview_tabs]",
            "enable_preview_from_project_panel",
            "enable_preview_from_file_finder",
            "enable_preview_from_multibuffer",
            "enable_preview_multibuffer_from_code_navigation",
            "enable_preview_file_from_code_navigation",
            "enable_keep_preview_on_code_navigation",
            "[file_tree]",
            "density",
            "flatten_empty_directories",
            "[file_ops]",
            "delete_behavior",
            "[lsp]",
            "project_lsp_startup",
            "startup_timeout_ms",
            "enable_fallback",
            "[project_markers]",
            "enable_project_markers",
            "detection_timeout_ms",
            "enable_builtin_fallback",
            "[project_markers.markers.<name>]",
            "markers",
            "language_server",
            "root_strategy",
            "priority",
        ] {
            assert!(
                NUCLEOTIDE_EXAMPLE_CONFIG.contains(setting),
                "example config should document `{setting}`"
            );
        }
    }

    #[test]
    fn ui_look_defaults_to_theme() {
        let config: GuiConfig = toml::from_str("").expect("empty config should parse");

        assert_eq!(config.ui.look, UiLook::Theme);
    }

    #[test]
    fn ui_look_parses_theme_and_system_values() {
        let themed: UiConfig = toml::from_str(r#"look = "theme""#).expect("theme look parses");
        let system: UiConfig = toml::from_str(r#"look = "system""#).expect("system look parses");

        assert_eq!(themed.look, UiLook::Theme);
        assert_eq!(system.look, UiLook::System);
    }

    #[test]
    fn window_background_appearance_uses_system_material_when_requested() {
        let mut config = Config {
            helix: HelixConfig::default(),
            gui: GuiConfig::default(),
        };
        config.gui.window.blur_dark_themes = true;

        assert_eq!(
            config.window_background_appearance(true),
            gpui::WindowBackgroundAppearance::Blurred
        );

        config.gui.ui.look = UiLook::System;

        #[cfg(target_os = "windows")]
        assert_eq!(
            config.window_background_appearance(true),
            gpui::WindowBackgroundAppearance::MicaBackdrop
        );

        #[cfg(target_os = "macos")]
        assert_eq!(
            config.window_background_appearance(true),
            gpui::WindowBackgroundAppearance::Blurred
        );

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert_eq!(
            config.window_background_appearance(true),
            gpui::WindowBackgroundAppearance::Opaque
        );
    }

    #[test]
    fn docs_example_matches_bundled_example_config() {
        assert_eq!(
            NUCLEOTIDE_EXAMPLE_CONFIG,
            include_str!("../../../docs/examples/nucleotide.example.toml")
        );
    }

    #[test]
    fn gui_config_keeps_project_markers_from_nucleotide_toml_without_override_file() {
        let temp_dir = tempfile::TempDir::new().expect("should create temp directory");
        std::fs::write(
            temp_dir.path().join("nucleotide.toml"),
            r#"
[project_markers]
enable_project_markers = true
detection_timeout_ms = 1500
enable_builtin_fallback = false

[project_markers.markers.rust]
markers = ["Cargo.toml"]
language_server = "rust-analyzer"
root_strategy = "closest"
priority = 80
"#,
        )
        .expect("should write nucleotide config");

        let config = load_gui_config(temp_dir.path()).expect("should load GUI config");

        assert!(config.project_markers.enable_project_markers);
        assert_eq!(config.project_markers.detection_timeout_ms, 1500);
        assert!(!config.project_markers.enable_builtin_fallback);
        assert_eq!(config.project_markers.markers.len(), 1);
        assert_eq!(
            config
                .project_markers
                .markers
                .get("rust")
                .expect("rust marker should load")
                .language_server,
            "rust-analyzer"
        );
    }

    #[test]
    fn project_markers_toml_overrides_nucleotide_toml_project_markers() {
        let temp_dir = tempfile::TempDir::new().expect("should create temp directory");
        std::fs::write(
            temp_dir.path().join("nucleotide.toml"),
            r#"
[project_markers]
enable_project_markers = true
detection_timeout_ms = 1500
enable_builtin_fallback = true
"#,
        )
        .expect("should write nucleotide config");
        std::fs::write(
            temp_dir.path().join("project_markers.toml"),
            r#"
enable_project_markers = false
detection_timeout_ms = 2500
enable_builtin_fallback = false
"#,
        )
        .expect("should write project markers config");

        let config = load_gui_config(temp_dir.path()).expect("should load GUI config");

        assert!(!config.project_markers.enable_project_markers);
        assert_eq!(config.project_markers.detection_timeout_ms, 2500);
        assert!(!config.project_markers.enable_builtin_fallback);
    }

    #[test]
    fn test_tab_close_button_visibility_parsing() {
        let always: TabsConfig =
            toml::from_str(r#"show_close_button = "always""#).expect("should parse always");
        let hover: TabsConfig =
            toml::from_str(r#"show_close_button = "hover""#).expect("should parse hover");
        let hidden: TabsConfig =
            toml::from_str(r#"show_close_button = "hidden""#).expect("should parse hidden");

        assert_eq!(always.show_close_button, TabCloseButtonVisibility::Always);
        assert_eq!(hover.show_close_button, TabCloseButtonVisibility::Hover);
        assert_eq!(hidden.show_close_button, TabCloseButtonVisibility::Hidden);
    }

    #[test]
    fn test_tab_close_position_parsing() {
        let left: TabsConfig =
            toml::from_str(r#"close_position = "left""#).expect("should parse left");
        let right: TabsConfig =
            toml::from_str(r#"close_position = "right""#).expect("should parse right");

        assert_eq!(left.close_position, TabClosePosition::Left);
        assert_eq!(right.close_position, TabClosePosition::Right);
    }

    #[test]
    fn test_tab_activate_on_close_parsing() {
        let history: TabsConfig =
            toml::from_str(r#"activate_on_close = "history""#).expect("should parse history");
        let neighbour: TabsConfig =
            toml::from_str(r#"activate_on_close = "neighbour""#).expect("should parse neighbour");
        let left_neighbour: TabsConfig = toml::from_str(r#"activate_on_close = "left_neighbour""#)
            .expect("should parse left neighbour");

        assert_eq!(history.activate_on_close, TabActivateOnClose::History);
        assert_eq!(neighbour.activate_on_close, TabActivateOnClose::Neighbour);
        assert_eq!(
            left_neighbour.activate_on_close,
            TabActivateOnClose::LeftNeighbour
        );
    }

    #[test]
    fn test_tab_bar_button_visibility_parsing() {
        let config: TabBarConfig = toml::from_str(
            r#"
show = false
show_nav_history_buttons = false
show_tab_bar_buttons = false
"#,
        )
        .expect("should parse tab bar button visibility settings");

        assert!(!config.show);
        assert!(!config.show_nav_history_buttons);
        assert!(!config.show_tab_bar_buttons);
    }

    #[test]
    fn test_tab_icon_and_git_status_parsing() {
        let config: TabsConfig = toml::from_str(
            r#"
file_icons = true
git_status = true
"#,
        )
        .expect("should parse tab icon settings");

        assert!(config.file_icons);
        assert!(config.git_status);

        let omitted_icons: TabsConfig = toml::from_str(
            r#"
git_status = true
"#,
        )
        .expect("should default tab icons to visible");
        assert!(omitted_icons.file_icons);
        assert!(omitted_icons.git_status);

        let disabled_icons: TabsConfig = toml::from_str(
            r#"
file_icons = false
"#,
        )
        .expect("should allow disabling tab icons");
        assert!(!disabled_icons.file_icons);
    }

    #[test]
    fn test_tab_diagnostics_visibility_parsing() {
        let off: TabsConfig =
            toml::from_str(r#"show_diagnostics = "off""#).expect("should parse off");
        let errors: TabsConfig =
            toml::from_str(r#"show_diagnostics = "errors""#).expect("should parse errors");
        let all: TabsConfig =
            toml::from_str(r#"show_diagnostics = "all""#).expect("should parse all");

        assert_eq!(off.show_diagnostics, TabDiagnosticsVisibility::Off);
        assert_eq!(errors.show_diagnostics, TabDiagnosticsVisibility::Errors);
        assert_eq!(all.show_diagnostics, TabDiagnosticsVisibility::All);
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

        assert!(config.lsp.project_lsp_startup);
        assert_eq!(config.lsp.startup_timeout_ms, 3000);
        assert!(!config.lsp.enable_fallback);
    }

    #[test]
    fn test_lsp_config_defaults() {
        let config = GuiConfig::default();

        // Test default values
        assert!(!config.lsp.project_lsp_startup);
        assert_eq!(config.lsp.startup_timeout_ms, 5000);
        assert!(config.lsp.enable_fallback);
        assert_eq!(config.max_tabs, None);
        assert!(config.tab_bar.show);
        assert!(config.tab_bar.show_nav_history_buttons);
        assert!(config.tab_bar.show_tab_bar_buttons);
        assert!(!config.tab_bar.show_pinned_tabs_in_separate_row);
        assert_eq!(
            config.tabs.show_close_button,
            TabCloseButtonVisibility::Hover
        );
        assert_eq!(config.tabs.close_position, TabClosePosition::Right);
        assert_eq!(config.tabs.activate_on_close, TabActivateOnClose::History);
        assert!(config.tabs.file_icons);
        assert!(!config.tabs.git_status);
        assert_eq!(config.tabs.show_diagnostics, TabDiagnosticsVisibility::Off);
        assert!(config.preview_tabs.enabled);
        assert!(config.preview_tabs.enable_preview_from_project_panel);
        assert!(!config.preview_tabs.enable_preview_from_file_finder);
        assert!(config.preview_tabs.enable_preview_from_multibuffer);
        assert!(
            !config
                .preview_tabs
                .enable_preview_multibuffer_from_code_navigation
        );
        assert!(config.preview_tabs.enable_preview_file_from_code_navigation);
        assert!(!config.preview_tabs.enable_keep_preview_on_code_navigation);
        assert_eq!(config.file_tree.density, FileTreeDisplayDensity::Default);
        assert!(config.file_tree.flatten_empty_directories);
    }

    #[test]
    fn test_file_tree_config_parsing() {
        let compact: FileTreeUiConfig =
            toml::from_str(r#"density = "compact""#).expect("should parse compact");
        let default: FileTreeUiConfig =
            toml::from_str(r#"density = "default""#).expect("should parse default");
        let relaxed: FileTreeUiConfig =
            toml::from_str(r#"density = "relaxed""#).expect("should parse relaxed");
        let unflattened: FileTreeUiConfig = toml::from_str(r#"flatten_empty_directories = false"#)
            .expect("should parse flatten option");

        assert_eq!(compact.density, FileTreeDisplayDensity::Compact);
        assert!(compact.flatten_empty_directories);
        assert_eq!(default.density, FileTreeDisplayDensity::Default);
        assert!(default.flatten_empty_directories);
        assert_eq!(relaxed.density, FileTreeDisplayDensity::Relaxed);
        assert!(relaxed.flatten_empty_directories);
        assert_eq!(unflattened.density, FileTreeDisplayDensity::Default);
        assert!(!unflattened.flatten_empty_directories);
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

        assert!(config.is_project_lsp_startup_enabled());
        assert_eq!(config.lsp_startup_timeout_ms(), 2000);
        assert!(!config.is_lsp_fallback_enabled());
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
        assert!(gui_config.lsp.project_lsp_startup);
        assert_eq!(gui_config.lsp.startup_timeout_ms, 3000);
        assert!(gui_config.lsp.enable_fallback);

        // Test convenience methods via full config
        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        assert!(config.is_project_lsp_startup_enabled());
        assert_eq!(config.lsp_startup_timeout_ms(), 3000);
        assert!(config.is_lsp_fallback_enabled());
    }

    #[test]
    fn test_invalid_lsp_configuration_validation() {
        // Test invalid timeout (0)
        let mut config = LspConfig {
            startup_timeout_ms: 0,
            ..LspConfig::default()
        };
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
        let mut config = LspConfig {
            startup_timeout_ms: 0,
            ..LspConfig::default()
        };
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

        assert!(config.enable_project_markers);
        assert_eq!(config.detection_timeout_ms, 2000);
        assert!(!config.enable_builtin_fallback);
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

        assert!(!config.enable_project_markers);
        assert_eq!(config.detection_timeout_ms, 1000);
        assert!(config.enable_builtin_fallback);
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
        let mut valid_config = ProjectMarkersConfig {
            enable_project_markers: true,
            detection_timeout_ms: 1000,
            ..Default::default()
        };
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
        let invalid_config = ProjectMarkersConfig {
            detection_timeout_ms: 0,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        // Invalid - timeout too high
        let invalid_config = ProjectMarkersConfig {
            detection_timeout_ms: 50000,
            ..Default::default()
        };
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
        let config_timeout0 = ProjectMarkersConfig {
            detection_timeout_ms: 0,
            ..Default::default()
        };
        let sanitized = config_timeout0.sanitized();
        assert_eq!(sanitized.detection_timeout_ms, 1000);

        // Test sanitizing timeout too high
        let config_timeout_high = ProjectMarkersConfig {
            detection_timeout_ms: 50000,
            ..Default::default()
        };
        let sanitized = config_timeout_high.sanitized();
        assert_eq!(sanitized.detection_timeout_ms, 30000);

        // Test sanitizing empty project name
        let mut config_with_markers = ProjectMarkersConfig::default();
        config_with_markers.markers.insert(
            "".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 50,
            },
        );
        config_with_markers.markers.insert(
            "valid_name".to_string(),
            ProjectMarker {
                markers: vec!["package.json".to_string()],
                language_server: "typescript-language-server".to_string(),
                root_strategy: RootStrategy::First,
                priority: 60,
            },
        );
        let sanitized = config_with_markers.sanitized();
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

        assert!(config.is_project_markers_enabled());
        assert_eq!(config.project_detection_timeout_ms(), 2000);
        assert!(!config.is_project_builtin_fallback_enabled());
        assert!(config.project_markers().markers.is_empty());
    }

    #[test]
    fn ui_font_resolves_system_ui_font_aliases_for_gpui() {
        let mut gui_config = GuiConfig::default();
        gui_config.ui.font = Some(FontConfig {
            family: "SF Pro Display".to_string(),
            weight: FontWeight::Normal,
            size: 13.0,
            line_height: 1.5,
        });

        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        let ui_font = config.ui_font();
        assert_eq!(ui_font.family, GPUI_SYSTEM_UI_FONT);
        assert_eq!(ui_font.size, 13.0);
    }

    #[test]
    fn default_ui_font_uses_platform_ui_font_defaults() {
        let config = Config {
            helix: HelixConfig::default(),
            gui: GuiConfig::default(),
        };

        let ui_font = config.ui_font();

        assert_eq!(ui_font.family, GPUI_SYSTEM_UI_FONT);
        assert_eq!(ui_font.weight, FontWeight::Normal);
        assert_eq!(ui_font.size, default_ui_font_size());
        assert_eq!(ui_font.line_height, DEFAULT_UI_FONT_LINE_HEIGHT);
        assert!(ui_font.size.is_finite());
        assert!(ui_font.size > 0.0);
    }

    #[test]
    fn explicit_ui_font_size_overrides_platform_default() {
        let mut gui_config = GuiConfig::default();
        gui_config.ui.font = Some(FontConfig {
            family: GPUI_SYSTEM_UI_FONT.to_string(),
            weight: FontWeight::Normal,
            size: 16.0,
            line_height: 1.4,
        });
        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        let ui_font = config.ui_font();

        assert_eq!(ui_font.size, 16.0);
        assert_eq!(ui_font.line_height, 1.4);
    }

    #[test]
    fn windows_default_ui_font_size_has_comfortable_floor() {
        assert_eq!(windows_default_ui_font_size(11.0), 14.0);
        assert_eq!(windows_default_ui_font_size(12.0), 14.0);
        assert_eq!(windows_default_ui_font_size(13.0), 14.0);
        assert_eq!(windows_default_ui_font_size(14.0), 14.0);
    }

    #[test]
    fn partial_ui_font_table_uses_ui_defaults() {
        let gui_config: GuiConfig = toml::from_str(
            r#"
[ui.font]
size = 20.0
"#,
        )
        .expect("size-only UI font config should parse");
        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        let ui_font = config.ui_font();
        assert_eq!(ui_font.family, GPUI_SYSTEM_UI_FONT);
        assert_eq!(ui_font.weight, FontWeight::Normal);
        assert_eq!(ui_font.size, 20.0);
        assert_eq!(ui_font.line_height, DEFAULT_UI_FONT_LINE_HEIGHT);

        let editor_font = config.editor_font();
        assert_eq!(editor_font.family, GPUI_SYSTEM_UI_FONT);
        assert_eq!(editor_font.size, 20.0);
    }

    #[test]
    fn partial_editor_font_table_uses_editor_defaults() {
        let gui_config: GuiConfig = toml::from_str(
            r#"
[editor.font]
size = 18.0
"#,
        )
        .expect("size-only editor font config should parse");
        let config = Config {
            helix: HelixConfig::default(),
            gui: gui_config,
        };

        let default_editor_font = FontConfig::default();
        let editor_font = config.editor_font();
        assert_eq!(editor_font.family, default_editor_font.family);
        assert_eq!(editor_font.weight, default_editor_font.weight);
        assert_eq!(editor_font.size, 18.0);
        assert_eq!(editor_font.line_height, default_editor_font.line_height);
    }

    #[test]
    fn windows_logfont_height_is_normalized_to_logical_pixels() {
        assert_eq!(
            logical_font_size_from_windows_logfont_height(-12, 96),
            Some(12.0)
        );
        assert_eq!(
            logical_font_size_from_windows_logfont_height(-18, 144),
            Some(12.0)
        );
        assert_eq!(
            logical_font_size_from_windows_logfont_height(-24, 192),
            Some(12.0)
        );
    }

    #[test]
    fn windows_logfont_height_rejects_invalid_values() {
        assert_eq!(logical_font_size_from_windows_logfont_height(0, 96), None);
        assert_eq!(logical_font_size_from_windows_logfont_height(-12, 0), None);
        assert_eq!(
            logical_font_size_from_windows_logfont_height(i32::MIN, 96),
            None
        );
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
        assert!(config.project_markers.enable_project_markers);
        assert_eq!(config.project_markers.detection_timeout_ms, 1500);
        assert!(config.project_markers.enable_builtin_fallback);

        // Test that other configs still work
        assert!(config.lsp.project_lsp_startup);
        assert_eq!(config.lsp.startup_timeout_ms, 3000);

        let ui_font = config.ui.font.as_ref().expect("UI font should be set");
        assert_eq!(ui_font.family, "Inter");
        assert_eq!(ui_font.size, 13.0);
    }
}
