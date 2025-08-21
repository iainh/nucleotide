// ABOUTME: Enhanced Helix theme bridge for seamless integration with Helix editor themes
// ABOUTME: Provides bi-directional conversion, theme discovery, and compatibility layer

use crate::Theme;
use gpui::Hsla;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Enhanced bridge between Helix themes and nucleotide-ui themes
#[derive(Debug, Clone)]
pub struct HelixThemeBridge {
    /// Theme discovery configuration
    discovery_config: ThemeDiscoveryConfig,
    /// Conversion mappings
    color_mappings: ColorMappings,
    /// Cache for loaded themes
    theme_cache: HashMap<String, CachedHelixTheme>,
}

/// Theme discovery configuration
#[derive(Debug, Clone)]
pub struct ThemeDiscoveryConfig {
    /// Helix runtime directory paths to search
    pub runtime_paths: Vec<PathBuf>,
    /// Theme file extensions to recognize
    pub theme_extensions: Vec<String>,
    /// Enable automatic theme watching
    pub auto_watch: bool,
    /// Refresh interval for theme discovery
    pub refresh_interval: std::time::Duration,
}

/// Color mapping configuration between Helix and nucleotide-ui
#[derive(Debug, Clone)]
pub struct ColorMappings {
    /// UI color mappings (Helix UI -> nucleotide-ui)
    pub ui_mappings: HashMap<String, String>,
    /// Syntax color mappings (Helix syntax -> nucleotide-ui)
    pub syntax_mappings: HashMap<String, String>,
    /// Fallback colors when mappings are missing
    pub fallback_colors: HashMap<String, Hsla>,
    /// Custom color transformations
    pub color_transformations: Vec<ColorTransformation>,
}

/// Compatibility settings for Helix integration
#[derive(Debug, Clone)]
pub struct CompatibilitySettings {
    /// Helix version compatibility
    pub target_helix_version: String,
    /// Enable backwards compatibility mode
    pub backwards_compatible: bool,
    /// Handle missing colors gracefully
    pub graceful_degradation: bool,
    /// Preserve Helix semantic meanings
    pub preserve_semantics: bool,
    /// Enable color adaptation for different contexts
    pub adaptive_colors: bool,
}

/// Color transformation function
#[derive(Debug, Clone)]
pub struct ColorTransformation {
    /// Source color selector
    pub source: String,
    /// Target color name
    pub target: String,
    /// Transformation type
    pub transformation: TransformationType,
}

/// Types of color transformations
#[derive(Debug, Clone)]
pub enum TransformationType {
    /// Direct copy
    Direct,
    /// Lighten by percentage
    Lighten(f32),
    /// Darken by percentage
    Darken(f32),
    /// Adjust saturation
    Saturate(f32),
    /// Desaturate
    Desaturate(f32),
    /// Shift hue by degrees
    HueShift(f32),
    /// Custom function
    Custom(fn(Hsla) -> Hsla),
}

/// Cached Helix theme data
#[derive(Debug, Clone)]
pub struct CachedHelixTheme {
    /// Original Helix theme data
    pub helix_data: HelixThemeData,
    /// Converted nucleotide-ui theme
    pub nucleotide_theme: Theme,
    /// Cache timestamp
    pub cached_at: std::time::SystemTime,
    /// Source file path
    pub source_path: PathBuf,
    /// Theme metadata
    pub metadata: HelixThemeMetadata,
}

/// Helix theme data structure
#[derive(Debug, Clone)]
pub struct HelixThemeData {
    /// Theme name
    pub name: String,
    /// Palette colors
    pub palette: HashMap<String, String>,
    /// UI colors
    pub ui: HashMap<String, String>,
    /// Syntax highlighting colors
    pub syntax: HashMap<String, String>,
    /// Theme inherits from another theme
    pub inherits: Option<String>,
}

/// Helix theme metadata
#[derive(Debug, Clone)]
pub struct HelixThemeMetadata {
    /// Theme author
    pub author: Option<String>,
    /// Theme description
    pub description: Option<String>,
    /// Theme version
    pub version: Option<String>,
    /// Supported Helix versions
    pub helix_versions: Vec<String>,
    /// Theme tags
    pub tags: Vec<String>,
}

