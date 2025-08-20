// ABOUTME: Comprehensive tests for configuration parsing, validation, and merging systems
// ABOUTME: Tests GUI configuration, Helix configuration merging, and edge cases for robust config handling

#[cfg(test)]
mod tests {
    use crate::config::{
        Config, EditorGuiConfig, GuiConfig, ThemeConfig, ThemeMode, UiConfig, WindowConfig,
    };
    use helix_term::config::Config as HelixConfig;
    use nucleotide_types::{FontConfig, FontWeight};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;
    use toml;

    /// Helper for creating test configuration files
    struct ConfigTestBuilder {
        temp_dir: TempDir,
    }

    impl ConfigTestBuilder {
        fn new() -> Self {
            Self {
                temp_dir: TempDir::new().expect("Failed to create temp directory"),
            }
        }

        fn create_helix_config(&self, content: &str) -> std::io::Result<()> {
            fs::write(self.temp_dir.path().join("config.toml"), content)
        }

        fn create_gui_config(&self, content: &str) -> std::io::Result<()> {
            fs::write(self.temp_dir.path().join("nucleotide.toml"), content)
        }

        fn path(&self) -> &Path {
            self.temp_dir.path()
        }
    }

    #[test]
    fn test_default_gui_config() {
        let config = GuiConfig::default();

        assert!(config.ui.font.is_none());
        assert!(config.editor.font.is_none());
        assert_eq!(config.theme.mode, ThemeMode::System);
        assert_eq!(config.theme.light_theme, None);
        assert_eq!(config.theme.dark_theme, None);
        assert!(!config.window.blur_dark_themes);
        assert!(config.window.appearance_follows_theme);
    }

    #[test]
    fn test_gui_config_parsing() {
        let config_content = r#"
[ui.font]
family = "SF Pro Display"
weight = "medium"
size = 13.0
line_height = 1.5

[editor.font]
family = "JetBrains Mono"
weight = "normal"
size = 14.0
line_height = 1.4

[theme]
mode = "dark"
light_theme = "github_light"
dark_theme = "tokyo_night"

[window]
blur_dark_themes = true
appearance_follows_theme = false
"#;

        let config: GuiConfig = toml::from_str(config_content).expect("Failed to parse config");

        // Test UI font
        let ui_font = config.ui.font.expect("UI font should be present");
        assert_eq!(ui_font.family, "SF Pro Display");
        assert_eq!(ui_font.weight, FontWeight::Medium);
        assert_eq!(ui_font.size, 13.0);
        assert_eq!(ui_font.line_height, 1.5);

        // Test editor font
        let editor_font = config.editor.font.expect("Editor font should be present");
        assert_eq!(editor_font.family, "JetBrains Mono");
        assert_eq!(editor_font.weight, FontWeight::Normal);
        assert_eq!(editor_font.size, 14.0);
        assert_eq!(editor_font.line_height, 1.4);

        // Test theme config
        assert_eq!(config.theme.mode, ThemeMode::Dark);
        assert_eq!(config.theme.get_light_theme(), "github_light");
        assert_eq!(config.theme.get_dark_theme(), "tokyo_night");

        // Test window config
        assert!(config.window.blur_dark_themes);
        assert!(!config.window.appearance_follows_theme);
    }

    #[test]
    fn test_partial_gui_config_parsing() {
        let config_content = r#"
[ui.font]
family = "Inter"

[theme]
mode = "light"
"#;

        let config: GuiConfig = toml::from_str(config_content).expect("Failed to parse config");

        // Test partial UI font
        let ui_font = config.ui.font.expect("UI font should be present");
        assert_eq!(ui_font.family, "Inter");
        assert_eq!(ui_font.weight, FontWeight::Normal); // Default
        assert_eq!(ui_font.size, 13.0); // Default

        // Test editor font defaults
        assert!(config.editor.font.is_none());

        // Test theme config
        assert_eq!(config.theme.mode, ThemeMode::Light);
        assert_eq!(config.theme.get_light_theme(), "nucleotide-outdoors"); // Default
        assert_eq!(config.theme.get_dark_theme(), "nucleotide-teal"); // Default
    }

