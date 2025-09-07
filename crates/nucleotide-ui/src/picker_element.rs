// ABOUTME: Generic picker element using delegate pattern like Zed
// ABOUTME: Handles rendering and interaction, delegates business logic

#![allow(dead_code)]

use crate::actions::picker::{
    ConfirmSelection, DismissPicker, SelectFirst, SelectLast, SelectNext, SelectPrev, TogglePreview,
};
use crate::picker_delegate::PickerDelegate;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, DismissEvent, Element, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollStrategy, Styled,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

/// Generic picker element that works with any PickerDelegate
pub struct Picker<D: PickerDelegate> {
    delegate: Entity<D>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    show_preview: bool,
}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(delegate: Entity<D>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        // Focus will be handled in render

        Self {
            delegate,
            focus_handle,
            scroll_handle: UniformListScrollHandle::new(),
            show_preview: true,
        }
    }

    pub fn toggle_preview_pane(&mut self, cx: &mut Context<Self>) {
        if self.delegate.read(cx).supports_preview() {
            self.show_preview = !self.show_preview;
        }
    }

    fn select_next(&mut self, cx: &mut Context<Self>) {
        self.delegate.update(cx, |delegate, cx| {
            let count = delegate.match_count();
            if count > 0 {
                let current = delegate.selected_index();
                let next = (current + 1) % count;
                delegate.set_selected_index(next, cx);
            }
        });
        self.autoscroll(cx);
        cx.notify();
    }

    fn select_prev(&mut self, cx: &mut Context<Self>) {
        self.delegate.update(cx, |delegate, cx| {
            let count = delegate.match_count();
            if count > 0 {
                let current = delegate.selected_index();
                let prev = if current == 0 { count - 1 } else { current - 1 };
                delegate.set_selected_index(prev, cx);
            }
        });
        self.autoscroll(cx);
        cx.notify();
    }

    fn select_first(&mut self, cx: &mut Context<Self>) {
        self.delegate.update(cx, |delegate, cx| {
            if delegate.match_count() > 0 {
                delegate.set_selected_index(0, cx);
            }
        });
        self.autoscroll(cx);
        cx.notify();
    }

    fn select_last(&mut self, cx: &mut Context<Self>) {
        self.delegate.update(cx, |delegate, cx| {
            let count = delegate.match_count();
            if count > 0 {
                delegate.set_selected_index(count - 1, cx);
            }
        });
        self.autoscroll(cx);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let selected = self.delegate.read(cx).selected_index();
        self.delegate.update(cx, |delegate, cx| {
            delegate.confirm(selected, cx);
        });
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.delegate.update(cx, |delegate, cx| {
            delegate.dismiss(cx);
        });
    }

    fn autoscroll(&mut self, cx: &mut Context<Self>) {
        let selected = self.delegate.read(cx).selected_index();
        self.scroll_handle
            .scroll_to_item(selected, ScrollStrategy::Center);
        cx.notify();
    }
}