/// Helix theme discovery result
#[derive(Debug, Clone)]
pub struct HelixThemeDiscovery {
    /// Discovered themes
    pub themes: Vec<DiscoveredHelixTheme>,
    /// Discovery errors
    pub errors: Vec<ThemeDiscoveryError>,
    /// Discovery timestamp
    pub discovered_at: std::time::SystemTime,
}

/// Discovered Helix theme information
#[derive(Debug, Clone)]
pub struct DiscoveredHelixTheme {
    /// Theme name
    pub name: String,
    /// Theme file path
    pub path: PathBuf,
    /// Theme metadata
    pub metadata: HelixThemeMetadata,
    /// Whether theme is built-in
    pub is_builtin: bool,
    /// Theme file size
    pub file_size: u64,
    /// Last modified time
    pub modified_at: std::time::SystemTime,
}

/// Theme discovery errors
#[derive(Debug, Clone)]
pub enum ThemeDiscoveryError {
    /// File system error
    FileSystemError(String, PathBuf),
    /// Parse error
    ParseError(String, PathBuf),
    /// Invalid theme structure
    InvalidTheme(String, PathBuf),
    /// Access denied
    AccessDenied(PathBuf),
}

impl Default for ThemeDiscoveryConfig {
    fn default() -> Self {
        Self {
            runtime_paths: vec![
                PathBuf::from("~/.config/helix/themes"),
                PathBuf::from("/usr/share/helix/runtime/themes"),
                PathBuf::from("./runtime/themes"),
            ],
            theme_extensions: vec!["toml".to_string()],
            auto_watch: false,
            refresh_interval: std::time::Duration::from_secs(30),
        }
    }
}

impl Default for ColorMappings {
    fn default() -> Self {
        let mut ui_mappings = HashMap::new();

        // Common UI element mappings
        ui_mappings.insert("ui.background".to_string(), "background".to_string());
        ui_mappings.insert(
            "ui.background.separator".to_string(),
            "border_default".to_string(),
        );
        ui_mappings.insert("ui.foreground".to_string(), "text_primary".to_string());
        ui_mappings.insert("ui.window".to_string(), "surface".to_string());
        ui_mappings.insert("ui.text".to_string(), "text_primary".to_string());
        ui_mappings.insert("ui.text.focus".to_string(), "primary".to_string());
        ui_mappings.insert("ui.selection".to_string(), "primary".to_string());
        ui_mappings.insert("ui.cursor".to_string(), "primary".to_string());
        ui_mappings.insert("ui.cursor.primary".to_string(), "primary".to_string());
        ui_mappings.insert("ui.cursor.match".to_string(), "secondary".to_string());
        ui_mappings.insert("ui.linenr".to_string(), "text_secondary".to_string());
        ui_mappings.insert("ui.statusline".to_string(), "surface".to_string());
        ui_mappings.insert("ui.statusline.active".to_string(), "primary".to_string());
        ui_mappings.insert("ui.popup".to_string(), "surface".to_string());
        ui_mappings.insert("ui.menu".to_string(), "surface".to_string());
        ui_mappings.insert("ui.help".to_string(), "surface".to_string());
        ui_mappings.insert("warning".to_string(), "warning".to_string());
        ui_mappings.insert("error".to_string(), "error".to_string());
        ui_mappings.insert("info".to_string(), "success".to_string());
        ui_mappings.insert("hint".to_string(), "text_secondary".to_string());

        let mut syntax_mappings = HashMap::new();

        // Syntax highlighting mappings
        syntax_mappings.insert("keyword".to_string(), "primary".to_string());
        syntax_mappings.insert("function".to_string(), "secondary".to_string());
        syntax_mappings.insert("string".to_string(), "success".to_string());
        syntax_mappings.insert("comment".to_string(), "text_secondary".to_string());
        syntax_mappings.insert("constant".to_string(), "warning".to_string());
        syntax_mappings.insert("type".to_string(), "primary".to_string());
        syntax_mappings.insert("variable".to_string(), "text_primary".to_string());

        let mut fallback_colors = HashMap::new();
        fallback_colors.insert(
            "primary".to_string(),
            Hsla {
                h: 220.0,
                s: 0.8,
                l: 0.6,
                a: 1.0,
            },
        );
        fallback_colors.insert(
            "secondary".to_string(),
            Hsla {
                h: 180.0,
                s: 0.7,
                l: 0.5,
                a: 1.0,
            },
        );
        fallback_colors.insert(
            "background".to_string(),
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.95,
                a: 1.0,
            },
        );
        fallback_colors.insert(
            "text_primary".to_string(),
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.1,
                a: 1.0,
            },
        );

        Self {
            ui_mappings,
            syntax_mappings,
            fallback_colors,
            color_transformations: Vec::new(),
        }
    }
}

