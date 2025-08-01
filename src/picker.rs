use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

use crate::picker_view::{PickerItem, PickerView};
use crate::utils::TextWithStyle;

#[derive(Clone)]
pub enum Picker {
    Legacy(TextWithStyle),
    Native {
        title: SharedString,
        items: Vec<PickerItem>,
        on_select: Arc<dyn Fn(usize) + Send + Sync>,
    },
}

impl Picker {
    pub fn as_legacy(&self) -> Option<&TextWithStyle> {
        match self {
            Picker::Legacy(text) => Some(text),
            _ => None,
        }
    }

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
            Picker::Legacy(text) => f.debug_tuple("Legacy").field(text).finish(),
            Picker::Native { title, items, .. } => f
                .debug_struct("Native")
                .field("title", title)
                .field("items", items)
                .field("on_select", &"<function>")
                .finish(),
        }
    }
}

// TODO: this is copy-paste from Prompt, refactor it later
impl Picker {
    pub fn make<T: Send + Sync + 'static, D: Send + Sync + 'static>(
        editor: &mut helix_view::Editor,
        prompt: &mut helix_term::ui::Picker<T, D>,
    ) -> Self {
        use helix_term::compositor::Component;
        let area = editor.tree.area();
        let compositor_rect = helix_view::graphics::Rect {
            x: 0,
            y: 0,
            width: area.width * 2 / 3,
            height: area.height,
        };

        let mut comp_ctx = helix_term::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut helix_term::job::Jobs::new(),
        };
        let mut buf = tui::buffer::Buffer::empty(compositor_rect);
        prompt.render(compositor_rect, &mut buf, &mut comp_ctx);
        Self::Legacy(TextWithStyle::from_buffer(buf))
    }

    pub fn make_jump_picker(
        editor: &mut helix_view::Editor,
        prompt: &mut helix_term::ui::Picker<crate::application::JumpMeta, ()>,
    ) -> Self {
        use helix_term::compositor::Component;
        let area = editor.tree.area();
        let compositor_rect = helix_view::graphics::Rect {
            x: 0,
            y: 0,
            width: area.width * 2 / 3,
            height: area.height,
        };

        let mut comp_ctx = helix_term::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut helix_term::job::Jobs::new(),
        };
        let mut buf = tui::buffer::Buffer::empty(compositor_rect);
        prompt.render(compositor_rect, &mut buf, &mut comp_ctx);
        Self::Legacy(TextWithStyle::from_buffer(buf))
    }

    pub fn make_diagnostic_picker(
        editor: &mut helix_view::Editor,
        prompt: &mut helix_term::ui::Picker<crate::application::PickerDiagnostic, ()>,
    ) -> Self {
        use helix_term::compositor::Component;
        let area = editor.tree.area();
        let compositor_rect = helix_view::graphics::Rect {
            x: 0,
            y: 0,
            width: area.width * 2 / 3,
            height: area.height,
        };

        let mut comp_ctx = helix_term::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut helix_term::job::Jobs::new(),
        };
        let mut buf = tui::buffer::Buffer::empty(compositor_rect);
        prompt.render(compositor_rect, &mut buf, &mut comp_ctx);
        Self::Legacy(TextWithStyle::from_buffer(buf))
    }

    pub fn make_symbol_picker(
        editor: &mut helix_view::Editor,
        prompt: &mut helix_term::ui::Picker<crate::application::SymbolInformationItem, ()>,
    ) -> Self {
        use helix_term::compositor::Component;
        let area = editor.tree.area();
        let compositor_rect = helix_view::graphics::Rect {
            x: 0,
            y: 0,
            width: area.width * 2 / 3,
            height: area.height,
        };

        let mut comp_ctx = helix_term::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut helix_term::job::Jobs::new(),
        };
        let mut buf = tui::buffer::Buffer::empty(compositor_rect);
        prompt.render(compositor_rect, &mut buf, &mut comp_ctx);
        Self::Legacy(TextWithStyle::from_buffer(buf))
    }
}

#[derive(IntoElement)]
pub struct PickerElement {
    pub picker: Picker,
    pub focus: FocusHandle,
    pub selected_index: usize,
}

impl RenderOnce for PickerElement {
    fn render(self, cx: &mut WindowContext) -> impl IntoElement {
        match &self.picker {
            Picker::Legacy(text_with_style) => {
                let bg_color = text_with_style
                    .style(0)
                    .and_then(|style| style.background_color);
                let mut default_style = TextStyle::default();
                default_style.font_family = "JetBrains Mono".into();
                default_style.font_size = px(12.).into();
                default_style.background_color = bg_color;

                let text = text_with_style.clone().into_styled_text(&default_style);
                cx.focus(&self.focus);
                div()
                    .track_focus(&self.focus)
                    .flex()
                    .flex_col()
                    .bg(bg_color.unwrap_or(black()))
                    .shadow_sm()
                    .rounded_sm()
                    .text_color(hsla(1., 1., 1., 1.))
                    .font(cx.global::<crate::FontSettings>().fixed_font.clone())
                    .text_size(px(12.))
                    .line_height(px(1.3) * px(12.))
                    .child(text)
            }
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
