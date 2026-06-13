use std::{cell::Cell, rc::Rc};

use gpui::prelude::FluentBuilder;
use gpui::size;
use gpui::{
    App, Bounds, Context, DefiniteLength, DismissEvent, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, ParentElement, Pixels, Point, Render, SharedString, Size, Style, Styled,
    TextStyle, Window, div, px, relative, white,
};
use helix_core::Uri;
use helix_lsp::lsp::Diagnostic;
// Import helix's syntax highlighting system
use helix_view::{DocumentId, Theme, ViewId};
use nucleotide_logging::{debug, error};
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::Core;
use nucleotide_editor::{
    EditorCursor, EditorDocumentMetrics, EditorLayout, EditorLineBackgroundStyle, EditorSurface,
    EditorSurfaceGeometry, EditorSurfaceMetrics, EditorSurfacePointerEvent, EditorTextMetrics,
    EditorViewport, GutterLineParams, HighlightLineParams, LineLayout, LineLayoutCache,
    block_cursor_text, build_gutter_lines, build_soft_wrap_gutter_lines, cursor_background_color,
    cursor_document_line, cursor_foreground_color, cursor_has_reversed_modifier,
    cursor_line_position, cursor_style_for_mode, cursor_viewport_position,
    decorate_soft_wrap_line_runs, diagnostic_marker_paint_style, diagnostic_marker_plan,
    diagnostic_overlay_spans, diagnostic_severity_by_line, document_text_format_for_surface,
    gpui_hsla_to_helix_color, highlight_line, hit_test_document_position, line_viewport_plan,
    paint_cursorline_background, paint_diagnostic_marker, paint_editor_background,
    paint_editor_line, paint_gutter_lines, paint_soft_wrap_gutter_lines, paint_visible_rulers,
    phantom_line_cursor_paint_position, shape_cursor_text,
    shared_line_text_without_trailing_newline, soft_wrap_cursor_paint_position,
    soft_wrap_gutter_line_paint_plans, soft_wrap_gutter_line_plans, soft_wrap_line_paint_plans,
    soft_wrap_viewport_height, soft_wrap_visual_lines, soft_wrap_visual_position,
    text_style_at_position, unwrapped_cursor_paint_position, unwrapped_line_paint_plans,
    unwrapped_visible_line_plans, visible_ruler_paint_plans,
};
use nucleotide_ui::scrollbar::{ScrollableHandle, Scrollbar, ScrollbarState};
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

/// Custom scroll handle for DocumentView that integrates with EditorViewport
#[derive(Clone)]
pub struct DocumentScrollHandle {
    viewport: EditorViewport,
    on_change: Option<Rc<dyn Fn()>>,
    view_id: ViewId,
}

impl std::fmt::Debug for DocumentScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentScrollHandle")
            .field("viewport", &self.viewport)
            .field("on_change", &self.on_change.is_some())
            .field("view_id", &self.view_id)
            .finish()
    }
}

impl DocumentScrollHandle {
    pub fn new(viewport: EditorViewport, view_id: ViewId) -> Self {
        Self {
            viewport,
            on_change: None,
            view_id,
        }
    }

    pub fn with_callback(viewport: EditorViewport, on_change: impl Fn() + 'static) -> Self {
        Self {
            viewport,
            on_change: Some(Rc::new(on_change)),
            view_id: ViewId::default(),
        }
    }
}

impl ScrollableHandle for DocumentScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        self.viewport.max_scroll_offset()
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.viewport.set_scroll_offset_from_scrollbar(point);

        // Mark that we need to sync back to Helix
        // This will be done in the next paint cycle when we have access to cx

        // Trigger callback if available to notify of change
        if let Some(on_change) = &self.on_change {
            on_change();
        }
    }

    fn offset(&self) -> Point<Pixels> {
        self.viewport.scroll_offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.viewport.viewport_bounds()
    }
}

