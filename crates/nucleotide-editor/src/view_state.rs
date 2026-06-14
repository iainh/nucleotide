// ABOUTME: Persistent native editor view state shared by GPUI render phases
// ABOUTME: Bundles viewport, metrics, overlay, scrollbar, and selection state

use gpui::{Pixels, TextStyle, px};

use crate::{
    EditorCursorReveal, EditorOverlayState, EditorScrollbarState, EditorSelectionDragState,
    EditorSurfaceMetrics, EditorTextMetrics, EditorViewport,
};

pub struct EditorViewState {
    viewport: EditorViewport,
    surface_metrics: EditorSurfaceMetrics,
    scrollbar_state: EditorScrollbarState,
    selection_drag_state: EditorSelectionDragState,
    overlay_state: EditorOverlayState,
    line_height: Pixels,
}

impl EditorViewState {
    pub fn new(line_height: Pixels, cell_width: Pixels) -> Self {
        Self {
            viewport: EditorViewport::new(line_height),
            surface_metrics: EditorSurfaceMetrics::new(line_height, cell_width),
            scrollbar_state: EditorScrollbarState::default(),
            selection_drag_state: EditorSelectionDragState::default(),
            overlay_state: EditorOverlayState::new(),
            line_height,
        }
    }

    pub fn apply_text_metrics(&mut self, metrics: EditorTextMetrics) {
        self.line_height = metrics.line_height;
        self.viewport.set_line_height(metrics.line_height);
        self.surface_metrics
            .set(metrics.line_height, metrics.cell_width);
    }

    pub fn update_line_height_from_text_style(&mut self, style: &TextStyle) {
        let font_size = style.font_size.to_pixels(px(16.0));
        self.line_height = style.line_height_in_pixels(font_size);
        self.viewport.set_line_height(self.line_height);

        let current_metrics = self.surface_metrics.get();
        self.surface_metrics
            .set(self.line_height, current_metrics.cell_width);
    }

    pub fn clear_shaped_lines_cache(&self) {
        self.surface_metrics.line_cache().clear_shaped_lines();
    }

    pub fn request_cursor_reveal(&self, reveal: EditorCursorReveal) {
        self.viewport.request_cursor_reveal(reveal);
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height
    }

    pub fn viewport(&self) -> &EditorViewport {
        &self.viewport
    }

    pub fn viewport_mut(&mut self) -> &mut EditorViewport {
        &mut self.viewport
    }

    pub fn surface_metrics(&self) -> &EditorSurfaceMetrics {
        &self.surface_metrics
    }

    pub fn scrollbar_state(&self) -> &EditorScrollbarState {
        &self.scrollbar_state
    }

    pub fn selection_drag_state(&self) -> &EditorSelectionDragState {
        &self.selection_drag_state
    }

    pub fn overlay_state(&self) -> &EditorOverlayState {
        &self.overlay_state
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    fn metrics(line_height: Pixels, cell_width: Pixels) -> EditorTextMetrics {
        EditorTextMetrics {
            font_size: px(16.0),
            line_height,
            em_width: cell_width,
            cell_width,
        }
    }

    #[test]
    fn view_state_initializes_native_editor_substate() {
        let state = EditorViewState::new(px(20.0), px(8.0));

        assert_eq!(state.line_height(), px(20.0));
        assert_eq!(state.viewport().line_height(), px(20.0));
        assert_eq!(state.surface_metrics().get().line_height, px(20.0));
        assert_eq!(state.surface_metrics().get().cell_width, px(8.0));
        assert_eq!(state.overlay_state().gutter_width(), px(0.0));
    }

    #[test]
    fn view_state_applies_text_metrics_to_viewport_and_surface() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));

        state.apply_text_metrics(metrics(px(24.0), px(9.0)));

        assert_eq!(state.line_height(), px(24.0));
        assert_eq!(state.viewport().line_height(), px(24.0));
        assert_eq!(state.surface_metrics().get().line_height, px(24.0));
        assert_eq!(state.surface_metrics().get().cell_width, px(9.0));
    }

    #[test]
    fn view_state_forwards_cursor_reveal_requests_to_viewport() {
        let state = EditorViewState::new(px(20.0), px(8.0));

        state.request_cursor_reveal(EditorCursorReveal::Center);

        assert_eq!(
            state.viewport().take_cursor_reveal_request(),
            Some(EditorCursorReveal::Center)
        );
    }
}
