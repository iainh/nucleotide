// ABOUTME: Theme Debug view for inspecting theme tokens and derived colors
// ABOUTME: Opens as an overlay modal from a menu action and shows grouped tokens

use crate::theme_manager::{HelixThemedContext, SurfaceColorSource, ThemeManager};
use crate::{DesignTokens, Theme};
use gpui::Element;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, UniformListScrollHandle, Window, div, px,
}; // For into_any on elements

#[derive(Debug)]
pub struct ThemeDebugView {
    visible: bool,
    scroll: UniformListScrollHandle,
}

impl Default for ThemeDebugView {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemeDebugView {
    pub fn new() -> Self {
        Self {
            visible: false,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        cx.notify();
    }
}

fn fmt_hsla(c: gpui::Hsla) -> String {
    // Show compact hsla values
    format!(
        "hsla({:.0}, {:.2}, {:.2}, {:.2})",
        c.h * 360.0,
        c.s,
        c.l,
        c.a
    )
}

fn color_swatch(color: gpui::Hsla, tokens: &DesignTokens) -> gpui::AnyElement {
    div()
        .w(px(28.0))
        .h(px(18.0))
        .rounded_sm()
        .border_1()
        .border_color(tokens.chrome.border_default)
        .bg(color)
        .into_any()
}

fn row(label: &SharedString, color: gpui::Hsla, tokens: &DesignTokens) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_2()
        .py_1()
        .child(
            div()
                .flex_1()
                .text_color(tokens.chrome.text_on_chrome)
                .child(label.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(color_swatch(color, tokens))
                .child(
                    div()
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(fmt_hsla(color)),
                ),
        )
        .hover(|this| this.bg(tokens.chrome.surface_hover))
        .into_any()
}

fn header(title: &SharedString, tokens: &DesignTokens) -> gpui::AnyElement {
    div()
        .mt_2()
        .px_2()
        .py_1()
        .text_size(tokens.sizes.text_md)
        .font_weight(FontWeight::BOLD)
        .text_color(tokens.chrome.text_chrome_secondary)
        .child(title.clone())
        .into_any()
}

#[derive(Clone)]
enum DebugItem {
    Header(SharedString),
    Row {
        label: SharedString,
        color: gpui::Hsla,
    },
}

impl Render for ThemeDebugView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div();
        }

        let theme = cx.global::<Theme>();
        let tokens = theme.tokens;

        // Build a flat list of items in logical groups (cheap to clone)
        let mut items: Vec<DebugItem> = Vec::new();

        // Overview and surface extraction
        let tm = cx.theme_manager();
        let (surface_extracted, src) =
            ThemeManager::extract_surface_color(tm.helix_theme(), tm.system_appearance());

        let source_label: &str = match src {
            SurfaceColorSource::UiBackground => "Source: ui.background",
            SurfaceColorSource::UiWindow => "Source: ui.window",
            SurfaceColorSource::UiMenu => "Source: ui.menu",
            SurfaceColorSource::SystemFallback => "Source: system fallback",
        };

        items.push(DebugItem::Header(SharedString::from("Overview")));
        items.push(DebugItem::Row {
            label: SharedString::from("Editor Background (tokens.editor.background)"),
            color: tokens.editor.background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("Chrome Surface (tokens.chrome.surface)"),
            color: tokens.chrome.surface,
        });
        items.push(DebugItem::Row {
            label: SharedString::from(source_label),
            color: surface_extracted,
        });

