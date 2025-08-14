use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    black, div, fill, hsla, px, relative, white, App, Bounds, Context, DefiniteLength,
    DismissEvent, Element, ElementId, Entity, EventEmitter, FocusHandle, Focusable, Font,
    GlobalElementId, Hitbox, Hsla, InspectorElementId, InteractiveElement, Interactivity,
    IntoElement, LayoutId, MouseButton, ParentElement, Pixels, Point, Render, ShapedLine,
    SharedString, Size, StatefulInteractiveElement, Style, Styled, TextStyle, Window,
    WindowTextSystem,
};
use gpui::{point, size, TextRun};
use helix_core::{
    doc_formatter::{DocumentFormatter, TextFormat},
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    ropey::RopeSlice,
    syntax::{self, Highlight, HighlightEvent, OverlayHighlights},
    text_annotations::TextAnnotations,
    Selection, Uri,
};
use helix_lsp::lsp::Diagnostic;
// Import helix's syntax highlighting system
use helix_view::{
    graphics::CursorKind, view::ViewPosition, Document, DocumentId, Editor, Theme, View, ViewId,
};
use nucleotide_logging::{debug, error};

use crate::Core;
use helix_stdx::rope::RopeSliceExt;
use nucleotide_editor::LineLayoutCache;
use nucleotide_editor::ScrollManager;
use nucleotide_ui::scrollbar::{ScrollableHandle, Scrollbar, ScrollbarState};
use nucleotide_ui::style_utils::{
    apply_color_modifiers, apply_font_modifiers, create_styled_text_run,
};
use nucleotide_ui::theme_utils::color_to_hsla;

#[cfg(debug_assertions)]
fn test_synthetic_click_accuracy(
    line_cache: &nucleotide_editor::LineLayoutCache,
    target_line_idx: usize,
    target_char_idx: usize,
    bounds_width: gpui::Pixels,
    line_height: gpui::Pixels,
) -> Option<(usize, usize)> {
    use nucleotide_logging::debug;

    debug!(
        target_line = target_line_idx,
        target_char = target_char_idx,
        "🎯 Synthetic click test starting - Testing click accuracy at known position"
    );

    // Find the target line in the cache
    if let Some(line_layout) = line_cache.find_line_by_index(target_line_idx) {
        // Calculate approximate pixel position for the target character
        // This is a simple approximation - real position would need character metrics
        let char_width_estimate =
            line_layout.shaped_line.width.0 / line_layout.shaped_line.len() as f32;
        let estimated_x = line_layout.origin.x.0 + (target_char_idx as f32 * char_width_estimate);
        let synthetic_position = gpui::point(gpui::px(estimated_x), line_layout.origin.y);

        debug!(
            estimated_x = estimated_x,
            line_origin = ?line_layout.origin,
            synthetic_position = ?synthetic_position,
            char_width_estimate = char_width_estimate,
            "🎯 Synthetic click position calculated - Calculated synthetic mouse position"
        );

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

            debug!(
                target_line = target_line_idx,
                target_char = target_char_idx,
                found_line = found_layout.line_idx,
                resolved_byte_index = resolved_byte_index,
                "🎯 Synthetic click test result - Click accuracy validation complete"
            );

            Some((found_layout.line_idx, resolved_byte_index))
        } else {
            debug!("🎯 Synthetic click test failed - position not found - Line lookup failed");
            None
        }
    } else {
        debug!("🎯 Synthetic click test failed - target line not in cache - Target line not found");
        None
    }
}

#[cfg(debug_assertions)]
fn test_shaped_line_accuracy(shaped_line: &gpui::ShapedLine, line_text: &str, font_size: f32) {
    use nucleotide_logging::debug;

    debug!(
        line_text_len = line_text.len(),
        line_char_count = line_text.chars().count(),
        shaped_line_width = %shaped_line.width,
        font_size = font_size,
        "🎯 ShapedLine validation test starting - Testing GPUI ShapedLine accuracy"
    );

    // Test various x positions and see if they map to sensible character indices
    let test_positions = vec![
        0.0,                        // Start of line
        shaped_line.width.0 * 0.25, // Quarter way
        shaped_line.width.0 * 0.5,  // Middle
        shaped_line.width.0 * 0.75, // Three quarters
        shaped_line.width.0,        // End of line
        shaped_line.width.0 + 10.0, // Beyond end
    ];

    for (i, x_pos) in test_positions.iter().enumerate() {
        let px_x = gpui::px(*x_pos);
        let byte_index = shaped_line.index_for_x(px_x).unwrap_or(0);

        // Convert byte index to character index for validation
        let char_index = line_text
            .char_indices()
            .take_while(|(byte_idx, _)| *byte_idx < byte_index)
            .count();

        debug!(
            test_case = i,
            x_position = %px_x,
            calculated_byte_index = byte_index,
            calculated_char_index = char_index,
            line_length_chars = line_text.chars().count(),
            line_length_bytes = line_text.len(),
            "🎯 ShapedLine position test - Position->character mapping validation"
        );
    }
}

/// Custom scroll handle for DocumentView that integrates with ScrollManager
#[derive(Clone)]
pub struct DocumentScrollHandle {
    scroll_manager: ScrollManager,
    on_change: Option<Rc<dyn Fn()>>,
    view_id: ViewId,
}

impl std::fmt::Debug for DocumentScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentScrollHandle")
            .field("scroll_manager", &self.scroll_manager)
            .field("on_change", &self.on_change.is_some())
            .field("view_id", &self.view_id)
            .finish()
    }
}

impl DocumentScrollHandle {
    pub fn new(scroll_manager: ScrollManager, view_id: ViewId) -> Self {
        Self {
            scroll_manager,
            on_change: None,
            view_id,
        }
    }

    pub fn with_callback(scroll_manager: ScrollManager, on_change: impl Fn() + 'static) -> Self {
        Self {
            scroll_manager,
            on_change: Some(Rc::new(on_change)),
            view_id: ViewId::default(),
        }
    }
}

impl ScrollableHandle for DocumentScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        self.scroll_manager.max_scroll_offset()
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.scroll_manager.set_scroll_offset(point);

        // Mark that we need to sync back to Helix
        // This will be done in the next paint cycle when we have access to cx

        // Trigger callback if available to notify of change
        if let Some(on_change) = &self.on_change {
            on_change();
        }
    }

    fn offset(&self) -> Point<Pixels> {
        self.scroll_manager.scroll_offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        let size = self.scroll_manager.viewport_size.get();
        Bounds::new(point(px(0.0), px(0.0)), size)
    }
}

/// Parameters for render_with_softwrap
#[allow(dead_code)]
struct SoftwrapRenderParams<'a> {
    document: &'a Document,
    view: &'a View,
    text_format: &'a TextFormat,
    viewport_height: usize,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    window: &'a mut Window,
}

/// Parameters for create_shaped_line
#[allow(dead_code)]
struct ShapedLineParams<'a> {
    line_idx: usize,
    text: RopeSlice<'a>,
    first_row: usize,
    end_char: usize,
    fg_color: Hsla,
    window: &'a mut Window,
    cx: &'a mut App,
}

/// Parameters for highlight_line_with_params
struct HighlightLineParams<'a> {
    doc: &'a Document,
    view: &'a View,
    theme: &'a Theme,
    editor_mode: helix_view::document::Mode,
    cursor_shape: &'a helix_view::editor::CursorShapeConfig,
    syn_loader: &'a std::sync::Arc<arc_swap::ArcSwap<helix_core::syntax::Loader>>,
    is_view_focused: bool,
    line_start: usize,
    line_end: usize,
    fg_color: Hsla,
    font: Font,
}

pub struct DocumentView {
    core: Entity<Core>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    scroll_manager: ScrollManager,
    scrollbar_state: ScrollbarState,
    line_height: Pixels,
}