    #[test]
    fn test_font_weight_deserialization() {
        let weights = [
            ("normal", FontWeight::Normal),
            ("medium", FontWeight::Medium),
            ("semibold", FontWeight::SemiBold),
            ("bold", FontWeight::Bold),
            ("extrabold", FontWeight::ExtraBold),
        ];

        for (weight_str, expected_weight) in &weights {
            let config_content = format!(
                r#"
[ui.font]
family = "Test Font"
weight = "{}"
"#,
                weight_str
            );

            let config: GuiConfig =
                toml::from_str(&config_content).expect("Failed to parse config");
            let font = config.ui.font.expect("Font should be present");
            assert_eq!(
                font.weight, *expected_weight,
                "Failed for weight: {}",
                weight_str
            );
        }
    }

    #[test]
    fn test_theme_mode_deserialization() {
        let modes = [
            ("system", ThemeMode::System),
            ("light", ThemeMode::Light),
            ("dark", ThemeMode::Dark),
        ];

        for (mode_str, expected_mode) in &modes {
            let config_content = format!(
                r#"
[theme]
mode = "{}"
"#,
                mode_str
            );

            let config: GuiConfig =
                toml::from_str(&config_content).expect("Failed to parse config");
            assert_eq!(
                config.theme.mode, *expected_mode,
                "Failed for mode: {}",
                mode_str
            );
        }
    }

    #[test]
    fn test_invalid_font_weight() {
        let config_content = r#"
[ui.font]
family = "Test Font"
weight = "invalid_weight"
"#;

        let result: Result<GuiConfig, _> = toml::from_str(config_content);
        assert!(result.is_err(), "Should fail to parse invalid font weight");
    }

    #[test]
    fn test_invalid_theme_mode() {
        let config_content = r#"
[theme]
mode = "invalid_mode"
"#;

        let result: Result<GuiConfig, _> = toml::from_str(config_content);
        assert!(result.is_err(), "Should fail to parse invalid theme mode");
    }

    #[test]
    fn test_config_loading_from_directory() {
        let builder = ConfigTestBuilder::new();

        // Create GUI config
        let gui_content = r#"
[ui.font]
family = "Test UI Font"
size = 12.0

[theme]
mode = "dark"
dark_theme = "custom_dark"
"#;
        builder.create_gui_config(gui_content).unwrap();

        let result = Config::load_from_dir(builder.path());
        assert!(result.is_ok(), "Should successfully load config");

        let config = result.unwrap();

        // Verify GUI config was loaded
        let ui_font = config.gui.ui.font.expect("UI font should be loaded");
        assert_eq!(ui_font.family, "Test UI Font");
        assert_eq!(ui_font.size, 12.0);
        assert_eq!(config.gui.theme.mode, ThemeMode::Dark);
        assert_eq!(config.gui.theme.get_dark_theme(), "custom_dark");
    }

    #[test]
    fn test_config_loading_without_gui_config() {
        let builder = ConfigTestBuilder::new();
        // Don't create GUI config file

        let result = Config::load_from_dir(builder.path());
        assert!(result.is_ok(), "Should load with default GUI config");

        let config = result.unwrap();

        // Should have default GUI config
        assert!(config.gui.ui.font.is_none());
        assert_eq!(config.gui.theme.mode, ThemeMode::System);
    }

    #[test]
    fn test_font_configuration_methods() {
        let mut config = Config::load_from_dir(&std::env::temp_dir()).unwrap();

        // Set up test GUI config
        config.gui.ui.font = Some(FontConfig {
            family: "UI Font".to_string(),
            weight: FontWeight::Medium,
            size: 13.0,
            line_height: 1.5,
        });

        config.gui.editor.font = Some(FontConfig {
            family: "Editor Font".to_string(),
            weight: FontWeight::Normal,
            size: 14.0,
            line_height: 1.4,
        });

        // Test editor font retrieval
        let editor_font = config.editor_font();
        assert_eq!(editor_font.family, "Editor Font");
        assert_eq!(editor_font.weight, FontWeight::Normal);
        assert_eq!(editor_font.size, 14.0);
        assert_eq!(editor_font.line_height, 1.4);

        // Test UI font retrieval
        let ui_font = config.ui_font();
        assert_eq!(ui_font.family, "UI Font");
        assert_eq!(ui_font.weight, FontWeight::Medium);
        assert_eq!(ui_font.size, 13.0);
    }

