use gpui::{
    App, Bounds, Context, DefiniteLength, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render, SharedString,
    Size, Styled, TextStyle, Window, div, px,
};
use helix_core::Uri;
use helix_lsp::lsp::Diagnostic;
// Import helix's syntax highlighting system
use helix_view::{DocumentId, ViewId};
use nucleotide_logging::debug;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::Core;
use nucleotide_editor::{
    EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorCursorReveal, EditorLayout, EditorPointerSelectionPhase,
    EditorSurfacePointerEvent, EditorTextMetrics, EditorViewState, EditorViewportContentLayout,
    EditorViewportSurfaceLayout, NativeEditorFramePaintParams, NativeEditorFramePaintStyleParams,
    NativeEditorFramePlanParams, NativeEditorView, cursor_document_line,
    native_editor_frame_paint_plan, native_editor_frame_paint_style, paint_native_editor_frame,
};

// Removed unused debug helper: test_synthetic_click_accuracy
/*
#[cfg(test)]
fn test_synthetic_click_accuracy(
    line_cache: &nucleotide_editor::LineLayoutCache,
    target_line_idx: usize,
    target_char_idx: usize,
    bounds_width: gpui::Pixels,
    line_height: gpui::Pixels,
) -> Option<(usize, usize)> {
    // Find the target line in the cache
    if let Some(line_layout) = line_cache.find_line_by_index(target_line_idx) {
        // Calculate approximate pixel position for the target character
        // This is a simple approximation - real position would need character metrics
        let char_width_estimate = f32::from(line_layout.shaped_line.width)
            / line_layout.shaped_line.len() as f32;
        let estimated_x =
            f32::from(line_layout.origin.x) + (target_char_idx as f32 * char_width_estimate);
        let synthetic_position = gpui::point(gpui::px(estimated_x), line_layout.origin.y);

        // Test if this position would be found correctly
        if let Some(found_layout) =
            line_cache.find_line_at_position(synthetic_position, bounds_width, line_height)
        {
            // Calculate what character position this would resolve to
            let relative_x = synthetic_position.x - found_layout.origin.x;
            let resolved_byte_index = found_layout
                .shaped_line
                .index_for_x(relative_x)
                .unwrap_or(0);

            Some((found_layout.line_idx, resolved_byte_index))
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
fn test_shaped_line_accuracy(shaped_line: &gpui::ShapedLine, line_text: &str, _font_size: f32) {
    // Test various x positions and see if they map to sensible character indices
    let width = f32::from(shaped_line.width);
    let test_positions = vec![
        0.0,                        // Start of line
        width * 0.25,               // Quarter way
        width * 0.5,                // Middle
        width * 0.75,               // Three quarters
        width,                      // End of line
        width + 10.0,               // Beyond end
    ];

    for x_pos in test_positions.iter() {
        let px_x = gpui::px(*x_pos);
        let byte_index = shaped_line.index_for_x(px_x).unwrap_or(0);

        // Convert byte index to character index for validation
        let _char_index = line_text
            .char_indices()
            .take_while(|(byte_idx, _)| *byte_idx < byte_index)
            .count();
    }
}
*/

fn handle_editor_pointer_selection(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    editor_state: &EditorViewState,
    phase: EditorPointerSelectionPhase,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let mut pointer_update = None;

    core.update(cx, |core, cx| {
        pointer_update = editor_state.handle_pointer_selection_at_event(
            &mut core.editor,
            doc_id,
            view_id,
            phase,
            event,
        );

        if pointer_update.is_some() {
            cx.notify();
        }
    });

    if let Some(pointer_update) = pointer_update {
        debug!(
            phase = ?phase,
            line_idx = pointer_update.hit_test.line_idx,
            char_offset = pointer_update.hit_test.char_offset,
            anchor = pointer_update.selection.anchor,
            target_pos = pointer_update.selection.head,
            "Applied editor pointer selection"
        );
    } else if matches!(phase, EditorPointerSelectionPhase::End) {
        debug!(position = ?event.position, "Mouse up event - pointer selection ended");
    } else {
        debug!(
            phase = ?phase,
            window_pos = ?event.position,
            bounds = ?event.bounds,
            line_height = %event.line_height,
            "Pointer hit test did not find a rendered line"
        );
    }
}

pub struct DocumentView {
    core: Entity<Core>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    editor_state: EditorViewState,
}

impl DocumentView {
    pub fn new(
        core: Entity<Core>,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
    ) -> Self {
        // Create viewport with placeholder document metrics (updated during render/paint).
        let line_height = px(20.0); // Default, will be updated
        let editor_state = EditorViewState::new(line_height, px(8.0));

        Self {
            core,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            editor_state,
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) {
        self.is_focused = is_focused;
    }

    pub fn update_text_style(&mut self, style: TextStyle) {
        // Recalculate line height with new font size
        // Use the actual font size as rem base for proper line height calculation
        self.editor_state.update_line_height_from_text_style(&style);
        self.style = style;
        self.editor_state.clear_shaped_lines_cache();
    }

    pub fn clear_shaped_lines_cache(&self) {
        self.editor_state.clear_shaped_lines_cache();
    }

    pub fn request_cursor_reveal(&self) {
        self.editor_state
            .request_cursor_reveal(EditorCursorReveal::Scrolloff);
    }

    pub fn request_cursor_center(&self) {
        self.editor_state
            .request_cursor_reveal(EditorCursorReveal::Center);
    }

    /// Convert a Helix anchor (character position) to scroll pixels
    #[allow(dead_code)]
    fn anchor_to_scroll_px(&self, anchor_char: usize, document: &helix_view::Document) -> Pixels {
        let row = document.text().char_to_line(anchor_char);
        self.editor_state.line_height() * (row as f32)
    }

    /// Convert scroll pixels to a Helix anchor (character position)
    #[allow(dead_code)]
    fn scroll_px_to_anchor(&self, y: Pixels, document: &helix_view::Document) -> usize {
        let row = (y / self.editor_state.line_height()).floor() as usize;
        let text = document.text();
        let clamped_row = row.min(text.len_lines().saturating_sub(1));
        text.line_to_char(clamped_row)
    }

    fn get_diagnostics(&self, cx: &mut Context<Self>) -> Vec<Diagnostic> {
        if !self.is_focused {
            return Vec::new();
        }

        let core = self.core.read(cx);
        let editor = &core.editor;

        let (cursor_line, doc_id) = {
            let view = match editor.tree.try_get(self.view_id) {
                Some(v) => v,
                None => return Vec::new(),
            };
            let doc_id = view.doc;
            let document = match editor.document(doc_id) {
                Some(doc) => doc,
                None => return Vec::new(), // Document was closed
            };
            let text = document.text();

            let primary_idx = document
                .selection(self.view_id)
                .primary()
                .cursor(text.slice(..));
            let cursor_at_trailing_newline = primary_idx == text.len_chars()
                && text.len_chars() > 0
                && text.char(text.len_chars() - 1) == '\n';
            (
                cursor_document_line(text.slice(..), primary_idx, cursor_at_trailing_newline),
                doc_id,
            )
        };

        let mut diags = Vec::new();
        if let Some(path) = editor.document(doc_id).and_then(|doc| doc.path()).cloned() {
            let uri = Uri::from(path);
            if let Some(diagnostics) = editor.diagnostics.get(&uri) {
                for (diag, _) in diagnostics.iter().filter(|(diag, _)| {
                    let (start_line, end_line) =
                        (diag.range.start.line as usize, diag.range.end.line as usize);
                    start_line <= cursor_line && cursor_line <= end_line
                }) {
                    diags.push(diag.clone());
                }
            }
        }
        diags
    }

    /// Get the actual line height used by this DocumentView
    pub fn get_line_height(&self) -> Pixels {
        self.editor_state.line_height()
    }

    /// Get the last painted gutter width in window pixels.
    pub fn get_gutter_width(&self) -> Pixels {
        self.editor_state.overlay_state().gutter_width()
    }

    /// Get the cursor's last painted top-left position and size in window coordinates.
    pub fn get_cursor_overlay_bounds(&self) -> Option<(Point<Pixels>, Size<Pixels>)> {
        self.editor_state.overlay_state().cursor_overlay_bounds()
    }

    /// Get the last cursor position and size in window coordinates
    /// Returns (position, size) where position is bottom-left corner for completion positioning
    pub fn get_cursor_coordinates(&self) -> Option<(Point<Pixels>, Size<Pixels>)> {
        self.editor_state.overlay_state().cursor_completion_anchor()
    }
}

impl EventEmitter<DismissEvent> for DocumentView {}

impl Render for DocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // DocumentView render creates the native editor element for actual painting.
        let doc_id = {
            let editor = &self.core.read(cx).editor;
            match editor.tree.try_get(self.view_id) {
                Some(view) => view.doc,
                None => {
                    // View no longer exists, render empty div
                    return div().id(SharedString::from(format!("doc-view-{:?}", self.view_id)));
                }
            }
        };

        let metrics = EditorTextMetrics::resolve(cx.text_system(), &self.style);
        self.editor_state.apply_text_metrics(metrics);

        // Prime viewport content metrics from the latest known native surface size.
        {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(view) = editor.tree.try_get(self.view_id)
                && let Some(document) = editor.document(doc_id)
            {
                let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
                let viewport_bounds = self.editor_state.viewport().viewport_bounds();
                let content_update = self.editor_state.sync_content_layout(
                    document,
                    view,
                    EditorViewportContentLayout::for_editor(
                        Some(&theme),
                        viewport_bounds,
                        metrics.cell_width,
                    ),
                );

                debug!(
                    physical_lines = document.text().len_lines(),
                    visual_rows = content_update.visual_rows,
                    soft_wrap = content_update.soft_wrap,
                    "Primed native editor viewport content metrics"
                );
            }
        }

        let editor_content = {
            let core = self.core.clone();
            let view_id = self.view_id;
            let style = self.style.clone();
            let focus = self.focus.clone();
            let is_focused = self.is_focused;

            NativeEditorView::new(
                cx.entity_id(),
                self.editor_state.clone(),
                style.clone(),
                move |editor_state, bounds, after_layout, window, cx| {
                    paint_document_content(DocumentPaintParams {
                        core: &core,
                        doc_id,
                        view_id,
                        style: &style,
                        focus: &focus,
                        is_focused,
                        editor_state,
                        bounds,
                        layout: after_layout,
                        window,
                        cx,
                    });
                },
            )
            .on_scroll({
                move |_viewport, scroll_update, _cx| {
                    debug!(
                        crossed_lines = scroll_update.crossed_visual_rows,
                        top_visual_row = scroll_update.top_visual_row,
                        offset_within_row = %scroll_update.offset_within_row,
                        "Scroll wheel event handled by editor surface"
                    );
                }
            })
            .on_pointer_selection({
                let core = self.core.clone();
                let view_id = self.view_id;
                let editor_state = self.editor_state.clone();

                move |phase, event, cx| {
                    handle_editor_pointer_selection(
                        &core,
                        doc_id,
                        view_id,
                        &editor_state,
                        phase,
                        event,
                        cx,
                    );
                }
            })
        };

        let diags = {
            let _theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

            self.get_diagnostics(cx).into_iter().map(move |diag| {
                // DIAGNOSTIC RENDERING:
                // DiagnosticView is disabled pending implementation of a GPUI-based diagnostic popup
                // This would need to render diag.message, diag.severity, and position the popup
                // relative to the diagnostic location in the editor
                // For now, diagnostics are handled through syntax highlighting in the editor
                div().id(("diagnostic", diag.range.start.line as usize)) // Unique ID for each diagnostic
            })
        };

        div()
            .id(SharedString::from(format!("doc-view-{:?}", self.view_id)))
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .child(editor_content)
            .child(
                div()
                    .flex()
                    .w(DefiniteLength::Fraction(0.33))
                    .h(DefiniteLength::Fraction(0.8))
                    .flex_col()
                    .absolute()
                    .top_8()
                    .right_5()
                    .gap_4()
                    .children(diags),
            )
    }
}

impl Focusable for DocumentView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct DocumentPaintParams<'a> {
    core: &'a Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    style: &'a TextStyle,
    focus: &'a FocusHandle,
    is_focused: bool,
    editor_state: &'a mut EditorViewState,
    bounds: Bounds<Pixels>,
    layout: &'a mut EditorLayout,
    window: &'a mut Window,
    cx: &'a mut App,
}

fn paint_document_content(params: DocumentPaintParams<'_>) {
    let DocumentPaintParams {
        core,
        doc_id,
        view_id,
        style,
        focus,
        is_focused,
        editor_state,
        bounds,
        layout,
        window,
        cx,
    } = params;

    let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
    let frame_state = core.update(cx, |core, _cx| {
        editor_state.sync_frame_layout(
            &mut core.editor,
            doc_id,
            view_id,
            EditorViewportSurfaceLayout::for_editor(
                Some(&theme),
                bounds,
                layout.cell_width,
                layout.line_height,
                None,
            ),
        )
    });
    let Some(frame_state) = frame_state else {
        return;
    };

    let paint_plan = {
        let core = core.read(cx);
        let editor = &core.editor;

        let view = match editor.tree.try_get(view_id) {
            Some(v) => v,
            None => return,
        };
        let _viewport = view.area;
        let tokens = cx.theme().tokens;
        let bg_color = tokens.editor.background;
        let fg_color = tokens.editor.text_primary;
        let document = match editor.document(doc_id) {
            Some(doc) => doc,
            None => return,
        };
        let ui_tokens = cx.ui_theme().tokens;
        let paint_style = native_editor_frame_paint_style(NativeEditorFramePaintStyleParams {
            editor,
            theme_style: |key| cx.theme_style(key),
            fg_color,
            bg_color,
            selection_primary: tokens.editor.selection_primary,
            selection_secondary: tokens.editor.selection_secondary,
            fallback_gutter_color: ui_tokens.editor.line_number,
            diagnostic_highlight_base: tokens.chrome.text_on_chrome,
            fallback_ruler_color: ui_tokens.chrome.border_default,
        });
        native_editor_frame_paint_plan(NativeEditorFramePlanParams {
            editor,
            document,
            view,
            view_id,
            theme: &theme,
            frame_state: &frame_state,
            bounds,
            layout,
            text_style: style,
            font_size: style.font_size.to_pixels(px(16.0)),
            is_focused,
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            style: paint_style,
        })
    };

    let element_focused = focus.is_focused(window);
    let overlay_plan = paint_native_editor_frame(
        window,
        cx,
        NativeEditorFramePaintParams {
            editor_state,
            frame_state: &frame_state,
            plan: &paint_plan,
            layout,
            text_style: style,
            diagnostic_theme: &theme,
            element_focused,
        },
    );

    let layout_info = cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
    if let Some(overlay_plan) = overlay_plan {
        layout_info.cursor_position = Some(overlay_plan.cursor_position);
        layout_info.cursor_size = Some(overlay_plan.cursor_size);
    } else {
        layout_info.cursor_position = None;
        layout_info.cursor_size = None;
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights
