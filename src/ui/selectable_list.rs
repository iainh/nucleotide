// ABOUTME: Reusable selectable list component for pickers and menus
// ABOUTME: Handles keyboard navigation and selection with proper scrolling

use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::ui::theme_utils::ListColors;
use std::ops::Range;

/// A selectable list item
pub trait SelectableItem: Clone {
    /// Get the unique ID for this item
    fn id(&self) -> ElementId;
    
    /// Render the item content
    fn render(&self, is_selected: bool, colors: &ListColors, cx: &mut App) -> impl IntoElement;
}

/// A generic selectable list component
pub struct SelectableList<T: SelectableItem> {
    items: Vec<T>,
    selected_index: usize,
    colors: ListColors,
    max_height: Option<Pixels>,
    item_height: Pixels,
    list_id: ElementId,
}

impl<T: SelectableItem> SelectableList<T> {
    pub fn new(id: impl Into<ElementId>, items: Vec<T>, colors: ListColors) -> Self {
        Self {
            items,
            selected_index: 0,
            colors,
            max_height: None,
            item_height: px(32.0),
            list_id: id.into(),
        }
    }
    
    pub fn with_selected_index(mut self, index: usize) -> Self {
        self.selected_index = index.min(self.items.len().saturating_sub(1));
        self
    }
    
    pub fn with_max_height(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }
    
    pub fn with_item_height(mut self, height: Pixels) -> Self {
        self.item_height = height;
        self
    }
    
    pub fn list_id(&self) -> &ElementId {
        &self.list_id
    }
    
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }
    
    pub fn selected_item(&self) -> Option<&T> {
        self.items.get(self.selected_index)
    }
    
    /// Move selection up
    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            // TODO: Add scroll to item when UniformListScrollHandle is available
        }
    }
    
    /// Move selection down
    pub fn select_next(&mut self) {
        if self.selected_index < self.items.len().saturating_sub(1) {
            self.selected_index += 1;
            // TODO: Add scroll to item when UniformListScrollHandle is available
        }
    }
    
    /// Select first item
    pub fn select_first(&mut self) {
        self.selected_index = 0;
        // TODO: Add scroll to item when UniformListScrollHandle is available
    }
    
    /// Select last item
    pub fn select_last(&mut self) {
        self.selected_index = self.items.len().saturating_sub(1);
        // TODO: Add scroll to item when UniformListScrollHandle is available
    }
}

impl<T: SelectableItem + 'static> RenderOnce for SelectableList<T> {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let item_count = self.items.len();
        let item_height = self.item_height;
        let colors = self.colors.clone();
        let selected_index = self.selected_index;
        
        div()
            .flex()
            .flex_col()
            .bg(self.colors.background)
            .when_some(self.max_height, |this, max_height| {
                this.max_h(max_height)
                    .overflow_hidden()
            })
            .child(
                uniform_list(
                    self.list_id.clone(),
                    item_count,
                    move |visible_range: Range<usize>, _window, cx| {
                        visible_range
                            .map(|idx| {
                                let item = self.items[idx].clone();
                                let is_selected = idx == selected_index;
                                let colors_clone = colors.clone();
                                
                                div()
                                    .id(item.id())
                                    .h(item_height)
                                    .child(item.render(is_selected, &colors_clone, cx))
                                    .into_any_element()
                            })
                            .collect()
                    }
                )
            )
    }
}

/// Simple text list item implementation
#[derive(Clone)]
pub struct TextListItem {
    pub id: ElementId,
    pub text: SharedString,
    pub subtext: Option<SharedString>,
}

impl SelectableItem for TextListItem {
    fn id(&self) -> ElementId {
        self.id.clone()
    }
    
    fn render(&self, is_selected: bool, colors: &ListColors, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(is_selected, |this| {
                this.bg(colors.selected_background)
                    .text_color(colors.selected_text)
            })
            .when(!is_selected, |this| {
                this.text_color(colors.text)
                    .hover(|this| this.bg(colors.hover_background))
            })
            .child(self.text.clone())
            .when_some(self.subtext.clone(), |this, subtext| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .opacity(0.7)
                        .child(subtext)
                )
            })
    }
}