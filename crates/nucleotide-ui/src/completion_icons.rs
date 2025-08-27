// ABOUTME: Completion icon mapping system using Lucide SVG icons
// ABOUTME: Maps CompletionItemKind to appropriate SVG icon paths in assets/icons/

use crate::completion_v2::CompletionItemKind;
use gpui::{Hsla, Styled, Svg, svg};

/// Icon configuration for completion items
#[derive(Debug, Clone)]
pub struct CompletionIconConfig {
    /// Size of the icon in pixels
    pub size: f32,
    /// Color for the icon
    pub color: Option<Hsla>,
}

impl CompletionIconConfig {
    pub fn new(size: f32) -> Self {
        Self { size, color: None }
    }

    pub fn with_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl Default for CompletionIconConfig {
    fn default() -> Self {
        Self::new(16.0) // Standard 16px size for completion items
    }
}

/// Get the appropriate SVG icon for a completion item kind
pub fn get_completion_icon_svg(kind: &CompletionItemKind) -> Svg {
    match kind {
        CompletionItemKind::Function => svg().path("icons/completion-function.svg"),
        CompletionItemKind::Method => svg().path("icons/completion-method.svg"),
        CompletionItemKind::Variable => svg().path("icons/completion-variable.svg"),
        CompletionItemKind::Field => svg().path("icons/completion-field.svg"),
        CompletionItemKind::Class => svg().path("icons/completion-class.svg"),
        CompletionItemKind::Constructor => svg().path("icons/completion-class.svg"), // Same as class
        CompletionItemKind::Interface => svg().path("icons/completion-interface.svg"),
        CompletionItemKind::Module => svg().path("icons/completion-module.svg"),
        CompletionItemKind::Property => svg().path("icons/completion-field.svg"), // Same as field
        CompletionItemKind::Enum => svg().path("icons/completion-enum.svg"),
        CompletionItemKind::EnumMember => svg().path("icons/completion-field.svg"), // Same as field
        CompletionItemKind::Constant => svg().path("icons/completion-constant.svg"),
        CompletionItemKind::Struct => svg().path("icons/completion-class.svg"), // Same as class
        CompletionItemKind::Keyword => svg().path("icons/completion-keyword.svg"),
        CompletionItemKind::Snippet => svg().path("icons/completion-snippet.svg"),
        CompletionItemKind::TypeParameter => svg().path("icons/completion-type.svg"),

        // File-related items use existing file icons
        CompletionItemKind::File => svg().path("icons/file.svg"),
        CompletionItemKind::Folder => svg().path("icons/folder.svg"),

        // Fallback cases use generic icons
        CompletionItemKind::Text => svg().path("icons/file-text.svg"),
        CompletionItemKind::Unit => svg().path("icons/completion-type.svg"),
        CompletionItemKind::Value => svg().path("icons/completion-variable.svg"),
        CompletionItemKind::Color => svg().path("icons/completion-variable.svg"), // Generic for now
        CompletionItemKind::Reference => svg().path("icons/link.svg"),
        CompletionItemKind::Event => svg().path("icons/completion-method.svg"), // Same as method
        CompletionItemKind::Operator => svg().path("icons/completion-keyword.svg"), // Same as keyword
    }
}

/// Create a configured completion icon
pub fn create_completion_icon(kind: &CompletionItemKind, config: CompletionIconConfig) -> Svg {
    let mut svg_icon = get_completion_icon_svg(kind)
        .size(gpui::px(config.size))
        .flex_shrink_0(); // Don't let the icon shrink

    if let Some(color) = config.color {
        svg_icon = svg_icon.text_color(color);
    }

    svg_icon
}

/// Get semantic color for completion item kind based on the theme
pub fn get_completion_icon_color(kind: &CompletionItemKind, theme: &crate::Theme) -> Hsla {
    let icon_tokens = theme
        .tokens
        .chrome
        .completion_icon_tokens(&theme.tokens.editor);

    match kind {
        CompletionItemKind::Function => icon_tokens.function_color,
        CompletionItemKind::Method => icon_tokens.method_color,
        CompletionItemKind::Variable => icon_tokens.variable_color,
        CompletionItemKind::Field => icon_tokens.field_color,
        CompletionItemKind::Property => icon_tokens.field_color, // Same as field
        CompletionItemKind::Class => icon_tokens.class_color,
        CompletionItemKind::Constructor => icon_tokens.class_color, // Same as class
        CompletionItemKind::Struct => icon_tokens.class_color,      // Same as class
        CompletionItemKind::Interface => icon_tokens.interface_color,
        CompletionItemKind::Module => icon_tokens.module_color,
        CompletionItemKind::Enum => icon_tokens.enum_color,
        CompletionItemKind::EnumMember => icon_tokens.field_color, // Same as field
        CompletionItemKind::Constant => icon_tokens.constant_color,
        CompletionItemKind::Keyword => icon_tokens.keyword_color,
        CompletionItemKind::Operator => icon_tokens.keyword_color, // Same as keyword
        CompletionItemKind::Snippet => icon_tokens.snippet_color,
        CompletionItemKind::TypeParameter => icon_tokens.type_color,

        // File-related items
        CompletionItemKind::File | CompletionItemKind::Folder => icon_tokens.file_color,
        CompletionItemKind::Reference => icon_tokens.generic_color,

        // Generic fallbacks
        _ => icon_tokens.generic_color,
    }
}

/// Create a themed completion icon with appropriate colors
pub fn create_themed_completion_icon(
    kind: &CompletionItemKind,
    theme: &crate::Theme,
    size: Option<f32>,
) -> Svg {
    let config = CompletionIconConfig::new(size.unwrap_or(16.0))
        .with_color(get_completion_icon_color(kind, theme));

    create_completion_icon(kind, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_mapping_coverage() {
        // Test that all CompletionItemKind variants have icon mappings
        let kinds = vec![
            CompletionItemKind::Text,
            CompletionItemKind::Method,
            CompletionItemKind::Function,
            CompletionItemKind::Constructor,
            CompletionItemKind::Field,
            CompletionItemKind::Variable,
            CompletionItemKind::Class,
            CompletionItemKind::Interface,
            CompletionItemKind::Module,
            CompletionItemKind::Property,
            CompletionItemKind::Unit,
            CompletionItemKind::Value,
            CompletionItemKind::Enum,
            CompletionItemKind::Keyword,
            CompletionItemKind::Snippet,
            CompletionItemKind::Color,
            CompletionItemKind::File,
            CompletionItemKind::Reference,
            CompletionItemKind::Folder,
            CompletionItemKind::EnumMember,
            CompletionItemKind::Constant,
            CompletionItemKind::Struct,
            CompletionItemKind::Event,
            CompletionItemKind::Operator,
            CompletionItemKind::TypeParameter,
        ];

        for kind in kinds {
            let svg = get_completion_icon_svg(&kind);
            // Just verify we get an SVG - the path() call doesn't fail
            assert!(!svg.to_string().is_empty(), "Missing icon for {:?}", kind);
        }
    }

    #[test]
    fn test_icon_config() {
        let config = CompletionIconConfig::new(20.0).with_color(gpui::red());

        assert_eq!(config.size, 20.0);
        assert!(config.color.is_some());
    }

    #[test]
    fn test_default_config() {
        let config = CompletionIconConfig::default();
        assert_eq!(config.size, 16.0);
        assert!(config.color.is_none());
    }
}