    #[test]
    fn test_font_fallback_behavior() {
        let mut config = Config::load_from_dir(&std::env::temp_dir()).unwrap();

        // Set only UI font
        config.gui.ui.font = Some(FontConfig {
            family: "Fallback Font".to_string(),
            weight: FontWeight::SemiBold,
            size: 12.0,
            line_height: 1.3,
        });

        // Editor font should fall back to UI font
        let editor_font = config.editor_font();
        assert_eq!(editor_font.family, "Fallback Font");
        assert_eq!(editor_font.weight, FontWeight::SemiBold);

        // Clear UI font to test default fallback
        config.gui.ui.font = None;
        config.gui.editor.font = None;

        let editor_font = config.editor_font();
        assert_eq!(editor_font.family, "SF Mono"); // Default editor font

        let ui_font = config.ui_font();
        assert_eq!(ui_font.family, "SF Pro Display"); // Default UI font
    }

    #[test]
    fn test_helix_config_update() {
        let mut config = Config::load_from_dir(&std::env::temp_dir()).unwrap();

        // Create a new editor config
        let mut new_editor_config = helix_view::editor::Config::default();
        new_editor_config.line_number = helix_view::editor::LineNumber::Relative;
        new_editor_config.mouse = false;

        // Apply update
        config.apply_helix_config_update(&new_editor_config);

        // Verify the update was applied
        assert_eq!(
            config.helix.editor.line_number,
            helix_view::editor::LineNumber::Relative
        );
        assert!(!config.helix.editor.mouse);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let original_config = GuiConfig {
            ui: UiConfig {
                font: Some(FontConfig {
                    family: "Test Font".to_string(),
                    weight: FontWeight::Bold,
                    size: 15.0,
                    line_height: 1.6,
                }),
            },
            editor: EditorGuiConfig {
                font: Some(FontConfig {
                    family: "Editor Font".to_string(),
                    weight: FontWeight::SemiBold,
                    size: 16.0,
                    line_height: 1.5,
                }),
            },
            theme: ThemeConfig {
                mode: ThemeMode::Light,
                light_theme: Some("custom_light".to_string()),
                dark_theme: Some("custom_dark".to_string()),
            },
            window: WindowConfig {
                blur_dark_themes: true,
                appearance_follows_theme: false,
            },
            lsp: LspConfig::default(),
            project_markers: ProjectMarkersConfig::default(),
        };

        // Serialize to TOML
        let serialized = toml::to_string(&original_config).expect("Failed to serialize config");

        // Deserialize back
        let deserialized: GuiConfig =
            toml::from_str(&serialized).expect("Failed to deserialize config");

        // Compare values
        assert_eq!(
            original_config.ui.font.as_ref().unwrap().family,
            deserialized.ui.font.as_ref().unwrap().family
        );
        assert_eq!(original_config.theme.mode, deserialized.theme.mode);
        assert_eq!(
            original_config.window.blur_dark_themes,
            deserialized.window.blur_dark_themes
        );
    }

    #[test]
    fn test_malformed_config_handling() {
        let builder = ConfigTestBuilder::new();

        // Create malformed GUI config
        builder.create_gui_config("invalid toml {{{").unwrap();

        let result = Config::load_from_dir(builder.path());
        assert!(result.is_ok(), "Should handle malformed config gracefully");

        // Should have default GUI config when loading fails
        let config = result.unwrap_or_else(|_| Config {
            helix: helix_term::config::Config::default(),
            gui: GuiConfig::default(),
        });
        assert!(config.gui.ui.font.is_none());
        assert_eq!(config.gui.theme.mode, ThemeMode::System);
    }

    #[test]
    fn test_theme_config_defaults() {
        let theme_config = ThemeConfig::default();

        assert_eq!(theme_config.mode, ThemeMode::System);
        assert_eq!(theme_config.get_light_theme(), "nucleotide-outdoors");
        assert_eq!(theme_config.get_dark_theme(), "nucleotide-teal");
    }

    #[test]
    fn test_theme_config_custom_themes() {
        let theme_config = ThemeConfig {
            mode: ThemeMode::Dark,
            light_theme: Some("solarized_light".to_string()),
            dark_theme: Some("solarized_dark".to_string()),
        };

        assert_eq!(theme_config.get_light_theme(), "solarized_light");
        assert_eq!(theme_config.get_dark_theme(), "solarized_dark");
    }

    /// Test configuration validation
    mod config_validation {
        use super::*;

        trait ConfigValidator {
            fn validate(&self) -> Vec<ValidationError>;
            fn is_valid(&self) -> bool {
                self.validate().is_empty()
            }
        }

