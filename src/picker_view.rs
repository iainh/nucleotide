// ABOUTME: GPUI-native picker component for fuzzy searching and selection
// ABOUTME: Replaces dependency on helix_term::ui::Picker with a proper GPUI implementation

use gpui::prelude::FluentBuilder;
use gpui::*;
use nucleo::Nucleo;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: SharedString,
    pub sublabel: Option<SharedString>,
    pub data: Arc<dyn std::any::Any + Send + Sync>,
}

pub struct PickerView {
    // Core picker state
    title: SharedString,
    query: SharedString,
    items: Vec<PickerItem>,
    filtered_indices: Vec<u32>,
    selected_index: usize,

    // Fuzzy matcher
    matcher: Option<Nucleo<PickerItem>>,

    // UI state
    focus_handle: FocusHandle,
    max_visible_items: usize,
    scroll_offset: usize,

    // Callbacks
    on_select: Option<Box<dyn FnMut(&PickerItem, &mut ViewContext<Self>) + 'static>>,
    on_cancel: Option<Box<dyn FnMut(&mut ViewContext<Self>) + 'static>>,

    // Styling
    style: PickerStyle,

}

#[derive(Clone)]
pub struct PickerStyle {
    pub background: Hsla,
    pub text: Hsla,
    pub selected_background: Hsla,
    pub selected_text: Hsla,
    pub border: Hsla,
    pub prompt_text: Hsla,
}

impl Default for PickerStyle {
    fn default() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.1, 1.0),
            text: hsla(0.0, 0.0, 0.9, 1.0),
            selected_background: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            selected_text: hsla(0.0, 0.0, 1.0, 1.0),
            border: hsla(0.0, 0.0, 0.3, 1.0),
            prompt_text: hsla(0.0, 0.0, 0.7, 1.0),
        }
    }
}

impl PickerView {
    pub fn new(cx: &mut ViewContext<Self>) -> Self {
        Self {
            title: "Picker".into(),
            query: SharedString::default(),
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected_index: 0,
            matcher: None,
            focus_handle: cx.focus_handle(),
            max_visible_items: 10,
            scroll_offset: 0,
            on_select: None,
            on_cancel: None,
            style: PickerStyle::default(),
        }
    }

    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.items = items;
        self.filtered_indices = (0..self.items.len() as u32).collect();
        self
    }

    pub fn with_style(mut self, style: PickerStyle) -> Self {
        self.style = style;
        self
    }
    
    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    pub fn on_select(mut self, callback: impl FnMut(&PickerItem, &mut ViewContext<Self>) + 'static) -> Self {
        self.on_select = Some(Box::new(callback));
        self
    }

    pub fn on_cancel(mut self, callback: impl FnMut(&mut ViewContext<Self>) + 'static) -> Self {
        self.on_cancel = Some(Box::new(callback));
        self
    }

    pub fn set_query(&mut self, query: impl Into<SharedString>, cx: &mut ViewContext<Self>) {
        self.query = query.into();
        self.filter_items(cx);
        self.selected_index = 0;
        self.scroll_offset = 0;
        cx.notify();
    }

    fn filter_items(&mut self, _cx: &mut ViewContext<Self>) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.items.len() as u32).collect();
        } else {
            // Simple substring matching for now
            // TODO: Integrate nucleo for proper fuzzy matching
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.label
                        .to_lowercase()
                        .contains(&self.query.to_lowercase())
                })
                .map(|(idx, _)| idx as u32)
                .collect();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut ViewContext<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let old_index = self.selected_index;
        let new_index = if delta > 0 {
            (self.selected_index + delta as usize).min(self.filtered_indices.len() - 1)
        } else {
            self.selected_index.saturating_sub((-delta) as usize)
        };

        self.selected_index = new_index;
        println!("🎯 Selection moved from {} to {} (delta: {})", old_index, new_index, delta);

        // Adjust scroll to keep selection visible
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible_items {
            self.scroll_offset = self.selected_index - self.max_visible_items + 1;
        }

        cx.notify();
    }

    fn confirm_selection(&mut self, cx: &mut ViewContext<Self>) {
        println!("🎯 confirm_selection called with selected_index: {}", self.selected_index);
        println!("🎯 filtered_indices length: {}", self.filtered_indices.len());
        
        if let Some(idx) = self.filtered_indices.get(self.selected_index) {
            println!("🎯 Found filtered index: {}", idx);
            if let Some(item) = self.items.get(*idx as usize) {
                println!("🎯 Found item: {}", item.label);
                if let Some(on_select) = &mut self.on_select {
                    println!("🎯 Calling on_select callback");
                    on_select(item, cx);
                } else {
                    println!("🚫 No on_select callback set");
                }
            } else {
                println!("🚫 Item not found at index {}", idx);
            }
        } else {
            println!("🚫 No filtered index found for selected_index {}", self.selected_index);
        }
    }

    fn cancel(&mut self, cx: &mut ViewContext<Self>) {
        if let Some(on_cancel) = &mut self.on_cancel {
            on_cancel(cx);
        }
    }
}

