// ABOUTME: Advanced rendering system for completion items with icons and documentation
// ABOUTME: Provides rich visual presentation using GPUI components and virtual scrolling

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Svg, UniformListScrollHandle, div, px, relative,
};

use crate::completion_icons::{create_themed_completion_icon, get_completion_icon_color};
use crate::completion_v2::{CompletionItem, CompletionItemKind, StringMatch};

/// Icon information for completion items using SVG
pub struct CompletionIcon {
    /// SVG icon for rendering
    pub svg: Svg,
    /// Color of the icon
    pub color: Hsla,
    /// Optional tooltip text
    pub tooltip: Option<String>,
}

impl CompletionIcon {
    pub fn new(svg: Svg, color: Hsla) -> Self {
        Self {
            svg,
            color,
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// Get icon for completion item kind using Lucide SVG icons
pub fn get_completion_icon(kind: &CompletionItemKind, theme: &crate::Theme) -> CompletionIcon {
    let svg = create_themed_completion_icon(kind, theme, Some(16.0));
    let color = get_completion_icon_color(kind, theme);

    let tooltip = match kind {
        CompletionItemKind::Function => "Function",
        CompletionItemKind::Method => "Method",
        CompletionItemKind::Variable => "Variable",
        CompletionItemKind::Field => "Field",
        CompletionItemKind::Class => "Class",
        CompletionItemKind::Constructor => "Constructor",
        CompletionItemKind::Interface => "Interface",
        CompletionItemKind::Module => "Module",
        CompletionItemKind::Property => "Property",
        CompletionItemKind::Enum => "Enum",
        CompletionItemKind::EnumMember => "Enum Member",
        CompletionItemKind::Constant => "Constant",
        CompletionItemKind::Struct => "Struct",
        CompletionItemKind::Keyword => "Keyword",
        CompletionItemKind::Snippet => "Snippet",
        CompletionItemKind::TypeParameter => "Type Parameter",
        CompletionItemKind::File => "File",
        CompletionItemKind::Folder => "Folder",
        CompletionItemKind::Reference => "Reference",
        CompletionItemKind::Event => "Event",
        CompletionItemKind::Operator => "Operator",
        CompletionItemKind::Text => "Text",
        CompletionItemKind::Unit => "Unit",
        CompletionItemKind::Value => "Value",
        CompletionItemKind::Color => "Color",
    };

    CompletionIcon::new(svg, color).with_tooltip(tooltip)
}

/// Rendered completion item with rich formatting
pub struct CompletionItemElement {
    item: CompletionItem,
    string_match: StringMatch,
    is_selected: bool,
    show_icon: bool,
    show_kind: bool,
    compact: bool,
}

impl CompletionItemElement {
    pub fn new(item: CompletionItem, string_match: StringMatch, is_selected: bool) -> Self {
        // Debug removed - enhanced completion display is working
        Self {
            item,
            string_match,
            is_selected,
            show_icon: true,
            show_kind: true,
            compact: false,
        }
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn hide_icon(mut self) -> Self {
        self.show_icon = false;
        self
    }

    pub fn hide_kind(mut self) -> Self {
        self.show_kind = false;
        self
    }

    fn render_icon(
        &self,
        theme: &crate::Theme,
        icon_size: f32,
        slot_size: f32,
    ) -> Option<AnyElement> {
        if !self.show_icon {
            return None;
        }

        let tokens = &theme.tokens;
        let icon_tokens = tokens.chrome.completion_icon_tokens(&tokens.editor);
        let icon_background = if self.is_selected {
            icon_tokens.icon_background_selected
        } else {
            icon_tokens.icon_background
        };
        let (svg, icon_color) = if let Some(kind) = self.item.kind.as_ref() {
            (
                crate::completion_icons::get_completion_icon_svg(kind),
                crate::completion_icons::get_completion_icon_color_on_background(
                    kind,
                    theme,
                    icon_background,
                ),
            )
        } else {
            (
                gpui::svg().path("icons/file-text.svg"),
                crate::styling::ColorTheory::ensure_contrast(
                    icon_background,
                    icon_tokens.generic_color,
                    crate::styling::ContrastRatios::AA_LARGE,
                ),
            )
        };

        Some(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(slot_size))
                .h(px(slot_size))
                .flex_shrink_0()
                .rounded_sm()
                .bg(icon_background)
                .border_1()
                .border_color(icon_tokens.icon_border)
                .when(!self.is_selected, |div| {
                    div.hover(|style| style.bg(icon_tokens.icon_background_hover))
                })
                .child(svg.size(px(icon_size)).text_color(icon_color))
                .into_any_element(),
        )
    }

    /// Render highlighted text with match positions
    fn render_highlighted_text(
        &self,
        text: &str,
        positions: &[usize],
        base_color: Hsla,
    ) -> impl IntoElement {
        if positions.is_empty() {
            // No highlighting needed
            return div().child(text.to_string());
        }

        let chars: Vec<char> = text.chars().collect();
        let mut elements = Vec::new();
        let mut last_pos = 0;

        for &pos in positions {
            if pos >= chars.len() {
                continue;
            }

            // Add non-highlighted text before this position
            if pos > last_pos {
                let before: String = chars[last_pos..pos].iter().collect();
                if !before.is_empty() {
                    elements.push(
                        div()
                            .text_color(base_color)
                            .child(before)
                            .into_any_element(),
                    );
                }
            }

            // Add highlighted character - VS Code style subtle highlighting
            let highlighted_char = chars[pos];
            elements.push(
                div()
                    .text_color(base_color)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(highlighted_char.to_string())
                    .into_any_element(),
            );

            last_pos = pos + 1;
        }

        // Add remaining text
        if last_pos < chars.len() {
            let remaining: String = chars[last_pos..].iter().collect();
            if !remaining.is_empty() {
                elements.push(
                    div()
                        .text_color(base_color)
                        .child(remaining)
                        .into_any_element(),
                );
            }
        }

        div().flex().flex_row().children(elements)
    }

    /// Build the element using the provided theme instead of a default.
    /// This avoids mismatched contrast when the app is using a light theme
    /// but `IntoElement` falls back to a dark default.
    pub fn into_element_with_theme(self, theme: &crate::Theme) -> AnyElement {
        let tokens = &theme.tokens;

        let display_text = self.item.display_text.as_ref().unwrap_or(&self.item.text);
        let detail_text = self
            .item
            .detail
            .as_ref()
            .or(self.item.description.as_ref())
            .map(ToString::to_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        let signature_text = self
            .item
            .signature_info
            .as_ref()
            .map(ToString::to_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        let type_text = self
            .item
            .type_info
            .as_ref()
            .map(ToString::to_string)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());

        if self.compact {
            let label_color = tokens.chrome.text_on_chrome;

            return div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .h(px(26.0))
                .px(tokens.sizes.space_2)
                .gap(tokens.sizes.space_2)
                .rounded(tokens.sizes.radius_sm)
                .line_height(relative(1.0))
                .when(self.is_selected, |div| div.bg(tokens.chrome.surface_active))
                .when(!self.is_selected, |div| {
                    div.hover(|style| style.bg(tokens.chrome.surface_hover))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .min_w(px(0.0))
                        .flex_1()
                        .gap(tokens.sizes.space_2)
                        .when_some(self.render_icon(theme, 12.0, 16.0), |row, icon| {
                            row.child(icon)
                        })
                        .child(
                            div()
                                .text_size(tokens.sizes.text_base)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(label_color)
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(self.render_highlighted_text(
                                    display_text,
                                    &self.string_match.positions,
                                    label_color,
                                )),
                        )
                        .when_some(signature_text, |row, signature| {
                            row.child(
                                div()
                                    .text_size(tokens.sizes.text_base)
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(signature),
                            )
                        })
                        .when_some(detail_text, |row, detail| {
                            row.child(
                                div()
                                    .text_size(tokens.sizes.text_base)
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(detail),
                            )
                        })
                        .when_some(type_text, |row, type_info| {
                            row.child(
                                div()
                                    .ml_auto()
                                    .text_size(tokens.sizes.text_sm)
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(type_info),
                            )
                        }),
                )
                .into_any_element();
        }

        let base_container = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_2()
            .py(px(3.0)) // This is 6px total (3px top + 3px bottom)
            .gap_2()
            .when(self.is_selected, |div| {
                div.bg(tokens.editor.selection_primary)
            })
            .when(!self.is_selected, |div| {
                div.hover(|style| style.bg(tokens.editor.selection_secondary))
            });

        // Icon section - using Lucide SVG icons with design tokens
        let with_icon = base_container
            .when_some(self.render_icon(theme, 12.0, 16.0), |div, icon| {
                div.child(icon)
            });

        // Main content section - Enhanced Zed-style layout with richer information
        let with_content = with_icon.child(
            div()
                .flex()
                .flex_col() // Stack main text and details vertically
                .min_w_0() // Allow text to truncate
                .flex_1() // Take up remaining space
                .gap_1() // This adds gap between rows
                .child(
                    // Top row: Primary text + signature/type info
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .w_full()
                        .child(
                            // Primary text with highlighting
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(tokens.chrome.text_on_chrome)
                                .child(self.render_highlighted_text(
                                    display_text,
                                    &self.string_match.positions,
                                    tokens.chrome.text_on_chrome,
                                )),
                        )
                        // Show signature info (parameters) directly after function name
                        .when_some(signature_text, |div_el, signature| {
                            div_el.child(
                                div()
                                    .text_sm()
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .child(signature),
                            )
                        })
                        // Show type info (return type) at the end
                        .when_some(type_text, |div_el, type_info| {
                            div_el.child(
                                div()
                                    .text_sm()
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .font_weight(gpui::FontWeight::LIGHT)
                                    .child(format!("→ {}", type_info)),
                            )
                        }),
                )
                // Bottom row: Detail or description (less prominent)
                .when_some(detail_text, |div_el, detail_text| {
                    div_el.child(
                        div()
                            .text_xs()
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .w_full()
                            .max_w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(detail_text),
                    )
                }),
        );

        // Score indicator (for debugging/development)
        #[cfg(debug_assertions)]
        let with_score = with_content.child(
            div()
                .ml_auto()
                .text_xs()
                .text_color(tokens.chrome.text_chrome_secondary)
                .px_1()
                .child(format!("{}", self.string_match.score)),
        );

        #[cfg(not(debug_assertions))]
        let with_score = with_content;

        // Ensure the final element fills the full width
        with_score.w_full().into_any_element()
    }
}

impl IntoElement for CompletionItemElement {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        // Fallback to the default theme for legacy call sites
        // Prefer calling `into_element_with_theme` to ensure correct contrast.
        let default_theme = crate::Theme::default();
        self.into_element_with_theme(&default_theme)
    }
}

// Note: CompletionItemElement implements IntoElement directly rather than RenderOnce
// to avoid trait conflicts and recursion issues. This provides a clean interface
// for using it as a child element in GPUI layouts.

/// Virtual list state for completion items
pub struct CompletionListState {
    /// Scroll handle for the virtual list
    pub scroll_handle: UniformListScrollHandle,
    /// Number of items to render
    pub item_count: usize,
    /// Height of each item in pixels
    pub item_height: f32,
    /// Maximum height of the list
    pub max_height: f32,
}

impl CompletionListState {
    pub fn new(item_height: f32, max_height: f32) -> Self {
        Self {
            scroll_handle: UniformListScrollHandle::new(),
            item_count: 0,
            item_height,
            max_height,
        }
    }

    pub fn update_item_count(&mut self, count: usize) {
        self.item_count = count;
    }

    /// Scroll to make the specified item visible, with smart positioning
    pub fn scroll_to_item(&mut self, index: usize) {
        if index >= self.item_count {
            return;
        }

        // For uniform_list, we can use the ScrollStrategy to determine positioning
        let strategy = if index == 0 {
            // First item should be at the top
            gpui::ScrollStrategy::Top
        } else if index >= self.item_count.saturating_sub(1) {
            // Last item - let Center handle it gracefully
            gpui::ScrollStrategy::Center
        } else {
            // Center the selected item for better visibility
            gpui::ScrollStrategy::Center
        };

        // Use the scroll handle to scroll to the item
        self.scroll_handle.scroll_to_item(index, strategy);
    }

    /// Get the currently visible range of items
    pub fn visible_item_range(&self) -> std::ops::Range<usize> {
        let visible_items = (self.max_height / self.item_height).floor() as usize;
        let max_items = visible_items.min(self.item_count);
        0..max_items
    }

    /// Check if an item index is currently visible
    pub fn is_item_visible(&self, index: usize) -> bool {
        let range = self.visible_item_range();
        range.contains(&index)
    }

    pub fn get_scroll_handle(&mut self) -> &mut UniformListScrollHandle {
        &mut self.scroll_handle
    }
}

/// Render completion items using virtual scrolling
pub fn render_completion_list<F, T: 'static>(
    items: &[CompletionItem],
    matches: &[StringMatch],
    selected_index: usize,
    list_state: &CompletionListState,
    cx: &mut Context<T>,
    _render_item: F,
) -> impl IntoElement
where
    F: Fn(usize, &CompletionItem, &StringMatch, bool) -> CompletionItemElement + 'static + Clone,
{
    nucleotide_logging::debug!(
        items = items.len(),
        matches = matches.len(),
        "COMP: render_completion_list called"
    );

    let theme = match cx.try_global::<crate::Theme>() {
        Some(theme) => theme,
        None => {
            nucleotide_logging::debug!(
                "COMP: render_completion_list - no theme found, returning empty div"
            );
            return div().id("completion-list-no-theme");
        }
    };
    let tokens = &theme.tokens;

    nucleotide_logging::trace!("COMP: About to create rendered_items vector");
    // Create items vector with proper matching
    let rendered_items: Vec<AnyElement> = matches
        .iter()
        .enumerate()
        .filter_map(|(index, string_match)| {
            nucleotide_logging::trace!(
                match_index = index,
                candidate_id = string_match.candidate_id,
                item_index = string_match.item_index,
                "COMP: Processing match"
            );

            let item = items.get(string_match.item_index)?;
            nucleotide_logging::trace!(item = %item.text, "COMP: Got item");
            nucleotide_logging::trace!(item = %item.text, "COMP: About to create simple div");

            // Create a simple div element instead of using CompletionItemElement for now
            let element = div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .when(index == selected_index, |div| {
                    div.bg(tokens.editor.selection_primary)
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.chrome.text_on_chrome)
                        .child(item.text.clone()),
                );

            nucleotide_logging::trace!("COMP: Created simple div, converting to AnyElement");
            Some(element.into_any_element())
        })
        .collect();

    nucleotide_logging::debug!(count = rendered_items.len(), "COMP: Items rendered");

    nucleotide_logging::trace!("COMP: About to create final container");
    let result = div()
        .id("completion-list")
        .flex()
        .flex_col()
        .bg(tokens.chrome.popup_background)
        .border_1()
        .border_color(tokens.chrome.popup_border)
        .rounded_md()
        .shadow(vec![
            tokens.chrome.shadow_md.to_box_shadow(false),
            tokens.chrome.inset_highlight.to_box_shadow(true),
        ])
        .max_h(px(list_state.max_height))
        .min_h(px(list_state.item_height * 3.0))
        .overflow_y_scroll()
        .children(rendered_items);

    nucleotide_logging::trace!("COMP: render_completion_list returning");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_icon_creation() {
        use crate::Theme;

        // Create a minimal theme for testing
        let theme = Theme::default();

        let icon = get_completion_icon(&CompletionItemKind::Function, &theme);
        // Test that we get an SVG icon (the specific implementation may vary)
        assert!(icon.tooltip.is_some());
        assert_eq!(icon.tooltip.unwrap(), "Function");
        // SVG icon should have the correct color for functions
        assert_eq!(icon.color, theme.tokens.editor.info);
    }

    #[test]
    fn test_completion_icon_kinds() {
        use crate::Theme;

        let theme = Theme::default();

        // Test that all kinds have icons
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
            let icon = get_completion_icon(&kind, &theme);
            // Test that we get a valid SVG icon and tooltip for each kind
            assert!(icon.tooltip.is_some(), "Missing tooltip for {:?}", kind);
        }
    }

    #[test]
    fn test_completion_list_state() {
        let mut state = CompletionListState::new(24.0, 300.0);

        assert_eq!(state.item_count, 0);
        assert_eq!(state.item_height, 24.0);
        assert_eq!(state.max_height, 300.0);

        state.update_item_count(50);
        assert_eq!(state.item_count, 50);

        // Test visible range calculation
        let range = state.visible_item_range();
        assert!(range.start <= range.end);
        assert!(range.end <= state.item_count);
    }

    #[test]
    fn test_completion_item_element_builder() {
        let item = CompletionItem::new("test_function")
            .with_kind(CompletionItemKind::Function)
            .with_description("A test function");

        let string_match = StringMatch::new(1, 100, vec![0, 1, 2]);

        let element = CompletionItemElement::new(item, string_match, true);
        assert!(element.is_selected);
        assert!(element.show_icon);
        assert!(element.show_kind);
        assert!(!element.compact);

        let compact_element = CompletionItemElement::new(
            CompletionItem::new("test"),
            StringMatch::new(1, 100, vec![]),
            false,
        )
        .compact()
        .hide_icon();

        assert!(!compact_element.is_selected);
        assert!(!compact_element.show_icon);
        assert!(compact_element.compact);
    }

    #[test]
    fn compact_completion_item_keeps_icon_by_default() {
        let element = CompletionItemElement::new(
            CompletionItem::new("test").with_kind(CompletionItemKind::Variable),
            StringMatch::new(1, 100, vec![]),
            false,
        )
        .compact();

        assert!(element.show_icon);
        assert!(element.compact);
    }
}
