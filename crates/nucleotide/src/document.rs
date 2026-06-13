use std::borrow::Cow;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, DefiniteLength, DismissEvent, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, ParentElement, Pixels, Point, Render, SharedString, Size, Style, Styled,
    TextStyle, Window, div, fill, px, relative, white,
};
use gpui::{TextRun, point, size};
use helix_core::{Uri, graphemes::next_grapheme_boundary, ropey::RopeSlice};
use helix_lsp::lsp::Diagnostic;
// Import helix's syntax highlighting system
use helix_view::{DocumentId, Theme, ViewId, graphics::CursorKind};
use nucleotide_logging::{debug, error};
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::Core;
use nucleotide_editor::{
    EditorCursor, EditorDocumentMetrics, EditorLayout, EditorLineBackgroundStyle, EditorSurface,
    EditorSurfaceGeometry, EditorSurfacePointerEvent, EditorTextMetrics, EditorViewport,
    GutterLineParams, HighlightLineParams, LineLayoutCache, build_gutter_lines,
    cursor_has_reversed_modifier, cursor_style_for_mode, cursor_text_run, diagnostic_overlay_spans,
    diagnostic_severity_by_line, document_text_format_for_surface, gpui_hsla_to_helix_color,
    highlight_line, hit_test_document_position, paint_line_backgrounds, soft_wrap_visual_lines,
    soft_wrap_visual_position, text_style_at_position,
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
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let line_cache = cx.global::<LineLayoutCache>();
    let hit_test = {
        let core = core.read(cx);
        let editor = &core.editor;
        if let (Some(document), Some(view)) =
            (editor.document(doc_id), editor.tree.try_get(view_id))
        {
            hit_test_document_position(event, view.gutter_offset(document), line_cache, document)
        } else {
            debug!("Could not get document/view for coordinate transformation");
            return;
        }
    };

    if let Some(hit_test) = hit_test {
        core.update(cx, |core, cx| {
            let editor = &mut core.editor;
            if let Some(document) = editor.document_mut(doc_id) {
                let target_pos = hit_test.char_idx.min(document.text().len_chars());
                let range = helix_core::Range::new(target_pos, target_pos);
                let selection = helix_core::Selection::new(helix_core::SmallVec::from([range]), 0);
                document.set_selection(view_id, selection);

                debug!(
                    line_idx = hit_test.line_idx,
                    char_offset = hit_test.char_offset,
                    target_pos,
                    "Applied editor click selection"
                );

                cx.notify();
            }
        });
    } else {
        debug!(
            window_pos = ?event.position,
            bounds = ?event.bounds,
            line_height = %event.line_height,
            "Click hit test did not find a rendered line"
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
    scrollbar_state: ScrollbarState,
    line_height: Pixels,
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

        let (cursor_pos, doc_id, first_row) = {
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
            let cursor_pos = view.screen_coords_at_pos(document, text.slice(..), primary_idx);

            let doc_view_offset = document.view_offset(self.view_id);
            let anchor = doc_view_offset.anchor;
            let first_row = text.char_to_line(anchor.min(text.len_chars()));
            (cursor_pos, doc_id, first_row)
        };
        let Some(cursor_pos) = cursor_pos else {
            return Vec::new();
        };

        let mut diags = Vec::new();
        if let Some(path) = editor.document(doc_id).and_then(|doc| doc.path()).cloned() {
            let uri = Uri::from(path);
            if let Some(diagnostics) = editor.diagnostics.get(&uri) {
                for (diag, _) in diagnostics.iter().filter(|(diag, _)| {
                    let (start_line, end_line) =
                        (diag.range.start.line as usize, diag.range.end.line as usize);
                    let row = cursor_pos.row + first_row;
                    start_line <= row && row <= end_line
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
        };

        let editor_surface = EditorSurface::new(
            self.viewport.clone(),
            metrics.line_height,
            metrics.cell_width,
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

            move |event: EditorSurfacePointerEvent, cx| {
                handle_editor_mouse_down(&core, doc_id, view_id, event, cx);
            }
        })
        .on_mouse_up(|event: EditorSurfacePointerEvent, _cx| {
            debug!(position = ?event.position, "Mouse up event - click completed");
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

struct RopeWrapper<'a>(RopeSlice<'a>);

impl<'a> From<RopeWrapper<'a>> for SharedString {
    fn from(val: RopeWrapper<'a>) -> Self {
        let cow: Cow<'_, str> = val.0.into();
        cow.to_string().into() // this is crazy
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

        // Set scroll manager viewport to the actual text-area height (exclude top padding)
        let text_area_height = bounds.size.height - px(1.0);
        let effective_viewport_size = size(bounds.size.width, text_area_height);
        self.viewport.set_viewport_size(effective_viewport_size);

        // TODO: Update shared cell_width for mouse handlers (requires structural change to pass between prepaint/paint)

        // Fill editor background from design tokens
        {
            let tokens = cx.theme().tokens;
            let bgc = tokens.editor.background;
            let _ = gpui::fill(bounds, bgc);
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

            // total_lines already updated above from document line count

            // Sync scroll position from Helix to ensure we reflect auto-scroll
            // This is important for keeping cursor visible during editing
            let view_offset = doc.view_offset(self.view_id);
            let text = doc.text();
            let anchor_line = text.char_to_line(view_offset.anchor);
            // Mirror Helix viewport: include vertical_offset (visual rows) for wrapped/non-wrapped
            let top_visual = anchor_line.saturating_add(view_offset.vertical_offset);
            // Preserve local sub-line wheel motion when Helix reports the same
            // top line, but snap to Helix when commands/cursor movement change it.
            self.viewport.sync_from_helix_top_visual_row(top_visual);

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

        // TODO: Add drag selection through EditorSurface once native pointer state is in place.

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
            let _bg = fill(bounds, bg_color);
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
            let cursor_pos = view.screen_coords_at_pos(document, text.slice(..), primary_idx);
            let gutter_width = view.gutter_offset(document);

            if let Some(pos) = cursor_pos {
                debug!(
                    "Cursor position - row: {}, col: {}, primary_idx: {}, gutter_width: {}",
                    pos.row, pos.col, primary_idx, gutter_width
                );

                // Additional debug: check what line and column we're actually at
                let line = text.char_to_line(primary_idx);
                let line_start = text.line_to_char(line);
                let col_in_line = primary_idx - line_start;
                debug!(
                    "Actual position - line: {line}, col_in_line: {col_in_line}, line_start: {line_start}"
                );
            } else {
                debug!(
                    "Warning: screen_coords_at_pos returned None for cursor position {primary_idx}"
                );
            }
            let gutter_overflow = gutter_width == 0;
            if !gutter_overflow {
                debug!("need to render gutter {gutter_width}");
            }

            let _cursor_row = cursor_pos.map(|p| p.row);
            let total_lines = text.len_lines();

            // Use scroll manager to determine visible lines
            let (first_row, last_row_from_scroll) = self.viewport.visible_visual_range();
            let scroll_line_offset = self.viewport.offset_within_row();

            // Get the character under the cursor for block cursor mode
            let cursor_text = if matches!(cursor_kind, CursorKind::Block) && self.is_focused {
                // Get the actual cursor position in the document
                let cursor_char_idx = document
                    .selection(self.view_id)
                    .primary()
                    .cursor(text.slice(..));

                if cursor_char_idx < text.len_chars() {
                    // Use grapheme boundary for proper character extraction
                    let grapheme_end = next_grapheme_boundary(text.slice(..), cursor_char_idx);
                    let char_slice = text.slice(cursor_char_idx..grapheme_end);
                    let char_str: SharedString = RopeWrapper(char_slice).into();

                    // Don't show visible characters for newlines in cursor
                    let char_str = if char_str == "\n" || char_str == "\r\n" || char_str == "\r" {
                        " ".into() // Use space for newlines so cursor is visible but no symbol
                    } else {
                        char_str
                    };

                    if !char_str.is_empty() {
                        // Check if cursor has reversed modifier
                        let has_reversed = cursor_has_reversed_modifier(&cursor_style);

                        // For block cursor, determine text color based on reversed state
                        let text_color = if has_reversed {
                            // For reversed cursor: text should use the document background for contrast
                            // since the cursor is now using the text's foreground color
                            bg_color
                        } else if let Some(fg) = cursor_style.fg {
                            // Normal cursor with explicit foreground
                            color_to_hsla(fg).unwrap_or(white())
                        } else {
                            // No cursor.fg defined, use white as default text color
                            white()
                        };

                        let _run = TextRun {
                            len: char_str.len(),
                            font: self.style.font(),
                            color: text_color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };

                        Some((char_str, text_color))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            // Extract cursor position early to check for phantom line
            let cursor_char_idx = document
                .selection(self.view_id)
                .primary()
                .cursor(text.slice(..));
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

            // Use the last row from scroll manager
            let mut last_row = last_row_from_scroll;

            // Check if cursor is at the very end of the file (phantom line)
            let cursor_at_end = cursor_char_idx == text.len_chars();
            let file_ends_with_newline =
                text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n';

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

            // If cursor is at end of file with trailing newline, we need to render the phantom line
            // For a file ending with \n, Rope counts the empty line after it, so we don't need to add 1
            if cursor_at_end && file_ends_with_newline {
                let cursor_line = text.char_to_line(cursor_char_idx.saturating_sub(1));
                debug!(
                    "Cursor at EOF with newline - cursor_line: {cursor_line}, last_row before: {last_row}, total_lines: {total_lines}"
                );

                // Ensure last_row includes the phantom line (which is at index total_lines - 1)
                last_row = last_row.max(total_lines);
                debug!("last_row after adjustment: {last_row}");
            }

            // println!("first row is {first_row} last row is {last_row}");
            // When rendering phantom line, end_char should be beyond the document end
            let end_char = if last_row > total_lines {
                text.len_chars() + 1 // Allow phantom line to be rendered
            } else {
                text.line_to_char(std::cmp::min(last_row, total_lines))
            };

            // Render text line by line to avoid newline issues
            let mut y_offset = -scroll_line_offset;
            // COORDINATE SYSTEM ANALYSIS: The original version stored in GLOBAL coordinates
            // but current version converts to LOCAL coordinates before storage
            // The px(2.) was part of the global calculation, but since we now convert to local,
            // we need to match the soft-wrap calculation which doesn't include px(2.)
            let text_origin_x =
                EditorSurfaceGeometry::new(bounds, gutter_width, after_layout.cell_width)
                    .text_origin_x();

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
            let horizontal_offset = view_offset.horizontal_offset as f32;

            // Render each ruler as a vertical line
            for &ruler_col in rulers {
                // Calculate x position based on column (account for horizontal scroll)
                // Rulers are at absolute column positions in the text, not including the gutter
                // We need to account for 0-based vs 1-based column indexing
                let ruler_x = text_origin_x
                    + (after_layout.cell_width * (f32::from(ruler_col - 1) - horizontal_offset));

                // Only render if the ruler is within our visible bounds
                if ruler_x >= text_origin_x && ruler_x < bounds.origin.x + bounds.size.width {
                    let ruler_bounds = Bounds {
                        origin: point(ruler_x, bounds.origin.y),
                        size: size(px(1.0), bounds.size.height),
                    };
                    window.paint_quad(fill(ruler_bounds, ruler_color));
                }
            }

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
            let (cursor_text_shaped, cursor_text_len) = cursor_text
                .map(|(char_str, text_color)| {
                    let text_len = char_str.len();
                    // Derive the original text style at the cursor to preserve italics/bold/underline
                    let text_style_at_cursor = {
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        if let Some(doc) = editor.document(self.doc_id) {
                            let theme = cx.global::<crate::ThemeManager>().helix_theme();
                            let loader = editor.syn_loader.load();
                            text_style_at_position(
                                doc,
                                self.view_id,
                                theme,
                                &loader,
                                cursor_char_idx,
                            )
                        } else {
                            helix_view::graphics::Style::default()
                        }
                    };

                    let run = cursor_text_run(
                        &self.style.font(),
                        text_len,
                        &text_style_at_cursor,
                        text_color,
                        bg_color,
                    );

                    let shaped = window.text_system().shape_line(
                        char_str,
                        self.style.font_size.to_pixels(px(16.0)),
                        &[run],
                        None,
                    );
                    (Some(shaped), text_len)
                })
                .unwrap_or((None, 0));

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

                let text_origin_x =
                    EditorSurfaceGeometry::new(bounds, gutter_offset, after_layout.cell_width)
                        .text_origin_x();
                // Account for padding in viewport height calculation - match EditorViewport exactly
                let effective_height = bounds.size.height - px(2.0); // Account for padding
                let calculated_height = (effective_height / after_layout.line_height) as usize;
                // Render one extra row when the top row is partially scrolled,
                // matching GPUI's pixel-scroll behavior at the bottom edge.
                let viewport_height =
                    calculated_height + usize::from(f32::from(scroll_line_offset) > 0.0);

                let soft_wrap_lines = soft_wrap_visual_lines(
                    text,
                    &text_format,
                    view_offset.anchor,
                    view_offset.vertical_offset,
                    viewport_height,
                );

                for visual in &soft_wrap_lines {
                    let y_offset = -scroll_line_offset
                        + after_layout.line_height
                            * visual.relative_row(view_offset.vertical_offset) as f32;
                    let line_y = bounds.origin.y + px(1.0) + y_offset;

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

                    // Adjust text runs to account for leading spaces and wrap indicator
                    if !line_runs.is_empty() {
                        // Handle indentation spaces separately from wrap indicators
                        if visual.line_start_col > 0 {
                            // Add run for indentation spaces using normal text color
                            let indent_run = TextRun {
                                len: visual.line_start_col,
                                font: self.style.font(),
                                color: fg_color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            line_runs.insert(0, indent_run);
                        }

                        if visual.wrap_indicator_len > 0 {
                            // Use pre-extracted wrap indicator color
                            let wrap_color = wrap_indicator_color.unwrap_or(fg_color); // Fallback to normal text color
                            // Add run for wrap indicator using ui.virtual.wrap theme color
                            let wrap_run = TextRun {
                                len: visual.wrap_indicator_len,
                                font: self.style.font(),
                                color: wrap_color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            line_runs
                                .insert(if visual.line_start_col > 0 { 1 } else { 0 }, wrap_run);
                        }
                    }

                    // Determine whether this visual line corresponds to the cursor's document line
                    let is_cursor_visual_line = visual.doc_line == cursor_line_num;

                    // Paint cursorline background before any run highlights so empty lines still render it
                    if is_cursor_visual_line && let Some(cursorline_bg) = cursorline_style {
                        let cursorline_bounds = Bounds {
                            origin: point(bounds.origin.x, line_y),
                            size: size(bounds.size.width, after_layout.line_height),
                        };
                        window.paint_quad(fill(cursorline_bounds, cursorline_bg));
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

                        paint_line_backgrounds(
                            window,
                            &shaped_line,
                            &line_runs,
                            point(text_origin_x, line_y),
                            after_layout.line_height,
                            EditorLineBackgroundStyle {
                                only_selection_backgrounds: is_cursor_visual_line,
                                selection_primary: tokens.editor.selection_primary,
                                selection_secondary: tokens.editor.selection_secondary,
                            },
                        );

                        if let Err(e) = shaped_line.paint(
                            point(text_origin_x, line_y),
                            after_layout.line_height,
                            window,
                            cx,
                        ) {
                            error!(error = ?e, "Failed to paint text");
                        }

                        if visual.is_phantom_line {
                            continue;
                        }

                        // Store line layout for mouse interaction
                        // FIXED: Store in text-area coordinates (gutter excluded)
                        // Use y_offset directly to match coordinate system used by mouse handler
                        let text_area_origin = point(
                            px(0.0),  // Line starts at x=0 in text-area coordinates
                            y_offset, // Use y_offset directly (no px(1.) like non-wrap mode)
                        );
                        let layout = nucleotide_editor::LineLayout {
                            line_idx: visual.doc_line,
                            shaped_line,
                            origin: text_area_origin,
                            segment_char_offset: visual.segment_char_offset,
                            text_start_byte_offset: visual.text_start_byte_offset,
                        };
                        line_cache.push(layout);
                    }
                }

                // Render gutter for soft wrap mode from the same visual rows as text painting.
                {
                    let mut gutter_origin = bounds.origin;
                    gutter_origin.x += px(2.);
                    gutter_origin.y += px(1.);

                    let mut doc_line_positions = Vec::new();
                    let mut last_doc_line = None;
                    for visual in &soft_wrap_lines {
                        if last_doc_line != Some(visual.doc_line) {
                            let y_pos = -scroll_line_offset
                                + after_layout.line_height
                                    * visual.relative_row(view_offset.vertical_offset) as f32;
                            doc_line_positions.push((
                                visual.doc_line,
                                visual.is_phantom_line,
                                y_pos,
                            ));
                            last_doc_line = Some(visual.doc_line);
                        }
                    }

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

                    for (doc_line, is_phantom_line, y_pos) in doc_line_positions {
                        let line_num_str = if is_phantom_line {
                            "   ~ ".to_string() // Match the format: right-aligned with space
                        } else {
                            format!("{:>4} ", doc_line + 1)
                        };
                        let y = gutter_origin.y + y_pos;

                        // Choose color based on whether this line contains a cursor (same logic as regular gutter)
                        // Use UI theme tokens for gutter fallback color
                        let default_gutter_color = cx.ui_theme().tokens.editor.line_number;
                        let selected = cursors.contains(&doc_line);

                        let gutter_color = gutter_style
                            .fg
                            .and_then(crate::utils::color_to_hsla)
                            .unwrap_or(default_gutter_color);
                        let gutter_selected_color = gutter_selected_style
                            .fg
                            .and_then(crate::utils::color_to_hsla)
                            .unwrap_or(default_gutter_color);

                        let line_color = if is_phantom_line {
                            // Phantom lines (tildes) always use regular gutter color, never selected
                            gutter_color
                        } else if selected {
                            // Current line - use selected gutter style
                            gutter_selected_color
                        } else {
                            // Other lines - use regular gutter style
                            gutter_color
                        };

                        let run = TextRun {
                            len: line_num_str.len(),
                            font: self.style.font(),
                            color: line_color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };

                        let shaped = window.text_system().shape_line(
                            line_num_str.into(),
                            self.style.font_size.to_pixels(px(16.0)),
                            &[run],
                            None,
                        );

                        let _ = shaped.paint(
                            point(gutter_origin.x, y),
                            after_layout.line_height,
                            window,
                            cx,
                        );

                        // Paint a small diagnostic marker in the gutter if this line has diagnostics
                        if let Some(sev) = diag_line_severity.get(&doc_line).copied()
                            && let Some(color) = Self::severity_color(cx.helix_theme(), sev)
                        {
                            use nucleotide_ui::tokens::utils;
                            // Indicator size: 60% of line height
                            let marker_size = (after_layout.line_height * 0.6).max(px(2.0));
                            // Place marker to the left of the line number
                            let marker_x = gutter_origin.x + px(2.0);
                            // Vertically centered
                            let marker_y = y + (after_layout.line_height - marker_size) * 0.5;
                            // Paint a background strip to hide built-in sign indicators
                            let strip_width = marker_size + px(4.0);
                            let strip_bounds = Bounds {
                                origin: point(gutter_origin.x, y),
                                size: size(strip_width, after_layout.line_height),
                            };
                            if let Some(gutter_bg) = cx
                                .theme_style("ui.gutter")
                                .bg
                                .and_then(crate::utils::color_to_hsla)
                            {
                                window.paint_quad(fill(strip_bounds, gutter_bg));
                            }

                            // Derived colors with slight transparency + subtle border for 3D effect
                            let base_fill = utils::with_alpha(color, 0.85);
                            let border_col = utils::with_alpha(utils::darken(color, 0.15), 0.9);

                            // Draw shape by severity: Info/Hint=Sphere, Warning=Triangle, Error=Square
                            match sev {
                                helix_core::diagnostic::Severity::Error => {
                                    let marker_bounds = Bounds {
                                        origin: point(marker_x, marker_y),
                                        size: size(marker_size, marker_size),
                                    };
                                    // Slightly rounded square with border and glossy dot
                                    window.paint_quad(gpui::quad(
                                        marker_bounds,
                                        px(1.0),
                                        base_fill,
                                        px(1.0),
                                        border_col,
                                        gpui::BorderStyle::default(),
                                    ));

                                    // Small top-left highlight
                                    let h_size = marker_size * 0.22;
                                    let h_bounds = Bounds {
                                        origin: point(
                                            marker_x + marker_size * 0.18,
                                            marker_y + marker_size * 0.18,
                                        ),
                                        size: size(h_size, h_size),
                                    };
                                    let h_color = utils::with_alpha(
                                        cx.theme().tokens.chrome.text_on_chrome,
                                        0.18,
                                    );
                                    window.paint_quad(gpui::quad(
                                        h_bounds,
                                        h_size * 0.5,
                                        h_color,
                                        0.0,
                                        gpui::transparent_black(),
                                        gpui::BorderStyle::default(),
                                    ));
                                }
                                helix_core::diagnostic::Severity::Warning => {
                                    // Upright triangle inside marker square
                                    let top = point(marker_x + marker_size * 0.5, marker_y);
                                    let bl = point(marker_x, marker_y + marker_size);
                                    let br = point(marker_x + marker_size, marker_y + marker_size);
                                    let mut pb = gpui::PathBuilder::fill();
                                    pb.move_to(top);
                                    pb.line_to(bl);
                                    pb.line_to(br);
                                    pb.close();
                                    if let Ok(path) = pb.build() {
                                        window.paint_path(path, base_fill);
                                    }

                                    // Small internal highlight near top-left edge
                                    let h_size = marker_size * 0.2;
                                    let h_bounds = Bounds {
                                        origin: point(
                                            marker_x + marker_size * 0.22,
                                            marker_y + marker_size * 0.18,
                                        ),
                                        size: size(h_size, h_size),
                                    };
                                    let h_color = utils::with_alpha(
                                        cx.theme().tokens.chrome.text_on_chrome,
                                        0.14,
                                    );
                                    window.paint_quad(gpui::quad(
                                        h_bounds,
                                        h_size * 0.5,
                                        h_color,
                                        0.0,
                                        gpui::transparent_black(),
                                        gpui::BorderStyle::default(),
                                    ));
                                }
                                helix_core::diagnostic::Severity::Info
                                | helix_core::diagnostic::Severity::Hint => {
                                    let marker_bounds = Bounds {
                                        origin: point(marker_x, marker_y),
                                        size: size(marker_size, marker_size),
                                    };
                                    // Sphere base with border
                                    let radius = marker_size * 0.5;
                                    window.paint_quad(gpui::quad(
                                        marker_bounds,
                                        radius,
                                        base_fill,
                                        px(1.0),
                                        border_col,
                                        gpui::BorderStyle::default(),
                                    ));

                                    // Specular highlights fully inside the circle bounds
                                    let offset = marker_size * 0.14;
                                    let halo_size = marker_size * 0.52;
                                    let core_size = marker_size * 0.26;
                                    let halo_bounds = Bounds {
                                        origin: point(marker_x + offset, marker_y + offset),
                                        size: size(halo_size, halo_size),
                                    };
                                    let core_bounds = Bounds {
                                        origin: point(
                                            marker_x + offset + (halo_size - core_size) * 0.25,
                                            marker_y + offset + (halo_size - core_size) * 0.25,
                                        ),
                                        size: size(core_size, core_size),
                                    };
                                    let highlight_halo = utils::with_alpha(
                                        cx.theme().tokens.chrome.text_on_chrome,
                                        0.14,
                                    );
                                    let highlight_core = utils::with_alpha(
                                        cx.theme().tokens.chrome.text_on_chrome,
                                        0.45,
                                    );
                                    window.paint_quad(gpui::quad(
                                        halo_bounds,
                                        halo_size * 0.5,
                                        highlight_halo,
                                        0.0,
                                        gpui::transparent_black(),
                                        gpui::BorderStyle::default(),
                                    ));
                                    window.paint_quad(gpui::quad(
                                        core_bounds,
                                        core_size * 0.5,
                                        highlight_core,
                                        0.0,
                                        gpui::transparent_black(),
                                        gpui::BorderStyle::default(),
                                    ));
                                }
                            }
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

                            // Get the character under the cursor for block cursor mode
                            let cursor_text =
                                if matches!(cursor_kind, CursorKind::Block) && self.is_focused {
                                    if cursor_char_idx < text.len_chars() {
                                        let grapheme_end =
                                            next_grapheme_boundary(text, cursor_char_idx);
                                        let char_slice = text.slice(cursor_char_idx..grapheme_end);
                                        let char_str: SharedString = RopeWrapper(char_slice).into();

                                        // Don't show visible characters for newlines in cursor
                                        let char_str = if char_str == "\n"
                                            || char_str == "\r\n"
                                            || char_str == "\r"
                                        {
                                            " ".into() // Use space for newlines so cursor is visible but no symbol
                                        } else {
                                            char_str
                                        };

                                        Some(char_str)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };

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

                    // If cursor is in viewport, render it
                    if let Some(cursor_position) = cursor_visual_position
                        && cursor_position.visual_line >= view_offset.vertical_offset
                        && cursor_position.visual_line
                            < view_offset.vertical_offset + viewport_height
                    {
                        // Do not auto-scroll here; rely on Helix ensure_cursor_in_view.
                        // Calculate cursor position - FIXED: Use text_bounds coordinate system to match mouse clicks
                        // Get text bounds (excluding gutter) to match mouse coordinate system
                        // Use existing gutter_width from outer scope instead of calling view.gutter_offset(document)
                        let text_bounds = EditorSurfaceGeometry::new(
                            bounds,
                            gutter_width,
                            after_layout.cell_width,
                        )
                        .text_bounds();

                        let relative_line =
                            cursor_position.visual_line - view_offset.vertical_offset;
                        let cursor_y = text_bounds.origin.y
                            + (after_layout.line_height * relative_line as f32);
                        // Account for horizontal scrolling when calculating cursor X position
                        let visual_col_in_viewport = cursor_position.visual_col as f32
                            - view_offset.horizontal_offset as f32;
                        let cursor_x = text_bounds.origin.x
                            + (after_layout.cell_width * visual_col_in_viewport);

                        // Check if cursor has reversed modifier
                        let has_reversed = cursor_has_reversed_modifier(&cursor_style);

                        let cursor_color = if has_reversed {
                            // For reversed cursor: use the text color at cursor position as cursor background
                            // Use the pre-calculated text style
                            text_style_at_cursor
                                .fg
                                .and_then(color_to_hsla)
                                .unwrap_or(fg_color)
                        } else {
                            // Normal cursor: use cursor's background color
                            cursor_style
                                .bg
                                .and_then(color_to_hsla)
                                .or_else(|| cursor_style.fg.and_then(color_to_hsla))
                                .unwrap_or(fg_color)
                        };

                        // Shape cursor text if available and calculate its width
                        let (cursor_text_shaped, cursor_text_len) = cursor_text
                            .map(|char_str| {
                                let text_len = char_str.len();
                                // For block cursor, text should contrast with cursor background
                                let text_color = if has_reversed {
                                    bg_color
                                } else if let Some(fg) = cursor_style.fg {
                                    color_to_hsla(fg).unwrap_or(white())
                                } else {
                                    white()
                                };

                                let run = cursor_text_run(
                                    &self.style.font(),
                                    text_len,
                                    &text_style_at_cursor,
                                    text_color,
                                    bg_color,
                                );

                                let shaped = window.text_system().shape_line(
                                    char_str,
                                    self.style.font_size.to_pixels(px(16.0)),
                                    &[run],
                                    None,
                                );
                                (Some(shaped), text_len)
                            })
                            .unwrap_or((None, 0));

                        // Calculate cursor width based on the actual character width
                        let cursor_width = if let Some(ref shaped_text) = cursor_text_shaped {
                            // Get the width of the shaped text by measuring to the end
                            // x_for_index gives us the x position at the end of the text
                            shaped_text.x_for_index(cursor_text_len)
                        } else {
                            // Default to cell width for empty cursor
                            after_layout.cell_width
                        };

                        // Create and paint cursor
                        let mut cursor = EditorCursor {
                            origin: point(px(0.0), px(0.0)), // No offset needed, will be applied in paint
                            kind: cursor_kind,
                            color: cursor_color,
                            block_width: cursor_width,
                            line_height: after_layout.line_height,
                            text: cursor_text_shaped,
                        };

                        // Store cursor position for overlay positioning
                        let cursor_point = point(cursor_x, cursor_y);

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

                        cursor.paint(cursor_point, window, cx);
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

            // Original rendering loop (without soft wrap)
            for line_idx in first_row..last_row {
                // Handle phantom line (empty line at EOF when file ends with newline)
                // For a file ending with \n, the last line is empty and is the phantom line
                // Also treat any empty line at the end as phantom line
                let line_start_char = if line_idx < total_lines {
                    text.line_to_char(line_idx)
                } else {
                    text.len_chars()
                };
                let line_end_char = if line_idx + 1 < total_lines {
                    text.line_to_char(line_idx + 1).saturating_sub(1)
                } else {
                    text.len_chars()
                };
                let line_is_empty = line_start_char >= line_end_char;
                let is_phantom_line =
                    (cursor_at_end && file_ends_with_newline && line_idx == total_lines - 1)
                        || (line_idx >= total_lines)
                        || (line_idx == total_lines - 1 && line_is_empty && total_lines > 1);

                // Skip phantom lines entirely - they shouldn't take up visual space
                if is_phantom_line {
                    debug!(
                        "Skipping phantom line layout creation for line_idx={}",
                        line_idx
                    );
                    continue;
                }

                let (line_start, line_end) = {
                    // Normal line - get actual line boundaries
                    let line_start = text.line_to_char(line_idx);
                    let line_end = if line_idx + 1 < total_lines {
                        text.line_to_char(line_idx + 1).saturating_sub(1) // Exclude newline
                    } else {
                        text.len_chars()
                    };
                    (line_start, line_end)
                };

                // Skip lines outside our view
                let anchor_char = text.line_to_char(first_row);
                if line_start >= end_char || line_end < anchor_char {
                    y_offset += after_layout.line_height;
                    continue;
                }

                // Adjust line bounds to our view
                let line_start = line_start.max(anchor_char);
                let line_end = line_end.min(end_char);

                // For empty lines, line_start may equal line_end, which is valid
                // We still need to render them for cursor positioning
                if line_start > line_end {
                    y_offset += after_layout.line_height;
                    continue;
                }

                let (line_str, line_runs) = {
                    let line_slice = text.slice(line_start..line_end);
                    // Convert to string and remove any trailing newlines
                    let line_str = {
                        let cow: Cow<'_, str> = line_slice.into();
                        let mut s = cow.into_owned();
                        // Remove any trailing newline characters to prevent GPUI panic
                        while s.ends_with('\n') || s.ends_with('\r') {
                            s.pop();
                        }
                        SharedString::from(s)
                    };

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
                let text_origin = point(text_origin_x, bounds.origin.y + px(1.) + y_offset);
                // Defer painting of cursorline until after per-run backgrounds are drawn

                // Always create a shaped line, even for empty lines (needed for cursor positioning)
                let shaped_line = if !line_str.is_empty() {
                    let shaped = line_cache.shape_line_cached(
                        window.text_system().as_ref(),
                        line_str.clone(),
                        font_size_px,
                        bounds.size.width,
                        &line_runs,
                    );

                    // Paint cursorline background BEFORE run backgrounds so selections render on top
                    if line_idx == cursor_line_num
                        && let Some(cursorline_bg) = cursorline_style
                    {
                        debug!(
                            "Painting cursorline for line {} (cursor at line {})",
                            line_idx, cursor_line_num
                        );
                        let cursorline_bounds = Bounds {
                            origin: point(bounds.origin.x, bounds.origin.y + px(1.) + y_offset),
                            size: size(bounds.size.width, after_layout.line_height),
                        };
                        window.paint_quad(fill(cursorline_bounds, cursorline_bg));
                    }

                    paint_line_backgrounds(
                        window,
                        &shaped,
                        &line_runs,
                        text_origin,
                        after_layout.line_height,
                        EditorLineBackgroundStyle {
                            only_selection_backgrounds: line_idx == cursor_line_num,
                            selection_primary: tokens.editor.selection_primary,
                            selection_secondary: tokens.editor.selection_secondary,
                        },
                    );

                    if let Err(e) = shaped.paint(text_origin, after_layout.line_height, window, cx)
                    {
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

                // COORDINATE SYSTEM CONVERSION FOR LINE CACHE:
                // Convert from global coordinates to element-local coordinates
                //
                // COORDINATE SPACES EXPLAINED:
                // - text_origin: Global screen coordinates where the line is painted
                // - bounds.origin: Global screen coordinates of the DocumentElement's top-left
                // - local_origin: Element-local coordinates (relative to DocumentElement)
                //
                // This conversion ensures that:
                // 1. Line cache stores positions in the same coordinate space as mouse events
                // 2. Mouse position lookup works without additional coordinate conversion
                // FIXED: Store in text-area coordinates (gutter excluded)
                // Remove the px(1.) top padding to avoid double-adding it during cursor positioning
                let text_area_origin = point(
                    px(0.0),  // Line starts at x=0 in text-area coordinates
                    y_offset, // Use y_offset directly (without the px(1.) padding)
                );
                let layout = nucleotide_editor::LineLayout {
                    line_idx,
                    shaped_line,
                    origin: text_area_origin,
                    segment_char_offset: 0, // Non-wrapped lines always start at beginning
                    text_start_byte_offset: 0, // No wrap indicators in non-wrapped lines
                };

                // Debug: log line layout creation
                debug!(
                    "💾 LINE LAYOUT CACHED: line_idx={}, y_offset={:?}, is_phantom={}",
                    line_idx, y_offset, false
                );

                line_cache.push(layout);

                y_offset += after_layout.line_height;
            }

            // Render tilde lines for empty viewport space (like Helix/Vim)
            // Calculate how many lines we've rendered vs viewport capacity
            // Since we skip phantom lines, we need to count actual rendered lines, not just last_row - first_row
            // Count lines by iterating through the range and checking which ones weren't skipped
            let mut _actual_lines_rendered = 0;
            for line_idx in first_row..last_row {
                let line_start_char = if line_idx < total_lines {
                    text.line_to_char(line_idx)
                } else {
                    text.len_chars()
                };
                let line_end_char = if line_idx + 1 < total_lines {
                    text.line_to_char(line_idx + 1).saturating_sub(1)
                } else {
                    text.len_chars()
                };
                let line_is_empty = line_start_char >= line_end_char;
                let is_phantom_line =
                    (cursor_at_end && file_ends_with_newline && line_idx == total_lines - 1)
                        || (line_idx >= total_lines)
                        || (line_idx == total_lines - 1 && line_is_empty && total_lines > 1);
                if !is_phantom_line {
                    _actual_lines_rendered += 1;
                }
            }
            let viewport_height_in_lines =
                (bounds.size.height - px(2.0)) / after_layout.line_height;
            let _viewport_capacity = viewport_height_in_lines as usize;

            // Note: Tilde rendering is handled by the gutter for consistency with Helix
            // The gutter shows "~" for phantom lines in the line number area

            // draw cursor
            let element_focused = self.focus.is_focused(window);
            debug!(
                "Cursor rendering check - is_focused: {}, element_focused: {}, cursor_pos: {:?}",
                self.is_focused, element_focused, cursor_pos
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
                // Try to render cursor even if screen_coords_at_pos failed
                let position = cursor_pos.or_else(|| {
                    // Fallback: calculate position manually if screen_coords_at_pos failed
                    let cursor_line = text.char_to_line(cursor_char_idx);

                    // Use the cursor line directly
                    let effective_cursor_line = cursor_line;

                    if effective_cursor_line >= first_row && effective_cursor_line < last_row {
                        let viewport_row = effective_cursor_line.saturating_sub(first_row);
                        Some(helix_core::Position {
                            row: viewport_row,
                            col: 0, // We'll calculate actual column from shaped line
                        })
                    } else {
                        None
                    }
                });

                if let Some(position) = position {
                    // The position from screen_coords_at_pos is already viewport-relative
                    let helix_core::Position {
                        row: viewport_row,
                        col: _,
                    } = position;

                    // Get the line containing the cursor
                    // Since we skip phantom lines, cursor at EOF should position on the last real line
                    let cursor_line = if cursor_at_end && file_ends_with_newline {
                        (total_lines - 1).min(text.len_lines().saturating_sub(1)) // Last real line, not phantom
                    } else {
                        text.char_to_line(cursor_char_idx)
                    };

                    debug!(
                        "Looking for cursor line {cursor_line} in range {first_row}..{last_row}"
                    );

                    // Check if cursor line is in the rendered range
                    // For phantom line, use the effective cursor line
                    let effective_cursor_line = cursor_line;

                    if effective_cursor_line >= first_row && effective_cursor_line < last_row {
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

                            // Get cursor position within the line
                            let line_start = text.line_to_char(cursor_line);
                            let cursor_char_offset = if cursor_at_end && file_ends_with_newline {
                                // When cursor is at EOF with newline, position at end of last real line
                                let line_end = if cursor_line + 1 < text.len_lines() {
                                    text.line_to_char(cursor_line + 1)
                                } else {
                                    text.len_chars()
                                };
                                let line_text = text.slice(line_start..line_end).to_string();
                                line_text
                                    .trim_end_matches(&['\n', '\r'][..])
                                    .chars()
                                    .count()
                            } else {
                                cursor_char_idx.saturating_sub(line_start)
                            };

                            // Get the line text for debugging
                            let line_end = if cursor_line + 1 < text.len_lines() {
                                text.line_to_char(cursor_line + 1)
                            } else {
                                text.len_chars()
                            };
                            let mut line_text = text.slice(line_start..line_end).to_string();
                            // Remove trailing newlines to match how the line was shaped
                            while line_text.ends_with('\n') || line_text.ends_with('\r') {
                                line_text.pop();
                            }
                            // Clamp cursor offset to line character count (without newline)
                            let cursor_char_offset =
                                cursor_char_offset.min(line_text.chars().count());

                            // Convert char offset to byte offset for GPUI's x_for_index
                            let cursor_byte_offset = line_text
                                .chars()
                                .take(cursor_char_offset)
                                .map(char::len_utf8)
                                .sum::<usize>();

                            // Get the x position from the shaped line using byte offset
                            let cursor_x_relative_to_line =
                                line_layout.shaped_line.x_for_index(cursor_byte_offset);

                            // Additional debug for x_for_index calculation

                            // FIXED: Convert from line-relative coordinates to text-area coordinates
                            // Line layouts are stored in text-area coordinates (x=0), so we need to add text bounds offset
                            // Use existing values from the outer scope (editor, document, view, gutter_width are already available)
                            let text_bounds = EditorSurfaceGeometry::new(
                                bounds,
                                gutter_width,
                                after_layout.cell_width,
                            )
                            .text_bounds();

                            // Convert to absolute coordinates by adding text bounds origin
                            let cursor_x = text_bounds.origin.x + cursor_x_relative_to_line;

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

                            // Calculate cursor position RELATIVE to line origin (for paint() method)
                            // cursor.paint() adds the line_layout.origin, so cursor_origin should be relative
                            let relative_cursor_x = cursor_x_relative_to_line; // Already relative to line
                            let relative_cursor_y = px(0.0); // Relative to line origin
                            let cursor_origin = gpui::Point::new(
                                relative_cursor_x, // Relative X coordinate
                                relative_cursor_y, // Relative Y coordinate (line-relative)
                            );

                            // Check if cursor has reversed modifier
                            let has_reversed = cursor_has_reversed_modifier(&cursor_style);

                            // For reversed cursor, we need to get the text style at cursor position
                            let cursor_color = if has_reversed {
                                // Get the styled text color at cursor position
                                // We need to access core again to get the document
                                let text_style_at_cursor = {
                                    let core = self.core.read(cx);
                                    let editor = &core.editor;
                                    let theme = cx.global::<crate::ThemeManager>().helix_theme();
                                    if let Some(doc) = editor.document(self.doc_id) {
                                        let loader = editor.syn_loader.load();
                                        text_style_at_position(
                                            doc,
                                            self.view_id,
                                            theme,
                                            &loader,
                                            cursor_char_idx,
                                        )
                                    } else {
                                        // Default style if document not found
                                        helix_view::graphics::Style::default()
                                    }
                                };

                                // Use the text's foreground color as cursor background
                                text_style_at_cursor
                                    .fg
                                    .and_then(color_to_hsla)
                                    .unwrap_or(fg_color)
                            } else {
                                // Normal cursor: use cursor's background color
                                cursor_style
                                    .bg
                                    .and_then(color_to_hsla)
                                    .or_else(|| cursor_style.fg.and_then(color_to_hsla))
                                    .unwrap_or(fg_color)
                            };

                            // Calculate cursor width based on the actual character width
                            let cursor_width = if let Some(ref shaped_text) = cursor_text_shaped {
                                // Get the width of the shaped text by measuring to the end
                                // x_for_index gives us the x position at the end of the text
                                shaped_text.x_for_index(cursor_text_len)
                            } else {
                                // Default to cell width for empty cursor
                                after_layout.cell_width
                            };

                            let mut cursor = EditorCursor {
                                origin: cursor_origin,
                                kind: cursor_kind,
                                color: cursor_color,
                                block_width: cursor_width,
                                line_height: after_layout.line_height,
                                text: cursor_text_shaped,
                            };

                            // Paint cursor at absolute window coordinates
                            // Convert text-area relative coordinates to absolute window coordinates
                            let absolute_cursor_position = point(
                                text_bounds.origin.x + line_layout.origin.x,
                                text_bounds.origin.y + line_layout.origin.y,
                            );
                            cursor.paint(absolute_cursor_position, window, cx);
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
                                // Calculate text bounds for phantom cursor positioning (same as normal cursor logic)
                                let phantom_text_bounds = EditorSurfaceGeometry::new(
                                    bounds,
                                    gutter_width,
                                    after_layout.cell_width,
                                )
                                .text_bounds();

                                // Calculate cursor position at the first tilde line
                                // Use the y_offset from the main loop (where the next line would be)
                                let cursor_x = phantom_text_bounds.origin.x; // Start of line
                                let cursor_y = phantom_text_bounds.origin.y + y_offset; // At the phantom line position

                                // Check if cursor has reversed modifier (same logic as normal cursor)
                                let has_reversed = cursor_has_reversed_modifier(&cursor_style);

                                // For reversed cursor, we need to get the text style at cursor position
                                let cursor_color = if has_reversed {
                                    // Get the styled text color at cursor position
                                    let text_style_at_cursor = {
                                        let core = self.core.read(cx);
                                        let editor = &core.editor;
                                        let theme =
                                            cx.global::<crate::ThemeManager>().helix_theme();
                                        if let Some(doc) = editor.document(self.doc_id) {
                                            let loader = editor.syn_loader.load();
                                            text_style_at_position(
                                                doc,
                                                self.view_id,
                                                theme,
                                                &loader,
                                                cursor_char_idx,
                                            )
                                        } else {
                                            // Default style if document not found
                                            helix_view::graphics::Style::default()
                                        }
                                    };

                                    // Use the text's foreground color as cursor background
                                    text_style_at_cursor
                                        .fg
                                        .and_then(color_to_hsla)
                                        .unwrap_or(fg_color)
                                } else {
                                    // Normal cursor: use cursor's background color
                                    cursor_style
                                        .bg
                                        .and_then(color_to_hsla)
                                        .or_else(|| cursor_style.fg.and_then(color_to_hsla))
                                        .unwrap_or(fg_color)
                                };

                                // Use default cursor width
                                let cursor_width = after_layout.cell_width;

                                let mut cursor = EditorCursor {
                                    origin: point(px(0.0), px(0.0)),
                                    kind: cursor_kind,
                                    color: cursor_color,
                                    block_width: cursor_width,
                                    line_height: after_layout.line_height,
                                    text: cursor_text_shaped,
                                };

                                // Store cursor position for overlay positioning
                                let cursor_point = point(cursor_x, cursor_y);

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

                                cursor.paint(cursor_point, window, cx);
                            } else {
                                debug!(
                                    "❌ CURSOR FAIL: Normal line layout missing for line {}",
                                    cursor_line
                                );
                            }
                        }
                    } else {
                        debug!(
                            "Cursor line {cursor_line} is outside rendered range {first_row}..{last_row}"
                        );
                    }
                } else {
                    debug!("Cursor rendering skipped - no cursor_pos from screen_coords_at_pos");
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

                // Now paint the gutter lines
                for line in gutter_lines {
                    if let Err(e) =
                        line.shaped_line
                            .paint(line.origin, after_layout.line_height, window, cx)
                    {
                        error!(error = ?e, "Failed to paint gutter line");
                    }
                }

                // Note: In non-soft-wrap mode, we rely on Helix's built-in sign gutters.
                // Custom diagnostic indicators (circle/triangle/square) are only drawn in soft-wrap mode.
            }
        }
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights
