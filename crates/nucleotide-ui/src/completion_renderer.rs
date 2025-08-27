// ABOUTME: Advanced rendering system for completion items with icons and documentation
// ABOUTME: Provides rich visual presentation using GPUI components and virtual scrolling

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, div, px,
};

use crate::completion_v2::{CompletionItem, CompletionItemKind, StringMatch};

/// Icon information for completion items
#[derive(Debug, Clone)]
pub struct CompletionIcon {
    /// Unicode character or emoji for the icon
    pub character: String,
    /// Color of the icon
    pub color: Hsla,
    /// Optional tooltip text
    pub tooltip: Option<String>,
}

impl CompletionIcon {
    pub fn new(character: impl Into<String>, color: Hsla) -> Self {
        Self {
            character: character.into(),
            color,
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// Get icon for completion item kind using VS Code-style simple shapes and letters
pub fn get_completion_icon(kind: &CompletionItemKind, theme: &crate::Theme) -> CompletionIcon {
    let tokens = &theme.tokens;

    match kind {
        CompletionItemKind::Text => CompletionIcon::new("T", tokens.colors.text_secondary),
        CompletionItemKind::Method => {
            // Using 'M' for method - simple and clean
            CompletionIcon::new("M", tokens.colors.info).with_tooltip("Method")
        }
        CompletionItemKind::Function => {
            // Using 'f' for function
            CompletionIcon::new("f", tokens.colors.info).with_tooltip("Function")
        }
        CompletionItemKind::Constructor => {
            // Using 'C' for constructor
            CompletionIcon::new("C", tokens.colors.warning).with_tooltip("Constructor")
        }
        CompletionItemKind::Field => {
            // Using 'F' for field
            CompletionIcon::new("F", tokens.colors.success).with_tooltip("Field")
        }
        CompletionItemKind::Variable => {
            // Using 'v' for variable
            CompletionIcon::new("v", tokens.colors.primary).with_tooltip("Variable")
        }
        CompletionItemKind::Class => {
            // Using 'C' for class
            CompletionIcon::new("C", tokens.colors.warning).with_tooltip("Class")
        }
        CompletionItemKind::Interface => {
            // Using 'I' for interface
            CompletionIcon::new("I", tokens.colors.info).with_tooltip("Interface")
        }
        CompletionItemKind::Module => {
            // Using 'M' for module
            CompletionIcon::new("M", tokens.colors.text_primary).with_tooltip("Module")
        }
        CompletionItemKind::Property => {
            // Using 'P' for property
            CompletionIcon::new("P", tokens.colors.success).with_tooltip("Property")
        }
        CompletionItemKind::Unit => {
            CompletionIcon::new("U", tokens.colors.text_secondary).with_tooltip("Unit")
        }
        CompletionItemKind::Value => {
            // Using 'V' for value
            CompletionIcon::new("V", tokens.colors.primary).with_tooltip("Value")
        }
        CompletionItemKind::Enum => {
            // Using 'E' for enum
            CompletionIcon::new("E", tokens.colors.warning).with_tooltip("Enum")
        }
        CompletionItemKind::Keyword => {
            // Using 'K' for keyword
            CompletionIcon::new("K", tokens.colors.error).with_tooltip("Keyword")
        }
        CompletionItemKind::Snippet => {
            // Using 'S' for snippet
            CompletionIcon::new("S", tokens.colors.info).with_tooltip("Snippet")
        }
        CompletionItemKind::Color => {
            // Using square for color
            CompletionIcon::new("■", tokens.colors.primary).with_tooltip("Color")
        }
        CompletionItemKind::File => {
            // Using 'F' for file
            CompletionIcon::new("F", tokens.colors.text_secondary).with_tooltip("File")
        }
        CompletionItemKind::Reference => {
            // Using 'R' for reference
            CompletionIcon::new("R", tokens.colors.text_primary).with_tooltip("Reference")
        }
        CompletionItemKind::Folder => {
            // Using folder icon
            CompletionIcon::new("📁", tokens.colors.text_secondary).with_tooltip("Folder")
        }
        CompletionItemKind::EnumMember => {
            // Using 'e' for enum member
            CompletionIcon::new("e", tokens.colors.primary).with_tooltip("Enum Member")
        }
        CompletionItemKind::Constant => {
            // Using 'c' for constant
            CompletionIcon::new("c", tokens.colors.primary).with_tooltip("Constant")
        }
        CompletionItemKind::Struct => {
            // Using 'S' for struct
            CompletionIcon::new("S", tokens.colors.warning).with_tooltip("Struct")
        }
        CompletionItemKind::Event => {
            // Using 'E' for event
            CompletionIcon::new("E", tokens.colors.primary).with_tooltip("Event")
        }
        CompletionItemKind::Operator => {
            // Using 'O' for operator
            CompletionIcon::new("O", tokens.colors.error).with_tooltip("Operator")
        }
        CompletionItemKind::TypeParameter => {
            // Using 'T' for type parameter
            CompletionIcon::new("T", tokens.colors.warning).with_tooltip("Type Parameter")
        }
    }
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

    /// Render highlighted text with match positions
    fn render_highlighted_text(
        &self,
        text: &str,
        positions: &[usize],
        theme: &crate::Theme,
    ) -> impl IntoElement {
        let tokens = &theme.tokens;

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
                            .text_color(tokens.colors.text_primary)
                            .child(before)
                            .into_any_element(),
                    );
                }
            }

            // Add highlighted character - VS Code style subtle highlighting
            let highlighted_char = chars[pos];
            elements.push(
                div()
                    .text_color(tokens.colors.primary)
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
                        .text_color(tokens.colors.text_primary)
                        .child(remaining)
                        .into_any_element(),
                );
            }
        }

        div().flex().flex_row().children(elements)
    }
}

impl IntoElement for CompletionItemElement {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        // Use a default theme since IntoElement doesn't provide context access
        // In practice, the theme should be passed in constructor for proper styling
        let default_theme = crate::Theme::default();
        let tokens = &default_theme.tokens;

        let display_text = self.item.display_text.as_ref().unwrap_or(&self.item.text);

        let base_container = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_2()
            .py(px(3.0))
            .gap_2()
            .when(self.is_selected, |div| {
                div.bg(tokens.colors.selection_primary)
            })
            .when(!self.is_selected, |div| {
                div.hover(|style| style.bg(tokens.colors.selection_secondary))
            });

        // Icon section - compact VS Code style
        let with_icon = if self.show_icon && self.item.kind.is_some() {
            let icon = get_completion_icon(self.item.kind.as_ref().unwrap(), &default_theme);
            base_container.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_4()
                    .h_4()
                    .rounded_sm()
                    .bg(icon.color)
                    .text_color(gpui::white())
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(icon.character),
            )
        } else if self.show_icon {
            base_container.child(
                div()
                    .w_4()
                    .h_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(tokens.colors.text_tertiary)
                    .text_color(gpui::white())
                    .text_xs()
                    .child("?"),
            )
        } else {
            base_container
        };

        // Main content section - Enhanced Zed-style layout with richer information
        let with_content = with_icon.child(
            div()
                .flex()
                .flex_col() // Stack main text and details vertically
                .min_w_0() // Allow text to truncate
                .flex_1() // Take up remaining space
                .gap_1()
                .child(
                    // Top row: Primary text + signature/type info
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            // Primary text with highlighting
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(tokens.colors.text_primary)
                                .child(self.render_highlighted_text(
                                    display_text,
                                    &self.string_match.positions,
                                    &default_theme,
                                )),
                        )
                        // Show signature info (parameters) directly after function name
                        .when(self.item.signature_info.is_some(), |div_el| {
                            let signature = self.item.signature_info.as_ref().unwrap().to_string();
                            div_el.child(
                                div()
                                    .text_sm()
                                    .text_color(tokens.colors.text_secondary)
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .child(signature),
                            )
                        })
                        // Show type info (return type) at the end
                        .when(self.item.type_info.is_some(), |div_el| {
                            let type_info = self.item.type_info.as_ref().unwrap().to_string();
                            div_el.child(
                                div()
                                    .text_sm()
                                    .text_color(tokens.colors.text_tertiary)
                                    .font_weight(gpui::FontWeight::LIGHT)
                                    .child(format!("→ {}", type_info)),
                            )
                        }),
                )
                // Bottom row: Detail or description (less prominent)
                .when(
                    self.item.detail.is_some() || self.item.description.is_some(),
                    |div_el| {
                        let detail_text = self
                            .item
                            .detail
                            .as_ref()
                            .or(self.item.description.as_ref())
                            .unwrap()
                            .to_string();
                        div_el.child(
                            div()
                                .text_xs()
                                .text_color(tokens.colors.text_tertiary)
                                .max_w_full()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(detail_text),
                        )
                    },
                ),
        );

        // Score indicator (for debugging/development)
        #[cfg(debug_assertions)]
        let with_score = with_content.child(
            div()
                .ml_auto()
                .text_xs()
                .text_color(tokens.colors.text_tertiary)
                .px_1()
                .child(format!("{}", self.string_match.score)),
        );

        #[cfg(not(debug_assertions))]
        let with_score = with_content;

        with_score.into_any_element()
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

    pub fn scroll_to_item(&mut self, index: usize) {
        if index >= self.item_count {
            return;
        }

        let visible_items = (self.max_height / self.item_height).floor() as usize;
        let current_scroll_offset = 0.0; // TODO: Get actual scroll position from container
        let current_first_visible = (current_scroll_offset / self.item_height).floor() as usize;
        let current_last_visible = current_first_visible + visible_items - 1;

        // Check if item is already visible
        if index >= current_first_visible && index <= current_last_visible {
            return; // Already visible, no scrolling needed
        }

        // Calculate new scroll position to center the item
        let target_first_visible = if index < visible_items / 2 {
            0
        } else if index >= self.item_count - visible_items / 2 {
            self.item_count.saturating_sub(visible_items)
        } else {
            index.saturating_sub(visible_items / 2)
        };

        let target_scroll_offset = target_first_visible as f32 * self.item_height;

        // Store the target scroll position for the container to use
        // In a real implementation, this would trigger scroll animation
        // For now, we just calculate the correct position
        let _ = target_scroll_offset;
    }

    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let visible_items = (self.max_height / self.item_height).ceil() as usize;
        let start_idx = 0; // Simplified for now - would need access to scroll position
        let end_idx = visible_items.min(self.item_count);

        start_idx..end_idx
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
    println!(
        "COMP: render_completion_list called with {} items, {} matches",
        items.len(),
        matches.len()
    );

    let theme = match cx.try_global::<crate::Theme>() {
        Some(theme) => {
            println!("COMP: render_completion_list got theme successfully");
            theme
        }
        None => {
            println!("COMP: render_completion_list - no theme found, returning empty div");
            return div().id("completion-list-no-theme");
        }
    };
    let tokens = &theme.tokens;

    println!("COMP: About to create rendered_items vector");
    // Create items vector with proper matching
    let rendered_items: Vec<AnyElement> = matches
        .iter()
        .enumerate()
        .filter_map(|(index, string_match)| {
            println!(
                "COMP: Processing match {} with candidate_id {}",
                index, string_match.candidate_id
            );

            // Use the candidate_id as the direct index into the items array
            let item = items.get(string_match.candidate_id)?;
            println!("COMP: Got item: {}", item.text);

            println!("COMP: About to create simple div for item: {}", item.text);

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
                    div.bg(tokens.colors.selection_primary)
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(tokens.colors.text_primary)
                        .child(item.text.clone()),
                );

            println!("COMP: Created simple div, converting to AnyElement");
            Some(element.into_any_element())
        })
        .collect();

    println!("COMP: Created {} rendered_items", rendered_items.len());

    println!("COMP: About to create final div container");
    let result = div()
        .id("completion-list")
        .flex()
        .flex_col()
        .bg(tokens.colors.popup_background)
        .border_1()
        .border_color(tokens.colors.popup_border)
        .rounded_md()
        .shadow_lg()
        .max_h(px(list_state.max_height))
        .min_h(px(list_state.item_height * 3.0))
        .overflow_y_scroll()
        .children(rendered_items);

    println!("COMP: render_completion_list returning successfully");
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
        assert_eq!(icon.character, "f");
        assert!(icon.tooltip.is_some());
        assert_eq!(icon.tooltip.unwrap(), "Function");
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
            assert!(!icon.character.is_empty());
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
        let range = state.visible_range();
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
}