impl DocumentView {
    pub fn new(
        core: Entity<Core>,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
    ) -> Self {
        // Create scroll manager with placeholder doc_id (will be updated in render)
        let line_height = px(20.0); // Default, will be updated
        let scroll_manager = ScrollManager::new(line_height);

        // Create custom scroll handle that wraps our scroll manager
        let scroll_handle = DocumentScrollHandle::new(scroll_manager.clone(), view_id);
        let scrollbar_state = ScrollbarState::new(scroll_handle);

        Self {
            core,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            scroll_manager,
            scrollbar_state,
            line_height,
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
        px(row as f32 * self.line_height.0)
    }

    /// Convert scroll pixels to a Helix anchor (character position)
    #[allow(dead_code)]
    fn scroll_px_to_anchor(&self, y: Pixels, document: &helix_view::Document) -> usize {
        let row = (y.0 / self.line_height.0).floor() as usize;
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
}

impl EventEmitter<DismissEvent> for DocumentView {}

impl Render for DocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        // Update scroll manager with document info
        {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(document) = editor.document(doc_id) {
                let total_lines = document.text().len_lines();
                self.scroll_manager.set_total_lines(total_lines);
                self.scroll_manager.set_line_height(self.line_height);

                // Set a reasonable default viewport size if not already set
                // This will be updated with actual size in the paint method
                // Use a height that shows fewer lines than total to ensure scrollbar appears
                let viewport_height = self.line_height * 30.0; // Show 30 lines
                self.scroll_manager
                    .set_viewport_size(size(px(800.0), viewport_height));

                // Don't recreate scrollbar state - it's already using our scroll manager

                debug!(
                    "Document has {} lines, viewport shows ~30 lines",
                    total_lines
                );
            }
        }

        // Create the DocumentElement that will handle the actual rendering
        // Pass the same scroll manager and scrollbar state to ensure state is shared
        let document_element = DocumentElement {
            core: self.core.clone(),
            doc_id,
            view_id: self.view_id,
            style: self.style.clone(),
            interactivity: Interactivity::new(),
            focus: self.focus.clone(),
            is_focused: self.is_focused,
            scroll_manager: self.scroll_manager.clone(),
            scrollbar_state: self.scrollbar_state.clone(),
            x_overshoot: Rc::new(Cell::new(px(0.0))),
        };

        // Create the scrollbar
        let scrollbar_opt = Scrollbar::vertical(self.scrollbar_state.clone());

        // Create scroll wheel handler for the editor area
        let scroll_manager_wheel = self.scroll_manager.clone();
        let core_wheel = self.core.clone();

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
                    .child(document_element)
                    .on_scroll_wheel({
                        move |event, _window, cx| {
                            use nucleotide_logging::debug;
                            debug!(
                                delta = ?event.delta,
                                "Scroll wheel event received on editor paint area"
                            );

                            // Get the actual line height from scroll manager
                            let line_height = scroll_manager_wheel.line_height.get();
                            let delta = event.delta.pixel_delta(line_height);

                            debug!(
                                line_height = %line_height,
                                pixel_delta = ?delta,
                                "Converted scroll delta to pixels on paint area"
                            );

                            // Update scroll position immediately for visual feedback
                            let current_offset = scroll_manager_wheel.scroll_offset();

                            // Clamp the scroll offset to valid bounds
                            let max_scroll = scroll_manager_wheel.max_scroll_offset();
                            debug!(
                                current_offset = ?current_offset,
                                max_scroll = ?max_scroll,
                                total_lines = scroll_manager_wheel.total_lines.get(),
                                viewport_size = ?scroll_manager_wheel.viewport_size.get(),
                                "Scroll bounds calculation on paint area"
                            );

                            let new_offset = point(
                                (current_offset.x + delta.x)
                                    .max(px(0.0))
                                    .min(max_scroll.width),
                                (current_offset.y + delta.y)
                                    .max(-max_scroll.height)
                                    .min(px(0.0)),
                            );

                            debug!(
                                new_offset = ?new_offset,
                                offset_changed = ?(new_offset != current_offset),
                                "Calculated new scroll offset on paint area"
                            );

                            // Only update if the offset actually changed
                            if new_offset != current_offset {
                                scroll_manager_wheel.set_scroll_offset(new_offset);

                                // Sync to Helix immediately for responsive scrolling
                                core_wheel.update(cx, |core, cx| {
                                    let editor = &mut core.editor;
                                    let scroll_lines = (delta.y.0 / line_height.0).round() as isize;
                                    if scroll_lines != 0 {
                                        use helix_core::movement::Direction;
                                        use helix_term::commands;

                                        let count = scroll_lines.unsigned_abs();
                                        let mut ctx = helix_term::commands::Context {
                                            editor,
                                            register: None,
                                            count: None,
                                            callback: Vec::new(),
                                            on_next_key_callback: None,
                                            jobs: &mut core.jobs,
                                        };

                                        if scroll_lines > 0 {
                                            commands::scroll(
                                                &mut ctx,
                                                count,
                                                Direction::Backward,
                                                false,
                                            );
                                        } else {
                                            commands::scroll(
                                                &mut ctx,
                                                count,
                                                Direction::Forward,
                                                false,
                                            );
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        }
                    }),
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
    interactivity: Interactivity,
    focus: FocusHandle,
    is_focused: bool,
    scroll_manager: ScrollManager,
    scrollbar_state: ScrollbarState,
    /// X-overshoot tracking for dragging selections past end-of-line (like Zed)
    /// Stores how far past the end of a line the selection extends in pixels
    x_overshoot: Rc<Cell<Pixels>>,
}

impl IntoElement for DocumentElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl DocumentElement {
    /// Get the text style (with syntax highlighting) at a specific character position
    fn get_text_style_at_position(
        doc: &Document,
        view_id: ViewId,
        theme: &Theme,
        syn_loader: &std::sync::Arc<arc_swap::ArcSwap<helix_core::syntax::Loader>>,
        position: usize,
    ) -> helix_view::graphics::Style {
        let loader = syn_loader.load();
        let text = doc.text().slice(..);
        let anchor = doc.view_offset(view_id).anchor;
        let lines_from_anchor = text.len_lines() - text.char_to_line(anchor);
        let height = u16::try_from(lines_from_anchor).unwrap_or(u16::MAX);

        // Get syntax highlighter
        let syntax_highlighter = Self::doc_syntax_highlights(doc, anchor, height, theme, &loader);

        // Get default style
        let default_style = theme.get("ui.text");
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: default_style.bg,
            ..Default::default()
        };

        // Create syntax highlighter and advance to position
        let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);

        // Advance to the position
        while position >= syntax_hl.pos {
            syntax_hl.advance();
        }

        syntax_hl.style
    }
    /// Convert a byte index within a line to a grapheme index
    /// GPUI's shaped line works with UTF-8 byte indices
    /// but Helix works with grapheme cluster indices (visual units)
    fn byte_idx_to_grapheme_idx(line_text: &str, byte_idx: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;

        let mut grapheme_idx = 0;
        let mut current_byte_idx = 0;

        for grapheme in line_text.graphemes(true) {
            if current_byte_idx >= byte_idx {
                break;
            }
            // Count UTF-8 bytes in this grapheme cluster
            current_byte_idx += grapheme.len();
            grapheme_idx += 1;
        }

        grapheme_idx
    }

    /// Calculate text area bounds excluding gutters, headers, scrollbars
    /// This returns the bounds of the actual text editing area in window coordinates
    fn text_bounds(
        &self,
        element_bounds: Bounds<Pixels>,
        gutter_offset: u16,
        cell_width: Pixels,
    ) -> Bounds<Pixels> {
        // Calculate gutter width in pixels
        let gutter_width = Pixels::from(gutter_offset as f32 * cell_width.0);

        // Add small padding to prevent text cutoff (matching existing pattern)
        let right_padding = cell_width * 2.0; // 2 characters of padding
        let top_padding = px(1.0); // Small top padding (matches line rendering)

        // Text area starts after the gutter
        let text_origin = Point {
            x: element_bounds.origin.x + gutter_width,
            y: element_bounds.origin.y + top_padding,
        };

        // Text area size excludes gutter and padding
        let text_size = Size {
            width: element_bounds.size.width - gutter_width - right_padding,
            height: element_bounds.size.height - top_padding,
        };

        Bounds {
            origin: text_origin,
            size: text_size,
        }
    }

    /// Convert window-local coordinates to text-area coordinates
    /// This is the critical transformation that fixes mouse positioning issues
    fn window_to_text_area(
        &self,
        window_pos: Point<Pixels>,
        text_bounds: Bounds<Pixels>,
    ) -> Point<Pixels> {
        Point {
            x: window_pos.x - text_bounds.origin.x,
            y: window_pos.y - text_bounds.origin.y,
        }
    }

    /// Convert text-area coordinates to content coordinates by applying scroll
    /// Uses positive scroll positions matching Zed's conventions
    fn text_area_to_content(&self, text_area_pos: Point<Pixels>) -> Point<Pixels> {
        let scroll_position = self.scroll_manager.scroll_position();

        // Zed convention: scroll_position.y is positive when scrolled down
        // To get content coordinates: content_y = text_area_y + scroll_position.y
        // Example: When scrolled down 100px, scroll_position.y = 100, so content_y = text_area_y + 100
        Point {
            x: text_area_pos.x + scroll_position.x, // Horizontal scroll
            y: text_area_pos.y + scroll_position.y, // Vertical scroll: add positive scroll distance
        }
    }

    /// Unified coordinate transformation: Window -> Text Area -> Content
    /// This implements the correct transformation chain from coordinate_system.md with bounds validation
    fn screen_to_content(
        &self,
        window_pos: Point<Pixels>,
        text_bounds: Bounds<Pixels>,
    ) -> Point<Pixels> {
        // Step 1: Window coordinates to text-area coordinates
        let text_area_pos = self.window_to_text_area(window_pos, text_bounds);

        // Step 2: Apply text-area bounds validation
        let clamped_text_area_pos = Point {
            x: text_area_pos.x.max(px(0.0)).min(text_bounds.size.width),
            y: text_area_pos.y.max(px(0.0)).min(text_bounds.size.height),
        };

        // Step 3: Text-area coordinates to content coordinates
        let content_pos = self.text_area_to_content(clamped_text_area_pos);

        // Step 4: Apply content bounds validation (Y only, allow X overshoot)
        Point {
            x: content_pos.x.max(px(0.0)), // Allow horizontal overshoot for selection dragging
            y: content_pos.y.max(px(0.0)), // Prevent negative Y coordinates
        }
    }

    /// Coordinate transformation with x-overshoot tracking
    /// Returns (clamped_point, x_overshoot_amount) for selection dragging past line ends
    fn screen_to_content_with_overshoot(
        &self,
        window_pos: Point<Pixels>,
        text_bounds: Bounds<Pixels>,
        line_width: Option<Pixels>,
    ) -> (Point<Pixels>, Pixels) {
        // Step 1: Window coordinates to text-area coordinates
        let text_area_pos = self.window_to_text_area(window_pos, text_bounds);

        // Step 2: Apply text-area bounds validation
        let clamped_text_area_pos = Point {
            x: text_area_pos.x.max(px(0.0)).min(text_bounds.size.width),
            y: text_area_pos.y.max(px(0.0)).min(text_bounds.size.height),
        };

        // Step 3: Text-area coordinates to content coordinates
        let content_pos = self.text_area_to_content(clamped_text_area_pos);

        // Step 4: Apply content bounds validation with x-overshoot calculation
        let y = content_pos.y.max(px(0.0)); // Prevent negative Y coordinates

        // Calculate x-overshoot if line width is provided
        let (x, x_overshoot) = if let Some(line_width) = line_width {
            self.calculate_x_overshoot(content_pos.x.max(px(0.0)), line_width)
        } else {
            (content_pos.x.max(px(0.0)), px(0.0))
        };

        (Point { x, y }, x_overshoot)
    }

    /// Get current x-overshoot value
    pub fn x_overshoot(&self) -> Pixels {
        self.x_overshoot.get()
    }

    /// Set x-overshoot value (used when dragging selections past line end)
    pub fn set_x_overshoot(&self, overshoot: Pixels) {
        self.x_overshoot.set(overshoot.max(px(0.0)));
    }

    /// Reset x-overshoot (used when starting a new selection or clicking)
    pub fn reset_x_overshoot(&self) {
        self.x_overshoot.set(px(0.0));
    }

    /// Calculate x-overshoot for a given position and line width
    /// Returns (clamped_x, overshoot_amount)
    fn calculate_x_overshoot(&self, x: Pixels, line_width: Pixels) -> (Pixels, Pixels) {
        if x > line_width {
            let overshoot = x - line_width;
            (line_width, overshoot)
        } else {
            (x, px(0.0))
        }
    }

    pub fn new(
        core: Entity<Core>,
        doc_id: DocumentId,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
    ) -> Self {
        // Create scroll manager for this element
        let line_height = px(20.0); // Default, will be updated
        let scroll_manager = ScrollManager::new(line_height);

        // Create a default scrollbar state
        let scroll_handle = DocumentScrollHandle::new(scroll_manager.clone(), view_id);
        let scrollbar_state = ScrollbarState::new(scroll_handle);

        Self {
            core,
            doc_id,
            view_id,
            style,
            interactivity: Interactivity::new(),
            focus: focus.clone(),
            is_focused,
            scroll_manager,
            scrollbar_state,
            x_overshoot: Rc::new(Cell::new(px(0.0))),
        }
        .track_focus(focus)
    }

    /// Get the TextFormat for soft wrap support
    #[allow(dead_code)]
    fn get_text_format(
        &self,
        document: &Document,
        viewport_width: u16,
        theme: &Theme,
    ) -> TextFormat {
        document.text_format(viewport_width, Some(theme))
    }

    /// Render lines with soft wrap support using DocumentFormatter
    #[allow(dead_code)]
    fn render_with_softwrap(
        &self,
        params: SoftwrapRenderParams,
        _cx: &mut App,
    ) -> Vec<nucleotide_editor::LineLayout> {
        let mut line_layouts = Vec::new();
        let text = params.document.text().slice(..);
        let view_offset = params.document.view_offset(self.view_id);

        // Create text annotations (empty for now, can be extended later)
        let annotations = TextAnnotations::default();

        // Create DocumentFormatter starting at the viewport anchor
        let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
            text,
            params.text_format,
            &annotations,
            view_offset.anchor,
        );

        let mut visual_line = 0;
        let mut current_doc_line = text.char_to_line(view_offset.anchor);
        let mut line_graphemes = Vec::new();

        // Skip lines before the viewport
        for _ in 0..view_offset.vertical_offset {
            if let Some(grapheme) = formatter.next() {
                if grapheme.visual_pos.row > visual_line {
                    visual_line = grapheme.visual_pos.row;
                }
            }
        }

        let text_origin_x = params.bounds.origin.x
            + (f32::from(params.view.gutter_offset(params.document)) * params.cell_width);
        let mut y_offset = px(0.0);

        // Render visible lines
        while visual_line < view_offset.vertical_offset + params.viewport_height {
            line_graphemes.clear();
            let line_y = params.bounds.origin.y + px(1.0) + y_offset;

            // Collect all graphemes for this visual line
            for grapheme in formatter.by_ref() {
                if grapheme.visual_pos.row > visual_line {
                    // We've moved to the next visual line
                    break;
                }
                line_graphemes.push(grapheme);
            }

            if line_graphemes.is_empty() {
                // End of document
                break;
            }

            // Build the line string from graphemes
            let mut line_str = String::new();
            for grapheme in &line_graphemes {
                if !grapheme.is_virtual() {
                    // Handle the Grapheme enum properly
                    match &grapheme.raw {
                        helix_core::graphemes::Grapheme::Tab { .. } => line_str.push('\t'),
                        helix_core::graphemes::Grapheme::Other { g } => line_str.push_str(g),
                        helix_core::graphemes::Grapheme::Newline => {} // Skip newlines in visual lines
                    }
                }
            }

            // Create shaped line for this visual line
            let shaped_line = params.window.text_system().shape_line(
                SharedString::from(line_str),
                self.style.font_size.to_pixels(px(16.0)),
                &[],
                None,
            );

            // Store line layout for interaction
            // FIXED: Store in text-area coordinates (gutter already excluded by text_bounds calculation)
            // text_origin_x already includes gutter, but we want text-area relative coordinates
            let text_area_origin = point(
                px(0.0), // Line starts at x=0 in text-area coordinates (gutter excluded)
                line_y - params.bounds.origin.y,
            );
            let layout = nucleotide_editor::LineLayout {
                line_idx: current_doc_line,
                shaped_line,
                origin: text_area_origin,
            };
            line_layouts.push(layout);

            // Update document line if we've crossed a line boundary
            if let Some(last_grapheme) = line_graphemes.last() {
                let new_doc_line = last_grapheme.line_idx;
                if new_doc_line != current_doc_line {
                    current_doc_line = new_doc_line;
                }
            }

            visual_line += 1;
            y_offset += params.line_height;
        }

        line_layouts
    }

    /// Create a shaped line for a specific line, used for cursor positioning and mouse interaction
    #[allow(dead_code)]
    fn create_shaped_line(&self, params: ShapedLineParams) -> Option<ShapedLine> {
        let line_start = params.text.line_to_char(params.line_idx);
        let line_end = if params.line_idx + 1 < params.text.len_lines() {
            params
                .text
                .line_to_char(params.line_idx + 1)
                .saturating_sub(1) // Exclude newline
        } else {
            params.text.len_chars()
        };

        // Check if line is within our view
        let anchor_char = params.text.line_to_char(params.first_row);
        if line_start >= params.end_char || line_end < anchor_char {
            return None;
        }

        // Adjust line bounds to our view
        let line_start = line_start.max(anchor_char);
        let line_end = line_end.min(params.end_char);

        // For empty lines, line_start may equal line_end, which is valid
        if line_start > line_end {
            return None;
        }

        let line_slice = params.text.slice(line_start..line_end);
        let line_str: SharedString = RopeWrapper(line_slice).into();

        // Get highlights for this line (re-read core)
        let core = self.core.read(params.cx);
        let editor = &core.editor;
        let document = match editor.document(self.doc_id) {
            Some(doc) => doc,
            None => {
                // Document was closed, return empty line runs
                return None;
            }
        };
        let view = editor.tree.try_get(self.view_id)?;

        let theme = params.cx.global::<crate::ThemeManager>().helix_theme();
        let line_runs = Self::highlight_line_with_params(HighlightLineParams {
            doc: document,
            view,
            theme,
            editor_mode: editor.mode(),
            cursor_shape: &editor.config().cursor_shape,
            syn_loader: &editor.syn_loader,
            is_view_focused: self.is_focused,
            line_start,
            line_end,
            fg_color: params.fg_color,
            font: self.style.font(),
        });

        // Create the shaped line
        let shaped_line = params.window.text_system().shape_line(
            line_str,
            self.style.font_size.to_pixels(px(16.0)),
            &line_runs,
            None,
        );

        Some(shaped_line)
    }

    // These 3 methods are just proxies for EditorView
    // TODO: make a PR to helix to extract them from helix_term into helix_view or smth.
    // This function is no longer needed as EditorView::doc_diagnostics_highlights_into
    // directly populates a Vec<OverlayHighlights>

    fn doc_syntax_highlights<'d>(
        doc: &'d helix_view::Document,
        anchor: usize,
        height: u16,
        _theme: &Theme,
        syn_loader: &'d helix_core::syntax::Loader,
    ) -> Option<syntax::Highlighter<'d>> {
        let syntax = doc.syntax()?;
        debug!(language = ?doc.language_name(), "Document has syntax support");

        let text = doc.text().slice(..);

        // Ensure anchor is within bounds
        let anchor = anchor.min(text.len_chars().saturating_sub(1));
        let row = text.char_to_line(anchor);

        // Get a valid viewport range
        let range = Self::viewport_byte_range(text, row, height);

        // Ensure the range is valid for u32 conversion
        let start = (range.start as u32).min(text.len_bytes() as u32);
        let end = (range.end as u32).min(text.len_bytes() as u32);

        if start >= end {
            // No valid range for highlighting
            return None;
        }

        let range = start..end;
        let highlighter = syntax.highlighter(text, syn_loader, range);
        Some(highlighter)
    }

    fn viewport_byte_range(text: RopeSlice, row: usize, height: u16) -> std::ops::Range<usize> {
        // Ensure row is within bounds
        let row = row.min(text.len_lines().saturating_sub(1));
        let start = text.line_to_byte(row);
        let end_row = (row + height as usize).min(text.len_lines());
        let end = text.line_to_byte(end_row);

        // Ensure we have a valid range
        if start >= end {
            // Return a minimal valid range
            0..text.len_bytes().min(1)
        } else {
            start..end
        }
    }

    #[allow(dead_code)]
    fn doc_selection_highlights(
        mode: helix_view::document::Mode,
        doc: &Document,
        view: &View,
        theme: &Theme,
        cursor_shape_config: &helix_view::editor::CursorShapeConfig,
        is_window_focused: bool,
    ) -> Vec<(usize, std::ops::Range<usize>)> {
        // Get the overlay highlights and convert to the expected format
        let overlay_highlights = Self::overlay_highlights(
            mode,
            doc,
            view,
            theme,
            cursor_shape_config,
            is_window_focused,
            true, // is_view_focused - assume true for selection highlights
        );

        // Convert OverlayHighlights to Vec<(usize, Range<usize>)>
        // where usize is an artificial highlight ID and Range is the text range
        // Note: This function is currently unused (#[allow(dead_code)]) and may need revision if actually needed
        // Since Highlight's internal structure is private, we use ordinal values as IDs
        match overlay_highlights {
            helix_core::syntax::OverlayHighlights::Homogeneous {
                highlight: _,
                ranges,
            } => {
                // All ranges use the same highlight - assign ID 0 for homogeneous highlights
                const HOMOGENEOUS_HIGHLIGHT_ID: usize = 0;
                ranges
                    .into_iter()
                    .map(|range| (HOMOGENEOUS_HIGHLIGHT_ID, range))
                    .collect()
            }
            helix_core::syntax::OverlayHighlights::Heterogenous { highlights } => {
                // Each range has its own highlight - assign sequential IDs
                highlights
                    .into_iter()
                    .enumerate()
                    .map(|(index, (_highlight, range))| (index, range))
                    .collect()
            }
        }
    }

    fn overlay_highlights(
        mode: helix_view::document::Mode,
        doc: &Document,
        view: &View,
        theme: &Theme,
        cursor_shape_config: &helix_view::editor::CursorShapeConfig,
        _is_window_focused: bool,
        _is_view_focused: bool,
    ) -> helix_core::syntax::OverlayHighlights {
        // In GUI mode, we need to handle selections but not cursor highlights
        // since we render the cursor separately as a block overlay.

        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);
        let primary_idx = selection.primary_index();

        let cursorkind = cursor_shape_config.from_mode(mode);
        let cursor_is_block = cursorkind == CursorKind::Block;

        let selection_scope = theme
            .find_highlight_exact("ui.selection")
            .expect("could not find `ui.selection` scope in the theme!");
        let primary_selection_scope = theme
            .find_highlight_exact("ui.selection.primary")
            .unwrap_or(selection_scope);

        let mut spans = Vec::new();
        for (i, range) in selection.iter().enumerate() {
            let selection_is_primary = i == primary_idx;
            let selection_scope = if selection_is_primary {
                primary_selection_scope
            } else {
                selection_scope
            };

            // Skip single-character "cursor" selections in block mode for primary selection
            // since we render the cursor separately in GUI
            if range.head == range.anchor {
                // This is just a cursor position, not a real selection
                // We don't add any highlight for this in GUI mode
                continue;
            }

            // Use min_width_1 to handle the selection properly
            let range = range.min_width_1(text);

            if range.head > range.anchor {
                // Forward selection
                let cursor_start = prev_grapheme_boundary(text, range.head);
                // For selections, we want to show the full selection minus the cursor position
                // if it's the primary selection in block mode
                let selection_end = if selection_is_primary
                    && cursor_is_block
                    && mode != helix_view::document::Mode::Insert
                {
                    cursor_start
                } else {
                    range.head
                };

                if range.anchor < selection_end {
                    spans.push((selection_scope, range.anchor..selection_end));
                }
            } else {
                // Reverse selection
                let cursor_end = next_grapheme_boundary(text, range.head);
                // For selections, show from cursor end to anchor
                let selection_start = if selection_is_primary
                    && cursor_is_block
                    && mode != helix_view::document::Mode::Insert
                {
                    cursor_end
                } else {
                    range.head
                };

                if selection_start < range.anchor {
                    spans.push((selection_scope, selection_start..range.anchor));
                }
            }
        }

        if spans.is_empty() {
            // Return empty highlights using Homogeneous with empty ranges
            OverlayHighlights::Homogeneous {
                highlight: Highlight::new(0), // Default highlight (won't be used with empty ranges)
                ranges: Vec::new(),
            }
        } else {
            OverlayHighlights::Heterogenous { highlights: spans }
        }
    }

