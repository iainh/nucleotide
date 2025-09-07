// ABOUTME: Placeholder terminal panel crate – tabs and actions wiring to follow

use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div};
use nucleotide_events::v2::terminal::TerminalId;
use nucleotide_terminal_view::{TerminalView, get_view_model};
use nucleotide_ui::ThemedContext;

/// Minimal terminal panel that mounts a TerminalView for a given TerminalId
pub struct TerminalPanel {
    pub active: TerminalId,
    pub height_px: f32,
    view_entity: Option<Entity<nucleotide_terminal_view::TerminalView>>,
}

impl TerminalPanel {
    pub fn new(active: TerminalId, height_px: f32) -> Self {
        Self {
            active,
            height_px,
            view_entity: None,
        }
    }

    pub fn initialize(&mut self, cx: &mut Context<Self>) {
        if self.view_entity.is_none() {
            if let Some(model) = get_view_model(self.active) {
                let created = cx.new(|_cx| TerminalView::new(model));
                self.view_entity = Some(created);
            }
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Lazy-initialize the view when the model becomes available
        if self.view_entity.is_none() {
            if let Some(model) = get_view_model(self.active) {
                let created = _cx.new(|_cx| TerminalView::new(model));
                self.view_entity = Some(created);
            }
        }

        let theme = _cx.theme();
        let tokens = &theme.tokens;
        let bg = tokens.chrome.surface;
        let border = tokens.chrome.border_muted;

        let mut container = div()
            .size_full()
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .flex()
            .flex_col();

        if let Some(view) = &self.view_entity {
            container = container.child(div().flex_grow().child(view.clone()));
        } else {
            container = container.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .child("Terminal session not available"),
            );
        }

        container
    }
}
