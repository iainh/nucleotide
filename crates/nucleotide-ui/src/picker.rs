use gpui::prelude::FluentBuilder;
use gpui::{
    App, FocusHandle, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};
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
                let font = cx
                    .global::<nucleotide_types::FontSettings>()
                    .var_font
                    .clone()
                    .into();

                {
                    // Get picker tokens using hybrid color system
                    let theme = cx.global::<crate::Theme>();
                    let picker_tokens = theme.tokens.picker_tokens();

                    div()
                        .track_focus(&self.focus)
                        .flex()
                        .flex_col()
                        .w(px(600.))
                        .max_h(px(400.))
                        .bg(picker_tokens.container_background)
                        .border_1()
                        .border_color(picker_tokens.border)
                        .rounded_md()
                        .shadow(vec![gpui::BoxShadow {
                            color: picker_tokens.shadow,
                            offset: gpui::point(px(picker_tokens.shadow_offset_x), px(picker_tokens.shadow_offset_y)),
                            blur_radius: px(picker_tokens.shadow_blur_radius),
                            spread_radius: px(0.0), // No spread for clean shadows
                        }])
                        .font(font)
                        .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size))
                        .child(
                            // Title bar - uses chrome header colors
                            div()
                                .flex()
                                .items_center()
                                .px_3()
                                .py_2()
                                .bg(picker_tokens.header_background)
                                .border_b_1()
                                .border_color(picker_tokens.border)
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(picker_tokens.header_text)
                                        .child(title.clone())
                                )
                        )
                        .child(
                            // Items list - uses Helix selection colors for familiarity
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
                                            this.bg(picker_tokens.item_background_selected)
                                                .text_color(picker_tokens.item_text_selected)
                                        })
                                        .when(!is_selected, |this| {
                                            this.bg(picker_tokens.item_background)
                                                .text_color(picker_tokens.item_text)
                                        })
                                        .hover(|this| {
                                            if !is_selected {
                                                this.bg(picker_tokens.item_background_hover)
                                            } else {
                                                this
                                            }
                                        })
                                        .child(item.label.clone())
                                        .when_some(item.sublabel.as_ref(), |this, sublabel| {
                                            this.child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(picker_tokens.item_text_secondary)
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
                                .border_color(picker_tokens.separator)
                                .text_size(px(11.))
                                .text_color(picker_tokens.item_text_secondary)
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