impl Default for CompatibilitySettings {
    fn default() -> Self {
        Self {
            target_helix_version: "23.10".to_string(),
            backwards_compatible: true,
            graceful_degradation: true,
            preserve_semantics: true,
            adaptive_colors: true,
        }
    }
}

impl HelixThemeBridge {
    /// Create a new Helix theme bridge
    pub fn new() -> Self {
        Self {
            discovery_config: ThemeDiscoveryConfig::default(),
            color_mappings: ColorMappings::default(),
            theme_cache: HashMap::new(),
        }
    }

    /// Create bridge with custom configuration
    pub fn with_config(
        discovery_config: ThemeDiscoveryConfig,
        color_mappings: ColorMappings,
        _compatibility: CompatibilitySettings,
    ) -> Self {
        Self {
            discovery_config,
            color_mappings,
            theme_cache: HashMap::new(),
        }
    }

    /// Discover available Helix themes
    pub fn discover_themes(&self) -> HelixThemeDiscovery {
        let mut themes = Vec::new();
        let mut errors = Vec::new();

        nucleotide_logging::debug!(
            paths = ?self.discovery_config.runtime_paths,
            "Starting Helix theme discovery"
        );

        for runtime_path in &self.discovery_config.runtime_paths {
            match self.scan_theme_directory(runtime_path) {
                Ok(discovered) => themes.extend(discovered),
                Err(discovery_errors) => errors.extend(discovery_errors),
            }
        }

        nucleotide_logging::info!(
            themes_found = themes.len(),
            errors_count = errors.len(),
            "Helix theme discovery completed"
        );

        HelixThemeDiscovery {
            themes,
            errors,
            discovered_at: std::time::SystemTime::now(),
        }
    }

    /// Load and convert a Helix theme by name
    pub fn load_helix_theme(&mut self, theme_name: &str) -> Result<Theme, HelixBridgeError> {
        // Check cache first
        if let Some(cached) = self.theme_cache.get(theme_name) {
            if cached.cached_at.elapsed().unwrap_or_default() < std::time::Duration::from_secs(300)
            {
                nucleotide_logging::debug!(theme_name = theme_name, "Retrieved theme from cache");
                return Ok(cached.nucleotide_theme.clone());
            }
        }

        // Discover and find the theme
        let discovery = self.discover_themes();
        let theme_info = discovery
            .themes
            .iter()
            .find(|t| t.name == theme_name)
            .ok_or_else(|| HelixBridgeError::ThemeNotFound(theme_name.to_string()))?;

        // Load and parse theme file
        let helix_data = self.parse_helix_theme_file(&theme_info.path)?;

        // Convert to nucleotide-ui theme
        let nucleotide_theme = self.convert_helix_to_nucleotide(&helix_data)?;

        // Cache the result
        let cached_theme = CachedHelixTheme {
            helix_data,
            nucleotide_theme: nucleotide_theme.clone(),
            cached_at: std::time::SystemTime::now(),
            source_path: theme_info.path.clone(),
            metadata: theme_info.metadata.clone(),
        };

        self.theme_cache
            .insert(theme_name.to_string(), cached_theme);

        nucleotide_logging::info!(
            theme_name = theme_name,
            source_path = ?theme_info.path,
            "Helix theme loaded and converted"
        );

        Ok(nucleotide_theme)
    }