    fn highlight_line_with_params(params: HighlightLineParams) -> Vec<TextRun> {
        let mut runs = vec![];
        let loader = params.syn_loader.load();

        // Get syntax highlighter for the entire document view
        let text = params.doc.text().slice(..);
        let anchor = params.doc.view_offset(params.view.id).anchor;
        let lines_from_anchor = text.len_lines() - text.char_to_line(anchor);
        let height = u16::try_from(lines_from_anchor).unwrap_or(u16::MAX);
        let syntax_highlighter =
            Self::doc_syntax_highlights(params.doc, anchor, height, params.theme, &loader);

        // Get overlay highlights
        let overlay_highlights = Self::overlay_highlights(
            params.editor_mode,
            params.doc,
            params.view,
            params.theme,
            params.cursor_shape,
            true,
            params.is_view_focused,
        );

        let default_style = params.theme.get("ui.text");
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: default_style.bg,
            ..Default::default()
        };

        // Create syntax and overlay highlighters
        let mut syntax_hl =
            SyntaxHighlighter::new(syntax_highlighter, text, params.theme, text_style);
        let mut overlay_hl = OverlayHighlighter::new(overlay_highlights, params.theme);

        // Get the line text slice to convert character positions to byte lengths
        let line_slice = text.slice(params.line_start..params.line_end);

        let mut position = params.line_start;
        while position < params.line_end {
            // Advance highlighters to current position
            while position >= syntax_hl.pos {
                syntax_hl.advance();
            }
            while position >= overlay_hl.pos {
                overlay_hl.advance();
            }

            // Calculate next position where style might change
            let next_pos = std::cmp::min(
                std::cmp::min(syntax_hl.pos, overlay_hl.pos),
                params.line_end,
            );

            let char_len = next_pos - position;
            if char_len == 0 {
                break;
            }

            // Convert character length to byte length for this segment
            // Get the text slice for this run and measure its byte length
            let run_start_in_line = position - params.line_start;
            let run_end_in_line = next_pos - params.line_start;
            let run_slice = line_slice.slice(run_start_in_line..run_end_in_line);
            let byte_len = run_slice.len_bytes();

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            let fg = style.fg.and_then(color_to_hsla).unwrap_or(params.fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            // Get default background color from theme for reversed modifier
            let default_bg = params
                .theme
                .get("ui.background")
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(black());

            let run = create_styled_text_run(
                byte_len,
                &params.font,
                &style,
                fg,
                bg,
                default_bg,
                underline,
            );
            runs.push(run);
            position = next_pos;
        }

        runs
    }
}

impl InteractiveElement for DocumentElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for DocumentElement {}

#[derive(Debug)]
#[allow(unused)]
pub struct DocumentLayout {
    rows: usize,
    columns: usize,
    line_height: Pixels,
    font_size: Pixels,
    cell_width: Pixels,
    hitbox: Option<Hitbox>,
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

    type PrepaintState = DocumentLayout;

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
        window: &mut Window,
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

        // ============================================================================
        // MOUSE EVENT HANDLING SYSTEM - FIXED COORDINATE SYSTEM
        // ============================================================================
        //
        // COORDINATE SYSTEM OVERVIEW (CORRECTED):
        // 1. GPUI provides mouse events in WINDOW-LOCAL coordinates (not element-local!)
        // 2. Line cache stores line origins in element-local coordinates
        // 3. We must transform: Window -> Text Area -> Content -> Line Lookup
        // 4. Text positioning uses byte offsets (GPUI) converted to character offsets (Helix)
        //
        // TRANSFORMATION CHAIN:
        // Window Coords -> (- text_bounds.origin) -> Text Area Coords -> (+ scroll_position) -> Content Coords
        //
        // EVENT FLOW:
        // Mouse Down -> Transform coordinates -> Set cursor position + start drag state
        // Mouse Move -> Transform coordinates -> Extend selection (only if dragging)
        // Mouse Up   -> End drag state
        //
        // CRITICAL: Register mouse event handlers BEFORE calling interactivity.prepaint()
        // This is required for GPUI to generate a hitbox for our element
        let core_for_down = self.core.clone();
        let scroll_manager_for_down = self.scroll_manager.clone();
        let view_id = self.view_id;
        let doc_id = self.doc_id;
        let line_height = self
            .style
            .line_height_in_pixels(self.style.font_size.to_pixels(px(16.0)));

        // Simplified state for click-to-cursor only (no drag selection)
        // When drag selection is re-implemented, we'll need to restore the drag state tracking

        // Shared state to store actual layout values for mouse handlers
        // These get updated during prepaint and used by mouse event handlers
        let bounds_width = std::rc::Rc::new(std::cell::Cell::new(px(800.0))); // Default fallback
        let bounds_width_for_down = bounds_width.clone();
        let element_bounds_shared =
            std::rc::Rc::new(std::cell::Cell::new(gpui::Point::new(px(0.0), px(0.0)))); // Default fallback
        let element_bounds_for_down = element_bounds_shared.clone();
        let cell_width_shared = std::rc::Rc::new(std::cell::Cell::new(px(16.0))); // Default fallback
        let cell_width_for_down = cell_width_shared.clone();

        // Clone x_overshoot for use in closures
        let x_overshoot_for_down = self.x_overshoot.clone();

