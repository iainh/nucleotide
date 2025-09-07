// ABOUTME: Picker delegate trait following Zed's idiomatic pattern
// ABOUTME: Separates picker business logic from UI rendering

#![allow(dead_code)]

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Context, DismissEvent, Div, EventEmitter, FontWeight, Hsla, IntoElement,
    ParentElement, SharedString, Styled, Task, Window, div, hsla, px,
};
use std::sync::Arc;

/// Theme colors for picker UI elements
#[derive(Clone)]
pub struct PickerThemeColors {
    pub background: Hsla,
    pub text: Hsla,
    pub selected_background: Hsla,
    pub selected_text: Hsla,
    pub border: Hsla,
    pub prompt_text: Hsla,
}

/// Trait for implementing picker behavior following Zed's delegate pattern
pub trait PickerDelegate: Sized + 'static {
    /// The type of items being picked
    type Item: Clone + Send + Sync + 'static;

    /// The element type for rendering list items
    type ListItem: IntoElement;

    /// The element type for rendering preview (optional)
    type Preview: IntoElement;

    /// Return the total number of matches
    fn match_count(&self) -> usize;

    /// Return the currently selected index
    fn selected_index(&self) -> usize;

    /// Set the selected index
    fn set_selected_index(&mut self, index: usize, cx: &mut Context<Self>);

    /// Update matches based on query
    fn update_matches(&mut self, query: SharedString, cx: &mut Context<Self>) -> Task<()>;

    /// Render a single match item
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &App,
    ) -> Option<Self::ListItem>;

    /// Render the picker header (optional)
    fn render_header(&self, _window: &mut Window, _cx: &App) -> Option<AnyElement> {
        None
    }

    /// Render the picker footer (optional)
    fn render_footer(&self, _window: &mut Window, _cx: &App) -> Option<AnyElement> {
        None
    }

    /// Render preview for the selected item (optional)
    fn render_preview(
        &self,
        _selected_index: usize,
        _window: &mut Window,
        _cx: &App,
    ) -> Option<Self::Preview> {
        None
    }

    /// Whether preview should be shown
    fn supports_preview(&self) -> bool {
        false
    }

    /// Called when an item is confirmed (enter pressed)
    fn confirm(&mut self, index: usize, cx: &mut Context<Self>);

    /// Called when picker is dismissed (escape pressed)
    fn dismiss(&mut self, cx: &mut Context<Self>);

    /// Return the current query
    fn query(&self) -> SharedString;

    /// Return placeholder text for the search input
    fn placeholder_text(&self) -> SharedString {
        "Search...".into()
    }

    /// Return theme colors for the picker UI (optional)
    fn theme_colors(&self) -> Option<PickerThemeColors> {
        None
    }
}

// Type alias for file picker select handler
type FilePickerSelectHandler = Arc<dyn Fn(std::path::PathBuf, &mut App) + Send + Sync>;

/// File picker delegate implementation
pub struct FilePickerDelegate {
    items: Vec<FilePickerItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    query: SharedString,
    on_select: Option<FilePickerSelectHandler>,
    theme_colors: Option<PickerThemeColors>,
}

#[derive(Clone)]
pub struct FilePickerItem {
    pub path: std::path::PathBuf,
    pub label: SharedString,
    pub sublabel: Option<SharedString>,
    pub icon: Option<SharedString>,
}

impl FilePickerDelegate {
    pub fn new(items: Vec<FilePickerItem>) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_index: 0,
            query: SharedString::default(),
            on_select: None,
            theme_colors: None,
        }
    }

    pub fn with_theme_colors(mut self, theme_colors: PickerThemeColors) -> Self {
        self.theme_colors = Some(theme_colors);
        self
    }

    pub fn with_on_select(
        mut self,
        on_select: impl Fn(std::path::PathBuf, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_select = Some(Arc::new(on_select));
        self
    }
}

impl EventEmitter<DismissEvent> for FilePickerDelegate {}

impl PickerDelegate for FilePickerDelegate {
    type Item = FilePickerItem;
    type ListItem = crate::ListItem;
    type Preview = Div;