impl<D: PickerDelegate> Focusable for Picker<D> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<D: PickerDelegate> EventEmitter<DismissEvent> for Picker<D> {}

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.delegate.read(cx);
        let match_count = delegate.match_count();
        let selected_index = delegate.selected_index();
        let supports_preview = delegate.supports_preview();
        let show_preview = self.show_preview && supports_preview;

        // Get UI colors from ThemeProvider tokens when available (preferred),
        // otherwise use delegate's theme colors or static fallbacks
        let (bg_color, border_color, _text_color, prompt_color, preview_bg) =
            if let Some(provider) = crate::providers::use_theme_provider() {
                let ui = provider.current_theme();
                let dt = ui.tokens;
                (
                    dt.chrome.popup_background,
                    dt.chrome.popup_border,
                    dt.chrome.text_on_chrome,
                    dt.chrome.text_chrome_secondary,
                    dt.chrome.surface_elevated,
                )
            } else if let Some(theme_colors) = delegate.theme_colors() {
                (
                    theme_colors.background,
                    theme_colors.border,
                    theme_colors.text,
                    theme_colors.prompt_text,
                    crate::DesignTokens::dark().chrome.surface_elevated,
                )
            } else {
                // Choose light/dark tokens based on current Theme if available
                let dt = cx
                    .try_global::<crate::Theme>()
                    .map(|t| {
                        if t.is_dark() {
                            crate::DesignTokens::dark()
                        } else {
                            crate::DesignTokens::light()
                        }
                    })
                    .unwrap_or_else(crate::DesignTokens::dark);
                (
                    dt.chrome.popup_background,
                    dt.chrome.popup_border,
                    dt.chrome.text_on_chrome,
                    dt.chrome.text_chrome_secondary,
                    dt.chrome.surface_elevated,
                )
            };

        // Calculate dimensions
        let window_size = window.viewport_size();
        let window_width = f64::from(window_size.width.0);
        let window_height = f64::from(window_size.height.0);
        let total_width = px((window_width * 0.8).min(1000.0) as f32);
        let max_height = px((window_height * 0.7).min(600.0) as f32);

        let (list_width, preview_width) = if show_preview {
            (total_width * 0.5, total_width * 0.5)
        } else {
            (total_width, px(0.0))
        };

        div()
            .key_context("Picker")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectNext, _window, cx| {
                this.select_next(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, _window, cx| {
                this.select_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectFirst, _window, cx| {
                this.select_first(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectLast, _window, cx| {
                this.select_last(cx);
            }))
            .on_action(cx.listener(|this, _: &ConfirmSelection, _window, cx| {
                this.confirm(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissPicker, _window, cx| {
                this.dismiss(cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePreview, _window, cx| {
                this.toggle_preview_pane(cx);
            }))
            .w(total_width)
            .max_h(max_height)
            .bg(bg_color)
            .border_1()
            .border_color(border_color)
            .rounded_md()
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            // Header
            .when_some(delegate.render_header(window, cx), |this, header| {
                this.child(header)
            })
            // Search input
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        div()
                            .flex_1()
                            .text_color(prompt_color)
                            .child(format!("🔍 {}", delegate.query()))
                            .when(delegate.query().is_empty(), |this| {
                                this.child(delegate.placeholder_text())
                            }),
                    )
                    .when(supports_preview, |this| {
                        this.child(
                            div()
                                .ml_2()
                                .text_size(cx.global::<crate::Theme>().tokens.sizes.text_sm)
                                .text_color(prompt_color)
                                .child(if show_preview {
                                    "⌘P: Hide Preview"
                                } else {
                                    "⌘P: Show Preview"
                                }),
                        )
                    }),
            )
            // Main content area
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    // Results list
                    .child(
                        div()
                            .w(list_width)
                            .flex_1()
                            .when(show_preview, |this| {
                                this.border_r_1().border_color(border_color)
                            })
                            .child(if match_count == 0 {
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h_full()
                                    .text_color(prompt_color)
                                    .child("No matches found")
                                    .into_element()
                                    .into_any()
                            } else {
                                let delegate = self.delegate.clone();
                                uniform_list(
                                    "picker-items",
                                    match_count,
                                    move |range: std::ops::Range<usize>, window, cx| {
                                        let delegate = delegate.read(cx);
                                        let selected_index = delegate.selected_index();
                                        range
                                            .map(|ix| {
                                                let is_selected = ix == selected_index;

                                                // Wrap delegate item with token-based styling for consistency.
                                                // Ensure foreground contrasts against the popup surface.
                                                let (bg, fg, hover_bg) = if let Some(provider) =
                                                    crate::providers::use_theme_provider()
                                                {
                                                    let dt = provider.current_theme().tokens;
                                                    let contrasted = crate::styling::ColorTheory::ensure_contrast(
                                                        dt.chrome.popup_background,
                                                        dt.chrome.text_on_chrome,
                                                        crate::styling::color_theory::ContrastRatios::AA_NORMAL,
                                                    );
                                                    if is_selected {
                                                        let dd = dt.dropdown_tokens();
                                                        (
                                                            dd.item_background_selected,
                                                            dd.item_text_selected,
                                                            dd.item_background_selected,
                                                        )
                                                    } else {
                                                        (
                                                            crate::styling::ColorTheory::transparent(),
                                                            contrasted,
                                                            crate::styling::ColorTheory::with_alpha(
                                                                dt.chrome.surface_hover,
                                                                0.3,
                                                            ),
                                                        )
                                                    }
                                                } else {
                                                    let dt = crate::DesignTokens::dark();
                                                    let contrasted = crate::styling::ColorTheory::ensure_contrast(
                                                        dt.chrome.popup_background,
                                                        dt.chrome.text_on_chrome,
                                                        crate::styling::color_theory::ContrastRatios::AA_NORMAL,
                                                    );
                                                    if is_selected {
                                                        let dd = dt.dropdown_tokens();
                                                        (
                                                            dd.item_background_selected,
                                                            dd.item_text_selected,
                                                            dd.item_background_selected,
                                                        )
                                                    } else {
                                                        (
                                                            crate::styling::ColorTheory::transparent(),
                                                            contrasted,
                                                            crate::styling::ColorTheory::with_alpha(
                                                                dt.chrome.surface_hover,
                                                                0.3,
                                                            ),
                                                        )
                                                    }
                                                };

                                                let content = delegate
                                                    .render_match(ix, is_selected, window, cx)
                                                    .map(|el| el.into_element().into_any())
                                                    .unwrap_or_else(|| div().into_element().into_any());

                                                div()
                                                    .bg(bg)
                                                    .text_color(fg)
                                                    .hover(|this| {
                                                        if !is_selected {
                                                            this.bg(hover_bg)
                                                        } else {
                                                            this
                                                        }
                                                    })
                                                    .child(content)
                                                    .into_element()
                                                    .into_any()
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .flex_1()
                                .track_scroll(self.scroll_handle.clone())
                                .into_element()
                                .into_any()
                            }),
                    )
                    // Preview panel
                    .when(show_preview, |this| {
                        this.child(div().w(preview_width).flex_1().bg(preview_bg).child(
                            if let Some(preview) =
                                delegate.render_preview(selected_index, window, cx)
                            {
                                preview.into_element().into_any()
                            } else {
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h_full()
                                    .text_color(prompt_color)
                                    .child("No preview available")
                                    .into_element()
                                    .into_any()
                            },
                        ))
                    }),
            )
            // Footer
            .when_some(delegate.render_footer(window, cx), |this, footer| {
                this.child(footer)
            })
    }
}
