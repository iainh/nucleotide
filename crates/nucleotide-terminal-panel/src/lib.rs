// ABOUTME: Bottom terminal panel with chrome, title, actions, and terminal view mounting

use gpui::{
    App, AppContext, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled,
    Window, div, px, svg,
};
use nucleotide_events::v2::terminal::TerminalId;
use nucleotide_terminal_view::{TerminalView, get_view_model};
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, ThemedContext, Toolbar, Tooltipped};
use std::sync::Arc;

pub const TERMINAL_PANEL_HEADER_HEIGHT_PX: f32 = 32.0;

type CloseHandler = Arc<dyn Fn(TerminalId, &mut Window, &mut App) + 'static>;

/// Minimal terminal panel that mounts a TerminalView for a given TerminalId
pub struct TerminalPanel {
    pub active: TerminalId,
    pub height_px: f32,
    pub view_entity: Option<Entity<nucleotide_terminal_view::TerminalView>>,
    title: String,
    title_poll_started: bool,
    on_close: Option<CloseHandler>,
}

impl TerminalPanel {
    pub fn new(active: TerminalId, height_px: f32) -> Self {
        Self {
            active,
            height_px,
            view_entity: None,
            title: terminal_display_title(active),
            title_poll_started: false,
            on_close: None,
        }
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(TerminalId, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Arc::new(handler));
        self
    }

    pub fn initialize(&mut self, cx: &mut Context<Self>) {
        self.ensure_title_poll(cx);

        if self.view_entity.is_none()
            && let Some(model) = get_view_model(self.active)
        {
            let created = cx.new(|cx| TerminalView::new(model, cx));
            self.view_entity = Some(created);
        }
    }

    fn ensure_title_poll(&mut self, cx: &mut Context<Self>) {
        if self.title_poll_started {
            return;
        }

        self.title_poll_started = true;
        let active = self.active;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;

                let next_title = terminal_display_title(active);
                if this
                    .update(cx, |panel, cx| {
                        if panel.title != next_title {
                            panel.title = next_title;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}

fn terminal_display_title(id: TerminalId) -> String {
    get_view_model(id)
        .map(|model| {
            model
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .display_title()
        })
        .unwrap_or_else(|| "Terminal".to_string())
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_title_poll(cx);

        // Lazy-initialize the view when the model becomes available
        if self.view_entity.is_none()
            && let Some(model) = get_view_model(self.active)
        {
            let created = cx.new(|cx| TerminalView::new(model, cx));
            self.view_entity = Some(created);
        }

        let theme = cx.theme();
        let tokens = &theme.tokens;
        let bg = tokens.chrome.surface;
        let border = tokens.chrome.border_muted;
        self.title = terminal_display_title(self.active);
        let title = self.title.clone();
        let terminal_id = self.active;
        let close_handler = self.on_close.clone();

        let mut close_button = Button::icon_only(
            format!("terminal-panel-close-{}", terminal_id.0),
            "icons/close.svg",
        )
        .variant(ButtonVariant::Ghost)
        .size(ButtonSize::ExtraSmall)
        .tooltip("Close Terminal")
        .activate_on_mouse_down();

        close_button = if let Some(close_handler) = close_handler {
            close_button.on_click(move |_event, window, cx| {
                close_handler(terminal_id, window, cx);
                cx.stop_propagation();
            })
        } else {
            close_button.disabled(true)
        };

        let header = Toolbar::new("terminal-panel-header")
            .compact(true)
            .child(
                svg()
                    .path("icons/terminal.svg")
                    .size(tokens.sizes.text_md)
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(tokens.sizes.text_sm)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.chrome.text_on_chrome)
                    .child(title),
            )
            .child(close_button);

        let mut container = div()
            .w_full()
            .h(gpui::px(self.height_px))
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .flex()
            .flex_col();

        container = container.child(header);

        if let Some(view) = &self.view_entity {
            container = container.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(view.clone()),
            );
        } else {
            container = container.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_3()
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .child("Terminal session not available")
                    .child(
                        div()
                            .text_size(tokens.sizes.text_xs)
                            .child("The terminal runtime has not registered this session yet. If this persists, check logs for spawn or remote SSH errors."),
                    ),
            );
        }

        container
    }
}