        // Editor tokens
        items.push(DebugItem::Header(SharedString::from("Editor Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("selection_primary"),
            color: tokens.editor.selection_primary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("selection_secondary"),
            color: tokens.editor.selection_secondary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("cursor_normal"),
            color: tokens.editor.cursor_normal,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("cursor_insert"),
            color: tokens.editor.cursor_insert,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("cursor_select"),
            color: tokens.editor.cursor_select,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("cursor_match"),
            color: tokens.editor.cursor_match,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_primary"),
            color: tokens.editor.text_primary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_secondary"),
            color: tokens.editor.text_secondary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_on_primary"),
            color: tokens.editor.text_on_primary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("error"),
            color: tokens.editor.error,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("warning"),
            color: tokens.editor.warning,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("success"),
            color: tokens.editor.success,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("info"),
            color: tokens.editor.info,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("diagnostic_error"),
            color: tokens.editor.diagnostic_error,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("diagnostic_warning"),
            color: tokens.editor.diagnostic_warning,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("diagnostic_info"),
            color: tokens.editor.diagnostic_info,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("diagnostic_hint"),
            color: tokens.editor.diagnostic_hint,
        });
        items.push(DebugItem::Row {
            label: "diagnostic_error_bg".into(),
            color: tokens.editor.diagnostic_error_bg,
        });
        items.push(DebugItem::Row {
            label: "diagnostic_warning_bg".into(),
            color: tokens.editor.diagnostic_warning_bg,
        });
        items.push(DebugItem::Row {
            label: "diagnostic_info_bg".into(),
            color: tokens.editor.diagnostic_info_bg,
        });
        items.push(DebugItem::Row {
            label: "diagnostic_hint_bg".into(),
            color: tokens.editor.diagnostic_hint_bg,
        });
        items.push(DebugItem::Row {
            label: "gutter_background".into(),
            color: tokens.editor.gutter_background,
        });
        items.push(DebugItem::Row {
            label: "gutter_selected".into(),
            color: tokens.editor.gutter_selected,
        });
        items.push(DebugItem::Row {
            label: "line_number".into(),
            color: tokens.editor.line_number,
        });
        items.push(DebugItem::Row {
            label: "line_number_active".into(),
            color: tokens.editor.line_number_active,
        });
        items.push(DebugItem::Row {
            label: "vcs_added".into(),
            color: tokens.editor.vcs_added,
        });
        items.push(DebugItem::Row {
            label: "vcs_modified".into(),
            color: tokens.editor.vcs_modified,
        });
        items.push(DebugItem::Row {
            label: "vcs_deleted".into(),
            color: tokens.editor.vcs_deleted,
        });
        items.push(DebugItem::Row {
            label: "focus_ring".into(),
            color: tokens.editor.focus_ring,
        });
        items.push(DebugItem::Row {
            label: "focus_ring_error".into(),
            color: tokens.editor.focus_ring_error,
        });
        items.push(DebugItem::Row {
            label: "focus_ring_warning".into(),
            color: tokens.editor.focus_ring_warning,
        });

        // Chrome tokens
        items.push(DebugItem::Header(SharedString::from("Chrome Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("titlebar_background"),
            color: tokens.chrome.titlebar_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("footer_background"),
            color: tokens.chrome.footer_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("file_tree_background"),
            color: tokens.chrome.file_tree_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_empty_background"),
            color: tokens.chrome.tab_empty_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("separator_color"),
            color: tokens.chrome.separator_color,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface"),
            color: tokens.chrome.surface,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_elevated"),
            color: tokens.chrome.surface_elevated,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_overlay"),
            color: tokens.chrome.surface_overlay,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_hover"),
            color: tokens.chrome.surface_hover,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_active"),
            color: tokens.chrome.surface_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_selected"),
            color: tokens.chrome.surface_selected,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("surface_disabled"),
            color: tokens.chrome.surface_disabled,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border_default"),
            color: tokens.chrome.border_default,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border_muted"),
            color: tokens.chrome.border_muted,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border_strong"),
            color: tokens.chrome.border_strong,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border_focus"),
            color: tokens.chrome.border_focus,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("primary"),
            color: tokens.chrome.primary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("primary_hover"),
            color: tokens.chrome.primary_hover,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("primary_active"),
            color: tokens.chrome.primary_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("popup_background"),
            color: tokens.chrome.popup_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("popup_border"),
            color: tokens.chrome.popup_border,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("menu_background"),
            color: tokens.chrome.menu_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("menu_selected"),
            color: tokens.chrome.menu_selected,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("menu_separator"),
            color: tokens.chrome.menu_separator,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("statusline_active"),
            color: tokens.chrome.statusline_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("statusline_inactive"),
            color: tokens.chrome.statusline_inactive,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("bufferline_background"),
            color: tokens.chrome.bufferline_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("bufferline_active"),
            color: tokens.chrome.bufferline_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("bufferline_inactive"),
            color: tokens.chrome.bufferline_inactive,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_on_chrome"),
            color: tokens.chrome.text_on_chrome,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_chrome_secondary"),
            color: tokens.chrome.text_chrome_secondary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_chrome_disabled"),
            color: tokens.chrome.text_chrome_disabled,
        });

        // Derived component tokens
        let titlebar = tokens.titlebar_tokens();
        items.push(DebugItem::Header(SharedString::from("TitleBar Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("background"),
            color: titlebar.background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("foreground"),
            color: titlebar.foreground,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border"),
            color: titlebar.border,
        });

        let file_tree = tokens.file_tree_tokens();
        items.push(DebugItem::Header(SharedString::from("FileTree Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("background"),
            color: file_tree.background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("item_background_hover"),
            color: file_tree.item_background_hover,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("item_background_selected"),
            color: file_tree.item_background_selected,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("item_text"),
            color: file_tree.item_text,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("item_text_secondary"),
            color: file_tree.item_text_secondary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border"),
            color: file_tree.border,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("separator"),
            color: file_tree.separator,
        });

        let status_bar = tokens.status_bar_tokens();
        items.push(DebugItem::Header(SharedString::from("StatusBar Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("background_active"),
            color: status_bar.background_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("background_inactive"),
            color: status_bar.background_inactive,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_primary"),
            color: status_bar.text_primary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_secondary"),
            color: status_bar.text_secondary,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("text_accent"),
            color: status_bar.text_accent,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("border"),
            color: status_bar.border,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("mode_normal"),
            color: status_bar.mode_normal,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("mode_insert"),
            color: status_bar.mode_insert,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("mode_select"),
            color: status_bar.mode_select,
        });

        let tab_bar = tokens.tab_bar_tokens();
        items.push(DebugItem::Header(SharedString::from("TabBar Tokens")));
        items.push(DebugItem::Row {
            label: SharedString::from("container_background"),
            color: tab_bar.container_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_active_background"),
            color: tab_bar.tab_active_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_inactive_background"),
            color: tab_bar.tab_inactive_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_hover_background"),
            color: tab_bar.tab_hover_background,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_text_active"),
            color: tab_bar.tab_text_active,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_text_inactive"),
            color: tab_bar.tab_text_inactive,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_border"),
            color: tab_bar.tab_border,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_separator"),
            color: tab_bar.tab_separator,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_close_button"),
            color: tab_bar.tab_close_button,
        });
        items.push(DebugItem::Row {
            label: SharedString::from("tab_modified_indicator"),
            color: tab_bar.tab_modified_indicator,
        });

        // Build a virtualized list with our rows and a scrollbar
        let list = gpui::uniform_list("theme-debug-list", items.len(), {
            let items = items.clone();
            let tokens = tokens; // Copy
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, _cx| {
                let mut els: Vec<gpui::AnyElement> = Vec::with_capacity(range.end - range.start);
                for ix in range {
                    match &items[ix] {
                        DebugItem::Header(title) => {
                            els.push(header(title, &tokens));
                        }
                        DebugItem::Row { label, color } => {
                            els.push(row(label, *color, &tokens));
                        }
                    }
                }
                els
            })
        })
        .with_sizing_behavior(gpui::ListSizingBehavior::Infer)
        .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::FitList)
        .track_scroll(self.scroll.clone())
        .h_full();

        // Overlay and dialog container
        div()
            .absolute()
            .inset_0()
            .bg(tokens.chrome.surface_overlay)
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.hide(cx)),
            )
            .child(
                div()
                    .bg(tokens.chrome.surface_elevated)
                    .border_1()
                    .border_color(tokens.chrome.border_strong)
                    .rounded_lg()
                    .shadow_lg()
                    .p_4()
                    .w(px(860.0))
                    .max_h(px(600.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    // Title bar
                    .child(
                        div()
                            .px_1()
                            .py_1()
                            .text_size(tokens.sizes.text_lg)
                            .font_weight(FontWeight::BOLD)
                            .text_color(tokens.chrome.text_on_chrome)
                            .child("Theme Debug"),
                    )
                    // Content area (list + scrollbar)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .w_full()
                            .h_full()
                            .min_h(px(0.0))
                            .child(div().flex_1().min_h(px(0.0)).h_full().child(list))
                            .when_some(
                                crate::scrollbar::Scrollbar::vertical(
                                    crate::scrollbar::ScrollbarState::new(self.scroll.clone()),
                                ),
                                ParentElement::child,
                            ),
                    )
                    .on_mouse_down(gpui::MouseButton::Left, |_event, _window, _cx| {}),
            )
    }
}