fn handle_editor_mouse_down(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    drag_anchor: &Cell<Option<usize>>,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    if let Some(hit_test) = hit_test_editor_pointer(core, doc_id, view_id, event, cx) {
        let anchor = if event.modifiers.shift {
            primary_selection_anchor(core, doc_id, view_id, cx).unwrap_or(hit_test.char_idx)
        } else {
            hit_test.char_idx
        };
        let target_pos =
            apply_editor_selection(core, doc_id, view_id, anchor, hit_test.char_idx, cx);

        if let Some(target_pos) = target_pos {
            drag_anchor.set(Some(anchor));
            debug!(
                line_idx = hit_test.line_idx,
                char_offset = hit_test.char_offset,
                anchor,
                target_pos,
                "Applied editor click selection"
            );
        }
    } else {
        drag_anchor.set(None);
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
    drag_anchor: &Cell<Option<usize>>,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let Some(anchor) = drag_anchor.get() else {
        return;
    };

    if let Some(hit_test) = hit_test_editor_pointer(core, doc_id, view_id, event, cx)
        && let Some(target_pos) =
            apply_editor_selection(core, doc_id, view_id, anchor, hit_test.char_idx, cx)
    {
        debug!(
            line_idx = hit_test.line_idx,
            char_offset = hit_test.char_offset,
            anchor,
            target_pos,
            "Applied editor drag selection"
        );
    }
}

fn hit_test_editor_pointer(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) -> Option<nucleotide_editor::EditorHitTestResult> {
    let line_cache = cx.global::<LineLayoutCache>();
    {
        let core = core.read(cx);
        let editor = &core.editor;
        if let (Some(document), Some(view)) =
            (editor.document(doc_id), editor.tree.try_get(view_id))
        {
            hit_test_document_position(event, view.gutter_offset(document), line_cache, document)
        } else {
            debug!("Could not get document/view for coordinate transformation");
            None
        }
    }
}

fn primary_selection_anchor(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    cx: &mut App,
) -> Option<usize> {
    let core = core.read(cx);
    let editor = &core.editor;
    let document = editor.document(doc_id)?;
    Some(document.selection(view_id).primary().anchor)
}

fn apply_editor_selection(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    anchor: usize,
    head: usize,
    cx: &mut App,
) -> Option<usize> {
    let mut applied_head = None;
    core.update(cx, |core, cx| {
        let editor = &mut core.editor;
        if let Some(document) = editor.document_mut(doc_id) {
            let text_len = document.text().len_chars();
            let anchor = anchor.min(text_len);
            let head = head.min(text_len);
            let range = helix_core::Range::new(anchor, head);
            let selection = helix_core::Selection::new(helix_core::SmallVec::from([range]), 0);
            document.set_selection(view_id, selection);
            applied_head = Some(head);
            cx.notify();
        }
    });
    applied_head
}

pub struct DocumentView {
    core: Entity<Core>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    viewport: EditorViewport,
    scrollbar_state: ScrollbarState,
    line_height: Pixels,
    drag_anchor: Rc<Cell<Option<usize>>>,
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

        // Create custom scroll handle that wraps our viewport.
        let scroll_handle = DocumentScrollHandle::new(viewport.clone(), view_id);
        let scrollbar_state = ScrollbarState::new(scroll_handle);

        Self {
            core,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            viewport,
            scrollbar_state,
            line_height,
            drag_anchor: Rc::new(Cell::new(None)),
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
        // DocumentView render creates the DocumentElement for actual painting
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
        let surface_metrics = EditorSurfaceMetrics::new(metrics.line_height, metrics.cell_width);

        // Update viewport with document info
        {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(document) = editor.document(doc_id) {
                let total_lines = document.text().len_lines();
                self.viewport.set_content_visual_rows(total_lines);
                self.viewport.set_line_height(metrics.line_height);

                // Set a reasonable default viewport size if not already set
                // This will be updated with actual size in the paint method
                // Use a height that shows fewer lines than total to ensure scrollbar appears
                let viewport_height = metrics.line_height * 30.0; // Show 30 lines
                self.viewport
                    .set_viewport_size(size(px(800.0), viewport_height));

                // Don't recreate scrollbar state - it's already using our viewport

                debug!(
                    "Document has {} lines, viewport shows ~30 lines",
                    total_lines
                );
            }
        }

        // Create the DocumentElement that will handle the actual rendering
        // Pass the same viewport and scrollbar state to ensure state is shared
        let document_element = DocumentElement {
            core: self.core.clone(),
            doc_id,
            view_id: self.view_id,
            style: self.style.clone(),
            focus: self.focus.clone(),
            is_focused: self.is_focused,
            viewport: self.viewport.clone(),
            surface_metrics: surface_metrics.clone(),
        };

        let editor_surface = EditorSurface::new(
            cx.entity_id(),
            self.viewport.clone(),
            surface_metrics,
            document_element,
        )
        .on_scroll({
            let core = self.core.clone();
            let view_id = self.view_id;

            move |viewport, scroll_update, cx| {
                debug!(
                    crossed_lines = scroll_update.crossed_visual_rows,
                    top_visual_row = scroll_update.top_visual_row,
                    offset_within_row = %scroll_update.offset_within_row,
                    "Scroll wheel event handled by editor surface"
                );

                if scroll_update.crossed_visual_rows != 0 {
                    core.update(cx, |core, cx| {
                        if viewport.sync_to_helix_view(&mut core.editor, doc_id, view_id) {
                            cx.notify();
                        }
                    });
                }
            }
        })
        .on_mouse_down({
            let core = self.core.clone();
            let view_id = self.view_id;
            let drag_anchor = self.drag_anchor.clone();

            move |event: EditorSurfacePointerEvent, cx| {
                handle_editor_mouse_down(&core, doc_id, view_id, &drag_anchor, event, cx);
            }
        })
        .on_mouse_drag({
            let core = self.core.clone();
            let view_id = self.view_id;
            let drag_anchor = self.drag_anchor.clone();

            move |event: EditorSurfacePointerEvent, cx| {
                handle_editor_mouse_drag(&core, doc_id, view_id, &drag_anchor, event, cx);
            }
        })
        .on_mouse_up({
            let drag_anchor = self.drag_anchor.clone();

            move |event: EditorSurfacePointerEvent, _cx| {
                drag_anchor.set(None);
                debug!(position = ?event.position, "Mouse up event - click completed");
            }
        });

        // Create the scrollbar
        let scrollbar_opt = Scrollbar::vertical(self.scrollbar_state.clone());

        // Create the editor content with custom scrollbar
        let editor_content = div()
            .id("editor-content")
            .w_full()
            .h_full()
            .flex() // Horizontal flex layout
            .child(
                // Main editor area with DocumentElement
                div()
                    .id("editor-paint-area")
                    .w_full()
                    .h_full()
                    .flex_1()
                    .child(editor_surface),
            )
            .when_some(scrollbar_opt, gpui::ParentElement::child);

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

pub struct DocumentElement {
    core: Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    viewport: EditorViewport,
    surface_metrics: EditorSurfaceMetrics,
}

impl IntoElement for DocumentElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl DocumentElement {
    fn severity_color(theme: &Theme, sev: helix_core::diagnostic::Severity) -> Option<Hsla> {
        let key = match sev {
            helix_core::diagnostic::Severity::Error => "diagnostic.error",
            helix_core::diagnostic::Severity::Warning => "diagnostic.warning",
            helix_core::diagnostic::Severity::Info => "diagnostic.info",
            helix_core::diagnostic::Severity::Hint => "diagnostic.hint",
        };
        let style = theme.get(key);
        // Prefer underline color (used by diagnostics), fallback to fg if present
        style
            .underline_color
            .or(style.fg)
            .and_then(crate::utils::color_to_hsla)
    }
}

impl Element for DocumentElement {
    type RequestLayoutState = ();

    type PrepaintState = EditorLayout;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _before_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        debug!(bounds = ?bounds, size = ?bounds.size, "Editor bounds for prepaint");

        // Check if bounds are valid
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            debug!(
                "INVALID BOUNDS: width={}, height={}",
                bounds.size.width, bounds.size.height
            );
        }

        EditorTextMetrics::resolve(cx.text_system(), &self.style).layout_for_bounds(bounds)
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        after_layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Note: GPUI may call paint() multiple times per frame for the same element
        // This can cause visual accumulation of overlapping elements like tildes
        let _focus = self.focus.clone();
        let core = self.core.clone();
        let view_id = self.view_id;
        let cell_width = after_layout.cell_width;
        let line_height = after_layout.line_height;

        // Update scroll manager with current layout info
        self.viewport.set_line_height(line_height);
        self.surface_metrics.set(line_height, cell_width);

        // Set scroll manager viewport to the actual text-area height (exclude top padding)
        let text_area_height = bounds.size.height - px(1.0);
        let effective_viewport_size = size(bounds.size.width, text_area_height);
        self.viewport.set_viewport_size(effective_viewport_size);

        // Fill editor background from design tokens
        {
            let tokens = cx.theme().tokens;
            let bgc = tokens.editor.background;
            paint_editor_background(window, bounds, bgc);
        }

        // Sync scroll position back to Helix only if scrollbar changed it
        // This prevents overriding Helix's auto-scroll behavior
        if self.viewport.has_pending_scrollbar_sync() {
            core.update(cx, |core, cx| {
                if self
                    .viewport
                    .sync_to_helix_view(&mut core.editor, self.doc_id, view_id)
                {
                    cx.notify();
                }
            });
            // Clear the flag after syncing
            self.viewport.clear_pending_scrollbar_sync();
        }

        // Determine total content height in "visual" lines for correct scrolling
        // This ensures the scrollbar range matches the wrapped content height.
        let visual_total_lines = {
            let core = self.core.read(cx);
            let editor = &core.editor;
            let view = match editor.tree.try_get(view_id) {
                Some(v) => v,
                None => return,
            };
            let doc = match editor.document(self.doc_id) {
                Some(doc) => doc,
                None => return,
            };

            let theme = cx.global::<crate::ThemeManager>().helix_theme();
            EditorDocumentMetrics::resolve(
                doc,
                Some(theme),
                bounds,
                view.gutter_offset(doc),
                after_layout.cell_width,
                1,
            )
            .visual_rows
        };

        self.viewport.set_content_visual_rows(visual_total_lines);

        let gutter_width_cells = {
            let editor = &core.read(cx).editor;
            let view = match editor.tree.try_get(view_id) {
                Some(v) => v,
                None => return,
            };
            let doc = match editor.document(self.doc_id) {
                Some(doc) => doc,
                None => return, // Document was closed
            };

            // Mirror Helix viewport while preserving local sub-line wheel motion
            // when Helix reports the same top line.
            self.viewport.sync_from_helix_view(doc, self.view_id);

            view.gutter_offset(doc)
        };
        let _gutter_width_px = cell_width * f32::from(gutter_width_cells);

        // Check if soft wrap is enabled early for mouse handlers
        let soft_wrap_enabled = {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(document) = editor.document(self.doc_id) {
                if let Some(view) = editor.tree.try_get(self.view_id) {
                    let gutter_offset = view.gutter_offset(document);
                    let theme = cx.global::<crate::ThemeManager>().helix_theme();

                    let (_, text_format) = document_text_format_for_surface(
                        document,
                        Some(theme),
                        bounds,
                        gutter_offset,
                        after_layout.cell_width,
                        10,
                    );
                    text_format.soft_wrap
                } else {
                    false
                }
            } else {
                false
            }
        };

        // Store line layouts in element state for mouse interaction
        // Using LineLayoutCache instead of RefCell for thread safety
        // Get or create the LineLayoutCache
        let line_cache = if let Some(cache) = cx.try_global::<LineLayoutCache>() {
            cache.clone()
        } else {
            let cache = LineLayoutCache::new();
            cx.set_global(cache.clone());
            cache
        };
        line_cache.clear(); // Clear previous layouts

        let is_focused = self.is_focused;

        {
            let core = self.core.read(cx);
            let editor = &core.editor;

            let view = match editor.tree.try_get(self.view_id) {
                Some(v) => v,
                None => return,
            };
            let _viewport = view.area;
            // Check if cursorline is enabled and view is focused
            // Use the effective config value which includes runtime overrides
            let config_cursorline = editor.config().cursorline;
            let cursorline_enabled = config_cursorline && is_focused;
            debug!(
                "Cursorline check - config value: {}, focused: {}, enabled: {}",
                config_cursorline, is_focused, cursorline_enabled
            );

            // Get cursorline style
            let cursorline_style = if cursorline_enabled {
                let style = cx.theme_style("ui.cursorline.primary");
                debug!(
                    "Cursorline style found: bg={:?}, fg={:?}",
                    style.bg, style.fg
                );
                style.bg.and_then(color_to_hsla)
            } else {
                None
            };
            let tokens = cx.theme().tokens;
            let bg_color = tokens.editor.background;
            // Get mode-specific cursor theme like terminal version
            let mode = editor.mode();
            let cursor_style = cursor_style_for_mode(mode, |key| cx.theme_style(key));
            let fg_color = tokens.editor.text_primary;
            let default_text_style = helix_view::graphics::Style {
                fg: gpui_hsla_to_helix_color(tokens.editor.text_primary),
                bg: gpui_hsla_to_helix_color(tokens.editor.background),
                ..Default::default()
            };

            let document = match editor.document(self.doc_id) {
                Some(doc) => doc,
                None => return,
            };
            let text = document.text();

            let (_, cursor_kind) = editor.cursor();
            let primary_idx = document
                .selection(self.view_id)
                .primary()
                .cursor(text.slice(..));
            let gutter_width = view.gutter_offset(document);

            let line = text.char_to_line(primary_idx);
            let line_start = text.line_to_char(line);
            let col_in_line = primary_idx - line_start;
            debug!(
                "Cursor position - line: {line}, col_in_line: {col_in_line}, primary_idx: {primary_idx}, gutter_width: {gutter_width}"
            );
            let gutter_overflow = gutter_width == 0;
            if !gutter_overflow {
                debug!("need to render gutter {gutter_width}");
            }

            let total_lines = text.len_lines();

            // Use scroll manager to determine visible lines
            let (first_row, last_row_from_scroll) = self.viewport.visible_visual_range();
            let scroll_line_offset = self.viewport.offset_within_row();

            // Extract cursor position early to check for phantom line
            let cursor_char_idx = document
                .selection(self.view_id)
                .primary()
                .cursor(text.slice(..));
            let cursor_text = block_cursor_text(
                text.slice(..),
                cursor_char_idx,
                cursor_kind,
                self.is_focused,
            )
            .map(|char_str| {
                let text_color = cursor_foreground_color(
                    &cursor_style,
                    cursor_has_reversed_modifier(&cursor_style),
                    bg_color,
                );
                (char_str, text_color)
            });
            // Get the line number where the cursor is located
            let cursor_line_num = text.char_to_line(cursor_char_idx);
            debug!(
                "Cursor position: line={}, char_idx={}",
                cursor_line_num, cursor_char_idx
            );

            // Get all cursor lines for gutter highlighting (same as regular gutter implementation)
            let cursors: std::rc::Rc<[_]> = document
                .selection(self.view_id)
                .iter()
                .map(|range| range.cursor_line(text.slice(..)))
                .collect();

            let line_viewport = line_viewport_plan(
                text.slice(..),
                first_row,
                last_row_from_scroll,
                cursor_char_idx,
            );
            let last_row = line_viewport.last_row;
            let cursor_at_end = line_viewport.cursor_at_end;
            let file_ends_with_newline = line_viewport.file_ends_with_newline;
            let cursor_doc_line = cursor_document_line(
                text.slice(..),
                cursor_char_idx,
                cursor_at_end && file_ends_with_newline,
            );
            let cursor_viewport_pos =
                cursor_viewport_position(cursor_doc_line, first_row, last_row);

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
                    "Cursor at EOF with newline - cursor_line: {cursor_line}, last_row: {last_row}, total_lines: {total_lines}"
                );
            }

            // Render rulers before text
            const THEME_KEY_VIRTUAL_RULER: &str = "ui.virtual.ruler";
            let ruler_style = cx.theme_style(THEME_KEY_VIRTUAL_RULER);
            let ruler_color = ruler_style.bg.and_then(color_to_hsla).unwrap_or_else(|| {
                // Use UI theme's border color from tokens
                cx.ui_theme().tokens.chrome.border_default
            });

            // Get rulers configuration - try language-specific first, then fall back to editor config
            let editor_config = editor.config();
            let rulers = document
                .language_config()
                .and_then(|config| config.rulers.as_ref())
                .unwrap_or(&editor_config.rulers);

            // Get horizontal scroll offset from view
            let view_offset = document.view_offset(view.id);
            let ruler_geometry =
                EditorSurfaceGeometry::new(bounds, gutter_width, after_layout.cell_width);
            let ruler_plans = visible_ruler_paint_plans(
                ruler_geometry,
                rulers,
                view_offset.horizontal_offset,
                ruler_color,
            );
            paint_visible_rulers(window, &ruler_plans);

            // Extract necessary values before the loop to avoid borrowing issues
            let _editor_theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
            let editor_mode = editor.mode();
            let cursor_shape = editor.config().cursor_shape.clone();
            let syn_loader = editor.syn_loader.clone();

            // Clone text to avoid borrowing issues
            let doc_text = document.text().clone();

            // Extract cursor-related data before dropping core
            // cursor_char_idx was already extracted earlier for phantom line check
            let _tab_width = document.tab_width() as u16;

            // Shape cursor text before dropping core borrow and keep its length
            let (cursor_text_shape, cursor_text_style) = {
                let text_style_at_cursor = {
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    if let Some(doc) = editor.document(self.doc_id) {
                        let theme = cx.global::<crate::ThemeManager>().helix_theme();
                        let loader = editor.syn_loader.load();
                        text_style_at_position(doc, self.view_id, theme, &loader, cursor_char_idx)
                    } else {
                        helix_view::graphics::Style::default()
                    }
                };

                let (cursor_text, text_color) =
                    cursor_text.map_or((None, white()), |(text, color)| (Some(text), color));
                let cursor_text_shape = shape_cursor_text(
                    window.text_system().as_ref(),
                    cursor_text,
                    &self.style.font(),
                    self.style.font_size.to_pixels(px(16.0)),
                    &text_style_at_cursor,
                    text_color,
                    bg_color,
                );

                (cursor_text_shape, text_style_at_cursor)
            };

            // Drop the core borrow before the loop
            // core goes out of scope here

            let text = doc_text.slice(..);
            let diag_overlay_spans = diagnostic_overlay_spans(document, cx.helix_theme());

            // Update the shared line layouts for mouse interaction
            if soft_wrap_enabled {
                let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

                // Extract wrap indicator color early to avoid borrow conflicts later
                let wrap_indicator_color =
                    cx.theme_style("ui.virtual.wrap").fg.and_then(color_to_hsla);

                // Re-read core to get document and view - extract what we need and drop the borrow
                let (text_format, view_offset, gutter_offset) = {
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    let document = match editor.document(self.doc_id) {
                        Some(doc) => doc,
                        None => return,
                    };
                    let view = match editor.tree.try_get(self.view_id) {
                        Some(v) => v,
                        None => return,
                    };
                    let view_offset = document.view_offset(self.view_id);
                    let gutter_offset = view.gutter_offset(document);

                    let (_, text_format) = document_text_format_for_surface(
                        document,
                        Some(&theme),
                        bounds,
                        gutter_offset,
                        after_layout.cell_width,
                        10,
                    );
                    (text_format, view_offset, gutter_offset)
                };

                let soft_wrap_geometry =
                    EditorSurfaceGeometry::new(bounds, gutter_offset, after_layout.cell_width);
                let viewport_height =
                    soft_wrap_viewport_height(bounds, after_layout.line_height, scroll_line_offset);

                let soft_wrap_lines = soft_wrap_visual_lines(
                    text,
                    &text_format,
                    view_offset.anchor,
                    view_offset.vertical_offset,
                    viewport_height,
                );
                let soft_wrap_paint_plans = soft_wrap_line_paint_plans(
                    &soft_wrap_lines,
                    soft_wrap_geometry,
                    after_layout.line_height,
                    scroll_line_offset,
                    view_offset.vertical_offset,
                    cursor_line_num,
                );

                for soft_wrap_plan in soft_wrap_paint_plans {
                    let visual = soft_wrap_plan.visual;

                    let mut line_runs = if let (Some(line_start), Some(line_end)) =
                        (visual.line_start_char, visual.line_end_char)
                    {
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        let document = match editor.document(self.doc_id) {
                            Some(doc) => doc,
                            None => return,
                        };
                        let view = match editor.tree.try_get(self.view_id) {
                            Some(v) => v,
                            None => return,
                        };
                        let loader = syn_loader.load();
                        highlight_line(HighlightLineParams {
                            doc: document,
                            view,
                            theme: cx.helix_theme(),
                            syntax_loader: &loader,
                            editor_mode,
                            cursor_shape: &cursor_shape,
                            is_view_focused: self.is_focused,
                            line_start,
                            line_end,
                            fg_color,
                            font: self.style.font(),
                            default_text_style,
                            default_bg: bg_color,
                            diagnostic_overlay_spans: diag_overlay_spans.clone(),
                        })
                    } else {
                        Vec::new()
                    };

                    line_runs = decorate_soft_wrap_line_runs(
                        line_runs,
                        visual,
                        &self.style.font(),
                        fg_color,
                        wrap_indicator_color,
                    );

                    // Paint cursorline background before any run highlights so empty lines still render it
                    if soft_wrap_plan.is_cursor_visual_line
                        && let Some(cursorline_bg) = cursorline_style
                    {
                        paint_cursorline_background(
                            window,
                            soft_wrap_plan.cursorline_bounds,
                            cursorline_bg,
                        );
                    }

                    // Paint the line text (only for non-empty lines)
                    if !visual.text.is_empty() {
                        let shaped_line = line_cache.shape_line_cached(
                            window.text_system().as_ref(),
                            SharedString::from(visual.text.clone()),
                            self.style.font_size.to_pixels(px(16.0)),
                            bounds.size.width,
                            &line_runs,
                        );

                        if let Err(e) = paint_editor_line(
                            window,
                            cx,
                            &shaped_line,
                            &line_runs,
                            soft_wrap_plan.text_origin,
                            after_layout.line_height,
                            EditorLineBackgroundStyle {
                                only_selection_backgrounds: soft_wrap_plan.is_cursor_visual_line,
                                selection_primary: tokens.editor.selection_primary,
                                selection_secondary: tokens.editor.selection_secondary,
                            },
                        ) {
                            error!(error = ?e, "Failed to paint text");
                        }

                        if visual.is_phantom_line {
                            continue;
                        }

                        let layout = LineLayout::wrapped(
                            visual.doc_line,
                            shaped_line,
                            soft_wrap_plan.y_offset,
                            visual.segment_char_offset,
                            visual.text_start_byte_offset,
                        );
                        line_cache.push(layout);
                    }
                }

                // Render gutter for soft wrap mode from the same visual rows as text painting.
                {
                    let mut gutter_origin = bounds.origin;
                    gutter_origin.x += px(2.);
                    gutter_origin.y += px(1.);

                    // Build a map of line -> highest diagnostic severity for quick lookup
                    let diag_line_severity = {
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        if let Some(document) = editor.document(self.doc_id) {
                            diagnostic_severity_by_line(document)
                        } else {
                            std::collections::BTreeMap::new()
                        }
                    };

                    // Now render the line numbers with highlighting for current line
                    let gutter_style = cx.theme_style("ui.linenr");
                    let gutter_selected_style = cx.theme_style("ui.linenr.selected");

                    let gutter_plans = soft_wrap_gutter_line_plans(
                        &soft_wrap_lines,
                        view_offset.vertical_offset,
                        after_layout.line_height,
                        scroll_line_offset,
                        cursors.as_ref(),
                    );
                    let default_gutter_color = cx.ui_theme().tokens.editor.line_number;
                    let gutter_color = gutter_style
                        .fg
                        .and_then(crate::utils::color_to_hsla)
                        .unwrap_or(default_gutter_color);
                    let gutter_selected_color = gutter_selected_style
                        .fg
                        .and_then(crate::utils::color_to_hsla)
                        .unwrap_or(default_gutter_color);
                    let gutter_paint_plans = soft_wrap_gutter_line_paint_plans(
                        &gutter_plans,
                        gutter_origin,
                        gutter_color,
                        gutter_selected_color,
                    );
                    let gutter_lines = build_soft_wrap_gutter_lines(
                        window.text_system().clone(),
                        &self.style,
                        self.style.font_size.to_pixels(px(16.0)),
                        &gutter_paint_plans,
                    );
                    paint_soft_wrap_gutter_lines(
                        window,
                        cx,
                        &gutter_lines,
                        after_layout.line_height,
                        |_| {},
                    );
                    for gutter_line in gutter_lines {
                        // Paint a small diagnostic marker in the gutter if this line has diagnostics
                        if let Some(sev) = diag_line_severity.get(&gutter_line.doc_line).copied()
                            && let Some(color) = Self::severity_color(cx.helix_theme(), sev)
                        {
                            let marker_plan = diagnostic_marker_plan(
                                gutter_origin,
                                gutter_line.origin.y,
                                after_layout.line_height,
                                sev,
                            );
                            let gutter_bg = cx
                                .theme_style("ui.gutter")
                                .bg
                                .and_then(crate::utils::color_to_hsla);
                            let marker_style = diagnostic_marker_paint_style(
                                color,
                                cx.theme().tokens.chrome.text_on_chrome,
                                gutter_bg,
                            );
                            paint_diagnostic_marker(window, &marker_plan, marker_style);
                        }
                    }
                }

                // Render cursor for soft wrap mode
                let element_focused = self.focus.is_focused(window);
                if self.is_focused || element_focused {
                    // Get cursor position and text under cursor for block mode
                    let (
                        cursor_char_idx,
                        cursor_style,
                        cursor_kind,
                        cursor_text,
                        text_style_at_cursor,
                    ) = {
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        if let Some(document) = editor.document(self.doc_id) {
                            let selection = document.selection(self.view_id);
                            let cursor_char_idx = selection.primary().cursor(text);
                            let (_, cursor_kind) = editor.cursor();

                            let cursor_text = block_cursor_text(
                                text,
                                cursor_char_idx,
                                cursor_kind,
                                self.is_focused,
                            );

                            let mode = editor.mode();
                            let cursor_style =
                                cursor_style_for_mode(mode, |key| cx.theme_style(key));

                            // Get text style at cursor for reversed modifier
                            let loader = editor.syn_loader.load();
                            let text_style_at_cursor = text_style_at_position(
                                document,
                                self.view_id,
                                &theme,
                                &loader,
                                cursor_char_idx,
                            );

                            (
                                cursor_char_idx,
                                cursor_style,
                                cursor_kind,
                                cursor_text,
                                text_style_at_cursor,
                            )
                        } else {
                            return;
                        }
                    };

                    let cursor_visual_position = soft_wrap_visual_position(
                        text,
                        &text_format,
                        view_offset.anchor,
                        cursor_char_idx,
                    );

                    if let Some(cursor_position) = cursor_visual_position
                        && let Some(cursor_paint_position) = soft_wrap_cursor_paint_position(
                            EditorSurfaceGeometry::new(
                                bounds,
                                gutter_width,
                                after_layout.cell_width,
                            ),
                            after_layout.line_height,
                            after_layout.cell_width,
                            cursor_position,
                            view_offset.vertical_offset,
                            viewport_height,
                            view_offset.horizontal_offset,
                        )
                    {
                        let has_reversed = cursor_has_reversed_modifier(&cursor_style);
                        let cursor_color =
                            cursor_background_color(&cursor_style, &text_style_at_cursor, fg_color);
                        let cursor_text_color =
                            cursor_foreground_color(&cursor_style, has_reversed, bg_color);
                        let cursor_text_shape = shape_cursor_text(
                            window.text_system().as_ref(),
                            cursor_text,
                            &self.style.font(),
                            self.style.font_size.to_pixels(px(16.0)),
                            &text_style_at_cursor,
                            cursor_text_color,
                            bg_color,
                        );
                        let cursor_width = cursor_text_shape.width_or(after_layout.cell_width);

                        // Create and paint cursor
                        let mut cursor = EditorCursor {
                            origin: cursor_paint_position.cursor_origin,
                            kind: cursor_kind,
                            color: cursor_color,
                            block_width: cursor_width,
                            line_height: after_layout.line_height,
                            text: cursor_text_shape.into_shaped_line(),
                        };

                        // Store cursor position for overlay positioning
                        let cursor_point = cursor_paint_position.cursor_point();

                        // Update the global WorkspaceLayoutInfo with exact cursor coordinates
                        {
                            let layout_info =
                                cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
                            layout_info.cursor_position = Some(cursor_point);
                            layout_info.cursor_size = Some(gpui::Size {
                                width: cursor_width,
                                height: after_layout.line_height,
                            });
                        }

                        cursor.paint(cursor_paint_position.paint_origin, window, cx);
                    }
                }

                // Render tilde lines for empty viewport space (soft-wrap mode)
                // Calculate how many visual lines we've rendered vs viewport capacity
                let _visual_lines_rendered = soft_wrap_lines.len();
                let viewport_height_in_lines =
                    (bounds.size.height - px(2.0)) / after_layout.line_height;
                let _viewport_capacity = viewport_height_in_lines as usize;

                // Note: Tilde rendering is handled by the gutter for consistency with Helix
                // The gutter shows "~" for phantom lines in the line number area

                // Skip the regular rendering loop when soft wrap is enabled
                return;
            }

            let line_plans = unwrapped_visible_line_plans(
                text.slice(..),
                line_viewport,
                after_layout.line_height,
                scroll_line_offset,
            );
            let next_unwrapped_line_y_offset =
                line_plans.last().map_or(-scroll_line_offset, |line| {
                    line.y_offset + after_layout.line_height
                });
            let unwrapped_paint_plans = unwrapped_line_paint_plans(
                &line_plans,
                EditorSurfaceGeometry::new(bounds, gutter_width, after_layout.cell_width),
                after_layout.line_height,
                cursor_line_num,
            );

            // Original rendering loop (without soft wrap)
            for unwrapped_plan in unwrapped_paint_plans {
                let line_plan = unwrapped_plan.line;
                let line_idx = line_plan.line_idx;
                let line_start = line_plan.line_start;
                let line_end = line_plan.line_end;
                let y_offset = line_plan.y_offset;

                let (line_str, line_runs) = {
                    let line_slice = text.slice(line_start..line_end);
                    let line_str = shared_line_text_without_trailing_newline(line_slice);

                    let line_runs = {
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        let document = match editor.document(self.doc_id) {
                            Some(doc) => doc,
                            None => return,
                        };
                        let view = match editor.tree.try_get(self.view_id) {
                            Some(v) => v,
                            None => return,
                        };
                        let loader = syn_loader.load();
                        highlight_line(HighlightLineParams {
                            doc: document,
                            view,
                            theme: cx.helix_theme(),
                            syntax_loader: &loader,
                            editor_mode,
                            cursor_shape: &cursor_shape,
                            is_view_focused: self.is_focused,
                            line_start,
                            line_end,
                            fg_color,
                            font: self.style.font(),
                            default_text_style,
                            default_bg: bg_color,
                            diagnostic_overlay_spans: diag_overlay_spans.clone(),
                        })
                    };

                    (line_str, line_runs)
                };

                // Drop core before painting
                // core goes out of scope here

                let font_size_px = self.style.font_size.to_pixels(px(16.0));
                let text_origin = unwrapped_plan.text_origin;

                if unwrapped_plan.is_cursor_line
                    && let Some(cursorline_bg) = cursorline_style
                {
                    debug!(
                        "Painting cursorline for line {} (cursor at line {})",
                        line_idx, cursor_line_num
                    );
                    paint_cursorline_background(
                        window,
                        unwrapped_plan.cursorline_bounds,
                        cursorline_bg,
                    );
                }

                // Always create a shaped line, even for empty lines (needed for cursor positioning)
                let shaped_line = if !line_str.is_empty() {
                    let shaped = line_cache.shape_line_cached(
                        window.text_system().as_ref(),
                        line_str.clone(),
                        font_size_px,
                        bounds.size.width,
                        &line_runs,
                    );

                    if let Err(e) = paint_editor_line(
                        window,
                        cx,
                        &shaped,
                        &line_runs,
                        text_origin,
                        after_layout.line_height,
                        EditorLineBackgroundStyle {
                            only_selection_backgrounds: unwrapped_plan.is_cursor_line,
                            selection_primary: tokens.editor.selection_primary,
                            selection_secondary: tokens.editor.selection_secondary,
                        },
                    ) {
                        error!(error = ?e, "Failed to paint text");
                    }
                    shaped
                } else {
                    line_cache.shape_line_cached(
                        window.text_system().as_ref(),
                        "".into(),
                        font_size_px,
                        bounds.size.width,
                        &[],
                    )
                };

                let layout = LineLayout::unwrapped(line_idx, shaped_line, y_offset);

                // Debug: log line layout creation
                debug!(
                    "💾 LINE LAYOUT CACHED: line_idx={}, y_offset={:?}, is_phantom={}",
                    line_idx, y_offset, false
                );

                line_cache.push(layout);
            }

            // Note: Tilde rendering is handled by the gutter for consistency with Helix
            // The gutter shows "~" for phantom lines in the line number area

            // draw cursor
            let element_focused = self.focus.is_focused(window);
            debug!(
                "Cursor rendering check - is_focused: {}, element_focused: {}, cursor_viewport_pos: {:?}",
                self.is_focused, element_focused, cursor_viewport_pos
            );

            // Debug: Log cursor position info
            {
                let core = self.core.read(cx);
                let editor = &core.editor;
                if let Some(doc) = editor.document(self.doc_id)
                    && let Some(_view) = editor.tree.try_get(self.view_id)
                {
                    let sel = doc.selection(self.view_id);
                    let cursor_char = sel.primary().cursor(text);
                    debug!(
                        "Cursor char idx: {}, line: {}, selection: {:?}",
                        cursor_char,
                        text.char_to_line(cursor_char),
                        sel
                    );
                }
            }

            // Check both is_focused flag and actual focus state
            if self.is_focused || element_focused {
                if let Some(cursor_viewport_pos) = cursor_viewport_pos {
                    let viewport_row = cursor_viewport_pos.viewport_row;
                    let cursor_line = cursor_viewport_pos.line;

                    debug!(
                        "Looking for cursor line {cursor_line} in range {first_row}..{last_row}"
                    );

                    {
                        // Debug: line layouts are now stored in LineLayoutCache

                        // Use the cursor line directly as the layout index
                        let layout_line_idx = cursor_line;

                        debug!(
                            "Looking for line layout with index {} (cursor_line: {}, is phantom: {})",
                            layout_line_idx,
                            cursor_line,
                            cursor_at_end && file_ends_with_newline
                        );

                        // Find the line layout for the cursor line
                        if let Some(line_layout) = line_cache.find_line_by_index(layout_line_idx) {
                            debug!(
                                "Found line layout - line_idx: {}, origin.y: {:?}, expected line: {}",
                                line_layout.line_idx, line_layout.origin.y, layout_line_idx
                            );

                            let cursor_position = cursor_line_position(
                                text.slice(..),
                                cursor_line,
                                cursor_char_idx,
                                cursor_at_end && file_ends_with_newline,
                            );
                            let line_text = cursor_position.line_text;
                            let cursor_char_offset = cursor_position.cursor_char_offset;
                            let cursor_byte_offset = cursor_position.cursor_byte_offset;

                            let cursor_paint_position = unwrapped_cursor_paint_position(
                                EditorSurfaceGeometry::new(
                                    bounds,
                                    gutter_width,
                                    after_layout.cell_width,
                                ),
                                &line_layout,
                                cursor_byte_offset,
                            );
                            let cursor_x_relative_to_line = cursor_paint_position.cursor_origin.x;
                            let cursor_x = cursor_paint_position.cursor_point().x;

                            // Debug logging
                            debug!(
                                "Cursor rendering - line: {cursor_line}, char_offset: {cursor_char_offset}, byte_offset: {cursor_byte_offset}, x_relative: {cursor_x_relative_to_line:?}, x_absolute: {cursor_x:?}, viewport_row: {viewport_row}"
                            );

                            // Debug info about the line content
                            debug!(
                                "Line content: {:?}, cursor at char offset {} (byte offset {}), at_eof: {}",
                                &line_text,
                                cursor_char_offset,
                                cursor_byte_offset,
                                cursor_at_end && file_ends_with_newline
                            );

                            // Additional debug for emoji detection
                            if !line_text.is_empty() {
                                use unicode_segmentation::UnicodeSegmentation;
                                let chars: Vec<char> = line_text.chars().collect();
                                debug!(
                                    "Line has {} chars, {} bytes, {} graphemes",
                                    chars.len(),
                                    line_text.len(),
                                    line_text.graphemes(true).count()
                                );
                                if cursor_char_offset < chars.len() {
                                    let ch = chars[cursor_char_offset];
                                    debug!(
                                        "Char at cursor offset {}: {:?} (U+{:04X})",
                                        cursor_char_offset, ch, ch as u32
                                    );
                                }
                            }

                            let cursor_color = cursor_background_color(
                                &cursor_style,
                                &cursor_text_style,
                                fg_color,
                            );
                            let cursor_width = cursor_text_shape.width_or(after_layout.cell_width);

                            let mut cursor = EditorCursor {
                                origin: cursor_paint_position.cursor_origin,
                                kind: cursor_kind,
                                color: cursor_color,
                                block_width: cursor_width,
                                line_height: after_layout.line_height,
                                text: cursor_text_shape.clone().into_shaped_line(),
                            };

                            cursor.paint(cursor_paint_position.paint_origin, window, cx);
                        } else {
                            debug!(
                                "❌ CURSOR FAIL: Could not find line layout for cursor line {} (layout_line_idx={})",
                                cursor_line, layout_line_idx
                            );

                            // Special handling for EOF phantom line cursor
                            if cursor_at_end
                                && file_ends_with_newline
                                && cursor_char_idx >= text.len_chars()
                            {
                                let cursor_color = cursor_background_color(
                                    &cursor_style,
                                    &cursor_text_style,
                                    fg_color,
                                );
                                let cursor_width = after_layout.cell_width;
                                let cursor_paint_position = phantom_line_cursor_paint_position(
                                    EditorSurfaceGeometry::new(
                                        bounds,
                                        gutter_width,
                                        after_layout.cell_width,
                                    ),
                                    next_unwrapped_line_y_offset,
                                );

                                let mut cursor = EditorCursor {
                                    origin: cursor_paint_position.cursor_origin,
                                    kind: cursor_kind,
                                    color: cursor_color,
                                    block_width: cursor_width,
                                    line_height: after_layout.line_height,
                                    text: cursor_text_shape.into_shaped_line(),
                                };

                                // Store cursor position for overlay positioning
                                let cursor_point = cursor_paint_position.cursor_point();

                                // Update the global WorkspaceLayoutInfo with exact cursor coordinates
                                {
                                    let layout_info =
                                        cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
                                    layout_info.cursor_position = Some(cursor_point);
                                    layout_info.cursor_size = Some(gpui::Size {
                                        width: cursor_width,
                                        height: after_layout.line_height,
                                    });
                                }

                                cursor.paint(cursor_paint_position.paint_origin, window, cx);
                            } else {
                                debug!(
                                    "❌ CURSOR FAIL: Normal line layout missing for line {}",
                                    cursor_line
                                );
                            }
                        }
                    }
                } else {
                    debug!(
                        "Cursor line {cursor_doc_line} is outside rendered range {first_row}..{last_row}"
                    );
                }
            } else {
                debug!(
                    "Cursor rendering skipped - is_focused: {}, element_focused: {}",
                    self.is_focused, element_focused
                );
            }
            // draw gutter
            {
                let mut gutter_origin = bounds.origin;
                gutter_origin.x += px(2.);
                gutter_origin.y += px(1.) - scroll_line_offset;

                // Build gutter lines inside a limited borrow scope, then paint
                let gutter_lines = {
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    let view = match editor.tree.try_get(self.view_id) {
                        Some(v) => v,
                        None => return,
                    };
                    let document = match editor.document(self.doc_id) {
                        Some(doc) => doc,
                        None => return,
                    };
                    let theme = cx.global::<crate::ThemeManager>().helix_theme();

                    build_gutter_lines(GutterLineParams {
                        layout: after_layout,
                        text_system: window.text_system().clone(),
                        text_style: self.style.clone(),
                        origin: gutter_origin,
                        first_row,
                        last_row,
                        editor,
                        document,
                        view,
                        theme,
                        is_focused,
                    })
                };

                paint_gutter_lines(
                    window,
                    cx,
                    &gutter_lines,
                    after_layout.line_height,
                    |result| {
                        let Err(e) = result else {
                            return;
                        };
                        error!(error = ?e, "Failed to paint gutter line");
                    },
                );

                // Note: In non-soft-wrap mode, we rely on Helix's built-in sign gutters.
                // Custom diagnostic indicators (circle/triangle/square) are only drawn in soft-wrap mode.
            }
        }
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights
