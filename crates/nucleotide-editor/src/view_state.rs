// ABOUTME: Persistent native editor view state shared by GPUI render phases
// ABOUTME: Bundles viewport, metrics, overlay, scrollbar, and selection state

use std::{cell::Cell, rc::Rc};

use gpui::{Pixels, Point, Size, TextStyle, TextSystem, px};
use helix_view::{DocumentId, Editor, Theme, ViewId};

use crate::{
    CursorOverlayPlan, EditorCursorReveal, EditorOverlayState, EditorPointerSelectionOutcome,
    EditorPointerSelectionPhase, EditorPointerSelectionUpdate, EditorScrollbarState,
    EditorSelectionDragState, EditorSurfaceMetrics, EditorSurfacePointerEvent, EditorTextMetrics,
    EditorViewport, EditorViewportContentLayout, EditorViewportContentUpdate,
    EditorViewportSurfaceLayout, EditorViewportSurfaceUpdate, LineLayoutCache,
    begin_editor_pointer_selection_at_event, update_editor_pointer_selection_at_event,
};

#[derive(Clone)]
pub struct EditorViewState {
    viewport: EditorViewport,
    surface_metrics: EditorSurfaceMetrics,
    scrollbar_state: EditorScrollbarState,
    selection_drag_state: EditorSelectionDragState,
    overlay_state: EditorOverlayState,
    line_height: Rc<Cell<Pixels>>,
}

pub struct EditorViewFrameState {
    pub viewport_update: EditorViewportSurfaceUpdate,
    pub line_cache: LineLayoutCache,
    pub first_row: usize,
    pub last_row_from_scroll: usize,
    pub scroll_line_offset: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorViewLayoutSnapshot {
    pub line_height: Pixels,
    pub cell_width: Pixels,
    pub gutter_width: Pixels,
    pub cursor_overlay_bounds: Option<(Point<Pixels>, Size<Pixels>)>,
    pub cursor_completion_anchor: Option<(Point<Pixels>, Size<Pixels>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewContentState {
    pub doc_id: DocumentId,
    pub update: EditorViewportContentUpdate,
    pub physical_lines: usize,
}

pub struct EditorViewContentPrepareParams<'a> {
    pub editor: &'a Editor,
    pub view_id: ViewId,
    pub theme: Option<&'a Theme>,
    pub text_system: &'a TextSystem,
    pub text_style: &'a TextStyle,
}

impl EditorViewState {
    pub fn new(line_height: Pixels, cell_width: Pixels) -> Self {
        Self {
            viewport: EditorViewport::new(line_height),
            surface_metrics: EditorSurfaceMetrics::new(line_height, cell_width),
            scrollbar_state: EditorScrollbarState::default(),
            selection_drag_state: EditorSelectionDragState::default(),
            overlay_state: EditorOverlayState::new(),
            line_height: Rc::new(Cell::new(line_height)),
        }
    }

    pub fn apply_text_metrics(&mut self, metrics: EditorTextMetrics) {
        self.line_height.set(metrics.line_height);
        self.viewport.set_line_height(metrics.line_height);
        self.surface_metrics
            .set(metrics.line_height, metrics.cell_width);
    }

    pub fn resolve_and_apply_text_metrics(
        &mut self,
        text_system: &TextSystem,
        style: &TextStyle,
    ) -> EditorTextMetrics {
        let metrics = EditorTextMetrics::resolve(text_system, style);
        self.apply_text_metrics(metrics);
        metrics
    }

    pub fn update_line_height_from_text_style(&mut self, style: &TextStyle) {
        let font_size = style.font_size.to_pixels(px(16.0));
        let line_height = style.line_height_in_pixels(font_size);
        self.line_height.set(line_height);
        self.viewport.set_line_height(line_height);

        let current_metrics = self.surface_metrics.get();
        self.surface_metrics
            .set(line_height, current_metrics.cell_width);
    }

    pub fn clear_shaped_lines_cache(&self) {
        self.surface_metrics.line_cache().clear_shaped_lines();
    }

    pub fn request_cursor_reveal(&self, reveal: EditorCursorReveal) {
        self.viewport.request_cursor_reveal(reveal);
    }

    pub fn sync_content_layout(
        &mut self,
        document: &helix_view::Document,
        view: &helix_view::View,
        layout: EditorViewportContentLayout<'_>,
    ) -> EditorViewportContentUpdate {
        self.viewport.sync_content_layout(document, view, layout)
    }

    pub fn sync_content_layout_for_editor(
        &mut self,
        editor: &Editor,
        view_id: ViewId,
        layout: EditorViewportContentLayout<'_>,
    ) -> Option<EditorViewContentState> {
        let view = editor.tree.try_get(view_id)?;
        let document = editor.document(view.doc)?;
        let update = self.sync_content_layout(document, view, layout);

        Some(EditorViewContentState {
            doc_id: view.doc,
            update,
            physical_lines: document.text().len_lines(),
        })
    }

    pub fn sync_content_layout_for_current_viewport(
        &mut self,
        editor: &Editor,
        view_id: ViewId,
        theme: Option<&Theme>,
        cell_width: Pixels,
    ) -> Option<EditorViewContentState> {
        self.sync_content_layout_for_editor(
            editor,
            view_id,
            EditorViewportContentLayout::for_editor(
                theme,
                self.viewport.viewport_bounds(),
                cell_width,
            ),
        )
    }

    pub fn prepare_content_for_render(
        &mut self,
        params: EditorViewContentPrepareParams<'_>,
    ) -> Option<EditorViewContentState> {
        let metrics = self.resolve_and_apply_text_metrics(params.text_system, params.text_style);
        self.sync_content_layout_for_current_viewport(
            params.editor,
            params.view_id,
            params.theme,
            metrics.cell_width,
        )
    }

    pub fn sync_frame_layout(
        &mut self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        mut layout: EditorViewportSurfaceLayout<'_>,
    ) -> Option<EditorViewFrameState> {
        self.line_height.set(layout.line_height);
        self.surface_metrics
            .set(layout.line_height, layout.cell_width);
        layout.cursor_reveal = layout
            .cursor_reveal
            .or_else(|| self.viewport.take_cursor_reveal_request());

        let viewport_update = self
            .viewport
            .sync_surface_layout(editor, doc_id, view_id, layout)?;

        self.overlay_state
            .set_gutter_width_from_columns(viewport_update.gutter_columns, layout.cell_width);

        let line_cache = self.surface_metrics.line_cache();
        line_cache.clear();
        let (first_row, last_row_from_scroll) = self.viewport.visible_visual_range();

        Some(EditorViewFrameState {
            viewport_update,
            line_cache,
            first_row,
            last_row_from_scroll,
            scroll_line_offset: self.viewport.offset_within_row(),
        })
    }

    pub fn apply_cursor_overlay_plan(&self, overlay_plan: Option<CursorOverlayPlan>) {
        self.overlay_state.apply_cursor_overlay_plan(overlay_plan);
    }

    pub fn layout_snapshot(&self) -> EditorViewLayoutSnapshot {
        let metrics = self.surface_metrics.get();
        EditorViewLayoutSnapshot {
            line_height: self.line_height(),
            cell_width: metrics.cell_width,
            gutter_width: self.overlay_state.gutter_width(),
            cursor_overlay_bounds: self.overlay_state.cursor_overlay_bounds(),
            cursor_completion_anchor: self.overlay_state.cursor_completion_anchor(),
        }
    }

    pub fn begin_pointer_selection_at_event(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        event: EditorSurfacePointerEvent,
    ) -> Option<EditorPointerSelectionUpdate> {
        let line_cache = self.surface_metrics.line_cache();
        begin_editor_pointer_selection_at_event(
            editor,
            doc_id,
            view_id,
            &line_cache,
            &self.selection_drag_state,
            event,
        )
    }

    pub fn update_pointer_selection_at_event(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        event: EditorSurfacePointerEvent,
    ) -> Option<EditorPointerSelectionUpdate> {
        let line_cache = self.surface_metrics.line_cache();
        update_editor_pointer_selection_at_event(
            editor,
            doc_id,
            view_id,
            &line_cache,
            &self.selection_drag_state,
            event,
        )
    }

    pub fn handle_pointer_selection_at_event(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        phase: EditorPointerSelectionPhase,
        event: EditorSurfacePointerEvent,
    ) -> Option<EditorPointerSelectionUpdate> {
        self.handle_pointer_selection_outcome(editor, doc_id, view_id, phase, event)
            .update()
    }

    pub fn handle_pointer_selection_for_view_at_event(
        &self,
        editor: &mut Editor,
        view_id: ViewId,
        phase: EditorPointerSelectionPhase,
        event: EditorSurfacePointerEvent,
    ) -> Option<EditorPointerSelectionUpdate> {
        self.handle_pointer_selection_for_view_outcome(editor, view_id, phase, event)?
            .update()
    }

    pub fn handle_pointer_selection_for_view_outcome(
        &self,
        editor: &mut Editor,
        view_id: ViewId,
        phase: EditorPointerSelectionPhase,
        event: EditorSurfacePointerEvent,
    ) -> Option<EditorPointerSelectionOutcome> {
        let doc_id = editor.tree.try_get(view_id).map(|view| view.doc)?;
        Some(self.handle_pointer_selection_outcome(editor, doc_id, view_id, phase, event))
    }

    pub fn handle_pointer_selection_outcome(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        phase: EditorPointerSelectionPhase,
        event: EditorSurfacePointerEvent,
    ) -> EditorPointerSelectionOutcome {
        match phase {
            EditorPointerSelectionPhase::Begin => self
                .begin_pointer_selection_at_event(editor, doc_id, view_id, event)
                .map_or(
                    EditorPointerSelectionOutcome::Missed { phase, event },
                    |update| EditorPointerSelectionOutcome::Applied { phase, update },
                ),
            EditorPointerSelectionPhase::Extend => self
                .update_pointer_selection_at_event(editor, doc_id, view_id, event)
                .map_or(
                    EditorPointerSelectionOutcome::Missed { phase, event },
                    |update| EditorPointerSelectionOutcome::Applied { phase, update },
                ),
            EditorPointerSelectionPhase::End => {
                self.clear_pointer_selection();
                EditorPointerSelectionOutcome::Ended { event }
            }
        }
    }

    pub fn clear_pointer_selection(&self) {
        self.selection_drag_state.clear();
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height.get()
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
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use gpui::{Bounds, TestAppContext, point, px, size};
    use helix_core::{Transaction, syntax};
    use helix_view::{
        DocumentId, Editor,
        editor::{Action, Config},
        graphics::Rect,
        handlers::Handlers,
        theme,
    };

    use crate::LineLayout;

    use super::*;

    fn metrics(line_height: Pixels, cell_width: Pixels) -> EditorTextMetrics {
        EditorTextMetrics {
            font_size: px(16.0),
            line_height,
            em_width: cell_width,
            cell_width,
        }
    }

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor_with_text(text: &str) -> (Editor, DocumentId, ViewId) {
        test_editor_with_config_and_text(Config::default(), text)
    }

    fn test_editor_with_config_and_text(
        config: Config,
        text: &str,
    ) -> (Editor, DocumentId, ViewId) {
        let config = Arc::new(ArcSwap::new(Arc::new(config)));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        (editor, doc_id, view_id)
    }

    fn pointer_event() -> EditorSurfacePointerEvent {
        EditorSurfacePointerEvent {
            position: point(px(4.0), px(4.0)),
            modifiers: gpui::Modifiers::none(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(80.0), px(20.0))),
            line_height: px(20.0),
            cell_width: px(8.0),
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
    fn view_state_clones_share_line_height_updates() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        let mut clone = state.clone();

        clone.apply_text_metrics(metrics(px(24.0), px(9.0)));

        assert_eq!(state.line_height(), px(24.0));
        assert_eq!(state.layout_snapshot().line_height, px(24.0));
    }

    #[gpui::test]
    fn view_state_resolves_and_applies_text_metrics(cx: &mut TestAppContext) {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let style = TextStyle::default();

        let metrics =
            cx.update(|cx| state.resolve_and_apply_text_metrics(cx.text_system(), &style));

        assert_eq!(state.line_height(), metrics.line_height);
        assert_eq!(state.viewport().line_height(), metrics.line_height);
        assert_eq!(
            state.surface_metrics().get().line_height,
            metrics.line_height
        );
        assert_eq!(state.surface_metrics().get().cell_width, metrics.cell_width);
    }

    #[gpui::test]
    fn view_state_prepares_render_content_with_resolved_metrics(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime_guard = runtime.enter();
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (editor, doc_id, view_id) = test_editor_with_text("one\ntwo\nthree\n");
        let style = TextStyle::default();

        let content_state = cx
            .update(|cx| {
                state.prepare_content_for_render(EditorViewContentPrepareParams {
                    editor: &editor,
                    view_id,
                    theme: None,
                    text_system: cx.text_system(),
                    text_style: &style,
                })
            })
            .unwrap();

        assert_eq!(content_state.doc_id, doc_id);
        assert_eq!(
            content_state.physical_lines,
            editor.document(doc_id).unwrap().text().len_lines()
        );
        assert!(content_state.update.visual_rows >= 3);
        assert_eq!(
            state.viewport().content_visual_rows(),
            content_state.update.visual_rows
        );
        assert_eq!(
            state.surface_metrics().get().line_height,
            state.line_height()
        );
        assert!(state.surface_metrics().get().cell_width > px(0.0));
    }

    #[test]
    fn view_state_layout_snapshot_collects_native_metrics() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        state.apply_text_metrics(metrics(px(24.0), px(9.0)));
        state
            .overlay_state()
            .set_gutter_width_from_columns(6, px(9.0));
        let overlay_plan = CursorOverlayPlan {
            cursor_position: point(px(4.0), px(6.0)),
            cursor_size: size(px(9.0), px(24.0)),
        };
        state.apply_cursor_overlay_plan(Some(overlay_plan));

        let snapshot = state.layout_snapshot();

        assert_eq!(snapshot.line_height, px(24.0));
        assert_eq!(snapshot.cell_width, px(9.0));
        assert_eq!(snapshot.gutter_width, px(54.0));
        assert_eq!(
            snapshot.cursor_overlay_bounds,
            Some((overlay_plan.cursor_position, overlay_plan.cursor_size))
        );
        assert_eq!(
            snapshot.cursor_completion_anchor,
            Some((point(px(4.0), px(30.0)), overlay_plan.cursor_size))
        );
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

    #[test]
    fn view_state_clears_owned_pointer_selection_state() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);

        state.clear_pointer_selection();

        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_begin_pointer_selection_uses_owned_cache_and_drag_state() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");

        let update =
            state.begin_pointer_selection_at_event(&mut editor, doc_id, view_id, pointer_event());

        assert!(update.is_none());
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_update_pointer_selection_uses_owned_cache() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(0);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");

        let update =
            state.update_pointer_selection_at_event(&mut editor, doc_id, view_id, pointer_event());

        assert!(update.is_none());
        assert_eq!(state.selection_drag_state().anchor(), Some(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_phase_begin_uses_owned_cache_and_drag_state() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");

        let update = state.handle_pointer_selection_at_event(
            &mut editor,
            doc_id,
            view_id,
            EditorPointerSelectionPhase::Begin,
            pointer_event(),
        );

        assert!(update.is_none());
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_phase_begin_reports_missed_outcome() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");
        let event = pointer_event();

        let outcome = state.handle_pointer_selection_outcome(
            &mut editor,
            doc_id,
            view_id,
            EditorPointerSelectionPhase::Begin,
            event,
        );

        assert_eq!(
            outcome,
            EditorPointerSelectionOutcome::Missed {
                phase: EditorPointerSelectionPhase::Begin,
                event
            }
        );
        assert!(!outcome.changed());
        assert_eq!(outcome.update(), None);
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_phase_extend_uses_owned_cache() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(0);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");

        let update = state.handle_pointer_selection_at_event(
            &mut editor,
            doc_id,
            view_id,
            EditorPointerSelectionPhase::Extend,
            pointer_event(),
        );

        assert!(update.is_none());
        assert_eq!(state.selection_drag_state().anchor(), Some(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_phase_end_clears_owned_drag_state() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");

        let update = state.handle_pointer_selection_at_event(
            &mut editor,
            doc_id,
            view_id,
            EditorPointerSelectionPhase::End,
            pointer_event(),
        );

        assert!(update.is_none());
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_phase_end_reports_ended_outcome() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\n");
        let event = pointer_event();

        let outcome = state.handle_pointer_selection_outcome(
            &mut editor,
            doc_id,
            view_id,
            EditorPointerSelectionPhase::End,
            event,
        );

        assert_eq!(outcome, EditorPointerSelectionOutcome::Ended { event });
        assert!(!outcome.changed());
        assert_eq!(outcome.update(), None);
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_selection_for_view_resolves_document() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        state.selection_drag_state().set_anchor(7);
        let (mut editor, _doc_id, view_id) = test_editor_with_text("one\n");
        let event = pointer_event();

        let outcome = state
            .handle_pointer_selection_for_view_outcome(
                &mut editor,
                view_id,
                EditorPointerSelectionPhase::End,
                event,
            )
            .unwrap();

        assert_eq!(outcome, EditorPointerSelectionOutcome::Ended { event });
        assert_eq!(state.selection_drag_state().anchor(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_pointer_selection_for_missing_view_returns_none() {
        let state = EditorViewState::new(px(20.0), px(8.0));
        let (mut editor, _doc_id, view_id) = test_editor_with_text("one\n");
        editor.tree.remove(view_id);

        let outcome = state.handle_pointer_selection_for_view_outcome(
            &mut editor,
            view_id,
            EditorPointerSelectionPhase::Begin,
            pointer_event(),
        );

        assert_eq!(outcome, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_sync_frame_layout_updates_frame_state() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\nthree\n");
        state
            .surface_metrics()
            .line_cache()
            .push(LineLayout::unwrapped(7, Default::default(), px(12.0)));

        let frame_state = state
            .sync_frame_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout {
                    theme: None,
                    bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0))),
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    cursor_reveal: None,
                },
            )
            .unwrap();

        assert_eq!(state.surface_metrics().get().line_height, px(20.0));
        assert_eq!(state.surface_metrics().get().cell_width, px(8.0));
        assert_eq!(
            state.overlay_state().gutter_width(),
            px(f32::from(frame_state.viewport_update.gutter_columns) * 8.0)
        );
        assert!(frame_state.viewport_update.visual_rows >= 3);
        assert_eq!(
            frame_state.first_row,
            state.viewport().visible_visual_range().0
        );
        assert_eq!(
            frame_state.last_row_from_scroll,
            state.viewport().visible_visual_range().1
        );
        assert_eq!(
            frame_state.scroll_line_offset,
            state.viewport().offset_within_row()
        );
        assert!(frame_state.line_cache.find_line_by_index(7).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_sync_content_layout_for_editor_resolves_view_document() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (editor, doc_id, view_id) = test_editor_with_text("one\ntwo\nthree\n");

        let content_state = state
            .sync_content_layout_for_editor(
                &editor,
                view_id,
                EditorViewportContentLayout {
                    theme: None,
                    bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0))),
                    cell_width: px(8.0),
                    minimum_columns: 1,
                },
            )
            .unwrap();

        assert_eq!(content_state.doc_id, doc_id);
        assert_eq!(
            content_state.physical_lines,
            editor.document(doc_id).unwrap().text().len_lines()
        );
        assert!(content_state.update.visual_rows >= 3);
        assert_eq!(
            state.viewport().content_visual_rows(),
            content_state.update.visual_rows
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn view_state_sync_content_layout_uses_current_viewport() {
        let mut config = Config::default();
        config.soft_wrap.enable = Some(true);
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (editor, doc_id, view_id) = test_editor_with_config_and_text(
            config,
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
        );
        state
            .viewport_mut()
            .set_layout(px(20.0), size(px(160.0), px(101.0)), 1);

        let content_state = state
            .sync_content_layout_for_current_viewport(&editor, view_id, None, px(8.0))
            .unwrap();

        assert_eq!(content_state.doc_id, doc_id);
        assert!(content_state.update.soft_wrap);
        assert!(content_state.update.visual_rows > content_state.physical_lines);
        assert_eq!(
            state.viewport().content_visual_rows(),
            content_state.update.visual_rows
        );
    }
}