    /// Convert nucleotide-ui theme to Helix format
    pub fn convert_nucleotide_to_helix(
        &self,
        theme: &Theme,
        name: &str,
    ) -> Result<HelixThemeData, HelixBridgeError> {
        let mut palette = HashMap::new();
        let mut ui = HashMap::new();
        let mut syntax = HashMap::new();

        // Convert colors to hex strings
        palette.insert(
            "primary".to_string(),
            self.hsla_to_hex(theme.tokens.colors.primary),
        );
        palette.insert(
            "secondary".to_string(),
            self.hsla_to_hex(theme.tokens.colors.text_secondary),
        );
        palette.insert(
            "background".to_string(),
            self.hsla_to_hex(theme.tokens.colors.background),
        );
        palette.insert(
            "surface".to_string(),
            self.hsla_to_hex(theme.tokens.colors.surface),
        );
        palette.insert(
            "text_primary".to_string(),
            self.hsla_to_hex(theme.tokens.colors.text_primary),
        );
        palette.insert(
            "text_secondary".to_string(),
            self.hsla_to_hex(theme.tokens.colors.text_secondary),
        );
        palette.insert(
            "border_default".to_string(),
            self.hsla_to_hex(theme.tokens.colors.border_default),
        );
        palette.insert(
            "error".to_string(),
            self.hsla_to_hex(theme.tokens.colors.error),
        );
        palette.insert(
            "warning".to_string(),
            self.hsla_to_hex(theme.tokens.colors.warning),
        );
        palette.insert(
            "success".to_string(),
            self.hsla_to_hex(theme.tokens.colors.success),
        );

        // Map UI colors using reverse mappings
        for (helix_key, nucleotide_key) in &self.color_mappings.ui_mappings {
            if let Some(color_name) = self.get_color_name_from_nucleotide_key(nucleotide_key) {
                if let Some(hex_color) = palette.get(color_name) {
                    ui.insert(helix_key.clone(), hex_color.clone());
                }
            }
        }

        // Map syntax colors
        for (helix_key, nucleotide_key) in &self.color_mappings.syntax_mappings {
            if let Some(color_name) = self.get_color_name_from_nucleotide_key(nucleotide_key) {
                if let Some(hex_color) = palette.get(color_name) {
                    syntax.insert(helix_key.clone(), hex_color.clone());
                }
            }
        }

        Ok(HelixThemeData {
            name: name.to_string(),
            palette,
            ui,
            syntax,
            inherits: None,
        })
    }

    /// Export nucleotide-ui theme to Helix TOML format
    pub fn export_to_helix_toml(
        &self,
        theme: &Theme,
        name: &str,
    ) -> Result<String, HelixBridgeError> {
        let helix_data = self.convert_nucleotide_to_helix(theme, name)?;

        let mut toml_content = String::new();

        // Add header comment
        toml_content.push_str(&format!("# {} theme exported from nucleotide-ui\n", name));
        toml_content.push_str(&format!(
            "# Generated on: {:?}\n\n",
            std::time::SystemTime::now()
        ));

        // Add palette section
        toml_content.push_str("[palette]\n");
        for (key, value) in &helix_data.palette {
            toml_content.push_str(&format!("{} = \"{}\"\n", key, value));
        }
        toml_content.push('\n');

        // Add UI section
        toml_content.push_str("[ui]\n");
        for (key, value) in &helix_data.ui {
            if value.starts_with('#') {
                toml_content.push_str(&format!("\"{}\" = \"{}\"\n", key, value));
            } else {
                toml_content.push_str(&format!("\"{}\" = \"{}\"\n", key, value));
            }
        }
        toml_content.push('\n');

        // Add syntax section
        toml_content.push_str("[syntax]\n");
        for (key, value) in &helix_data.syntax {
            if value.starts_with('#') {
                toml_content.push_str(&format!("\"{}\" = \"{}\"\n", key, value));
            } else {
                toml_content.push_str(&format!("\"{}\" = \"{}\"\n", key, value));
            }
        }

        Ok(toml_content)
    }

