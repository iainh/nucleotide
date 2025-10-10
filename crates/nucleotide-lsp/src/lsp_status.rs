// ABOUTME: LSP status indicator component for the status bar
// ABOUTME: Displays language server status, progress, and diagnostic counts

use crate::lsp_state::LspState;
use gpui::{
    App, Bounds, Context, Element, ElementId, Entity, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Render, SharedString, Style, Styled, TextRun,
    TextStyle, Window, div, px,
};
use nucleotide_logging::error;

/// LSP status indicator for the status bar
pub struct LspStatus {
    lsp_state: Entity<LspState>,
}

impl LspStatus {
    pub fn new(lsp_state: Entity<LspState>) -> Self {
        Self { lsp_state }
    }
}

impl Render for LspStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.lsp_state.read(cx);

        // Build status string
        let mut status_parts = Vec::new();

        // Show server count if any are running
        let running_count = state.running_servers_count();
        if running_count > 0 {
            status_parts.push(format!("LSP:{running_count}"));
        }

        // Show progress indicator if busy
        if state.is_busy()
            && let Some(progress_status) = state.status_string()
        {
            status_parts.push(progress_status);
        }

        // Show diagnostic count summary
        let total_diagnostics: usize = state.diagnostics.values().map(std::vec::Vec::len).sum();
        if total_diagnostics > 0 {
            status_parts.push(format!("⚠ {total_diagnostics}"));
        }

        // If nothing to show, return empty
        if status_parts.is_empty() {
            return div().size_0();
        }

        // Render the status (color comes from surrounding status bar style)
        div()
            .flex()
            .flex_row()
            .gap(px(6.))
            .children(status_parts.into_iter().map(|part| div().child(part)))
    }
}

/// Inline LSP status element for embedding in other components
pub struct LspStatusElement {
    lsp_state: Entity<LspState>,
    style: TextStyle,
}

impl LspStatusElement {
    pub fn new(lsp_state: Entity<LspState>, style: TextStyle) -> Self {
        Self { lsp_state, style }
    }
}

impl IntoElement for LspStatusElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LspStatusElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<SharedString>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = self.lsp_state.read(cx);

        // Build compact status
        let mut status = String::new();
        let running = state.running_servers_count();
        if running > 0 {
            status.push_str(&format!("LSP:{running} "));
        }
        if state.is_busy() {
            status.push_str("⟳ ");
        }

        let width = if status.is_empty() {
            px(0.)
        } else {
            px(status.len() as f32 * 8.) // Approximate width
        };

        let mut style = Style::default();
        style.size.width = width.into();
        let font_size = self.style.font_size.to_pixels(px(16.0));
        style.size.height = self.style.line_height_in_pixels(font_size).into();

        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.lsp_state.read(cx);

        // Build compact status
        let mut status = String::new();
        let running = state.running_servers_count();
        if running > 0 {
            status.push_str(&format!("LSP:{running} "));
        }
        if state.is_busy() {
            status.push_str("⟳ ");
        }

        if status.is_empty() {
            None
        } else {
            Some(status.trim().to_string().into())
        }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(text) = prepaint {
            let run = TextRun {
                len: text.len(),
                font: self.style.font(),
                color: self.style.color,
                background_color: None,
                strikethrough: None,
                underline: None,
            };

            let shaped = window.text_system().shape_line(
                text.clone(),
                self.style.font_size.to_pixels(px(16.0)),
                &[run],
                None,
            );

            let font_size = self.style.font_size.to_pixels(px(16.0));
            let line_height = self.style.line_height_in_pixels(font_size);
            let vertical_padding = f32::from(bounds.size.height - line_height) / 2.0;
            let y_center = bounds.origin.y + px(vertical_padding);

            if let Err(e) = shaped.paint(
                gpui::Point::new(bounds.origin.x, y_center),
                line_height,
                window,
                cx,
            ) {
                error!(error = ?e, "Failed to paint LSP status");
            }
        }
    }
}