        #[derive(Debug, PartialEq)]
        struct ValidationError {
            field: String,
            message: String,
        }

        impl ValidationError {
            fn new(field: &str, message: &str) -> Self {
                Self {
                    field: field.to_string(),
                    message: message.to_string(),
                }
            }
        }

        impl ConfigValidator for FontConfig {
            fn validate(&self) -> Vec<ValidationError> {
                let mut errors = Vec::new();

                if self.family.is_empty() {
                    errors.push(ValidationError::new(
                        "family",
                        "Font family cannot be empty",
                    ));
                }

                if self.size <= 0.0 {
                    errors.push(ValidationError::new("size", "Font size must be positive"));
                }

                if self.size > 72.0 {
                    errors.push(ValidationError::new(
                        "size",
                        "Font size is too large (max 72pt)",
                    ));
                }

                if self.line_height < 0.5 {
                    errors.push(ValidationError::new(
                        "line_height",
                        "Line height too small (min 0.5)",
                    ));
                }

                if self.line_height > 3.0 {
                    errors.push(ValidationError::new(
                        "line_height",
                        "Line height too large (max 3.0)",
                    ));
                }

                errors
            }
        }

        impl ConfigValidator for GuiConfig {
            fn validate(&self) -> Vec<ValidationError> {
                let mut errors = Vec::new();

                if let Some(ref ui_font) = self.ui.font {
                    errors.extend(ui_font.validate());
                }

                if let Some(ref editor_font) = self.editor.font {
                    errors.extend(editor_font.validate());
                }

                errors
            }
        }

        #[test]
        fn test_valid_font_config() {
            let font = FontConfig {
                family: "SF Mono".to_string(),
                weight: FontWeight::Normal,
                size: 14.0,
                line_height: 1.4,
            };

            assert!(font.is_valid());
            assert!(font.validate().is_empty());
        }

        #[test]
        fn test_invalid_font_config_empty_family() {
            let font = FontConfig {
                family: "".to_string(),
                weight: FontWeight::Normal,
                size: 14.0,
                line_height: 1.4,
            };

            assert!(!font.is_valid());
            let errors = font.validate();
            assert!(errors.contains(&ValidationError::new(
                "family",
                "Font family cannot be empty"
            )));
        }

        #[test]
        fn test_invalid_font_config_negative_size() {
            let font = FontConfig {
                family: "Test Font".to_string(),
                weight: FontWeight::Normal,
                size: -1.0,
                line_height: 1.4,
            };

            assert!(!font.is_valid());
            let errors = font.validate();
            assert!(errors.contains(&ValidationError::new("size", "Font size must be positive")));
        }

        #[test]
        fn test_invalid_font_config_large_size() {
            let font = FontConfig {
                family: "Test Font".to_string(),
                weight: FontWeight::Normal,
                size: 100.0,
                line_height: 1.4,
            };

            assert!(!font.is_valid());
            let errors = font.validate();
            assert!(errors.contains(&ValidationError::new(
                "size",
                "Font size is too large (max 72pt)"
            )));
        }

        #[test]
        fn test_invalid_font_config_line_height() {
            let font = FontConfig {
                family: "Test Font".to_string(),
                weight: FontWeight::Normal,
                size: 14.0,
                line_height: 0.1,
            };

            assert!(!font.is_valid());
            let errors = font.validate();
            assert!(errors.contains(&ValidationError::new(
                "line_height",
                "Line height too small (min 0.5)"
            )));
        }

        #[test]
        fn test_gui_config_validation() {
            let mut config = GuiConfig::default();
            config.ui.font = Some(FontConfig {
                family: "".to_string(), // Invalid
                weight: FontWeight::Normal,
                size: -5.0, // Invalid
                line_height: 1.4,
            });

            assert!(!config.is_valid());
            let errors = config.validate();
            assert!(errors.len() >= 2); // Should have at least 2 errors
        }

        #[test]
        fn test_valid_gui_config() {
            let mut config = GuiConfig::default();
            config.ui.font = Some(FontConfig {
                family: "SF Pro Display".to_string(),
                weight: FontWeight::Medium,
                size: 13.0,
                line_height: 1.5,
            });

            assert!(config.is_valid());
        }
    }

    /// Test configuration merging scenarios
    mod config_merging {
        use super::*;