    /// Configure color mappings
    pub fn configure_mappings<F>(&mut self, configurator: F)
    where
        F: FnOnce(&mut ColorMappings),
    {
        configurator(&mut self.color_mappings);
    }

    /// Scan a directory for theme files
    fn scan_theme_directory(
        &self,
        path: &Path,
    ) -> Result<Vec<DiscoveredHelixTheme>, Vec<ThemeDiscoveryError>> {
        let mut themes = Vec::new();
        let mut errors = Vec::new();

        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) => {
                errors.push(ThemeDiscoveryError::FileSystemError(
                    e.to_string(),
                    path.to_path_buf(),
                ));
                return Err(errors);
            }
        };

        for entry in entries {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if let Some(extension) = path.extension() {
                        if self
                            .discovery_config
                            .theme_extensions
                            .contains(&extension.to_string_lossy().to_string())
                        {
                            match self.parse_theme_metadata(&path) {
                                Ok(discovered) => themes.push(discovered),
                                Err(error) => errors.push(error),
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(ThemeDiscoveryError::FileSystemError(
                        e.to_string(),
                        path.to_path_buf(),
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(themes)
        } else {
            Err(errors)
        }
    }

    /// Parse theme metadata from file
    fn parse_theme_metadata(
        &self,
        path: &Path,
    ) -> Result<DiscoveredHelixTheme, ThemeDiscoveryError> {
        let metadata = std::fs::metadata(path)
            .map_err(|e| ThemeDiscoveryError::FileSystemError(e.to_string(), path.to_path_buf()))?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // For now, create basic metadata - in a real implementation,
        // this would parse the actual theme file
        let theme_metadata = HelixThemeMetadata {
            author: None,
            description: None,
            version: None,
            helix_versions: vec!["23.10".to_string()],
            tags: Vec::new(),
        };

        Ok(DiscoveredHelixTheme {
            name,
            path: path.to_path_buf(),
            metadata: theme_metadata,
            is_builtin: path.to_string_lossy().contains("/usr/share/helix/"),
            file_size: metadata.len(),
            modified_at: metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        })
    }

    /// Parse Helix theme file
    fn parse_helix_theme_file(&self, path: &Path) -> Result<HelixThemeData, HelixBridgeError> {
        let _content = std::fs::read_to_string(path)
            .map_err(|e| HelixBridgeError::FileError(e.to_string()))?;

        // This is a simplified parser - a real implementation would use a proper TOML parser
        // and handle inheritance, complex color definitions, etc.

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        Ok(HelixThemeData {
            name,
            palette: HashMap::new(),
            ui: HashMap::new(),
            syntax: HashMap::new(),
            inherits: None,
        })
    }

    /// Convert Helix theme to nucleotide-ui theme
    fn convert_helix_to_nucleotide(
        &self,
        helix_data: &HelixThemeData,
    ) -> Result<Theme, HelixBridgeError> {
        // Start with a base theme
        let mut theme = Theme::light();

        // Apply color mappings
        for (helix_key, nucleotide_key) in &self.color_mappings.ui_mappings {
            if let Some(helix_color) = helix_data.ui.get(helix_key) {
                if let Ok(hsla_color) = self.parse_helix_color(helix_color, &helix_data.palette) {
                    self.apply_color_to_theme(&mut theme, nucleotide_key, hsla_color);
                }
            }
        }

        // Apply fallback colors for any missing essential colors
        for (nucleotide_key, fallback_color) in &self.color_mappings.fallback_colors {
            if self.is_color_missing(&theme, nucleotide_key) {
                self.apply_color_to_theme(&mut theme, nucleotide_key, *fallback_color);
            }
        }

        // Rebuild theme with new tokens
        theme = Theme::from_tokens(theme.tokens);

        Ok(theme)
    }

    /// Parse Helix color string to HSLA
    fn parse_helix_color(
        &self,
        color_str: &str,
        palette: &HashMap<String, String>,
    ) -> Result<Hsla, HelixBridgeError> {
        // Handle palette references
        if let Some(palette_color) = palette.get(color_str) {
            return self.parse_helix_color(palette_color, palette);
        }

        // Handle hex colors
        if color_str.starts_with('#') {
            return self.hex_to_hsla(color_str);
        }

        // Handle named colors (simplified)
        match color_str {
            "black" => Ok(Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0,
            }),
            "white" => Ok(Hsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 1.0,
            }),
            "red" => Ok(Hsla {
                h: 0.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
            "green" => Ok(Hsla {
                h: 120.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
            "blue" => Ok(Hsla {
                h: 240.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
            _ => Err(HelixBridgeError::InvalidColor(color_str.to_string())),
        }
    }

    /// Convert hex color to HSLA
    fn hex_to_hsla(&self, hex: &str) -> Result<Hsla, HelixBridgeError> {
        let hex = hex.trim_start_matches('#');

        if hex.len() != 6 {
            return Err(HelixBridgeError::InvalidColor(hex.to_string()));
        }

        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| HelixBridgeError::InvalidColor(hex.to_string()))? as f32
            / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| HelixBridgeError::InvalidColor(hex.to_string()))? as f32
            / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| HelixBridgeError::InvalidColor(hex.to_string()))? as f32
            / 255.0;

        Ok(self.rgb_to_hsla(r, g, b))
    }

    /// Convert HSLA to hex string
    fn hsla_to_hex(&self, hsla: Hsla) -> String {
        let (r, g, b) = self.hsla_to_rgb(hsla);
        format!(
            "#{:02x}{:02x}{:02x}",
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8
        )
    }

    /// Convert RGB to HSLA
    fn rgb_to_hsla(&self, r: f32, g: f32, b: f32) -> Hsla {
        let max = r.max(g.max(b));
        let min = r.min(g.min(b));
        let delta = max - min;

        let l = (max + min) / 2.0;

        let s = if delta == 0.0 {
            0.0
        } else if l < 0.5 {
            delta / (max + min)
        } else {
            delta / (2.0 - max - min)
        };

        let h = if delta == 0.0 {
            0.0
        } else if max == r {
            60.0 * (((g - b) / delta) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };

        let h = if h < 0.0 { h + 360.0 } else { h };

        Hsla { h, s, l, a: 1.0 }
    }

    /// Convert HSLA to RGB
    fn hsla_to_rgb(&self, hsla: Hsla) -> (f32, f32, f32) {
        let c = (1.0 - (2.0 * hsla.l - 1.0).abs()) * hsla.s;
        let x = c * (1.0 - ((hsla.h / 60.0) % 2.0 - 1.0).abs());
        let m = hsla.l - c / 2.0;

        let (r_prime, g_prime, b_prime) = if hsla.h < 60.0 {
            (c, x, 0.0)
        } else if hsla.h < 120.0 {
            (x, c, 0.0)
        } else if hsla.h < 180.0 {
            (0.0, c, x)
        } else if hsla.h < 240.0 {
            (0.0, x, c)
        } else if hsla.h < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };

        (r_prime + m, g_prime + m, b_prime + m)
    }

    /// Apply color to theme based on nucleotide key
    fn apply_color_to_theme(&self, theme: &mut Theme, key: &str, color: Hsla) {
        match key {
            "primary" => theme.tokens.colors.primary = color,
            "secondary" => theme.tokens.colors.text_secondary = color,
            "background" => theme.tokens.colors.background = color,
            "surface" => theme.tokens.colors.surface = color,
            "text_primary" => theme.tokens.colors.text_primary = color,
            "text_secondary" => theme.tokens.colors.text_secondary = color,
            "border_default" => theme.tokens.colors.border_default = color,
            "error" => theme.tokens.colors.error = color,
            "warning" => theme.tokens.colors.warning = color,
            "success" => theme.tokens.colors.success = color,
            _ => {
                nucleotide_logging::debug!(
                    color_key = key,
                    "Unknown color key in theme conversion"
                );
            }
        }
    }

    /// Check if a color is missing from theme
    fn is_color_missing(&self, theme: &Theme, key: &str) -> bool {
        match key {
            "primary" => theme.tokens.colors.primary.a == 0.0,
            "secondary" => theme.tokens.colors.text_secondary.a == 0.0,
            "background" => theme.tokens.colors.background.a == 0.0,
            "surface" => theme.tokens.colors.surface.a == 0.0,
            "text_primary" => theme.tokens.colors.text_primary.a == 0.0,
            "text_secondary" => theme.tokens.colors.text_secondary.a == 0.0,
            "border_default" => theme.tokens.colors.border_default.a == 0.0,
            "error" => theme.tokens.colors.error.a == 0.0,
            "warning" => theme.tokens.colors.warning.a == 0.0,
            "success" => theme.tokens.colors.success.a == 0.0,
            _ => false,
        }
    }

    /// Get color name from nucleotide key
    fn get_color_name_from_nucleotide_key(&self, key: &str) -> Option<&str> {
        Some(match key {
            "primary" => "primary",
            "secondary" => "secondary",
            "background" => "background",
            "surface" => "surface",
            "text_primary" => "text_primary",
            "text_secondary" => "text_secondary",
            "border_default" => "border_default",
            "error" => "error",
            "warning" => "warning",
            "success" => "success",
            _ => return None,
        })
    }
}

impl Default for HelixThemeBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Helix bridge errors
#[derive(Debug, Clone)]
pub enum HelixBridgeError {
    /// Theme not found
    ThemeNotFound(String),
    /// File operation error
    FileError(String),
    /// Invalid color format
    InvalidColor(String),
    /// Parse error
    ParseError(String),
    /// Conversion error
    ConversionError(String),
}

impl std::fmt::Display for HelixBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelixBridgeError::ThemeNotFound(name) => write!(f, "Theme not found: {}", name),
            HelixBridgeError::FileError(msg) => write!(f, "File error: {}", msg),
            HelixBridgeError::InvalidColor(color) => write!(f, "Invalid color: {}", color),
            HelixBridgeError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            HelixBridgeError::ConversionError(msg) => write!(f, "Conversion error: {}", msg),
        }
    }
}

impl std::error::Error for HelixBridgeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_bridge_creation() {
        let bridge = HelixThemeBridge::new();
        assert!(!bridge.color_mappings.ui_mappings.is_empty());
        // Note: compatibility field was removed from HelixThemeBridge structure
    }

    #[test]
    fn test_hex_to_hsla_conversion() {
        let bridge = HelixThemeBridge::new();

        let red_hex = "#ff0000";
        let red_hsla = bridge.hex_to_hsla(red_hex).unwrap();

        assert_eq!(red_hsla.h, 0.0);
        assert_eq!(red_hsla.s, 1.0);
        assert_eq!(red_hsla.l, 0.5);
        assert_eq!(red_hsla.a, 1.0);
    }

    #[test]
    fn test_hsla_to_hex_conversion() {
        let bridge = HelixThemeBridge::new();

        let blue_hsla = Hsla {
            h: 240.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let blue_hex = bridge.hsla_to_hex(blue_hsla);

        assert_eq!(blue_hex, "#0000ff");
    }

    #[test]
    fn test_color_mapping() {
        let bridge = HelixThemeBridge::new();

        assert_eq!(
            bridge.color_mappings.ui_mappings.get("ui.background"),
            Some(&"background".to_string())
        );
        assert_eq!(
            bridge.color_mappings.ui_mappings.get("ui.foreground"),
            Some(&"text_primary".to_string())
        );
    }

    #[test]
    fn test_nucleotide_to_helix_conversion() {
        let bridge = HelixThemeBridge::new();
        let theme = Theme::light();

        let helix_data = bridge.convert_nucleotide_to_helix(&theme, "test").unwrap();

        assert_eq!(helix_data.name, "test");
        assert!(!helix_data.palette.is_empty());
        assert!(!helix_data.ui.is_empty());
    }

    #[test]
    fn test_toml_export() {
        let bridge = HelixThemeBridge::new();
        let theme = Theme::light();

        let toml_content = bridge.export_to_helix_toml(&theme, "test").unwrap();

        assert!(toml_content.contains("[palette]"));
        assert!(toml_content.contains("[ui]"));
        assert!(toml_content.contains("[syntax]"));
        assert!(toml_content.contains("# test theme exported from nucleotide-ui"));
    }
}