    fn match_count(&self) -> usize {
        self.filtered_indices.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, index: usize, _cx: &mut Context<Self>) {
        self.selected_index = index.min(self.filtered_indices.len().saturating_sub(1));
    }

    fn update_matches(&mut self, query: SharedString, _cx: &mut Context<Self>) -> Task<()> {
        self.query = query.clone();

        if query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            // Simple fuzzy matching
            let query_lower = query.to_lowercase();
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let label_lower = item.label.to_lowercase();
                    fuzzy_match(&query_lower, &label_lower)
                })
                .map(|(idx, _)| idx)
                .collect();
        }

        // Reset selection if needed
        if self.selected_index >= self.filtered_indices.len() {
            self.selected_index = 0;
        }

        Task::ready(())
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &App,
    ) -> Option<Self::ListItem> {
        let item_idx = *self.filtered_indices.get(ix)?;
        let item = self.items.get(item_idx)?;

        let mut list_item = crate::ListItem::new(("file-picker-item", ix)).selected(selected);

        if let Some(icon) = &item.icon {
            list_item = list_item.start_slot(div().child(icon.clone()));
        }

        let tokens = &_cx.global::<crate::Theme>().tokens;
        list_item = list_item.child(
            div()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .text_ellipsis()
                        .text_size(tokens.sizes.text_md)
                        .child(item.label.clone()),
                )
                .when_some(item.sublabel.as_ref(), |this, sublabel| {
                    let sublabel_color = self
                        .theme_colors
                        .as_ref()
                        .map(|colors| colors.prompt_text)
                        .unwrap_or_else(|| hsla(0.0, 0.0, 0.7, 1.0));

                    this.child(
                        div()
                            .text_size(tokens.sizes.text_sm)
                            .text_color(sublabel_color)
                            .text_ellipsis()
                            .child(sublabel.clone()),
                    )
                }),
        );

        Some(list_item)
    }

    fn supports_preview(&self) -> bool {
        true
    }

    fn render_preview(
        &self,
        selected_index: usize,
        _window: &mut Window,
        cx: &App,
    ) -> Option<Self::Preview> {
        let item_idx = *self.filtered_indices.get(selected_index)?;
        let item = self.items.get(item_idx)?;

        let font = cx
            .global::<nucleotide_types::FontSettings>()
            .fixed_font
            .clone()
            .into();

        // Simple preview - in real implementation would load file content
        let tokens = &cx.global::<crate::Theme>().tokens;
        Some(
            div()
                .flex()
                .flex_col()
                .p_4()
                .font(font)
                .text_size(tokens.sizes.text_sm)
                .child(
                    div()
                        .text_size(tokens.sizes.text_md)
                        .font_weight(FontWeight::BOLD)
                        .mb_2()
                        .child("Preview"),
                )
                .child({
                    let text_color = self
                        .theme_colors
                        .as_ref()
                        .map(|colors| colors.prompt_text)
                        .unwrap_or_else(|| hsla(0.0, 0.0, 0.7, 1.0));

                    div()
                        .text_color(text_color)
                        .child(format!("File: {}", item.path.display()))
                }),
        )
    }

    fn confirm(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(item_idx) = self.filtered_indices.get(index)
            && let Some(item) = self.items.get(*item_idx)
        {
            if let Some(on_select) = &self.on_select {
                on_select(item.path.clone(), cx);
            }
            // Emit event to close picker
            cx.emit(DismissEvent);
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn query(&self) -> SharedString {
        self.query.clone()
    }

    fn placeholder_text(&self) -> SharedString {
        "Search files...".into()
    }

    fn theme_colors(&self) -> Option<PickerThemeColors> {
        self.theme_colors.clone()
    }
}

// Simple fuzzy matching helper
fn fuzzy_match(query: &str, target: &str) -> bool {
    let mut query_chars = query.chars();
    let mut current_char = query_chars.next();

    if current_char.is_none() {
        return true;
    }

    for target_char in target.chars() {
        if let Some(q_char) = current_char
            && target_char == q_char
        {
            current_char = query_chars.next();
            if current_char.is_none() {
                return true;
            }
        }
    }

    current_char.is_none()
}