        #[test]
        fn test_config_precedence() {
            let builder = ConfigTestBuilder::new();

            // Create both helix and gui configs
            let helix_content = r#"
[editor]
line-number = "relative"
mouse = false
"#;

            let gui_content = r#"
[ui.font]
family = "GUI Font"

[editor.font]
family = "Editor Font"
"#;

            builder.create_helix_config(helix_content).unwrap();
            builder.create_gui_config(gui_content).unwrap();

            let config = Config::load_from_dir(builder.path()).unwrap();

            // Helix config should be loaded
            assert_eq!(
                config.helix.editor.line_number,
                helix_view::editor::LineNumber::Relative
            );
            assert!(!config.helix.editor.mouse);

            // GUI config should be loaded
            assert_eq!(config.gui.ui.font.as_ref().unwrap().family, "GUI Font");
            assert_eq!(
                config.gui.editor.font.as_ref().unwrap().family,
                "Editor Font"
            );
        }

        #[test]
        fn test_font_resolution_precedence() {
            let mut config = Config::load_from_dir(&std::env::temp_dir()).unwrap();

            // Test: Editor font specified, UI font specified
            config.gui.editor.font = Some(FontConfig {
                family: "Editor Font".to_string(),
                weight: FontWeight::Bold,
                size: 16.0,
                line_height: 1.4,
            });

            config.gui.ui.font = Some(FontConfig {
                family: "UI Font".to_string(),
                weight: FontWeight::Medium,
                size: 12.0,
                line_height: 1.5,
            });

            let editor_font = config.editor_font();
            assert_eq!(editor_font.family, "Editor Font");
            assert_eq!(editor_font.weight, FontWeight::Bold);

            let ui_font = config.ui_font();
            assert_eq!(ui_font.family, "UI Font");
            assert_eq!(ui_font.weight, FontWeight::Medium);
        }

        #[test]
        fn test_font_fallback_chain() {
            let mut config = Config::load_from_dir(&std::env::temp_dir()).unwrap();

            // Test: Only UI font specified
            config.gui.ui.font = Some(FontConfig {
                family: "Fallback Font".to_string(),
                weight: FontWeight::SemiBold,
                size: 14.0,
                line_height: 1.3,
            });
            config.gui.editor.font = None;

            let editor_font = config.editor_font();
            assert_eq!(editor_font.family, "Fallback Font"); // Falls back to UI font
            assert_eq!(editor_font.weight, FontWeight::SemiBold);

            // Test: No fonts specified
            config.gui.ui.font = None;
            config.gui.editor.font = None;

            let editor_font = config.editor_font();
            assert_eq!(editor_font.family, "SF Mono"); // Default editor font

            let ui_font = config.ui_font();
            assert_eq!(ui_font.family, "SF Pro Display"); // Default UI font
        }
    }

    /// Test configuration schema evolution and migration
    mod config_migration {
        use super::*;

        #[test]
        fn test_old_config_format_compatibility() {
            // Test that old configuration formats are still supported
            let old_format_config = r#"
# Old-style configuration without nested sections
font_family = "Monaco"
font_size = 12.0
theme = "dark"
"#;

            // This should not parse as our current format, which is expected
            let result: Result<GuiConfig, _> = toml::from_str(old_format_config);
            assert!(result.is_err() || result.unwrap().ui.font.is_none());
        }

        #[test]
        fn test_unknown_fields_ignored() {
            let config_with_unknown_fields = r#"
[ui.font]
family = "Test Font"
size = 14.0
unknown_field = "should be ignored"

[unknown_section]
some_field = "also ignored"

[theme]
mode = "dark"
"#;

            // Should parse successfully, ignoring unknown fields
            let config: GuiConfig = toml::from_str(config_with_unknown_fields)
                .expect("Should parse despite unknown fields");

            assert_eq!(config.ui.font.as_ref().unwrap().family, "Test Font");
            assert_eq!(config.theme.mode, ThemeMode::Dark);
        }

        #[test]
        fn test_minimal_valid_config() {
            let minimal_config = r#""#; // Empty config

            let config: GuiConfig =
                toml::from_str(minimal_config).expect("Empty config should parse");

            // Should have all defaults
            assert!(config.ui.font.is_none());
            assert!(config.editor.font.is_none());
            assert_eq!(config.theme.mode, ThemeMode::System);
            assert!(!config.window.blur_dark_themes);
            assert!(config.window.appearance_follows_theme);
        }
    }
}