impl FocusableView for PickerView {
    fn focus_handle(&self, _cx: &AppContext) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PickerView {}

impl Render for PickerView {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let font = cx.global::<crate::FontSettings>().fixed_font.clone();
        let visible_items = self
            .filtered_indices
            .iter()
            .skip(self.scroll_offset)
            .take(self.max_visible_items)
            .enumerate();

        div()
            .flex()
            .flex_col()
            .w(px(600.))
            .max_h(px(400.))
            .bg(self.style.background)
            .border_1()
            .border_color(self.style.border)
            .rounded_md()
            .shadow_lg()
            .font(font)
            .text_size(px(14.))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, cx| {
                println!("🔥 PickerView received key: {}", event.keystroke.key);
                match event.keystroke.key.as_str() {
                    "up" => {
                        println!("🔥 Direct up key handling");
                        this.move_selection(-1, cx);
                    }
                    "down" => {
                        println!("🔥 Direct down key handling");
                        this.move_selection(1, cx);
                    }
                    "enter" => {
                        println!("🔥 Direct enter key handling");
                        this.confirm_selection(cx);
                    }
                    "escape" => {
                        println!("🔥 Direct escape key handling");
                        this.cancel(cx);
                    }
                    _ => {
                        println!("🔥 Unhandled key: {}", event.keystroke.key);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Cancel, cx| {
                println!("🔥 PickerView received Cancel action");
                this.cancel(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::Confirm, cx| {
                println!("🔥 PickerView received Confirm action");
                this.confirm_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::SelectPrev, cx| {
                println!("🔥 PickerView received SelectPrev action");
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::SelectNext, cx| {
                println!("🔥 PickerView received SelectNext action");
                this.move_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::MoveUp, cx| {
                println!("🔥 PickerView received MoveUp action");
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::MoveDown, cx| {
                println!("🔥 PickerView received MoveDown action");
                this.move_selection(1, cx);
            }))
            .child(
                // Title bar
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(self.style.border)
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(self.style.text)
                            .child(self.title.clone())
                    )
            )
            .child(
                // Search input
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(self.style.border)
                    .child(
                        div()
                            .flex_1()
                            .child(self.query.clone())
                            .text_color(self.style.prompt_text),
                    ),
            )
            .child(
                // Item list
                div().flex_1().overflow_hidden().children(visible_items.map(
                    |(visible_idx, &item_idx)| {
                        let item = &self.items[item_idx as usize];
                        let is_selected = visible_idx + self.scroll_offset == self.selected_index;

                        div()
                            .flex()
                            .flex_col()
                            .px_3()
                            .py_1()
                            .when(is_selected, |this| {
                                this.bg(self.style.selected_background)
                                    .text_color(self.style.selected_text)
                            })
                            .when(!is_selected, |this| this.text_color(self.style.text))
                            .child(item.label.clone())
                            .when_some(item.sublabel.as_ref(), |this, sublabel| {
                                this.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(self.style.prompt_text)
                                        .child(sublabel.clone()),
                                )
                            })
                    },
                )),
            )
            .when(self.filtered_indices.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h_24()
                        .text_color(self.style.prompt_text)
                        .child("No matches found"),
                )
            })
    }
}
