use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

use crate::picker_view::PickerItem;

#[derive(Clone)]
pub enum Picker {
    Native {
        title: SharedString,
        items: Vec<PickerItem>,
        on_select: Arc<dyn Fn(usize) + Send + Sync>,
    },
}

impl Picker {

    /// Create a new native GPUI picker
    pub fn native(
        title: impl Into<SharedString>,
        items: Vec<PickerItem>,
        on_select: impl Fn(usize) + Send + Sync + 'static,
    ) -> Self {
        Picker::Native {
            title: title.into(),
            items,
            on_select: Arc::new(on_select),
        }
    }
}

impl std::fmt::Debug for Picker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Picker::Native { title, items, .. } => f
                .debug_struct("Native")
                .field("title", title)
                .field("items", items)
                .field("on_select", &"<function>")
                .finish(),
        }
    }
}


#[derive(IntoElement)]
pub struct PickerElement {
    pub picker: Picker,
    pub focus: FocusHandle,
    pub selected_index: usize,
}

impl RenderOnce for PickerElement {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        match &self.picker {
            Picker::Native {
                title,
                items,
                on_select: _,
            } => {
                // Native GPUI picker rendering
                let font = cx.global::<crate::FontSettings>().fixed_font.clone();

                div()
                    .track_focus(&self.focus)
                    .flex()
                    .flex_col()
                    .w(px(600.))
                    .max_h(px(400.))
                    .bg(hsla(0.0, 0.0, 0.1, 1.0))
                    .border_1()
                    .border_color(hsla(0.0, 0.0, 0.3, 1.0))
                    .rounded_md()
                    .shadow_lg()
                    .font(font)
                    .text_size(px(14.))
                    .child(
                        // Title bar
                        div()
                            .flex()
                            .items_center()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(hsla(0.0, 0.0, 0.3, 1.0))
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(hsla(0.0, 0.0, 0.9, 1.0))
                                    .child(title.clone())
                            )
                    )
                    .child(
                        // Items list  
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .children(items.iter().enumerate().take(8).map(|(idx, item)| {
                                let is_selected = idx == self.selected_index;

                                div()
                                    .flex()
                                    .flex_col()
                                    .px_3()
                                    .py_1()
                                    .when(is_selected, |this| {
                                        this.bg(hsla(220.0 / 360.0, 0.6, 0.5, 1.0))
                                            .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                                    })
                                    .when(!is_selected, |this| {
                                        this.text_color(hsla(0.0, 0.0, 0.9, 1.0))
                                    })
                                    .child(item.label.clone())
                                    .when_some(item.sublabel.as_ref(), |this, sublabel| {
                                        this.child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(hsla(0.0, 0.0, 0.7, 1.0))
                                                .child(sublabel.clone())
                                        )
                                    })
                            }))
                    )
                    .child(
                        // Footer with instructions
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_3()
                            .py_1()
                            .border_t_1()
                            .border_color(hsla(0.0, 0.0, 0.3, 1.0))
                            .text_size(px(11.))
                            .text_color(hsla(0.0, 0.0, 0.6, 1.0))
                            .child(format!(
                                "Native GPUI Picker [{}/{}] - ↑↓ to navigate, Enter to select, Esc to cancel",
                                self.selected_index + 1,
                                items.len().min(8)
                            ))
                    )
            }
        }
    }
}
