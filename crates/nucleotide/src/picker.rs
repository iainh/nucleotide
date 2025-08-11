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

    /// Create a native directory picker
    pub fn native_directory(
        title: impl Into<SharedString>,
        _on_select: impl Fn(Option<std::path::PathBuf>) + Send + Sync + 'static,
    ) -> Self {
        // For now, we'll create an empty picker that will trigger native dialog
        // The actual directory selection will be handled by the OS
        Picker::Native {
            title: title.into(),
            items: vec![],
            on_select: Arc::new(move |_| {
                // This won't be called for directory picker
                // Directory selection will be handled through events
            }),
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
                let font = cx.global::<crate::FontSettings>().var_font.clone();

                {
                    // Create default theme colors for fallback
                    let background = hsla(0.0, 0.0, 0.1, 1.0);
                    let border = hsla(0.0, 0.0, 0.3, 1.0);
                    let text = hsla(0.0, 0.0, 0.9, 1.0);
                    let selected_bg = hsla(220.0 / 360.0, 0.6, 0.5, 1.0);
                    let selected_text = hsla(0.0, 0.0, 1.0, 1.0);
                    let prompt_text = hsla(0.0, 0.0, 0.7, 1.0);

                    div()
                        .track_focus(&self.focus)
                        .flex()
                        .flex_col()
                        .w(px(600.))
                        .max_h(px(400.))
                        .bg(background)
                        .border_1()
                        .border_color(border)
                        .rounded_md()
                        .shadow_lg()
                        .font(font)
                        .text_size(px(cx.global::<crate::UiFontConfig>().size))
                        .child(
                            // Title bar
                            div()
                                .flex()
                                .items_center()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(border)
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text)
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
                                            this.bg(selected_bg)
                                                .text_color(selected_text)
                                        })
                                        .when(!is_selected, |this| {
                                            this.text_color(text)
                                        })
                                        .child(item.label.clone())
                                        .when_some(item.sublabel.as_ref(), |this, sublabel| {
                                            this.child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(prompt_text)
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
                                .border_color(border)
                                .text_size(px(11.))
                                .text_color(prompt_text)
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
}