        // FIXED: Register mouse down handler with proper coordinate transformation
        self.interactivity
            .on_mouse_down(gpui::MouseButton::Left, move |event, _window, cx| {
                debug!(
                    window_pos = ?event.position,
                    view_id = ?view_id,
                    doc_id = ?doc_id,
                    line_height = %line_height,
                    "🎯 Mouse click received - Starting coordinate transformation"
                );

                // Reset x-overshoot at the start of a new click
                // This ensures that each new click/selection starts without overshoot
                // Overshoot will be recalculated based on the new cursor position
                // self.reset_x_overshoot(); // TODO: Uncomment when ready to test
                // Get gutter offset and calculate text bounds
                let (gutter_offset, cell_width, element_bounds) = {
                    let core = core_for_down.read(cx);
                    let editor = &core.editor;
                    if let (Some(document), Some(view)) = (editor.document(doc_id), editor.tree.try_get(view_id)) {
                        let gutter_offset = view.gutter_offset(document);
                        // Use stored bounds width, with fallback calculation
                        let bounds_width = bounds_width_for_down.get();
                        let element_bounds = gpui::Bounds {
                            origin: element_bounds_for_down.get(), // Use actual bounds from prepaint
                            size: gpui::Size { width: bounds_width, height: px(600.0) }, // Approximate height
                        };
                        debug!(
                            actual_cell_width = %cell_width_for_down.get(),
                            gutter_offset = gutter_offset,
                            "🎯 CELL_WIDTH DEBUG: Using actual calculated cell width from prepaint"
                        );
                        (gutter_offset, cell_width_for_down.get(), element_bounds) // Use actual cell_width
                    } else {
                        debug!("Could not get document/view for coordinate transformation");
                        return;
                    }
                };
                // CRITICAL FIX: Calculate text bounds to get actual text area
                // This is the missing piece - we need to know where the text area starts
                let text_bounds = {
                    let gutter_width = Pixels::from(gutter_offset as f32 * cell_width.0);
                    let right_padding = cell_width * 2.0;
                    let top_padding = px(1.0);
                    gpui::Bounds {
                        origin: gpui::Point {
                            x: element_bounds.origin.x + gutter_width,
                            y: element_bounds.origin.y + top_padding,
                        },
                        size: gpui::Size {
                            width: element_bounds.size.width - gutter_width - right_padding,
                            height: element_bounds.size.height - top_padding,
                        },
                    }
                };

                let expected_text_origin_x = element_bounds.origin.x + Pixels::from(gutter_offset as f32 * cell_width.0);
                debug!(
                    text_bounds = ?text_bounds,
                    gutter_offset = gutter_offset,
                    gutter_width_pixels = %(gutter_offset as f32 * cell_width.0),
                    cell_width = %cell_width,
                    element_bounds = ?element_bounds,
                    expected_text_origin_x = %expected_text_origin_x,
                    calculated_text_bounds_origin_x = %text_bounds.origin.x,
                    text_origin_matches = expected_text_origin_x == text_bounds.origin.x,
                    "🎯 CRITICAL TEXT BOUNDS DEBUG - This should match line layout calculations!"
                );

                // STEP 1: Convert window coordinates to text-area coordinates
                let text_area_pos = gpui::Point {
                    x: event.position.x - text_bounds.origin.x,
                    y: event.position.y - text_bounds.origin.y,
                };

                // STEP 2: Detect soft-wrap mode for branched coordinate transformation
                let soft_wrap_enabled = {
                    let core = core_for_down.read(cx);
                    let editor = &core.editor;
                    if let Some(document) = editor.document(doc_id) {
                        if let Some(_view) = editor.tree.try_get(view_id) {
                            let theme = cx.global::<crate::ThemeManager>().helix_theme();
                            // Calculate viewport width for text formatting (matching paint calculation)
                            let gutter_width_px = f32::from(gutter_offset) * cell_width.0;
                            let right_padding = cell_width.0 * 2.0; // 2 characters of padding
                            let text_area_width = text_bounds.size.width.0 - gutter_width_px - right_padding;
                            let viewport_width = (text_area_width / cell_width.0).max(10.0) as u16;

                            let text_format = document.text_format(viewport_width, Some(theme));
                            text_format.soft_wrap
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                debug!(
                    soft_wrap_enabled = soft_wrap_enabled,
                    viewport_width = (text_bounds.size.width.0 / cell_width.0).max(10.0) as u16,
                    "🎯 SOFT-WRAP DETECTION: Detected soft-wrap mode for coordinate transformation"
                );

                // STEP 3: Convert text-area coordinates to content coordinates
                // Branch based on soft-wrap mode for different coordinate transformation logic
                let scroll_position = scroll_manager_for_down.scroll_position();
                let content_pos = if soft_wrap_enabled {
                    // TODO: Implement proper wrapped mode coordinate transformation in next task
                    // For now, use the same transformation as non-wrapped mode
                    debug!("🎯 COORDINATE TRANSFORM: Using wrapped mode transformation (placeholder)");
                    gpui::Point {
                        x: text_area_pos.x + scroll_position.x, // Horizontal scroll (currently unused)
                        y: text_area_pos.y + scroll_position.y, // Add positive scroll distance
                    }
                } else {
                    debug!("🎯 COORDINATE TRANSFORM: Using non-wrapped mode transformation");
                    // Zed convention: scroll_position.y is positive when scrolled down
                    // To get content coordinates: content_y = text_area_y + scroll_position.y
                    gpui::Point {
                        x: text_area_pos.x + scroll_position.x, // Horizontal scroll (currently unused)
                        y: text_area_pos.y + scroll_position.y, // Add positive scroll distance
                    }
                };

                // STEP 4: Apply bounds validation and clamping
                // Clamp coordinates to valid ranges to prevent out-of-bounds access
                let clamped_text_area_pos = Point {
                    x: text_area_pos.x.max(px(0.0)).min(text_bounds.size.width),
                    y: text_area_pos.y.max(px(0.0)).min(text_bounds.size.height),
                };

                // Calculate total content height based on document lines
                let total_content_height = {
                    let core = core_for_down.read(cx);
                    let editor = &core.editor;
                    if let Some(document) = editor.document(doc_id) {
                        let total_lines = document.text().len_lines();
                        px(total_lines as f32 * line_height.0)
                    } else {
                        px(1000.0) // Fallback
                    }
                };

                // For now, clamp content position without x-overshoot tracking
                // X-overshoot will be calculated later when we have line width information
                let clamped_content_pos = Point {
                    x: content_pos.x.max(px(0.0)), // Will be updated with x-overshoot tracking below
                    y: content_pos.y.max(px(0.0)).min(total_content_height),
                };

                debug!(
                    window_pos = ?event.position,
                    text_area_pos = ?text_area_pos,
                    clamped_text_area_pos = ?clamped_text_area_pos,
                    scroll_position = ?scroll_position,
                    content_pos = ?content_pos,
                    clamped_content_pos = ?clamped_content_pos,
                    total_content_height = %total_content_height,
                    "🎯 Coordinate transformation with bounds validation complete"
                );

                // STEP 4: Find line using clamped coordinates
                let line_cache = cx.global::<nucleotide_editor::LineLayoutCache>();

                // NOTE: Line cache still stores in element-local coordinates, so we use clamped_text_area_pos
                // This ensures we don't access out-of-bounds positions in the line cache
                let line_layout = line_cache.find_line_at_position(
                    clamped_text_area_pos, // Use clamped text-area coordinates
                    text_bounds.size.width,
                    line_height,
                );

                if let Some(line_layout) = line_layout {
                    debug!(
                        found_line_idx = line_layout.line_idx,
                        line_origin = ?line_layout.origin,
                        shaped_line_width = %line_layout.shaped_line.width,
                        shaped_line_len_bytes = line_layout.shaped_line.len(),
                        "🎯 Line found using corrected coordinates - DETAILED ANALYSIS"
                    );

                    // STEP 4.5: Calculate and track x-overshoot for selection dragging
                    let line_width = line_layout.shaped_line.width;
                    let raw_content_x = content_pos.x.max(px(0.0));
                    let (clamped_x, x_overshoot) = if raw_content_x > line_width {
                        let overshoot = raw_content_x - line_width;
                        (line_width, overshoot)
                    } else {
                        (raw_content_x, px(0.0))
                    };

                    // Store x-overshoot for future selection operations
                    x_overshoot_for_down.set(x_overshoot.max(px(0.0)));
                    debug!(
                        raw_content_x = %raw_content_x,
                        line_width = %line_width,
                        clamped_x = %clamped_x,
                        x_overshoot = %x_overshoot,
                        "🎯 X-overshoot tracking: calculated overshoot for selection dragging"
                    );

                    // STEP 5: Calculate character position within the line using clamped coordinates
                    // line_layout.origin is in element-local coordinates (text-area relative)
                    let relative_x = clamped_text_area_pos.x - line_layout.origin.x;

                    debug!(
                        raw_window_x = %event.position.x,
                        calculated_text_bounds_origin_x = %text_bounds.origin.x,
                        resulting_text_area_x = %text_area_pos.x,
                        clamped_text_area_x = %clamped_text_area_pos.x,
                        line_origin_x = %line_layout.origin.x,
                        relative_x = %relative_x,
                        "🎯 DETAILED X-AXIS DEBUG: relative_x should be distance from line start in pixels"
                    );

                    // Convert pixel position to byte offset with proper bounds checking
                    let byte_index = if relative_x < px(0.0) {
                        0 // Click before line start
                    } else if relative_x > line_layout.shaped_line.width {
                        line_layout.shaped_line.len() // Click beyond line end
                    } else {
                        line_layout.shaped_line.index_for_x(relative_x).unwrap_or(0)
                    };

                    debug!(
                        relative_x = %relative_x,
                        line_width = %line_layout.shaped_line.width,
                        byte_index = byte_index,
                        "🎯 Byte index calculated with bounds checking"
                    );

                    // Update Helix editor selection
                    core_for_down.update(cx, |core, cx| {
                        let editor = &mut core.editor;
                        if let Some(document) = editor.document_mut(doc_id) {
                            let text = document.text();
                            let line_start = text.line_to_char(line_layout.line_idx);

                            // Convert byte offset to character offset for Unicode support
                            let line_text = text.line(line_layout.line_idx).to_string();
                            let char_offset = line_text.char_indices()
                                .take_while(|(byte_idx, _)| *byte_idx < byte_index)
                                .count();

                            let target_pos = (line_start + char_offset).min(text.len_chars());

                            debug!(
                                line_idx = line_layout.line_idx,
                                line_start = line_start,
                                char_offset = char_offset,
                                target_pos = target_pos,
                                "🎯 Final cursor position calculated"
                            );

                            // Create cursor selection
                            let range = helix_core::Range::new(target_pos, target_pos);
                            let selection = helix_core::Selection::new(helix_core::SmallVec::from([range]), 0);
                            document.set_selection(view_id, selection);

                            cx.notify();
                        }
                    });
                } else {
                    debug!(
                        window_pos = ?event.position,
                        text_area_pos = ?text_area_pos,
                        content_pos = ?content_pos,
                        "🎯 No line found - click may be outside text area"
                    );
                }
            });

        // TODO: Implement proper click and drag selection
        // For now, disable drag selection to focus on basic click-to-cursor functionality
        // The drag selection logic needs more investigation to work properly with Helix's
        // selection model and GPUI's coordinate systems.
        //
        // Issues to investigate:
        // 1. Proper anchor handling during drag operations
        // 2. Integration with Helix's Range and Selection types
        // 3. Visual feedback during selection operations
        // 4. Performance implications of frequent selection updates

        // Disabled mouse move handler - no drag selection for now
        // self.interactivity.on_mouse_move(...)

        // Mouse up handler - simplified since drag selection is disabled
        self.interactivity
            .on_mouse_up(gpui::MouseButton::Left, move |event, _window, _cx| {
                debug!(position = ?event.position, "Mouse up event - click completed");
                // Note: No drag state to clean up since drag selection is disabled
            });

        let _core = self.core.clone();
        self.interactivity.prepaint(
            _global_id,
            _inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, _window, cx| {
                // Calculate actual cell width here (same as lines 1613-1617)
                let font_id = cx.text_system().resolve_font(&self.style.font());
                let font_size = self.style.font_size.to_pixels(px(16.0));
                let em_width = cx
                    .text_system()
                    .typographic_bounds(font_id, font_size, 'm')
                    .map(|bounds| bounds.size.width)
                    .unwrap_or(px(8.0));
                let actual_cell_width = cx
                    .text_system()
                    .advance(font_id, font_size, 'm')
                    .map(|advance| advance.width)
                    .unwrap_or(em_width);

                // Update layout values for mouse handlers
                bounds_width.set(bounds.size.width);
                element_bounds_shared.set(bounds.origin);
                cell_width_shared.set(actual_cell_width);
                debug!(
                    bounds_width = ?bounds.size.width,
                    element_bounds_origin = ?bounds.origin,
                    actual_cell_width = %actual_cell_width,
                    "Updated layout values for mouse handlers - CRITICAL FIX WITH REAL CELL WIDTH"
                );

                // Font metrics consistency validation - temporarily disabled
                // #[cfg(debug_assertions)]
                // { ... font validation disabled for now }

                // Hitbox should now be generated due to registered mouse handlers
                debug!(?hitbox, ">> GPUI returned hitbox for editor");
                if hitbox.is_none() {
                    error!("UNEXPECTED: NO HITBOX despite registered mouse handlers");
                    // This should not happen now that we have mouse handlers
                } else {
                    debug!("SUCCESS: Hitbox generated, mouse events will reach this element");
                }

                // CONTENT MASKING:
                // Modern GPUI handles content clipping automatically through the layout system
                // Content is automatically clipped to element bounds during rendering
                // No explicit masking API calls are needed in current GPUI version
                {
                    let font_id = cx.text_system().resolve_font(&self.style.font());
                    let font_size = self.style.font_size.to_pixels(px(16.0));
                    let line_height = self.style.line_height_in_pixels(font_size);
                    let em_width = cx
                        .text_system()
                        .typographic_bounds(font_id, font_size, 'm')
                        .map(|bounds| bounds.size.width)
                        .unwrap_or(px(8.0)); // Default em width
                    let cell_width = cx
                        .text_system()
                        .advance(font_id, font_size, 'm')
                        .map(|advance| advance.width)
                        .unwrap_or(em_width); // Use em_width as fallback
                                              // Division of Pixels returns f32
                    let columns_f32 = (bounds.size.width / em_width).floor();
                    let rows_f32 = (bounds.size.height / line_height).floor();
                    let columns = (columns_f32 as usize).max(1);
                    let rows = (rows_f32 as usize).max(1);

                    // Don't update editor state during layout/prepaint phase
                    // The editor should be resized elsewhere, not during rendering
                    DocumentLayout {
                        hitbox,
                        rows,
                        columns,
                        line_height,
                        font_size,
                        cell_width,
                    }
                }
            },
        )
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
        let focus = self.focus.clone();
        let core = self.core.clone();
        let view_id = self.view_id;
        let cell_width = after_layout.cell_width;
        let line_height = after_layout.line_height;

        // Update scroll manager with current layout info
        self.scroll_manager.set_line_height(line_height);
        self.scroll_manager.set_viewport_size(bounds.size);

        // TODO: Update shared cell_width for mouse handlers (requires structural change to pass between prepaint/paint)

        // Sync scroll position back to Helix only if scrollbar changed it
        // This prevents overriding Helix's auto-scroll behavior
        if self.scroll_manager.scrollbar_changed.get() {
            core.update(cx, |core, _| {
                let editor = &mut core.editor;
                if let Some(doc) = editor.document(self.doc_id) {
                    let new_offset = self.scroll_manager.sync_to_helix(doc);
                    // Convert our ViewOffset to helix ViewPosition
                    let view_position = ViewPosition {
                        anchor: new_offset.anchor,
                        horizontal_offset: new_offset.horizontal_offset,
                        vertical_offset: new_offset.vertical_offset,
                    };
                    if let Some(doc_mut) = editor.document_mut(self.doc_id) {
                        doc_mut.set_view_offset(view_id, view_position);
                    }
                }
            });
            // Clear the flag after syncing
            self.scroll_manager.scrollbar_changed.set(false);
        }

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

            // Update scroll manager with document info
            let total_lines = doc.text().len_lines();
            self.scroll_manager.total_lines.set(total_lines);

            // Sync scroll position from Helix to ensure we reflect auto-scroll
            // This is important for keeping cursor visible during editing
            let view_offset = doc.view_offset(self.view_id);
            let text = doc.text();
            let anchor_line = text.char_to_line(view_offset.anchor);
            let y = px(anchor_line as f32 * self.scroll_manager.line_height.get().0);
            // GPUI convention: negative offset when scrolled down
            // Use set_scroll_offset_from_helix to avoid marking as scrollbar-changed
            self.scroll_manager
                .set_scroll_offset_from_helix(point(px(0.0), -y));

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

                    // Calculate viewport width accounting for gutter and some padding
                    let gutter_width_px = f32::from(gutter_offset) * after_layout.cell_width;
                    let right_padding = after_layout.cell_width * 2.0; // 2 characters of padding
                    let text_area_width = bounds.size.width - gutter_width_px - right_padding;
                    let viewport_width =
                        (text_area_width / after_layout.cell_width).max(10.0) as u16;

                    let text_format = document.text_format(viewport_width, Some(theme));
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

        let line_cache_mouse = line_cache.clone();
        let scrollbar_state_mouse = self.scrollbar_state.clone();
        // OLD MOUSE HANDLER REMOVED - Using new comprehensive handler above

        // Handle mouse drag for selection
        // Removed drag-related variables since mouse move handler is disabled
        // let core_drag = self.core.clone();
        // let view_id_drag = self.view_id;
        // let line_cache_drag = line_cache.clone();
        // let scrollbar_state_drag = self.scrollbar_state.clone();
        // let _scroll_manager_drag = self.scroll_manager.clone();

        // TODO: This was the ACTUAL source of mouse selection following mouse movement!
        // This handler was extending selection on every mouse move without any drag state checking.
        // Disabled for now - will need proper drag state integration when re-implementing.
        //
        // self.interactivity.on_mouse_move(move |ev, _window, cx| {
        //     // ... drag selection logic was here ...
        // });

        // OLD MOUSE UP HANDLER REMOVED - Using new handler above

        let is_focused = self.is_focused;

        self.interactivity
            .paint(_global_id, _inspector_id, bounds, after_layout.hitbox.as_ref(), window, cx, |_, window, cx| {
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
                debug!("Cursorline check - config value: {}, focused: {}, enabled: {}",
                    config_cursorline, is_focused, cursorline_enabled);

                let theme = cx.global::<crate::ThemeManager>().helix_theme();
                // Get cursorline style
                let cursorline_style = if cursorline_enabled {
                    let style = theme.get("ui.cursorline.primary");
                    debug!("Cursorline style found: bg={:?}, fg={:?}", style.bg, style.fg);
                    style.bg.and_then(color_to_hsla)
                } else {
                    None
                };
                let default_style = theme.get("ui.background");
                let bg_color = default_style.bg
                    .and_then(color_to_hsla)
                    .unwrap_or(black());
                // Get mode-specific cursor theme like terminal version
                let mode = editor.mode();
                let base_cursor_style = theme.get("ui.cursor");
                let base_primary_cursor_style = theme.get("ui.cursor.primary");

                // Try to get mode-specific cursor style, fallback to base
                // Important: we need to patch styles to combine colors with modifiers
                let cursor_style = match mode {
                    helix_view::document::Mode::Insert => {
                        let style = theme.get("ui.cursor.primary.insert");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                    helix_view::document::Mode::Select => {
                        let style = theme.get("ui.cursor.primary.select");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                    helix_view::document::Mode::Normal => {
                        let style = theme.get("ui.cursor.primary.normal");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                };
                let _bg = fill(bounds, bg_color);
                let fg_color = color_to_hsla(
                    default_style
                        .fg
                        .unwrap_or(helix_view::graphics::Color::White),
                )
                .unwrap_or(white());

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
                    debug!("Cursor position - row: {}, col: {}, primary_idx: {}, gutter_width: {}",
                           pos.row, pos.col, primary_idx, gutter_width);

                    // Additional debug: check what line and column we're actually at
                    let line = text.char_to_line(primary_idx);
                    let line_start = text.line_to_char(line);
                    let col_in_line = primary_idx - line_start;
                    debug!("Actual position - line: {line}, col_in_line: {col_in_line}, line_start: {line_start}");
                } else {
                    debug!("Warning: screen_coords_at_pos returned None for cursor position {primary_idx}");
                }
                let gutter_overflow = gutter_width == 0;
                if !gutter_overflow {
                    debug!("need to render gutter {gutter_width}");
                }

                let _cursor_row = cursor_pos.map(|p| p.row);
                let total_lines = text.len_lines();

                // Use scroll manager to determine visible lines
                let (first_row, last_row_from_scroll) = self.scroll_manager.visible_line_range();

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

                        // Replace newlines with visible character for display
                        let char_str = if char_str == "\n" || char_str == "\r\n" || char_str == "\r" {
                            "⏎".into() // Use return symbol for newlines
                        } else {
                            char_str
                        };

                        if !char_str.is_empty() {
                                    // Check if cursor has reversed modifier
                                    let has_reversed = cursor_style.add_modifier.contains(helix_view::graphics::Modifier::REVERSED) &&
                                                       !cursor_style.sub_modifier.contains(helix_view::graphics::Modifier::REVERSED);

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
                debug!("Cursor is at line: {}, char_idx: {}", cursor_line_num, cursor_char_idx);

                // Use the last row from scroll manager
                let mut last_row = last_row_from_scroll;

                // Check if cursor is at the very end of the file (phantom line)
                let cursor_at_end = cursor_char_idx == text.len_chars();
                let file_ends_with_newline = text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n';

                debug!("End of file check - cursor_char_idx: {}, text.len_chars(): {}, last_char: {:?}, cursor_at_end: {}, ends_with_newline: {}",
                    cursor_char_idx, text.len_chars(),
                    if text.len_chars() > 0 { Some(text.char(text.len_chars() - 1)) } else { None },
                    cursor_at_end, file_ends_with_newline);

                // If cursor is at end of file with trailing newline, we need to render the phantom line
                // For a file ending with \n, Rope counts the empty line after it, so we don't need to add 1
                if cursor_at_end && file_ends_with_newline {
                    let cursor_line = text.char_to_line(cursor_char_idx.saturating_sub(1));
                    debug!("Cursor at EOF with newline - cursor_line: {cursor_line}, last_row before: {last_row}, total_lines: {total_lines}");

                    // Ensure last_row includes the phantom line (which is at index total_lines - 1)
                    last_row = last_row.max(total_lines);
                    debug!("last_row after adjustment: {last_row}");
                }

                // println!("first row is {first_row} last row is {last_row}");
                // When rendering phantom line, end_char should be beyond the document end
                let end_char = if last_row > total_lines {
                    text.len_chars() + 1  // Allow phantom line to be rendered
                } else {
                    text.line_to_char(std::cmp::min(last_row, total_lines))
                };

                // Render text line by line to avoid newline issues
                let mut y_offset = px(0.);
                // COORDINATE SYSTEM ANALYSIS: The original version stored in GLOBAL coordinates
                // but current version converts to LOCAL coordinates before storage
                // The px(2.) was part of the global calculation, but since we now convert to local,
                // we need to match the soft-wrap calculation which doesn't include px(2.)
                let text_origin_x = bounds.origin.x + (after_layout.cell_width * f32::from(gutter_width));

                debug!(
                    bounds_origin_x = %bounds.origin.x,
                    coordinate_system = "LOCAL", // Now using local coordinate system like soft-wrap
                    cell_width = %after_layout.cell_width,
                    gutter_width = gutter_width,
                    calculated_text_origin_x = %text_origin_x,
                    will_become_local_x = %(text_origin_x - bounds.origin.x),
                    "🎯 UNIFIED NON-SOFT-WRAP calculation - Matching soft-wrap coordinate system"
                );

                // Render rulers before text
                let ruler_style = theme.get("ui.virtual.ruler");
                let ruler_color = ruler_style.bg
                    .and_then(color_to_hsla)
                    .unwrap_or_else(|| hsla(0.0, 0.0, 0.3, 0.2)); // Default to subtle gray

                // Get rulers configuration - try language-specific first, then fall back to editor config
                let editor_config = editor.config();
                let rulers = document.language_config()
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
                    let ruler_x = text_origin_x + (after_layout.cell_width * (f32::from(ruler_col - 1) - horizontal_offset));

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
                let editor_theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
                let editor_mode = editor.mode();
                let cursor_shape = editor.config().cursor_shape.clone();
                let syn_loader = editor.syn_loader.clone();

                // Clone text to avoid borrowing issues
                let doc_text = document.text().clone();

                // Also extract document_id and view_id for use in the loop
                let doc_id = self.doc_id;
                let view_id = self.view_id;

                // Extract cursor-related data before dropping core
                // cursor_char_idx was already extracted earlier for phantom line check
                let _tab_width = document.tab_width() as u16;

                // Shape cursor text before dropping core borrow and keep its length
                let (cursor_text_shaped, cursor_text_len) = cursor_text.map(|(char_str, text_color)| {
                    let text_len = char_str.len();
                    let run = TextRun {
                        len: text_len,
                        font: self.style.font(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };

                    let shaped = window.text_system()
                        .shape_line(char_str, self.style.font_size.to_pixels(px(16.0)), &[run], None);
                    (Some(shaped), text_len)
                }).unwrap_or((None, 0));

                // Drop the core borrow before the loop
                // core goes out of scope here

                let text = doc_text.slice(..);

                // Update the shared line layouts for mouse interaction
                if soft_wrap_enabled {
                    // Use DocumentFormatter for soft wrap rendering

                    // Get text format and create DocumentFormatter
                    let theme = cx.global::<crate::ThemeManager>().helix_theme();

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

                        // Calculate viewport width accounting for gutter and some padding
                        // The text area width is the total width minus the gutter width
                        // We also subtract a small amount for right padding to prevent text cutoff
                        let gutter_width_px = f32::from(gutter_offset) * after_layout.cell_width;
                        let right_padding = after_layout.cell_width * 2.0; // 2 characters of padding
                        let text_area_width = bounds.size.width - gutter_width_px - right_padding;
                        let viewport_width = (text_area_width / after_layout.cell_width).max(10.0) as u16;

                        let text_format = document.text_format(viewport_width, Some(theme));
                        (text_format, view_offset, gutter_offset)
                    };

                    let annotations = TextAnnotations::default();

                    // Create DocumentFormatter starting at the viewport anchor
                    let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
                        text,
                        &text_format,
                        &annotations,
                        view_offset.anchor,
                    );

                    let text_origin_x = bounds.origin.x + (f32::from(gutter_offset) * after_layout.cell_width);
                    debug!(
                        bounds_origin_x = %bounds.origin.x,
                        cell_width = %after_layout.cell_width,
                        gutter_offset = gutter_offset,
                        calculated_text_origin_x = %text_origin_x,
                        "🎯 SOFT-WRAP text_origin_x calculation - Soft-wrap coordinate calculation"
                    );
                    let mut y_offset = px(0.0);
                    let mut visual_line = 0;
                    let mut current_doc_line = text.char_to_line(view_offset.anchor);
                    let viewport_height = (bounds.size.height / after_layout.line_height) as usize;

                    // Skip lines before the viewport - need to consume all graphemes for skipped lines
                    let mut pending_grapheme = None;
                    while visual_line < view_offset.vertical_offset {
                        for grapheme in formatter.by_ref() {
                            if grapheme.visual_pos.row > visual_line {
                                // We've moved to the next visual line
                                visual_line = grapheme.visual_pos.row;
                                if visual_line >= view_offset.vertical_offset {
                                    // This grapheme is part of the first visible line
                                    pending_grapheme = Some(grapheme);
                                    break;
                                }
                            }
                        }
                        if visual_line < view_offset.vertical_offset {
                            // Move to next line if we haven't reached viewport yet
                            visual_line += 1;
                        }
                    }

                    // Render visible lines with DocumentFormatter
                    while visual_line < view_offset.vertical_offset + viewport_height {
                        let mut line_graphemes = Vec::new();
                        let line_y = bounds.origin.y + px(1.0) + y_offset;

                        // Add any pending grapheme from previous iteration
                        if let Some(grapheme) = pending_grapheme.take() {
                            if grapheme.visual_pos.row == visual_line {
                                line_graphemes.push(grapheme);
                            } else if grapheme.visual_pos.row > visual_line {
                                // This grapheme is for a future line, put it back
                                pending_grapheme = Some(grapheme);
                                // This visual line is empty, but we still need to render it
                            }
                            // If grapheme.visual_pos.row < visual_line, we've somehow skipped it (shouldn't happen)
                        }

                        // Collect all graphemes for this visual line
                        let mut has_content = !line_graphemes.is_empty();
                        for grapheme in formatter.by_ref() {
                            if grapheme.visual_pos.row > visual_line {
                                // This grapheme belongs to the next visual line
                                pending_grapheme = Some(grapheme);
                                break;
                            } else if grapheme.visual_pos.row == visual_line {
                                line_graphemes.push(grapheme);
                                has_content = true;
                            }
                            // If grapheme.visual_pos.row < visual_line, skip it (shouldn't happen)
                        }

                        // Check if this might be an empty line that needs rendering
                        // Empty lines still need to be displayed even if they have no graphemes
                        if !has_content && line_graphemes.is_empty() {
                            if let Some(ref pending) = pending_grapheme {
                                // We have a pending grapheme - check if it's for a future line
                                // If so, this current visual line is empty and should be rendered
                                if pending.visual_pos.row > visual_line {
                                    // This is an empty visual line - render it
                                }
                            } else {
                                // No more content - end of document
                                break;
                            }
                        }

                        // Build the line string from graphemes including wrap indicators and indentation
                        let mut line_str = String::new();
                        let mut wrap_indicator_len = 0usize; // Track wrap indicator byte length

                        // Track the starting column position for wrapped lines with indentation
                        let line_start_col = line_graphemes.first().map(|g| g.visual_pos.col).unwrap_or(0);

                        // If the line starts at a column > 0, we need to add leading spaces
                        // This happens for wrapped lines with indentation carry-over
                        if line_start_col > 0 {
                            // Add spaces for the indentation
                            for _ in 0..line_start_col {
                                line_str.push(' ');
                            }
                        }

                        // Process graphemes
                        for grapheme in &line_graphemes {
                            if grapheme.is_virtual() {
                                // This is virtual text (likely wrap indicator)
                                if let helix_core::graphemes::Grapheme::Other { g } = &grapheme.raw {
                                    wrap_indicator_len += g.len(); // Track byte length
                                    line_str.push_str(g);
                                }
                            } else {
                                // Real text content
                                match &grapheme.raw {
                                    helix_core::graphemes::Grapheme::Tab { .. } => line_str.push('\t'),
                                    helix_core::graphemes::Grapheme::Other { g } => line_str.push_str(g),
                                    helix_core::graphemes::Grapheme::Newline => {}, // Skip newlines in visual lines
                                }
                            }
                        }

                        // Get highlights for this line
                        let mut line_runs = if let Some(first_grapheme) = line_graphemes.iter().find(|g| !g.is_virtual()) {
                            // Use the first non-virtual grapheme for the start position
                            let line_start = first_grapheme.char_idx;
                            let line_end = line_graphemes.iter()
                                .filter(|g| !g.is_virtual())
                                .next_back()
                                .map(|g| g.char_idx + g.doc_chars())
                                .unwrap_or(line_start);

                            // Re-read core to get highlights and immediately drop the borrow
                            {
                                let core = self.core.read(cx);
                                let editor = &core.editor;
                                if let Some(document) = editor.document(self.doc_id) {
                                    let view = match editor.tree.try_get(self.view_id) {
                                        Some(v) => v,
                                        None => return,
                                    };
                                    Self::highlight_line_with_params(HighlightLineParams {
                                        doc: document,
                                        view,
                                        theme: &editor_theme,
                                        editor_mode,
                                        cursor_shape: &cursor_shape,
                                        syn_loader: &syn_loader,
                                        is_view_focused: self.is_focused,
                                        line_start,
                                        line_end,
                                        fg_color,
                                        font: self.style.font(),
                                    })
                                } else {
                                    Vec::new()
                                }
                            }
                        } else {
                            Vec::new()
                        };

                        // Adjust text runs to account for leading spaces and wrap indicator
                        let mut prefix_len = line_start_col; // Indentation spaces
                        if wrap_indicator_len > 0 {
                            prefix_len += wrap_indicator_len;
                        }

                        if prefix_len > 0 && !line_runs.is_empty() {
                            // Add a default-styled run for the prefix (indentation + wrap indicator)
                            let prefix_run = TextRun {
                                len: prefix_len,
                                font: self.style.font(),
                                color: fg_color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };

                            // Prepend the prefix run
                            line_runs.insert(0, prefix_run);
                        }

                        // Paint the line
                        if !line_str.is_empty() {
                            let shaped_line = window.text_system()
                                .shape_line(SharedString::from(line_str.clone()), self.style.font_size.to_pixels(px(16.0)), &line_runs, None);

                            // Paint background highlights using the shaped line for accurate positioning
                            let mut byte_offset = 0;
                            for run in &line_runs {
                                if let Some(bg_color) = run.background_color {
                                    // Calculate the x positions using the shaped line
                                    let start_x = shaped_line.x_for_index(byte_offset);
                                    let end_x = shaped_line.x_for_index(byte_offset + run.len);

                                    let bg_bounds = Bounds {
                                        origin: point(text_origin_x + start_x, line_y),
                                        size: size(end_x - start_x, after_layout.line_height),
                                    };
                                    window.paint_quad(fill(bg_bounds, bg_color));
                                }
                                byte_offset += run.len;
                            }

                            if let Err(e) = shaped_line.paint(point(text_origin_x, line_y), after_layout.line_height, window, cx) {
                                error!(error = ?e, "Failed to paint text");
                            }

                            // Store line layout for mouse interaction
                            // FIXED: Store in text-area coordinates (gutter excluded)
                            // Use y_offset directly to match coordinate system used by mouse handler
                            let text_area_origin = point(
                                px(0.0), // Line starts at x=0 in text-area coordinates
                                y_offset, // Use y_offset directly (no px(1.) like non-wrap mode)
                            );
                            debug!(
                                line_idx = current_doc_line,
                                visual_line = visual_line,
                                global_text_origin_x = %text_origin_x,
                                global_line_y = %line_y,
                                y_offset = %y_offset,
                                element_bounds_origin = ?bounds.origin,
                                fixed_text_area_origin = ?text_area_origin,
                                shaped_line_width = %shaped_line.width,
                                "🎯 FIXED Line layout storage (soft-wrap) - Using y_offset directly to match mouse handler"
                            );
                            let layout = nucleotide_editor::LineLayout {
                                line_idx: current_doc_line,
                                shaped_line,
                                origin: text_area_origin,
                            };
                            line_cache.push(layout);
                        }

                        // Update document line based on the first grapheme of this visual line
                        if let Some(first_grapheme) = line_graphemes.first() {
                            current_doc_line = first_grapheme.line_idx;
                        }

                        // Always move to the next visual line
                        // We should never skip visual lines, even if they're empty
                        visual_line += 1;
                        y_offset += after_layout.line_height;
                    }

                    // Render gutter for soft wrap mode
                    // We need a different approach for the gutter - we'll track actual document lines
                    // and match them with visual lines
                    {
                        let mut gutter_origin = bounds.origin;
                        gutter_origin.x += px(2.);
                        gutter_origin.y += px(1.);

                        // Calculate viewport dimensions
                        let _viewport_height_in_lines = (bounds.size.height / after_layout.line_height) as usize;

                        // We need to figure out the mapping between document lines and visual lines
                        // For now, let's use a simpler approach: render line numbers based on
                        // the actual document lines we rendered in the main loop

                        // Track which document lines we've seen and at what visual positions
                        let mut doc_line_positions = Vec::new();
                        let mut current_y = px(0.0);

                        // Re-create formatter to match what we did in the main rendering loop
                        let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
                            text,
                            &text_format,
                            &annotations,
                            view_offset.anchor,
                        );

                        // Skip to viewport (same as main loop)
                        let mut visual_line = 0;
                        let mut pending_grapheme = None;
                        while visual_line < view_offset.vertical_offset {
                            for grapheme in formatter.by_ref() {
                                if grapheme.visual_pos.row > visual_line {
                                    visual_line = grapheme.visual_pos.row;
                                    if visual_line >= view_offset.vertical_offset {
                                        pending_grapheme = Some(grapheme);
                                        break;
                                    }
                                }
                            }
                            if visual_line < view_offset.vertical_offset {
                                visual_line += 1;
                            }
                        }

                        // Track document lines as we iterate through visual lines
                        let mut last_doc_line = None;
                        while visual_line < view_offset.vertical_offset + viewport_height {
                            let mut line_graphemes = Vec::new();

                            // Add pending grapheme
                            if let Some(grapheme) = pending_grapheme.take() {
                                if grapheme.visual_pos.row == visual_line {
                                    line_graphemes.push(grapheme);
                                } else if grapheme.visual_pos.row > visual_line {
                                    pending_grapheme = Some(grapheme);
                                }
                            }

                            // Collect graphemes for this visual line
                            for grapheme in formatter.by_ref() {
                                if grapheme.visual_pos.row > visual_line {
                                    pending_grapheme = Some(grapheme);
                                    break;
                                } else if grapheme.visual_pos.row == visual_line {
                                    line_graphemes.push(grapheme);
                                }
                            }

                            // Determine the document line for this visual line
                            if let Some(first_grapheme) = line_graphemes.first() {
                                let doc_line = first_grapheme.line_idx;
                                // Only add line number for the first visual line of each document line
                                if last_doc_line != Some(doc_line) {
                                    doc_line_positions.push((doc_line, current_y));
                                    last_doc_line = Some(doc_line);
                                }
                            } else if line_graphemes.is_empty() && pending_grapheme.is_none() {
                                // End of document
                                break;
                            } else if line_graphemes.is_empty() {
                                // Empty line - we need to figure out its document line number
                                // This is tricky because DocumentFormatter doesn't give us empty lines
                                // For now, increment the line number if we have a gap
                                if let Some(last) = last_doc_line {
                                    let next_line = last + 1;
                                    if next_line < text.len_lines() {
                                        doc_line_positions.push((next_line, current_y));
                                        last_doc_line = Some(next_line);
                                    }
                                }
                            }

                            visual_line += 1;
                            current_y += after_layout.line_height;
                        }

                        // Now render the line numbers
                        let theme = cx.global::<crate::ThemeManager>().helix_theme();
                        let style = theme.get("ui.linenr");
                        let color = style.fg.and_then(color_to_hsla).unwrap_or(hsla(0.5, 0., 0.5, 1.));

                        for (doc_line, y_pos) in doc_line_positions {
                            let line_num_str = format!("{:>4} ", doc_line + 1);
                            let y = gutter_origin.y + y_pos;

                            let run = TextRun {
                                len: line_num_str.len(),
                                font: self.style.font(),
                                color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };

                            let shaped = window.text_system()
                                .shape_line(line_num_str.into(), self.style.font_size.to_pixels(px(16.0)), &[run], None);

                            let _ = shaped.paint(point(gutter_origin.x, y), after_layout.line_height, window, cx);
                        }
                    }

                    // Render cursor for soft wrap mode
                    let element_focused = self.focus.is_focused(window);
                    if self.is_focused || element_focused {
                        // Get cursor position and text under cursor for block mode
                        let (cursor_char_idx, cursor_style, cursor_kind, cursor_text, text_style_at_cursor) = {
                            let core = self.core.read(cx);
                            let editor = &core.editor;
                            if let Some(document) = editor.document(self.doc_id) {
                                let selection = document.selection(self.view_id);
                                let cursor_char_idx = selection.primary().cursor(text);
                                let (_, cursor_kind) = editor.cursor();

                                // Get the character under the cursor for block cursor mode
                                let cursor_text = if matches!(cursor_kind, CursorKind::Block) && self.is_focused {
                                    if cursor_char_idx < text.len_chars() {
                                        let grapheme_end = next_grapheme_boundary(text, cursor_char_idx);
                                        let char_slice = text.slice(cursor_char_idx..grapheme_end);
                                        let char_str: SharedString = RopeWrapper(char_slice).into();

                                        // Replace newlines with visible character for display
                                        let char_str = if char_str == "\n" || char_str == "\r\n" || char_str == "\r" {
                                            "⏎".into()
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

                                // Get cursor style
                                let theme = cx.global::<crate::ThemeManager>().helix_theme();
                                let mode = editor.mode();
                                let base_cursor_style = theme.get("ui.cursor");
                                let base_primary_cursor_style = theme.get("ui.cursor.primary");
                                // Important: we need to patch styles to combine colors with modifiers
                                let cursor_style = match mode {
                                    helix_view::document::Mode::Insert => {
                                        let style = theme.get("ui.cursor.primary.insert");
                                        if style.fg.is_some() || style.bg.is_some() {
                                            // Patch with base cursor to get modifiers
                                            base_cursor_style.patch(style)
                                        } else {
                                            base_cursor_style.patch(base_primary_cursor_style)
                                        }
                                    }
                                    helix_view::document::Mode::Select => {
                                        let style = theme.get("ui.cursor.primary.select");
                                        if style.fg.is_some() || style.bg.is_some() {
                                            // Patch with base cursor to get modifiers
                                            base_cursor_style.patch(style)
                                        } else {
                                            base_cursor_style.patch(base_primary_cursor_style)
                                        }
                                    }
                                    helix_view::document::Mode::Normal => {
                                        let style = theme.get("ui.cursor.primary.normal");
                                        if style.fg.is_some() || style.bg.is_some() {
                                            // Patch with base cursor to get modifiers
                                            base_cursor_style.patch(style)
                                        } else {
                                            base_cursor_style.patch(base_primary_cursor_style)
                                        }
                                    }
                                };

                                // Get text style at cursor for reversed modifier
                                let text_style_at_cursor = Self::get_text_style_at_position(
                                    document,
                                    self.view_id,
                                    theme,
                                    &editor.syn_loader,
                                    cursor_char_idx,
                                );

                                (cursor_char_idx, cursor_style, cursor_kind, cursor_text, text_style_at_cursor)
                            } else {
                                return;
                            }
                        };

                        // Find the cursor position in the visual lines
                        // We need to re-create the formatter to find the cursor
                        let formatter = DocumentFormatter::new_at_prev_checkpoint(
                            text,
                            &text_format,
                            &annotations,
                            view_offset.anchor,
                        );

                        let mut _visual_line = 0;
                        let mut cursor_visual_line = None;
                        let mut cursor_visual_col = 0;

                        // Iterate through graphemes to find cursor position (following Helix's approach)
                        for grapheme in formatter {
                            // Check if the cursor position is before the next grapheme
                            // This matches Helix's logic: formatter.next_char_pos() > pos
                            let next_char_pos = grapheme.char_idx + grapheme.doc_chars();
                            if next_char_pos > cursor_char_idx {
                                // Cursor is at this grapheme's visual position
                                // The DocumentFormatter already accounts for wrap indicators in visual_pos.col
                                cursor_visual_line = Some(grapheme.visual_pos.row);
                                cursor_visual_col = grapheme.visual_pos.col;
                                break;
                            }
                            _visual_line = grapheme.visual_pos.row;
                        }

                        // If cursor is in viewport, render it
                        if let Some(cursor_line) = cursor_visual_line {
                            if cursor_line >= view_offset.vertical_offset &&
                               cursor_line < view_offset.vertical_offset + viewport_height {
                                // Calculate cursor position - FIXED: Use text_bounds coordinate system to match mouse clicks
                                // Get text bounds (excluding gutter) to match mouse coordinate system
                                // Use existing gutter_width from outer scope instead of calling view.gutter_offset(document)
                                let gutter_offset = gutter_width;
                                let text_bounds = {
                                    let gutter_width = Pixels::from(gutter_offset as f32 * after_layout.cell_width.0);
                                    let right_padding = after_layout.cell_width * 2.0;
                                    let top_padding = px(1.0);

                                    gpui::Bounds {
                                        origin: gpui::Point {
                                            x: bounds.origin.x + gutter_width,
                                            y: bounds.origin.y + top_padding,
                                        },
                                        size: gpui::Size {
                                            width: bounds.size.width - gutter_width - right_padding,
                                            height: bounds.size.height - top_padding,
                                        },
                                    }
                                };

                                let relative_line = cursor_line - view_offset.vertical_offset;
                                let cursor_y = text_bounds.origin.y + (after_layout.line_height * relative_line as f32);
                                let cursor_x = text_bounds.origin.x + (after_layout.cell_width * cursor_visual_col as f32);

                                // Check if cursor has reversed modifier
                                let has_reversed = cursor_style.add_modifier.contains(helix_view::graphics::Modifier::REVERSED) &&
                                                   !cursor_style.sub_modifier.contains(helix_view::graphics::Modifier::REVERSED);

                                let cursor_color = if has_reversed {
                                    // For reversed cursor: use the text color at cursor position as cursor background
                                    // Use the pre-calculated text style
                                    text_style_at_cursor.fg.and_then(color_to_hsla)
                                        .unwrap_or(fg_color)
                                } else {
                                    // Normal cursor: use cursor's background color
                                    cursor_style.bg.and_then(color_to_hsla)
                                        .or_else(|| cursor_style.fg.and_then(color_to_hsla))
                                        .unwrap_or(fg_color)
                                };

                                // Shape cursor text if available and calculate its width
                                let (cursor_text_shaped, cursor_text_len) = cursor_text.map(|char_str| {
                                    let text_len = char_str.len();

                                    // For block cursor, text should contrast with cursor background
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

                                    let run = TextRun {
                                        len: text_len,
                                        font: self.style.font(),
                                        color: text_color,
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    };

                                    let shaped = window.text_system()
                                        .shape_line(char_str, self.style.font_size.to_pixels(px(16.0)), &[run], None);
                                    (Some(shaped), text_len)
                                }).unwrap_or((None, 0));

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
                                let mut cursor = Cursor {
                                    origin: point(px(0.0), px(0.0)),  // No offset needed, will be applied in paint
                                    kind: cursor_kind,
                                    color: cursor_color,
                                    block_width: cursor_width,
                                    line_height: after_layout.line_height,
                                    text: cursor_text_shaped,
                                };

                                cursor.paint(point(cursor_x, cursor_y), window, cx);
                            }
                        }
                    }

                    // Skip the regular rendering loop when soft wrap is enabled
                    return;
                }

                // Original rendering loop (without soft wrap)
                for (loop_index, line_idx) in (first_row..last_row).enumerate() {
                    debug!("Rendering line {line_idx} (loop index {loop_index}), y_offset: {y_offset:?}");
                    // Handle phantom line (empty line at EOF when file ends with newline)
                    // For a file ending with \n, the last line is empty and is the phantom line
                    let is_phantom_line = cursor_at_end && file_ends_with_newline && line_idx == total_lines - 1;

                    let (line_start, line_end) = if is_phantom_line {
                        // Phantom line is empty, positioned at end of file
                        debug!("Rendering phantom line at index {line_idx}");
                        (text.len_chars(), text.len_chars())
                    } else {
                        let line_start = text.line_to_char(line_idx);
                        let line_end = if line_idx + 1 < total_lines {
                            text.line_to_char(line_idx + 1).saturating_sub(1) // Exclude newline
                        } else {
                            text.len_chars()
                        };
                        (line_start, line_end)
                    };

                    // Skip lines outside our view
                    // For phantom line, we need special handling since line_start == end_char == text.len_chars()
                    let anchor_char = text.line_to_char(first_row);
                    if !is_phantom_line && (line_start >= end_char || line_end < anchor_char) {
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

                    let (line_str, line_runs) = if is_phantom_line {
                        // Phantom line is always empty with no highlights
                        (SharedString::from(""), Vec::new())
                    } else {
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

                        // Get highlights for this specific line using the extracted values
                        // Re-read core for this iteration
                        let core = self.core.read(cx);
                        let editor = &core.editor;
                        let document = match editor.document(doc_id) {
                            Some(doc) => doc,
                            None => return,
                        };
                        let view = match editor.tree.try_get(view_id) {
                            Some(v) => v,
                            None => return,
                        };

                        let line_runs = Self::highlight_line_with_params(HighlightLineParams {
                            doc: document,
                            view,
                            theme: &editor_theme,
                            editor_mode,
                            cursor_shape: &cursor_shape,
                            syn_loader: &syn_loader,
                            is_view_focused: self.is_focused,
                            line_start,
                            line_end,
                            fg_color,
                            font: self.style.font(),
                        });

                        (line_str, line_runs)
                    };

                    // Drop core before painting
                    // core goes out of scope here

                    let text_origin = point(text_origin_x, bounds.origin.y + px(1.) + y_offset);
                    // Paint cursorline background if this is the cursor's line
                    if let Some(cursorline_bg) = cursorline_style {
                        if line_idx == cursor_line_num || (is_phantom_line && cursor_at_end && file_ends_with_newline) {
                            debug!("Painting cursorline for line {} (cursor at line {})", line_idx, cursor_line_num);
                            let cursorline_bounds = Bounds {
                                origin: point(bounds.origin.x, bounds.origin.y + px(1.) + y_offset),
                                size: size(bounds.size.width, after_layout.line_height),
                            };
                            window.paint_quad(fill(cursorline_bounds, cursorline_bg));
                        }
                    }

                    // Always create a shaped line, even for empty lines (needed for cursor positioning)
                    let shaped_line = if !line_str.is_empty() {
                        // Try to get cached shaped line first
                        let cache_key = nucleotide_editor::ShapedLineKey {
                            line_text: line_str.to_string(),
                            font_size: self.style.font_size.to_pixels(px(16.0)).0 as u32,
                            viewport_width: bounds.size.width.0 as u32,
                        };

                        let shaped = if let Some(cached) = line_cache.get_shaped_line(&cache_key) {
                            cached
                        } else {
                            // Shape and cache the line
                            let shaped = window.text_system()
                                .shape_line(line_str.clone(), self.style.font_size.to_pixels(px(16.0)), &line_runs, None);
                            line_cache.store_shaped_line(cache_key, shaped.clone());
                            shaped
                        };

                        // Paint background highlights using the shaped line for accurate positioning
                        let mut byte_offset = 0;
                        for run in &line_runs {
                            if let Some(bg_color) = run.background_color {
                                // Calculate the x positions using the shaped line
                                let start_x = shaped.x_for_index(byte_offset);
                                let end_x = shaped.x_for_index(byte_offset + run.len);

                                let bg_bounds = Bounds {
                                    origin: point(text_origin.x + start_x, text_origin.y),
                                    size: size(end_x - start_x, after_layout.line_height),
                                };
                                window.paint_quad(fill(bg_bounds, bg_color));
                            }
                            byte_offset += run.len;
                        }

                        if let Err(e) = shaped.paint(text_origin, after_layout.line_height, window, cx) {
                            error!(error = ?e, "Failed to paint text");
                        }
                        shaped
                    } else {
                        // Create an empty shaped line for cursor positioning
                        let cache_key = nucleotide_editor::ShapedLineKey {
                            line_text: String::new(),
                            font_size: self.style.font_size.to_pixels(px(16.0)).0 as u32,
                            viewport_width: bounds.size.width.0 as u32,
                        };

                        if let Some(cached) = line_cache.get_shaped_line(&cache_key) {
                            cached
                        } else {
                            let shaped = window.text_system()
                                .shape_line("".into(), self.style.font_size.to_pixels(px(16.0)), &[], None);
                            line_cache.store_shaped_line(cache_key, shaped.clone());
                            shaped
                        }
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
                        px(0.0), // Line starts at x=0 in text-area coordinates
                        y_offset, // Use y_offset directly (without the px(1.) padding)
                    );
                    debug!(
                        line_idx = line_idx,
                        global_origin = ?text_origin,
                        bounds_origin = ?bounds.origin,
                        y_offset = %y_offset,
                        fixed_text_area_origin = ?text_area_origin,
                        shaped_line_width = %shaped_line.width,
                        "🎯 FIXED Line layout storage (no-wrap) - Using y_offset directly (no px(1.) double-add)"
                    );
                    let layout = nucleotide_editor::LineLayout {
                        line_idx,
                        shaped_line,
                        origin: text_area_origin,
                    };

                    // Debug: log phantom line layout creation
                    if is_phantom_line {
                        debug!("Created phantom line layout - line_idx: {}, origin.y: {:?}, y_offset: {:?}",
                            line_idx, text_origin.y, y_offset);
                    }

                    line_cache.push(layout);

                    y_offset += after_layout.line_height;
                }

                // draw cursor
                let element_focused = self.focus.is_focused(window);
                debug!("Cursor rendering check - is_focused: {}, element_focused: {}, cursor_pos: {:?}",
                    self.is_focused, element_focused, cursor_pos);

                // Debug: Log cursor position info
                {
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    if let Some(doc) = editor.document(self.doc_id) {
                        if let Some(_view) = editor.tree.try_get(self.view_id) {
                            let sel = doc.selection(self.view_id);
                            let cursor_char = sel.primary().cursor(text);
                            debug!("Cursor char idx: {}, line: {}, selection: {:?}",
                                cursor_char, text.char_to_line(cursor_char), sel);
                        }
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
                        let helix_core::Position { row: viewport_row, col: _ } = position;

                        // Get the line containing the cursor
                        // When cursor is at EOF with newline, the phantom line is the last line
                        let cursor_line = if cursor_at_end && file_ends_with_newline {
                            total_lines - 1  // Phantom line is the last line
                        } else {
                            text.char_to_line(cursor_char_idx)
                        };

                        debug!("Looking for cursor line {cursor_line} in range {first_row}..{last_row}");

                        // Check if cursor line is in the rendered range
                        // For phantom line, use the effective cursor line
                        let effective_cursor_line = cursor_line;

                        if effective_cursor_line >= first_row && effective_cursor_line < last_row {
                            // Debug: line layouts are now stored in LineLayoutCache

                            // Use the cursor line directly as the layout index
                            let layout_line_idx = cursor_line;

                            debug!("Looking for line layout with index {} (cursor_line: {}, is phantom: {})",
                                layout_line_idx, cursor_line, cursor_at_end && file_ends_with_newline);

                            // Find the line layout for the cursor line
                            if let Some(line_layout) = line_cache.find_line_by_index(layout_line_idx) {
                                debug!("Found line layout - line_idx: {}, origin.y: {:?}, expected line: {}",
                                    line_layout.line_idx, line_layout.origin.y, layout_line_idx);

                                // Additional debug for phantom line
                                if cursor_at_end && file_ends_with_newline {
                                    // Line layouts are now stored in LineLayoutCache
                                }
                                // Special handling for phantom line
                                let is_phantom_line = layout_line_idx >= text.len_lines();

                                let (_line_start, cursor_char_offset, cursor_byte_offset, line_text) = if is_phantom_line {
                                    // Phantom line: cursor is at end of file
                                    (text.len_chars(), 0, 0, String::new())
                                } else {
                                    // Normal line
                                    let line_start = text.line_to_char(cursor_line);
                                    let cursor_char_offset = cursor_char_idx.saturating_sub(line_start);

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
                                    let cursor_char_offset = cursor_char_offset.min(line_text.chars().count());

                                    // Convert char offset to byte offset for GPUI's x_for_index
                                    let cursor_byte_offset = line_text.chars().take(cursor_char_offset).map(char::len_utf8).sum::<usize>();

                                    (line_start, cursor_char_offset, cursor_byte_offset, line_text)
                                };

                                // Get the x position from the shaped line using byte offset
                                let cursor_x_relative_to_line = line_layout.shaped_line.x_for_index(cursor_byte_offset);

                                // Additional debug for x_for_index calculation
                                debug!(
                                    cursor_char_offset = cursor_char_offset,
                                    cursor_byte_offset = cursor_byte_offset,
                                    line_text_len = line_text.len(),
                                    line_text_preview = ?&line_text.chars().take(20).collect::<String>(),
                                    cursor_x_from_x_for_index = %cursor_x_relative_to_line,
                                    shaped_line_width = %line_layout.shaped_line.width,
                                    "🎯 X_FOR_INDEX DEBUG: Investigating cursor X position calculation"
                                );

                                // FIXED: Convert from line-relative coordinates to text-area coordinates
                                // Line layouts are stored in text-area coordinates (x=0), so we need to add text bounds offset
                                // Use existing values from the outer scope (editor, document, view, gutter_width are already available)
                                let cell_width = after_layout.cell_width; // Use actual calculated cell width, not hardcoded
                                let gutter_offset_u16 = gutter_width;
                                let element_bounds = bounds;

                                // Calculate text bounds (same as mouse coordinate system)
                                let text_bounds = {
                                    let gutter_width = Pixels::from(gutter_offset_u16 as f32 * cell_width.0);
                                    let right_padding = cell_width * 2.0;
                                    let top_padding = px(1.0);

                                    gpui::Bounds {
                                        origin: gpui::Point {
                                            x: element_bounds.origin.x + gutter_width,
                                            y: element_bounds.origin.y + top_padding,
                                        },
                                        size: gpui::Size {
                                            width: element_bounds.size.width - gutter_width - right_padding,
                                            height: element_bounds.size.height - top_padding,
                                        },
                                    }
                                };

                                // Convert to absolute coordinates by adding text bounds origin
                                let cursor_x = text_bounds.origin.x + cursor_x_relative_to_line;

                                // Debug logging
                                debug!("Cursor rendering - line: {cursor_line}, char_offset: {cursor_char_offset}, byte_offset: {cursor_byte_offset}, x_relative: {cursor_x_relative_to_line:?}, x_absolute: {cursor_x:?}, viewport_row: {viewport_row}");

                                // Debug info about the line content
                                debug!("Line content: {:?}, cursor at char offset {} (byte offset {}), is_phantom: {}",
                                    &line_text, cursor_char_offset, cursor_byte_offset, is_phantom_line);

                                // Additional debug for emoji detection
                                if !line_text.is_empty() {
                                    use unicode_segmentation::UnicodeSegmentation;
                                    let chars: Vec<char> = line_text.chars().collect();
                                    debug!("Line has {} chars, {} bytes, {} graphemes",
                                        chars.len(),
                                        line_text.len(),
                                        line_text.graphemes(true).count());
                                    if cursor_char_offset < chars.len() {
                                        let ch = chars[cursor_char_offset];
                                        debug!("Char at cursor offset {}: {:?} (U+{:04X})",
                                            cursor_char_offset, ch, ch as u32);
                                    }
                                }

                                // Calculate cursor position RELATIVE to line origin (for paint() method)
                                // cursor.paint() adds the line_layout.origin, so cursor_origin should be relative
                                let relative_cursor_x = cursor_x_relative_to_line; // Already relative to line
                                let relative_cursor_y = px(0.0); // Relative to line origin
                                let cursor_origin = gpui::Point::new(
                                    relative_cursor_x, // Relative X coordinate
                                    relative_cursor_y // Relative Y coordinate (line-relative)
                                );

                                debug!(
                                    cursor_line = cursor_line,
                                    cursor_x_relative_to_line = %cursor_x_relative_to_line,
                                    cursor_x_absolute = %cursor_x,
                                    text_bounds_origin_x = %text_bounds.origin.x,
                                    line_layout_origin = ?line_layout.origin,
                                    cursor_origin = ?cursor_origin,
                                    will_paint_at = ?(cursor_origin.x + line_layout.origin.x, cursor_origin.y + line_layout.origin.y),
                                    "🎯 CURSOR DEBUG: Final cursor positioning calculation"
                                );

                                // Check if cursor has reversed modifier
                                let has_reversed = cursor_style.add_modifier.contains(helix_view::graphics::Modifier::REVERSED) &&
                                                   !cursor_style.sub_modifier.contains(helix_view::graphics::Modifier::REVERSED);

                                // For reversed cursor, we need to get the text style at cursor position
                                let cursor_color = if has_reversed {
                                    // Get the styled text color at cursor position
                                    // We need to access core again to get the document
                                    let text_style_at_cursor = {
                                        let core = self.core.read(cx);
                                        let editor = &core.editor;
                                        let theme = cx.global::<crate::ThemeManager>().helix_theme();
                                        if let Some(doc) = editor.document(self.doc_id) {
                                            Self::get_text_style_at_position(
                                                doc,
                                                self.view_id,
                                                theme,
                                                &editor.syn_loader,
                                                cursor_char_idx,
                                            )
                                        } else {
                                            // Default style if document not found
                                            helix_view::graphics::Style::default()
                                        }
                                    };

                                    // Use the text's foreground color as cursor background
                                    text_style_at_cursor.fg.and_then(color_to_hsla)
                                        .unwrap_or(fg_color)
                                } else {
                                    // Normal cursor: use cursor's background color
                                    cursor_style.bg.and_then(color_to_hsla)
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

                                let mut cursor = Cursor {
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
                                debug!(
                                    text_bounds_origin = ?text_bounds.origin,
                                    line_layout_origin = ?line_layout.origin,
                                    absolute_cursor_position = ?absolute_cursor_position,
                                    "🎯 CURSOR PAINT: Converting text-area coordinates to absolute window coordinates"
                                );
                                cursor.paint(absolute_cursor_position, window, cx);
                            } else {
                                debug!("Warning: Could not find line layout for cursor line {cursor_line}");
                            }
                        } else {
                            debug!("Cursor line {cursor_line} is outside rendered range {first_row}..{last_row}");
                        }
                    } else {
                        debug!("Cursor rendering skipped - no cursor_pos from screen_coords_at_pos");
                    }
                } else {
                    debug!("Cursor rendering skipped - is_focused: {}, element_focused: {}",
                        self.is_focused, element_focused);
                }
                // draw gutter
                {
                    let mut gutter_origin = bounds.origin;
                    gutter_origin.x += px(2.);
                    gutter_origin.y += px(1.);

                    // Re-read core for gutter rendering
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    let theme = cx.global::<crate::ThemeManager>().helix_theme();
                    let view = match editor.tree.try_get(self.view_id) {
                        Some(v) => v,
                        None => return,
                    };
                    let document = match editor.document(self.doc_id) {
                    Some(doc) => doc,
                    None => return,
                };

                    // Clone necessary values before creating mutable references
                    let text_system = window.text_system().clone();
                    let style = self.style.clone();

                    // Only pass actual document lines to gutter, not phantom lines
                    let gutter_last_row = last_row.min(total_lines);

                    // SOFTWRAP HANDLING: Generate LinePos entries for each visual line
                    // When softwrap is enabled, long document lines can span multiple visual lines
                    let mut lines = Vec::new();
                    let mut current_visual_line = 0u16;

                    for doc_line in first_row..gutter_last_row {
                        // For now, assume each document line maps to one visual line (no softwrap)
                        // TODO: Integrate with Helix's softwrap calculation when available
                        // This would require checking document.softwrap settings and calculating
                        // how many visual lines each document line spans based on line width
                        lines.push(LinePos {
                            first_visual_line: true, // Always true since we're not handling softwrap yet
                            doc_line,
                            visual_line: current_visual_line,
                            start_char_idx: 0, // Start of the document line
                        });
                        current_visual_line += 1;
                    }

                    let lines = lines.into_iter();

                    let mut gutter = Gutter {
                        after_layout,
                        text_system,
                        lines: Vec::new(),
                        style,
                        origin: gutter_origin,
                    };

                    let mut gutters = Vec::new();
                    Gutter::init_gutter(
                        editor,
                        document,
                        view,
                        theme,
                        is_focused,
                        &mut gutters,
                    );

                    // Execute gutters while we still have the borrow
                    for line in lines {
                        for gut in &mut gutters {
                            gut(line, &mut gutter)
                        }
                    }

                    // Drop gutters first (contains references to core)
                    drop(gutters);

                    // Drop core borrow before painting
                    // core goes out of scope here

                    // Now paint the gutter lines
                    for (origin, line) in gutter.lines {
                        if let Err(e) = line.paint(origin, after_layout.line_height, window, cx) {
                            error!(error = ?e, "Failed to paint gutter line");
                        }
                    }
                }
            });

        // CRITICAL: Paint the interactivity to enable mouse event handling
        self.interactivity.paint(
            _global_id,
            _inspector_id,
            bounds,
            after_layout.hitbox.as_ref(), // Use actual hitbox for proper hit-testing
            window,
            cx,
            |_style, _window, _cx| {
                // The interactivity system will handle mouse events automatically
                // based on the handlers set up during prepaint
            },
        );
    }
}

struct Gutter<'a> {
    after_layout: &'a DocumentLayout,
    text_system: std::sync::Arc<WindowTextSystem>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    style: TextStyle,
    origin: Point<Pixels>,
}

impl<'a> Gutter<'a> {
    fn init_gutter<'d>(
        editor: &'d Editor,
        doc: &'d Document,
        view: &'d View,
        theme: &Theme,
        is_focused: bool,
        gutters: &mut Vec<GutterDecoration<'d, Self>>,
    ) {
        let text = doc.text().slice(..);
        let cursors: std::rc::Rc<[_]> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let mut offset = 0;

        let gutter_style = theme.get("ui.gutter");
        let gutter_selected_style = theme.get("ui.gutter.selected");
        let gutter_style_virtual = theme.get("ui.gutter.virtual");
        let gutter_selected_style_virtual = theme.get("ui.gutter.selected.virtual");

        for gutter_type in view.gutters() {
            let mut gutter = gutter_type.style(editor, doc, view, theme, is_focused);
            let width = gutter_type.width(view, doc);
            // avoid lots of small allocations by reusing a text buffer for each line
            let mut text = String::with_capacity(width);
            let cursors = cursors.clone();
            let gutter_decoration = move |pos: LinePos, renderer: &mut Self| {
                // SOFTWRAP GUTTER HANDLING:
                // Currently assumes each document line = one visual line
                // When true softwrap is implemented, this needs to:
                // 1. Show line numbers only on first visual line of wrapped lines
                // 2. Show appropriate wrap indicators on continuation lines
                // 3. Handle gutter width for wrapped line numbers
                let selected = cursors.contains(&pos.doc_line);
                let x = offset;
                let y = pos.visual_line;

                let gutter_style = match (selected, pos.first_visual_line) {
                    (false, true) => gutter_style,
                    (true, true) => gutter_selected_style,
                    (false, false) => gutter_style_virtual,
                    (true, false) => gutter_selected_style_virtual,
                };

                if let Some(style) =
                    gutter(pos.doc_line, selected, pos.first_visual_line, &mut text)
                {
                    renderer.render(x, y, width, gutter_style.patch(style), Some(&text));
                } else {
                    renderer.render(x, y, width, gutter_style, None);
                }
                text.clear();
            };
            gutters.push(Box::new(gutter_decoration));

            offset += width as u16;
        }
    }
}

impl<'a> GutterRenderer for Gutter<'a> {
    fn render(
        &mut self,
        x: u16,
        y: u16,
        _width: usize,
        style: helix_view::graphics::Style,
        text: Option<&str>,
    ) {
        let origin_y = self.origin.y + self.after_layout.line_height * f32::from(y);
        let origin_x = self.origin.x + self.after_layout.cell_width * f32::from(x);

        let base_fg = style
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0., 0., 1., 1.));
        let base_bg = style.bg.and_then(color_to_hsla);

        if let Some(text) = text {
            // Apply modifiers to font and colors
            let font = apply_font_modifiers(&self.style.font(), &style);
            let default_bg = black(); // Default background for gutters
            let (fg_color, bg_color) = apply_color_modifiers(base_fg, base_bg, &style, default_bg);

            let run = create_styled_text_run(
                text.len(),
                &font,
                &style,
                fg_color,
                bg_color,
                default_bg,
                None, // No underline for gutter text
            );
            let shaped = self.text_system.shape_line(
                text.to_string().into(),
                self.after_layout.font_size,
                &[run],
                None,
            );
            self.lines.push((
                Point {
                    x: origin_x,
                    y: origin_y,
                },
                shaped,
            ));
        }
    }
}

struct Cursor {
    origin: gpui::Point<Pixels>,
    kind: CursorKind,
    color: Hsla,
    block_width: Pixels,
    line_height: Pixels,
    text: Option<ShapedLine>,
}

impl Cursor {
    fn bounds(&self, origin: gpui::Point<Pixels>) -> Bounds<Pixels> {
        match self.kind {
            CursorKind::Bar => Bounds {
                origin: self.origin + origin,
                size: size(px(2.0), self.line_height),
            },
            CursorKind::Block => Bounds {
                origin: self.origin + origin,
                size: size(self.block_width, self.line_height),
            },
            CursorKind::Underline => Bounds {
                origin: self.origin
                    + origin
                    + gpui::Point::new(Pixels::ZERO, self.line_height - px(2.0)),
                size: size(self.block_width, px(2.0)),
            },
            CursorKind::Hidden => todo!(),
        }
    }

    pub fn paint(&mut self, origin: gpui::Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds(origin);

        // Paint the cursor quad first
        window.paint_quad(fill(bounds, self.color));

        // Then paint text on top of the cursor block
        if let Some(text) = &self.text {
            // For block cursor, text should be painted at the cursor position
            // Use the bounds origin to ensure text aligns with the cursor block
            let text_origin = bounds.origin;
            if let Err(e) = text.paint(text_origin, self.line_height, window, cx) {
                error!(error = ?e, "Failed to paint cursor text");
            }
        }
    }
}

type GutterDecoration<'a, T> = Box<dyn FnMut(LinePos, &mut T) + 'a>;

trait GutterRenderer {
    fn render(
        &mut self,
        x: u16,
        y: u16,
        width: usize,
        style: helix_view::graphics::Style,
        text: Option<&str>,
    );
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
struct LinePos {
    /// Indicates whether the given visual line
    /// is the first visual line of the given document line
    pub first_visual_line: bool,
    /// The line index of the document line that contains the given visual line
    pub doc_line: usize,
    /// Vertical offset from the top of the inner view area
    pub visual_line: u16,
    /// The first char index of this visual line.
    /// Note that if the visual line is entirely filled by
    /// a very long inline virtual text then this index will point
    /// at the next (non-virtual) char after this visual line
    pub start_char_idx: usize,
}

// Syntax highlighting support based on helix-term implementation

/// Safe wrapper for theme.highlight() that handles out of bounds access
fn safe_highlight(theme: &Theme, highlight: syntax::Highlight) -> helix_view::graphics::Style {
    // The theme.highlight() method can panic if the highlight index is out of bounds
    // This can happen when syntax highlighting returns indices for highlights that
    // don't exist in the current theme. We handle this gracefully by returning
    // a default style instead of panicking.
    use std::panic::{catch_unwind, AssertUnwindSafe};

    match catch_unwind(AssertUnwindSafe(|| theme.highlight(highlight))) {
        Ok(style) => style,
        Err(_) => helix_view::graphics::Style::default(),
    }
}

struct SyntaxHighlighter<'h, 'r, 't> {
    inner: Option<syntax::Highlighter<'h>>,
    text: RopeSlice<'r>,
    /// The character index of the next highlight event, or `usize::MAX` if the highlighter is
    /// finished.
    pos: usize,
    theme: &'t Theme,
    text_style: helix_view::graphics::Style,
    style: helix_view::graphics::Style,
}

impl<'h, 'r, 't> SyntaxHighlighter<'h, 'r, 't> {
    fn new(
        inner: Option<syntax::Highlighter<'h>>,
        text: RopeSlice<'r>,
        theme: &'t Theme,
        text_style: helix_view::graphics::Style,
    ) -> Self {
        let mut highlighter = Self {
            inner,
            text,
            pos: 0,
            theme,
            style: text_style,
            text_style,
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self
            .inner
            .as_ref()
            .and_then(|highlighter| {
                let next_byte_idx = highlighter.next_event_offset();
                (next_byte_idx != u32::MAX).then(|| {
                    // Move the byte index to the nearest character boundary (rounding up) and
                    // convert it to a character index.
                    self.text
                        .byte_to_char(self.text.ceil_char_boundary(next_byte_idx as usize))
                })
            })
            .unwrap_or(usize::MAX);
    }

    fn advance(&mut self) {
        let Some(highlighter) = self.inner.as_mut() else {
            return;
        };

        let (event, highlights) = highlighter.advance();
        let base = match event {
            HighlightEvent::Refresh => self.text_style,
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            let highlight_style = safe_highlight(self.theme, highlight);
            let patched = acc.patch(highlight_style);
            if patched != acc {
                debug!(
                    "Applying highlight: {:?} -> style: {:?}",
                    highlight, patched.fg
                );
            }
            patched
        });
        self.update_pos();
    }
}

struct OverlayHighlighter<'t> {
    inner: syntax::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: helix_view::graphics::Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: syntax::OverlayHighlights, theme: &'t Theme) -> Self {
        let inner = syntax::OverlayHighlighter::new(vec![overlays]);
        let mut highlighter = Self {
            inner,
            pos: 0,
            theme,
            style: helix_view::graphics::Style::default(),
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self.inner.next_event_offset();
    }

    fn advance(&mut self) {
        let (event, highlights) = self.inner.advance();
        let base = match event {
            HighlightEvent::Refresh => helix_view::graphics::Style::default(),
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            let highlight_style = safe_highlight(self.theme, highlight);
            acc.patch(highlight_style)
        });
        self.update_pos();
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights
