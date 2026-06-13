use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, Context, DefiniteLength, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    Styled, TextStyle, Window, div, px,
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
    DocumentFramePaintParams, EditorCursorReveal, EditorDocumentElement,
    EditorDocumentFrameGutterParams, EditorDocumentFrameParams, EditorLayout, EditorScrollbarState,
    EditorSelectionDragState, EditorSurface, EditorSurfaceMetrics, EditorSurfacePointerEvent,
    EditorTextMetrics, EditorViewport, EditorViewportContentLayout, EditorViewportSurfaceLayout,
    LineLayoutCache, begin_editor_pointer_selection_at_event, cursor_document_line,
    cursor_style_for_mode, editor_document_frame, gpui_hsla_to_helix_color, paint_document_frame,
    paint_editor_background, shape_cursor_text, update_editor_pointer_selection_at_event,
};
use nucleotide_ui::theme_utils::color_to_hsla;

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

fn handle_editor_mouse_down(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    drag_state: &EditorSelectionDragState,
    line_cache: &LineLayoutCache,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let mut pointer_update = None;

    core.update(cx, |core, cx| {
        pointer_update = begin_editor_pointer_selection_at_event(
            &mut core.editor,
            doc_id,
            view_id,
            line_cache,
            drag_state,
            event,
        );

        if pointer_update.is_some() {
            cx.notify();
        }
    });

    if let Some(pointer_update) = pointer_update {
        debug!(
            line_idx = pointer_update.hit_test.line_idx,
            char_offset = pointer_update.hit_test.char_offset,
            anchor = pointer_update.selection.anchor,
            target_pos = pointer_update.selection.head,
            "Applied editor click selection"
        );
    } else {
        debug!(
            window_pos = ?event.position,
            bounds = ?event.bounds,
            line_height = %event.line_height,
            "Click hit test did not find a rendered line"
        );
    }
}

fn handle_editor_mouse_drag(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    drag_state: &EditorSelectionDragState,
    line_cache: &LineLayoutCache,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let mut pointer_update = None;

    core.update(cx, |core, cx| {
        pointer_update = update_editor_pointer_selection_at_event(
            &mut core.editor,
            doc_id,
            view_id,
            line_cache,
            drag_state,
            event,
        );

        if pointer_update.is_some() {
            cx.notify();
        }
    });

    if let Some(pointer_update) = pointer_update {
        debug!(
            line_idx = pointer_update.hit_test.line_idx,
            char_offset = pointer_update.hit_test.char_offset,
            anchor = pointer_update.selection.anchor,
            target_pos = pointer_update.selection.head,
            "Applied editor drag selection"
        );
    }
}

pub struct DocumentView {
    core: Entity<Core>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    viewport: EditorViewport,
    scrollbar_state: EditorScrollbarState,
    surface_metrics: EditorSurfaceMetrics,
    cursor_reveal_requested: Rc<Cell<Option<EditorCursorReveal>>>,
    line_height: Pixels,
    selection_drag_state: EditorSelectionDragState,
    /// Last cursor position in window coordinates (for completion positioning)
    last_cursor_position: Option<gpui::Point<Pixels>>,
    /// Last cursor dimensions (for completion positioning)  
    last_cursor_size: Option<gpui::Size<Pixels>>,
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
        let viewport = EditorViewport::new(line_height);
        let surface_metrics = EditorSurfaceMetrics::new(line_height, px(8.0));

        Self {
            core,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            viewport,
            scrollbar_state: EditorScrollbarState::default(),
            surface_metrics,
            cursor_reveal_requested: Rc::new(Cell::new(None)),
            line_height,
            selection_drag_state: EditorSelectionDragState::default(),
            last_cursor_position: None,
            last_cursor_size: None,
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) {
        self.is_focused = is_focused;
    }

    pub fn update_text_style(&mut self, style: TextStyle) {
        // Recalculate line height with new font size
        // Use the actual font size as rem base for proper line height calculation
        let font_size = style.font_size.to_pixels(px(16.0));
        self.line_height = style.line_height_in_pixels(font_size);
        self.style = style;
        self.surface_metrics.line_cache().clear_shaped_lines();
    }

    pub fn clear_shaped_lines_cache(&self) {
        self.surface_metrics.line_cache().clear_shaped_lines();
    }

    pub fn request_cursor_reveal(&self) {
        self.cursor_reveal_requested
            .set(Some(EditorCursorReveal::Scrolloff));
    }

    pub fn request_cursor_center(&self) {
        self.cursor_reveal_requested
            .set(Some(EditorCursorReveal::Center));
    }

    /// Convert a Helix anchor (character position) to scroll pixels
    #[allow(dead_code)]
    fn anchor_to_scroll_px(&self, anchor_char: usize, document: &helix_view::Document) -> Pixels {
        let row = document.text().char_to_line(anchor_char);
        self.line_height * (row as f32)
    }

    /// Convert scroll pixels to a Helix anchor (character position)
    #[allow(dead_code)]
    fn scroll_px_to_anchor(&self, y: Pixels, document: &helix_view::Document) -> usize {
        let row = (y / self.line_height).floor() as usize;
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

    /// Get the current cursor position in window coordinates
    /// Returns None if cursor is not visible or cannot be calculated
    /// TODO: Implement this method once we figure out the correct Helix API usage
    #[allow(dead_code)]
    /// Get the actual line height used by this DocumentView
    pub fn get_line_height(&self) -> Pixels {
        self.line_height
    }

    /// Get the last cursor position and size in window coordinates
    /// Returns (position, size) where position is bottom-left corner for completion positioning
    pub fn get_cursor_coordinates(&self) -> Option<(gpui::Point<Pixels>, gpui::Size<Pixels>)> {
        if let (Some(pos), Some(size)) = (self.last_cursor_position, self.last_cursor_size) {
            // Return bottom-left corner of cursor for completion positioning
            let bottom_left = gpui::Point {
                x: pos.x,
                y: pos.y + size.height,
            };
            Some((bottom_left, size))
        } else {
            None
        }
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
        self.line_height = metrics.line_height;
        self.surface_metrics
            .set(metrics.line_height, metrics.cell_width);
        let surface_metrics = self.surface_metrics.clone();

        // Prime viewport content metrics from the latest known native surface size.
        {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(view) = editor.tree.try_get(self.view_id)
                && let Some(document) = editor.document(doc_id)
            {
                self.viewport.set_line_height(metrics.line_height);
                let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
                let viewport_bounds = self.viewport.viewport_bounds();
                let content_update = self.viewport.sync_content_layout(
                    document,
                    view,
                    EditorViewportContentLayout {
                        theme: Some(&theme),
                        bounds: viewport_bounds,
                        cell_width: metrics.cell_width,
                        minimum_columns: 1,
                    },
                );

                debug!(
                    physical_lines = document.text().len_lines(),
                    visual_rows = content_update.visual_rows,
                    soft_wrap = content_update.soft_wrap,
                    "Primed native editor viewport content metrics"
                );
            }
        }

        let document_element = {
            let core = self.core.clone();
            let view_id = self.view_id;
            let style = self.style.clone();
            let focus = self.focus.clone();
            let is_focused = self.is_focused;
            let mut viewport = self.viewport.clone();
            let surface_metrics = surface_metrics.clone();
            let cursor_reveal_requested = Rc::clone(&self.cursor_reveal_requested);

            EditorDocumentElement::new(style.clone(), move |bounds, after_layout, window, cx| {
                paint_document_content(DocumentPaintParams {
                    core: &core,
                    doc_id,
                    view_id,
                    style: &style,
                    focus: &focus,
                    is_focused,
                    viewport: &mut viewport,
                    surface_metrics: &surface_metrics,
                    cursor_reveal_requested: cursor_reveal_requested.as_ref(),
                    bounds,
                    layout: after_layout,
                    window,
                    cx,
                });
            })
        };

        let editor_surface = EditorSurface::new(
            cx.entity_id(),
            self.viewport.clone(),
            surface_metrics.clone(),
            self.scrollbar_state.clone(),
            document_element,
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
        .on_mouse_down({
            let core = self.core.clone();
            let view_id = self.view_id;
            let selection_drag_state = self.selection_drag_state.clone();
            let line_cache = surface_metrics.line_cache();

            move |event: EditorSurfacePointerEvent, cx| {
                handle_editor_mouse_down(
                    &core,
                    doc_id,
                    view_id,
                    &selection_drag_state,
                    &line_cache,
                    event,
                    cx,
                );
            }
        })
        .on_mouse_drag({
            let core = self.core.clone();
            let view_id = self.view_id;
            let selection_drag_state = self.selection_drag_state.clone();
            let line_cache = surface_metrics.line_cache();

            move |event: EditorSurfacePointerEvent, cx| {
                handle_editor_mouse_drag(
                    &core,
                    doc_id,
                    view_id,
                    &selection_drag_state,
                    &line_cache,
                    event,
                    cx,
                );
            }
        })
        .on_mouse_up({
            let selection_drag_state = self.selection_drag_state.clone();

            move |event: EditorSurfacePointerEvent, _cx| {
                selection_drag_state.clear();
                debug!(position = ?event.position, "Mouse up event - click completed");
            }
        });

        let editor_content = div().id("editor-content").w_full().h_full().flex().child(
            div()
                .id("editor-paint-area")
                .w_full()
                .h_full()
                .flex_1()
                .child(editor_surface),
        );

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
    viewport: &'a mut EditorViewport,
    surface_metrics: &'a EditorSurfaceMetrics,
    cursor_reveal_requested: &'a Cell<Option<EditorCursorReveal>>,
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
        viewport,
        surface_metrics,
        cursor_reveal_requested,
        bounds,
        layout,
        window,
        cx,
    } = params;

    let _focus = focus.clone();
    let cell_width = layout.cell_width;
    let line_height = layout.line_height;

    surface_metrics.set(line_height, cell_width);

    {
        let tokens = cx.theme().tokens;
        let bgc = tokens.editor.background;
        paint_editor_background(window, bounds, bgc);
    }

    let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
    let mut defer_core_redraw = false;
    let viewport_update = core.update(cx, |core, _cx| {
        let update = viewport.sync_surface_layout(
            &mut core.editor,
            doc_id,
            view_id,
            EditorViewportSurfaceLayout {
                theme: Some(&theme),
                bounds,
                cell_width: layout.cell_width,
                line_height: layout.line_height,
                minimum_columns: 1,
                cursor_reveal: cursor_reveal_requested.replace(None),
            },
        );

        if update
            .as_ref()
            .is_some_and(|update| update.helix_view_synced || update.cursor_revealed)
        {
            defer_core_redraw = true;
        }

        update
    });
    if defer_core_redraw {
        let core_entity_id = core.entity_id();
        cx.defer(move |cx| {
            cx.notify(core_entity_id);
        });
    }
    let Some(viewport_update) = viewport_update else {
        return;
    };
    let gutter_width_cells = viewport_update.gutter_columns;
    let _gutter_width_px = cell_width * f32::from(gutter_width_cells);

    let soft_wrap_enabled = viewport_update.soft_wrap;

    let line_cache = surface_metrics.line_cache();
    line_cache.clear();

    {
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
        let default_text_style = helix_view::graphics::Style {
            fg: gpui_hsla_to_helix_color(tokens.editor.text_primary),
            bg: gpui_hsla_to_helix_color(tokens.editor.background),
            ..Default::default()
        };

        let document = match editor.document(doc_id) {
            Some(doc) => doc,
            None => return,
        };
        let text = document.text();
        let editor_config = editor.config();
        let editor_mode = editor.mode();
        let (_, cursor_kind) = editor.cursor();

        let (first_row, last_row_from_scroll) = viewport.visible_visual_range();
        let scroll_line_offset = viewport.offset_within_row();

        let cursor_style = cursor_style_for_mode(editor_mode, |key| cx.theme_style(key));
        let wrap_indicator_color = cx.theme_style("ui.virtual.wrap").fg.and_then(color_to_hsla);
        const THEME_KEY_VIRTUAL_RULER: &str = "ui.virtual.ruler";
        let ruler_style = cx.theme_style(THEME_KEY_VIRTUAL_RULER);
        let ruler_color = ruler_style.bg.and_then(color_to_hsla).unwrap_or_else(|| {
            // Use UI theme's border color from tokens
            cx.ui_theme().tokens.chrome.border_default
        });
        let loader = editor.syn_loader.load();
        let frame = editor_document_frame(EditorDocumentFrameParams {
            document,
            view,
            view_id,
            theme: &theme,
            syntax_loader: &loader,
            first_row,
            last_row_from_scroll,
            soft_wrap_enabled,
            unwrapped_gutter: Some(EditorDocumentFrameGutterParams { editor, layout }),
            bounds,
            cell_width: layout.cell_width,
            line_height: layout.line_height,
            scroll_line_offset,
            soft_wrap_minimum_columns: 10,
            fg_color,
            font: style.font(),
            default_text_style,
            default_bg: bg_color,
            wrap_indicator_color,
            ruler_color,
            editor_mode,
            cursor_kind,
            cursor_style,
            cursor_shape: editor_config.cursor_shape.clone(),
            editor_rulers: editor_config.rulers.clone(),
            cursorline_enabled: editor_config.cursorline && is_focused,
            is_focused,
        });
        debug!(
            "Cursorline check - config value: {}, focused: {}, enabled: {}",
            editor_config.cursorline, is_focused, frame.cursorline_enabled
        );

        let cursorline_style = if frame.cursorline_enabled {
            let style = cx.theme_style("ui.cursorline.primary");
            debug!(
                "Cursorline style found: bg={:?}, fg={:?}",
                style.bg, style.fg
            );
            style.bg.and_then(color_to_hsla)
        } else {
            None
        };

        let gutter_width = frame.gutter_width;
        let cursor_char_idx = frame.cursor_presentation.cursor_char_idx;
        let cursor_line_num = frame.render_snapshot.cursor_line;
        debug!(
            "Cursor position: line={}, char_idx={}",
            cursor_line_num, cursor_char_idx
        );
        debug!(
            "Cursor position - line: {}, col_in_line: {}, primary_idx: {}, gutter_width: {}",
            frame.primary_cursor_line,
            frame.primary_cursor_col,
            frame.primary_cursor_idx,
            gutter_width
        );
        if gutter_width != 0 {
            debug!("need to render gutter {gutter_width}");
        }

        let line_viewport = frame.render_snapshot.line_viewport;
        let last_row = frame.render_snapshot.last_row;
        let cursor_at_end = line_viewport.cursor_at_end;
        let file_ends_with_newline = line_viewport.file_ends_with_newline;

        debug!(
            "End of file check - cursor_char_idx: {}, text.len_chars(): {}, last_char: {:?}, cursor_at_end: {}, ends_with_newline: {}",
            cursor_char_idx,
            text.len_chars(),
            if text.len_chars() > 0 {
                Some(text.char(text.len_chars() - 1))
            } else {
                None
            },
            cursor_at_end,
            file_ends_with_newline
        );

        if cursor_at_end && file_ends_with_newline {
            let cursor_line = text.char_to_line(cursor_char_idx.saturating_sub(1));
            debug!(
                "Cursor at EOF with newline - cursor_line: {cursor_line}, last_row: {last_row}, total_lines: {}",
                frame.total_lines
            );
        }

        let diagnostic_theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
        let doc_text = document.text().clone();
        let _tab_width = document.tab_width() as u16;

        let cursor_text_shape = shape_cursor_text(
            window.text_system().as_ref(),
            frame.cursor_presentation.block_text.clone(),
            &style.font(),
            style.font_size.to_pixels(px(16.0)),
            &frame.cursor_presentation.text_style_at_cursor,
            frame.cursor_presentation.block_text_color(bg_color),
            bg_color,
        );

        let text = doc_text.slice(..);

        let gutter_style = cx.theme_style("ui.linenr");
        let gutter_selected_style = cx.theme_style("ui.linenr.selected");
        let default_gutter_color = cx.ui_theme().tokens.editor.line_number;
        let gutter_color = gutter_style
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(default_gutter_color);
        let gutter_selected_color = gutter_selected_style
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(default_gutter_color);
        let gutter_bg = cx
            .theme_style("ui.gutter")
            .bg
            .and_then(crate::utils::color_to_hsla);
        let element_focused = focus.is_focused(window);
        if let Some(overlay_plan) = paint_document_frame(
            window,
            cx,
            DocumentFramePaintParams {
                frame: &frame,
                text,
                bounds,
                layout,
                text_style: style,
                line_cache: &line_cache,
                font_size: style.font_size.to_pixels(px(16.0)),
                fg_color,
                default_bg: bg_color,
                cursorline_color: cursorline_style,
                cursor_text_shape: &cursor_text_shape,
                is_focused,
                element_focused,
                selection_primary: tokens.editor.selection_primary,
                selection_secondary: tokens.editor.selection_secondary,
                gutter_color,
                gutter_selected_color,
                diagnostic_theme: &diagnostic_theme,
                diagnostic_highlight_base: cx.theme().tokens.chrome.text_on_chrome,
                gutter_bg,
                scroll_line_offset,
            },
        ) {
            let layout_info = cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
            layout_info.cursor_position = Some(overlay_plan.cursor_position);
            layout_info.cursor_size = Some(overlay_plan.cursor_size);
        }
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights
