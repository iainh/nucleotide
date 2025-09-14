use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, DefiniteLength, DismissEvent, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, Font, GlobalElementId, Hitbox, Hsla, InspectorElementId,
    InteractiveElement, Interactivity, IntoElement, LayoutId, ParentElement, Pixels, Point, Render,
    ShapedLine, SharedString, Size, StatefulInteractiveElement, Style, Styled, TextStyle, Window,
    WindowTextSystem, black, div, fill, px, relative, white,
};
use gpui::{TextRun, point, size};
use helix_core::{
    Uri,
    doc_formatter::{DocumentFormatter, TextFormat},
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    ropey::RopeSlice,
    syntax::{self, Highlight, HighlightEvent, OverlayHighlights},
    text_annotations::TextAnnotations,
};
use helix_lsp::lsp::Diagnostic;
// Import helix's syntax highlighting system
use helix_view::{
    Document, DocumentId, Editor, Theme, View, ViewId, graphics::CursorKind, view::ViewPosition,
};
use nucleotide_logging::{debug, error};
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::Core;
use helix_stdx::rope::RopeSliceExt;
use nucleotide_editor::LineLayoutCache;
use nucleotide_editor::ScrollManager;
use nucleotide_ui::scrollbar::{ScrollableHandle, Scrollbar, ScrollbarState};
use nucleotide_ui::style_utils::{
    apply_color_modifiers, apply_font_modifiers, create_styled_text_run,
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
        let char_width_estimate =
            line_layout.shaped_line.width.0 / line_layout.shaped_line.len() as f32;
        let estimated_x = line_layout.origin.x.0 + (target_char_idx as f32 * char_width_estimate);
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
    let test_positions = vec![
        0.0,                        // Start of line
        shaped_line.width.0 * 0.25, // Quarter way
        shaped_line.width.0 * 0.5,  // Middle
        shaped_line.width.0 * 0.75, // Three quarters
        shaped_line.width.0,        // End of line
        shaped_line.width.0 + 10.0, // Beyond end
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
    cx: &'a App,
    editor_mode: helix_view::document::Mode,
    cursor_shape: &'a helix_view::editor::CursorShapeConfig,
    syn_loader: &'a std::sync::Arc<arc_swap::ArcSwap<helix_core::syntax::Loader>>,
    is_view_focused: bool,
    line_start: usize,
    line_end: usize,
    fg_color: Hsla,
    font: Font,
    /// Optional diagnostic overlays to merge (underline ranges)
    diag_overlays: Option<helix_core::syntax::OverlayHighlights>,
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

                            // Delegate scroll to Helix only; do not adjust local scroll immediately
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
    /// Build diagnostic overlay highlights for the entire document.
    /// Uses theme keys diagnostic.error|warning|info|hint to set underline color.
    fn diagnostics_overlays(
        doc: &Document,
        theme: &Theme,
    ) -> Option<helix_core::syntax::OverlayHighlights> {
        let mut spans: Vec<(helix_core::syntax::Highlight, std::ops::Range<usize>)> = Vec::new();
        // Resolve highlight ids once
        let error_h = theme.find_highlight_exact("diagnostic.error");
        let warn_h = theme.find_highlight_exact("diagnostic.warning");
        let info_h = theme.find_highlight_exact("diagnostic.info");
        let hint_h = theme.find_highlight_exact("diagnostic.hint");

        let diagnostics = doc.diagnostics();
        if diagnostics.is_empty() {
            return None;
        }
        for d in diagnostics.iter() {
            let (start, end) = (d.range.start, d.range.end);
            // Skip invalid ranges
            if start >= end {
                // Zero-width or invalid; underline a single char if possible
                let s = start;
                let e = s.saturating_add(1);
                let h = match d.severity {
                    Some(helix_core::diagnostic::Severity::Error) => error_h,
                    Some(helix_core::diagnostic::Severity::Warning) => warn_h,
                    Some(helix_core::diagnostic::Severity::Info) => info_h,
                    Some(helix_core::diagnostic::Severity::Hint) | None => hint_h,
                };
                if let Some(h) = h {
                    spans.push((h, s..e));
                }
            } else {
                let h = match d.severity {
                    Some(helix_core::diagnostic::Severity::Error) => error_h,
                    Some(helix_core::diagnostic::Severity::Warning) => warn_h,
                    Some(helix_core::diagnostic::Severity::Info) => info_h,
                    Some(helix_core::diagnostic::Severity::Hint) | None => hint_h,
                };
                if let Some(h) = h {
                    spans.push((h, start..end));
                }
            }
        }

        if spans.is_empty() {
            None
        } else {
            Some(helix_core::syntax::OverlayHighlights::Heterogenous { highlights: spans })
        }
    }
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

        // Get default text style from theme, but explicitly drop any background.
        // The editor background and row highlights (cursorline/selection) are painted separately.
        const THEME_KEY_UI_TEXT: &str = "ui.text";
        let default_style = theme.get(THEME_KEY_UI_TEXT);
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: None, // avoid overriding cursorline/selection backgrounds
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

    /// Create a TextRun for the cursor glyph that preserves the original text style
    /// (bold/italic/underline/strikethrough) while allowing a custom foreground color
    /// and default background for contrast.
    fn make_cursor_text_run(
        base_font: &gpui::Font,
        text_len: usize,
        text_style_at_cursor: &helix_view::graphics::Style,
        text_color: gpui::Hsla,
        default_bg: gpui::Hsla,
    ) -> gpui::TextRun {
        let underline_color = text_style_at_cursor
            .underline_color
            .and_then(nucleotide_ui::theme_utils::color_to_hsla);
        nucleotide_ui::style_utils::create_styled_text_run(
            text_len,
            base_font,
            text_style_at_cursor,
            text_color,
            None,
            default_bg,
            underline_color,
        )
    }
    /// Convert a byte index within a line to a grapheme index
    /// GPUI's shaped line works with UTF-8 byte indices
    /// but Helix works with grapheme cluster indices (visual units)
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

    /// Convert text-area coordinates to visual coordinates for wrapped mode
    /// This handles the complex case where document lines are wrapped across multiple visual lines
    fn text_area_to_visual_wrapped(
        text_area_pos: Point<Pixels>,
        text_format: &helix_core::doc_formatter::TextFormat,
        view_offset: ViewPosition,
        document: &helix_view::Document,
        cell_width: Pixels,
        line_height: Pixels,
    ) -> Option<Point<Pixels>> {
        use helix_core::{doc_formatter::DocumentFormatter, text_annotations::TextAnnotations};

        let text = document.text().slice(..);
        let annotations = TextAnnotations::default();

        // Convert pixel position to visual row and column
        let visual_row = (text_area_pos.y.0 / line_height.0) as usize;
        let visual_col = (text_area_pos.x.0 / cell_width.0) as usize;

        // Adjust visual row to account for viewport offset
        let absolute_visual_row = visual_row + view_offset.vertical_offset;

        // Create DocumentFormatter to find the character at this visual position
        let formatter = DocumentFormatter::new_at_prev_checkpoint(
            text,
            text_format,
            &annotations,
            view_offset.anchor,
        );

        // Search for grapheme at the target visual position
        let mut target_char_pos = None;
        let mut last_char_pos = view_offset.anchor;

        for grapheme in formatter {
            // Track character position
            let char_pos = text.byte_to_char(grapheme.char_idx);

            // Check if this grapheme is at our target visual position
            if grapheme.visual_pos.row == absolute_visual_row {
                if grapheme.visual_pos.col <= visual_col {
                    // This is the closest grapheme to our target column
                    target_char_pos = Some(char_pos);
                } else {
                    // We've passed the target column, use the previous character
                    break;
                }
            } else if grapheme.visual_pos.row > absolute_visual_row {
                // We've passed the target row
                break;
            } else {
                // Row is before target; track last known position
                last_char_pos = char_pos;
            }
        }

        // Use the found character position or the last valid position
        let final_char_pos = target_char_pos.unwrap_or(last_char_pos);

        // Convert character position back to document line and column
        let doc_line = text.char_to_line(final_char_pos);
        let line_start = text.line_to_char(doc_line);
        let char_offset = final_char_pos - line_start;

        // Convert to pixel coordinates within the document line
        // This gives us the position within the unwrapped document coordinates
        let result_x = px(char_offset as f32 * cell_width.0);
        let result_y = px(doc_line as f32 * line_height.0);

        Some(Point {
            x: result_x,
            y: result_y,
        })
    }

    /// Calculate x-overshoot for a given position and line width
    /// Returns (clamped_x, overshoot_amount)
    #[allow(dead_code)]
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
        // Focus tracking removed - now handled by InputCoordinator
        // .track_focus(focus)
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
            if let Some(grapheme) = formatter.next()
                && grapheme.visual_pos.row > visual_line
            {
                visual_line = grapheme.visual_pos.row;
            }
        }

        let _text_origin_x = params.bounds.origin.x
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
                segment_char_offset: 0, // TODO: Calculate properly for wrapped segments
                text_start_byte_offset: 0, // TODO: Calculate properly for wrapped segments
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

        let diag_overlays = Self::diagnostics_overlays(document, params.cx.helix_theme());
        let line_runs = Self::highlight_line_with_params(HighlightLineParams {
            doc: document,
            view,
            cx: params.cx,
            editor_mode: editor.mode(),
            cursor_shape: &editor.config().cursor_shape,
            syn_loader: &editor.syn_loader,
            is_view_focused: self.is_focused,
            line_start,
            line_end,
            fg_color: params.fg_color,
            font: self.style.font(),
            diag_overlays,
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

        // Get the theme from context
        let theme = params.cx.helix_theme();

        // Helper to convert HSLA to Helix Color
        let hsla_to_helix = |c: gpui::Hsla| -> Option<helix_view::graphics::Color> {
            // Basic HSLA->RGB conversion (matches usage in ThemeManager bridge)
            let c_chroma = (1.0 - (2.0 * c.l - 1.0).abs()) * c.s;
            let x = c_chroma * (1.0 - (((c.h * 360.0) / 60.0) % 2.0 - 1.0).abs());
            let (r1, g1, b1) = if c.h * 360.0 < 60.0 {
                (c_chroma, x, 0.0)
            } else if c.h * 360.0 < 120.0 {
                (x, c_chroma, 0.0)
            } else if c.h * 360.0 < 180.0 {
                (0.0, c_chroma, x)
            } else if c.h * 360.0 < 240.0 {
                (0.0, x, c_chroma)
            } else if c.h * 360.0 < 300.0 {
                (x, 0.0, c_chroma)
            } else {
                (c_chroma, 0.0, x)
            };
            let m = c.l - c_chroma / 2.0;
            let (r, g, b) = (r1 + m, g1 + m, b1 + m);
            Some(helix_view::graphics::Color::Rgb(
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ))
        };

        // Get syntax highlighter for the entire document view
        let text = params.doc.text().slice(..);
        let anchor = params.doc.view_offset(params.view.id).anchor;
        let lines_from_anchor = text.len_lines() - text.char_to_line(anchor);
        let height = u16::try_from(lines_from_anchor).unwrap_or(u16::MAX);
        let syntax_highlighter =
            Self::doc_syntax_highlights(params.doc, anchor, height, theme, &loader);

        // Get overlay highlights
        let selection_overlay = Self::overlay_highlights(
            params.editor_mode,
            params.doc,
            params.view,
            theme,
            params.cursor_shape,
            true,
            params.is_view_focused,
        );

        let tokens = params.cx.theme().tokens;
        let text_style = helix_view::graphics::Style {
            fg: hsla_to_helix(tokens.editor.text_primary),
            bg: hsla_to_helix(tokens.editor.background),
            ..Default::default()
        };

        // Create syntax and overlay highlighters
        let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);
        // Build overlay list: selections + optional diagnostics
        let mut overlays = Vec::new();
        overlays.push(selection_overlay);
        if let Some(diag) = params.diag_overlays {
            overlays.push(diag);
        }
        let mut overlay_hl = OverlayHighlighter::new(overlays, theme);

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
            let default_bg = tokens.editor.background;

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
                // Reset x-overshoot at the start of a new click
                // This ensures that each new click/selection starts without overshoot
                // Overshoot will be recalculated based on the new cursor position
                // self.reset_x_overshoot(); // TODO: Uncomment when ready to test
                // Get gutter offset and calculate text bounds
                let (gutter_offset, cell_width, element_bounds) = {
                    let core = core_for_down.read(cx);
                    let editor = &core.editor;
                    if let (Some(document), Some(view)) =
                        (editor.document(doc_id), editor.tree.try_get(view_id))
                    {
                        let gutter_offset = view.gutter_offset(document);
                        // Use stored bounds width, with fallback calculation
                        let bounds_width = bounds_width_for_down.get();
                        let element_bounds = gpui::Bounds {
                            origin: element_bounds_for_down.get(), // Use actual bounds from prepaint
                            size: gpui::Size {
                                width: bounds_width,
                                height: px(600.0),
                            }, // Approximate height
                        };
                        (gutter_offset, cell_width_for_down.get(), element_bounds)
                    // Use actual cell_width
                    } else {
                        debug!("Could not get document/view for coordinate transformation");
                        return;
                    }
                };
                // Calculate text bounds to get actual text area
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

                let _expected_text_origin_x =
                    element_bounds.origin.x + Pixels::from(gutter_offset as f32 * cell_width.0);

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
                            let text_area_width =
                                text_bounds.size.width.0 - gutter_width_px - right_padding;
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

                // STEP 3: Convert text-area coordinates to content coordinates
                // Branch based on soft-wrap mode for different coordinate transformation logic
                let scroll_position = scroll_manager_for_down.scroll_position();
                let content_pos = if soft_wrap_enabled {
                    // Implement wrapped mode coordinate transformation
                    let wrapped_content_pos = {
                        // Get core data needed for wrapped coordinate transformation
                        let core = core_for_down.read(cx);
                        let editor = &core.editor;
                        if let Some(document) = editor.document(doc_id) {
                            if let Some(_view) = editor.tree.try_get(view_id) {
                                let theme = cx.global::<crate::ThemeManager>().helix_theme();
                                // Calculate viewport width for text formatting
                                let gutter_width_px = f32::from(gutter_offset) * cell_width.0;
                                let right_padding = cell_width.0 * 2.0;
                                let text_area_width =
                                    text_bounds.size.width.0 - gutter_width_px - right_padding;
                                let viewport_width =
                                    (text_area_width / cell_width.0).max(10.0) as u16;

                                // Get text format and view offset
                                let text_format = document.text_format(viewport_width, Some(theme));
                                let view_offset = document.view_offset(view_id);

                                // Get line height from scroll manager or use default
                                let line_height = scroll_manager_for_down.line_height.get();

                                // Convert text-area position to visual coordinates
                                Self::text_area_to_visual_wrapped(
                                    text_area_pos,
                                    &text_format,
                                    view_offset,
                                    document,
                                    cell_width,
                                    line_height,
                                )
                                .unwrap_or({
                                    // Fallback to non-wrapped transformation
                                    text_area_pos
                                })
                            } else {
                                text_area_pos
                            }
                        } else {
                            text_area_pos
                        }
                    };

                    // Add scroll position to get final content coordinates
                    gpui::Point {
                        x: wrapped_content_pos.x + scroll_position.x,
                        y: wrapped_content_pos.y + scroll_position.y,
                    }
                } else {
                    // Zed convention: scroll_position.y is positive when scrolled down
                    // To get content coordinates: content_y = text_area_y + scroll_position.y
                    gpui::Point {
                        x: text_area_pos.x + scroll_position.x, // Horizontal scroll (currently unused)
                        y: text_area_pos.y + scroll_position.y, // Add positive scroll distance
                    }
                };

                // STEP 4: Apply bounds validation and clamping
                // Clamp coordinates to valid ranges to prevent out-of-bounds access
                // FIXED: Don't clamp Y to viewport height - allow clicks in rendered content area
                let clamped_text_area_pos = Point {
                    x: text_area_pos.x.max(px(0.0)).min(text_bounds.size.width),
                    y: text_area_pos.y.max(px(0.0)), // Only clamp to positive, not to viewport height
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
                let _clamped_content_pos = Point {
                    x: content_pos.x.max(px(0.0)), // Will be updated with x-overshoot tracking below
                    y: content_pos.y.max(px(0.0)).min(total_content_height),
                };

                // STEP 4: Find line using clamped coordinates
                let line_cache = cx.global::<nucleotide_editor::LineLayoutCache>();

                // NOTE: Line cache still stores in element-local coordinates, so we use clamped_text_area_pos
                // This ensures we don't access out-of-bounds positions in the line cache

                // DEBUG: Log line cache contents and search
                debug!(
                    original_window_pos = ?event.position,
                    text_area_pos = ?text_area_pos,
                    clamped_text_area_pos = ?clamped_text_area_pos,
                    text_bounds = ?text_bounds,
                    line_height = %line_height,
                    "🔍 CLICK DEBUG: About to search line cache"
                );
                let line_layout = line_cache.find_line_at_position(
                    clamped_text_area_pos, // Use clamped text-area coordinates
                    text_bounds.size.width,
                    line_height,
                );

                if let Some(line_layout) = line_layout {
                    // STEP 4.5: Calculate and track x-overshoot for selection dragging
                    let line_width = line_layout.shaped_line.width;
                    let raw_content_x = content_pos.x.max(px(0.0));
                    let (_clamped_x, x_overshoot) = if raw_content_x > line_width {
                        let overshoot = raw_content_x - line_width;
                        (line_width, overshoot)
                    } else {
                        (raw_content_x, px(0.0))
                    };

                    // Store x-overshoot for future selection operations
                    x_overshoot_for_down.set(x_overshoot.max(px(0.0)));

                    // STEP 5: Calculate character position within the line using clamped coordinates
                    // line_layout.origin is in element-local coordinates (text-area relative)
                    let relative_x = clamped_text_area_pos.x - line_layout.origin.x;

                    // Convert pixel position to byte offset with proper bounds checking
                    let byte_index = if relative_x < px(0.0) {
                        0 // Click before line start
                    } else if relative_x > line_layout.shaped_line.width {
                        line_layout.shaped_line.len() // Click beyond line end
                    } else {
                        line_layout.shaped_line.index_for_x(relative_x).unwrap_or(0)
                    };

                    // Update Helix editor selection
                    core_for_down.update(cx, |core, cx| {
                        let editor = &mut core.editor;
                        if let Some(document) = editor.document_mut(doc_id) {
                            let text = document.text();
                            let line_start = text.line_to_char(line_layout.line_idx);

                            // FIXED: Handle wrapped line segments correctly, accounting for wrap indicators
                            let line_text = text.line(line_layout.line_idx).to_string();
                            let char_offset = if line_layout.segment_char_offset == 0 {
                                // Non-wrapped line: byte_index is relative to full line
                                line_text
                                    .char_indices()
                                    .take_while(|(byte_idx, _)| *byte_idx < byte_index)
                                    .count()
                            } else {
                                // Wrapped line: adjust byte_index to account for wrap indicators
                                // byte_index is offset within the complete shaped line text (including indicators)
                                // text_start_byte_offset tells us where the real text begins
                                let adjusted_byte_index =
                                    byte_index.saturating_sub(line_layout.text_start_byte_offset);

                                // Get the segment text (real text only, starting at segment_char_offset)
                                let segment_text = line_text
                                    .chars()
                                    .skip(line_layout.segment_char_offset)
                                    .collect::<String>();

                                // Convert adjusted byte offset to character offset within the segment
                                let char_offset_in_segment = segment_text
                                    .char_indices()
                                    .take_while(|(byte_idx, _)| *byte_idx < adjusted_byte_index)
                                    .count();

                                line_layout.segment_char_offset + char_offset_in_segment
                            };

                            let target_pos = (line_start + char_offset).min(text.len_chars());

                            // Create cursor selection
                            let range = helix_core::Range::new(target_pos, target_pos);
                            let selection =
                                helix_core::Selection::new(helix_core::SmallVec::from([range]), 0);
                            document.set_selection(view_id, selection);

                            cx.notify();
                        }
                    });
                } else {
                    debug!(
                        window_pos = ?event.position,
                        text_area_pos = ?text_area_pos,
                        clamped_text_area_pos = ?clamped_text_area_pos,
                        content_pos = ?content_pos,
                        text_bounds = ?text_bounds,
                        line_height = %line_height,
                        "🚨 NO LINE FOUND - CLICK FAILED in bottom area"
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
        // Note: GPUI may call paint() multiple times per frame for the same element
        // This can cause visual accumulation of overlapping elements like tildes
        let _focus = self.focus.clone();
        let core = self.core.clone();
        let view_id = self.view_id;
        let cell_width = after_layout.cell_width;
        let line_height = after_layout.line_height;

        // Update scroll manager with current layout info
        self.scroll_manager.set_line_height(line_height);

        // Set scroll manager viewport to the actual text-area height (exclude top padding)
        let text_area_height = bounds.size.height - px(1.0);
        let effective_viewport_size = size(bounds.size.width, text_area_height);
        self.scroll_manager
            .set_viewport_size(effective_viewport_size);

        // TODO: Update shared cell_width for mouse handlers (requires structural change to pass between prepaint/paint)

        // Fill editor background from design tokens
        {
            let tokens = cx.theme().tokens;
            let bgc = tokens.editor.background;
            let _ = gpui::fill(bounds, bgc);
        }

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

        // Determine total content height in "visual" lines for correct scrolling
        // This ensures the scrollbar range matches the wrapped content height.
        let _visual_total_lines = {
            let core = self.core.read(cx);
            let editor = &core.editor;
            let _view = match editor.tree.try_get(view_id) {
                Some(v) => v,
                None => return,
            };
            let doc = match editor.document(self.doc_id) {
                Some(doc) => doc,
                None => return,
            };

            // Compute approximate columns from current bounds and font metrics
            let font_id = cx.text_system().resolve_font(&self.style.font());
            let font_size = self.style.font_size.to_pixels(px(16.0));
            let em_width = cx
                .text_system()
                .typographic_bounds(font_id, font_size, 'm')
                .map(|bounds| bounds.size.width)
                .unwrap_or(px(8.0));
            let cell_width = cx
                .text_system()
                .advance(font_id, font_size, 'm')
                .map(|advance| advance.width)
                .unwrap_or(em_width);

            // Columns based on available width in cells (approximate)
            let columns = ((bounds.size.width / cell_width).floor() as usize).max(1);

            // Check soft-wrap setting from Helix for this document/view
            // Build TextFormat to read soft_wrap flag; viewport_width expects u16 columns
            let theme = cx.global::<crate::ThemeManager>().helix_theme();
            let tf = doc.text_format(columns as u16, Some(theme));
            let soft_wrap_enabled = tf.soft_wrap;

            if soft_wrap_enabled {
                // Estimate visual lines by wrapping each document line to columns.
                // This is an approximation (tabs/variable widths not considered),
                // but greatly improves scrollbar range vs. raw line count.
                let text = doc.text();
                let line_count = text.len_lines();
                let mut visual = 0usize;
                for line_idx in 0..line_count {
                    let start = text.line_to_char(line_idx);
                    let end = if line_idx + 1 < line_count {
                        text.line_to_char(line_idx + 1)
                    } else {
                        text.len_chars()
                    };
                    let chars = end.saturating_sub(start).max(1);
                    // ceil(chars / columns)
                    visual = visual.saturating_add(chars.div_ceil(columns));
                }
                visual.max(1)
            } else {
                doc.text().len_lines().max(1)
            }
        };

        // PROACTIVE VIEWPORT-ALIGN SCROLL WITH HELIX (non-wrap mode)
        // Compute our viewport height in lines and enforce Helix view offset to keep the cursor
        // within [scrolloff, height - scrolloff] like Helix does, but using our measured height.
        {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let (Some(view), Some(doc)) =
                (editor.tree.try_get(view_id), editor.document(self.doc_id))
            {
                // Skip when soft-wrap is enabled; wrapped logic requires visual line mapping
                let theme = cx.global::<crate::ThemeManager>().helix_theme();
                let font_id = cx.text_system().resolve_font(&self.style.font());
                let font_size = self.style.font_size.to_pixels(px(16.0));
                let em_width = cx
                    .text_system()
                    .typographic_bounds(font_id, font_size, 'm')
                    .map(|b| b.size.width)
                    .unwrap_or(px(8.0));
                let cell_w = cx
                    .text_system()
                    .advance(font_id, font_size, 'm')
                    .map(|a| a.width)
                    .unwrap_or(em_width);
                let columns = ((bounds.size.width / cell_w).floor() as usize).max(1);
                let tf = doc.text_format(columns as u16, Some(theme));
                let soft_wrap = tf.soft_wrap;

                // Collect a desired new anchor char if we need to adjust scrolling.
                let mut desired_anchor_char: Option<usize> = None;
                let mut desired_vertical_offset: Option<usize> = None;

                // Use Helix's notion of viewport height to avoid rounding asymmetry
                let height_rows: usize = view.inner_height().max(1);

                if !soft_wrap {
                    let view_offset = doc.view_offset(view_id);
                    let text = doc.text();
                    let total_lines = text.len_lines();
                    let anchor_line = text.char_to_line(view_offset.anchor);

                    // Determine cursor line
                    let cursor_char = doc.selection(view_id).primary().cursor(text.slice(..));
                    let cursor_line = text.char_to_line(cursor_char);

                    // Viewport height in lines from Helix (avoids rounding differences)
                    let viewport_lines = height_rows;
                    let scrolloff = editor.config().scrolloff.max(0);

                    // Visible is [top, top + height)
                    let top = anchor_line;
                    let bottom_exclusive = anchor_line.saturating_add(viewport_lines);

                    let mut desired_anchor = anchor_line;
                    // If cursor above top + scrolloff -> move up
                    if cursor_line < top.saturating_add(scrolloff) {
                        desired_anchor = cursor_line.saturating_sub(scrolloff);
                    }
                    // If cursor below bottom - scrolloff - 1 (i.e., cursor >= bottom - scrolloff)
                    else if cursor_line >= bottom_exclusive.saturating_sub(scrolloff) {
                        desired_anchor = cursor_line
                            .saturating_add(scrolloff + 1)
                            .saturating_sub(viewport_lines);
                    }

                    // Clamp desired_anchor to valid range
                    let max_anchor = total_lines.saturating_sub(viewport_lines);
                    if desired_anchor > max_anchor {
                        desired_anchor = max_anchor;
                    }

                    if desired_anchor != anchor_line {
                        desired_anchor_char = Some(text.line_to_char(desired_anchor));
                    }
                } else {
                    // Soft-wrap alignment using visual line indices
                    let view_offset = doc.view_offset(view_id);
                    let text = doc.text();
                    // Determine viewport height in visual lines using Helix
                    let viewport_lines = height_rows;
                    let scrolloff = editor.config().scrolloff.max(0);

                    // Build formatter from current anchor
                    use helix_core::{
                        doc_formatter::DocumentFormatter, text_annotations::TextAnnotations,
                    };
                    let annotations = TextAnnotations::default();
                    let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
                        text.slice(..),
                        &tf,
                        &annotations,
                        view_offset.anchor,
                    );

                    // Find cursor visual row relative to current anchor
                    let cursor_char = doc.selection(view_id).primary().cursor(text.slice(..));

                    let mut cursor_visual_row: Option<usize> = None;
                    let mut last_row = 0usize;
                    for g in formatter {
                        let char_pos = text.byte_to_char(g.char_idx);
                        if char_pos > cursor_char {
                            break;
                        }
                        last_row = g.visual_pos.row;
                        cursor_visual_row = Some(last_row);
                    }
                    let cursor_vrow = cursor_visual_row.unwrap_or(last_row);

                    // Current top visual row (relative to anchor)
                    let top = view_offset.vertical_offset;
                    let bottom = top.saturating_add(viewport_lines.saturating_sub(1));

                    // Decide desired top to honor scrolloff
                    let desired_top = if cursor_vrow < top.saturating_add(scrolloff) {
                        cursor_vrow.saturating_sub(scrolloff)
                    } else if cursor_vrow > bottom.saturating_sub(scrolloff) {
                        cursor_vrow.saturating_sub(viewport_lines.saturating_sub(1 + scrolloff))
                    } else {
                        top
                    };

                    if desired_top != top {
                        // Prefer adjusting vertical_offset to avoid anchor jumps in soft-wrap
                        let desired_v_off_usize: usize = desired_top;
                        if desired_v_off_usize != view_offset.vertical_offset {
                            desired_vertical_offset = Some(desired_v_off_usize);
                        }
                    }
                }

                // After leaving the read-borrowing scope, apply updates if needed
                if let Some(new_anchor_char) = desired_anchor_char {
                    let core2 = self.core.clone();
                    core2.update(cx, |core, _| {
                        if let Some(doc_mut) = core.editor.document_mut(self.doc_id) {
                            let current = doc_mut.view_offset(view_id);
                            // Only update anchor here (non-wrap path) and preserve vertical_offset
                            let new_pos = ViewPosition {
                                anchor: new_anchor_char,
                                horizontal_offset: current.horizontal_offset,
                                vertical_offset: current.vertical_offset,
                            };
                            doc_mut.set_view_offset(view_id, new_pos);
                        }
                    });
                }

                if let Some(new_v_off) = desired_vertical_offset {
                    let core2 = self.core.clone();
                    core2.update(cx, |core, _| {
                        if let Some(doc_mut) = core.editor.document_mut(self.doc_id) {
                            let current = doc_mut.view_offset(view_id);
                            let new_pos = ViewPosition {
                                anchor: current.anchor,
                                horizontal_offset: current.horizontal_offset,
                                vertical_offset: new_v_off,
                            };
                            doc_mut.set_view_offset(view_id, new_pos);
                        }
                    });
                }
            }
        }

        // Update scrollbar range approximately to document line count
        // (Helix drives actual scrolling; this is for UI scale only.)
        {
            let core = self.core.read(cx);
            if let Some(doc) = core.editor.document(self.doc_id) {
                self.scroll_manager.total_lines.set(doc.text().len_lines());
            }
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

            // total_lines already updated above from document line count

            // Sync scroll position from Helix to ensure we reflect auto-scroll
            // This is important for keeping cursor visible during editing
            let view_offset = doc.view_offset(self.view_id);
            let text = doc.text();
            let anchor_line = text.char_to_line(view_offset.anchor);
            // Mirror Helix viewport: include vertical_offset (visual rows) for wrapped/non-wrapped
            let top_visual = anchor_line.saturating_add(view_offset.vertical_offset as usize);
            let y = px(top_visual as f32 * self.scroll_manager.line_height.get().0);
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

        let _line_cache_mouse = line_cache.clone();
        let _scrollbar_state_mouse = self.scrollbar_state.clone();
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

                // Get cursorline style
                let cursorline_style = if cursorline_enabled {
                    let style = cx.theme_style("ui.cursorline.primary");
                    debug!("Cursorline style found: bg={:?}, fg={:?}", style.bg, style.fg);
                    style.bg.and_then(color_to_hsla)
                } else {
                    None
                };
                let tokens = cx.theme().tokens;
                let bg_color = tokens.editor.background;
                // Get mode-specific cursor theme like terminal version
                let mode = editor.mode();
                let base_cursor_style = cx.theme_style("ui.cursor");
                let base_primary_cursor_style = cx.theme_style("ui.cursor.primary");

                // Try to get mode-specific cursor style, fallback to base
                // Important: we need to patch styles to combine colors with modifiers
                let cursor_style = match mode {
                    helix_view::document::Mode::Insert => {
                        let style = cx.theme_style("ui.cursor.primary.insert");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                    helix_view::document::Mode::Select => {
                        let style = cx.theme_style("ui.cursor.primary.select");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                    helix_view::document::Mode::Normal => {
                        let style = cx.theme_style("ui.cursor.primary.normal");
                        if style.fg.is_some() || style.bg.is_some() {
                            // Patch with base cursor to get modifiers
                            base_cursor_style.patch(style)
                        } else {
                            base_cursor_style.patch(base_primary_cursor_style)
                        }
                    }
                };
                let _bg = fill(bounds, bg_color);
                let fg_color = tokens.editor.text_primary;

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

                        // Don't show visible characters for newlines in cursor
                        let char_str = if char_str == "\n" || char_str == "\r\n" || char_str == "\r" {
                            " ".into() // Use space for newlines so cursor is visible but no symbol
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
                debug!("Cursor position: line={}, char_idx={}", cursor_line_num, cursor_char_idx);

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


                // Render rulers before text
                const THEME_KEY_VIRTUAL_RULER: &str = "ui.virtual.ruler";
                let ruler_style = cx.theme_style(THEME_KEY_VIRTUAL_RULER);
                let ruler_color = ruler_style.bg
                    .and_then(color_to_hsla)
                    .unwrap_or_else(|| {
                        // Use UI theme's border color from tokens
                        cx.ui_theme().tokens.chrome.border_default
                    });

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
                let _editor_theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
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
                let (cursor_text_shaped, cursor_text_len) = cursor_text
                    .map(|(char_str, text_color)| {
                        let text_len = char_str.len();
                        // Derive the original text style at the cursor to preserve italics/bold/underline
                        let text_style_at_cursor = {
                            let core = self.core.read(cx);
                            let editor = &core.editor;
                            if let Some(doc) = editor.document(self.doc_id) {
                                let theme = cx.global::<crate::ThemeManager>().helix_theme();
                                Self::get_text_style_at_position(
                                    doc,
                                    self.view_id,
                                    theme,
                                    &editor.syn_loader,
                                    cursor_char_idx,
                                )
                            } else {
                                helix_view::graphics::Style::default()
                            }
                        };

                        let run = Self::make_cursor_text_run(
                            &self.style.font(),
                            text_len,
                            &text_style_at_cursor,
                            text_color,
                            bg_color,
                        );

                        let shaped = window
                            .text_system()
                            .shape_line(
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

                // Update the shared line layouts for mouse interaction
                if soft_wrap_enabled {
                    // Use DocumentFormatter for soft wrap rendering

                    // Get text format and create DocumentFormatter
                    let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

                    // Extract wrap indicator color early to avoid borrow conflicts later
                    let wrap_indicator_color = cx.theme_style("ui.virtual.wrap").fg.and_then(color_to_hsla);

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

                        let text_format = document.text_format(viewport_width, Some(&theme));
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
                    let mut y_offset = px(0.0);
                    let mut visual_line = 0;
                    let mut current_doc_line = text.char_to_line(view_offset.anchor);
                    // Account for padding in viewport height calculation - match ScrollManager exactly
                    let effective_height = bounds.size.height - px(2.0); // Account for padding
                    let calculated_height = (effective_height / after_layout.line_height) as usize;
                    // IMPORTANT: Don't add buffer lines here - this must match ScrollManager's viewport calculation
                    // The ScrollManager uses exactly this height for max scroll calculations
                    let viewport_height = calculated_height;

                    // Skip lines before the viewport - need to consume all graphemes for skipped lines
                    // Skip lines before viewport if needed
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
                                    // Precompute diagnostic overlays once
                                    let diag_overlays = Self::diagnostics_overlays(document, cx.helix_theme());
                                    Self::highlight_line_with_params(HighlightLineParams {
                                        doc: document,
                                        view,
                                        cx,
                                        editor_mode,
                                        cursor_shape: &cursor_shape,
                                        syn_loader: &syn_loader,
                                        is_view_focused: self.is_focused,
                                        line_start,
                                        line_end,
                                        fg_color,
                                        font: self.style.font(),
                                        diag_overlays,
                                    })
                                } else {
                                    Vec::new()
                                }
                            }
                        } else {
                            Vec::new()
                        };

                        // Adjust text runs to account for leading spaces and wrap indicator
                        if !line_runs.is_empty() {
                            // Handle indentation spaces separately from wrap indicators
                            if line_start_col > 0 {
                                // Add run for indentation spaces using normal text color
                                let indent_run = TextRun {
                                    len: line_start_col,
                                    font: self.style.font(),
                                    color: fg_color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                line_runs.insert(0, indent_run);
                            }

                            if wrap_indicator_len > 0 {
                                // Use pre-extracted wrap indicator color
                                let wrap_color = wrap_indicator_color.unwrap_or(fg_color); // Fallback to normal text color
                                // Add run for wrap indicator using ui.virtual.wrap theme color
                                let wrap_run = TextRun {
                                    len: wrap_indicator_len,
                                    font: self.style.font(),
                                    color: wrap_color,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                line_runs.insert(if line_start_col > 0 { 1 } else { 0 }, wrap_run);
                            }
                        }

                        // Determine whether this visual line corresponds to the cursor's document line
                        let is_cursor_visual_line = {
                            if let Some(first_grapheme) = line_graphemes.first() {
                                first_grapheme.line_idx == cursor_line_num
                            } else {
                                current_doc_line == cursor_line_num
                            }
                        };

                        // Paint the line text (only for non-empty lines)
                        if !line_str.is_empty() {
                            let shaped_line = window.text_system()
                                .shape_line(SharedString::from(line_str.clone()), self.style.font_size.to_pixels(px(16.0)), &line_runs, None);

                            // Paint background highlights using the shaped line for accurate positioning
                            let mut byte_offset = 0;
                            for run in &line_runs {
                                // Do not overpaint the cursor row background with per-run backgrounds
                                if !is_cursor_visual_line
                                    && let Some(bg_color) = run.background_color
                                {
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

                            // Paint cursorline background after per-run backgrounds so it applies across the row
                            if is_cursor_visual_line
                                && let Some(cursorline_bg) = cursorline_style
                            {
                                let cursorline_bounds = Bounds {
                                    origin: point(bounds.origin.x, line_y),
                                    size: size(bounds.size.width, after_layout.line_height),
                                };
                                window.paint_quad(fill(cursorline_bounds, cursorline_bg));
                            }

                            if let Err(e) = shaped_line.paint(point(text_origin_x, line_y), after_layout.line_height, window, cx) {
                                error!(error = ?e, "Failed to paint text");
                            }

                            // FIXED: Update document line BEFORE storing layout to prevent off-by-one error
                            if let Some(first_grapheme) = line_graphemes.first() {
                                current_doc_line = first_grapheme.line_idx;
                            }

                            // Skip phantom lines in soft-wrap mode - they shouldn't take up visual space
                            let line_start = text.line_to_char(current_doc_line);
                            let line_end = if current_doc_line + 1 < text.len_lines() {
                                text.line_to_char(current_doc_line + 1)
                            } else {
                                text.len_chars()
                            };
                            let is_phantom_line = line_start >= line_end;

                            if is_phantom_line {
                                // Phantom lines should still increment visual position for UI elements (gutter, cursorline)
                                // but don't need text layout since they have no content
                                visual_line += 1;
                                y_offset += after_layout.line_height;
                                continue;
                            }

                            // Store line layout for mouse interaction
                            // FIXED: Store in text-area coordinates (gutter excluded)
                            // Use y_offset directly to match coordinate system used by mouse handler
                            let text_area_origin = point(
                                px(0.0), // Line starts at x=0 in text-area coordinates
                                y_offset, // Use y_offset directly (no px(1.) like non-wrap mode)
                            );
                            // Calculate segment character offset for wrapped lines
                            let segment_char_offset = if let Some(first_grapheme) = line_graphemes.iter().find(|g| !g.is_virtual()) {
                                let line_start = text.line_to_char(current_doc_line);
                                first_grapheme.char_idx.saturating_sub(line_start)
                            } else {
                                0 // No real content in this segment
                            };

                            // Calculate where the real text starts in the shaped line (after wrap indicators)
                            let text_start_byte_offset = line_start_col + wrap_indicator_len;
                            let layout = nucleotide_editor::LineLayout {
                                line_idx: current_doc_line,
                                shaped_line,
                                origin: text_area_origin,
                                segment_char_offset,
                                text_start_byte_offset,
                            };
                            line_cache.push(layout);
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

                        // Compute gutter width in pixels to reserve space for diagnostic markers
                        let _gutter_width_px = {
                            let core = self.core.read(cx);
                            let editor = &core.editor;
                            let document = match editor.document(self.doc_id) { Some(d) => d, None => return };
                            let view = match editor.tree.try_get(self.view_id) { Some(v) => v, None => return };
                            let gutter_cells = view.gutter_offset(document);
                            f32::from(gutter_cells) * after_layout.cell_width
                        };

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

                        // Build a map of line -> highest diagnostic severity for quick lookup
                        let diag_line_severity = {
                            let core = self.core.read(cx);
                            let editor = &core.editor;
                            if let Some(document) = editor.document(self.doc_id) {
                                let mut m: std::collections::BTreeMap<usize, helix_core::diagnostic::Severity> = std::collections::BTreeMap::new();
                                for d in document.diagnostics().iter() {
                                    // Derive start/end lines from character positions
                                    let start_line = text.char_to_line(d.range.start);
                                    let end_char = d.range.end.min(text.len_chars());
                                    let end_line = text.char_to_line(end_char);
                                    if let Some(sev) = d.severity {
                                        for line in start_line..=end_line {
                                            m.entry(line)
                                                .and_modify(|s| {
                                                    // Keep highest severity (Error > Warning > Info > Hint)
                                                    if matches!((sev, *s), (helix_core::diagnostic::Severity::Error, _)
                                                        | (helix_core::diagnostic::Severity::Warning, helix_core::diagnostic::Severity::Info | helix_core::diagnostic::Severity::Hint)
                                                        | (helix_core::diagnostic::Severity::Info, helix_core::diagnostic::Severity::Hint)) {
                                                        *s = sev;
                                                    }
                                                })
                                                .or_insert(sev);
                                        }
                                    }
                                }
                                m
                            } else {
                                std::collections::BTreeMap::new()
                            }
                        };

                        // Now render the line numbers with highlighting for current line
                        let gutter_style = cx.theme_style("ui.linenr");
                        let gutter_selected_style = cx.theme_style("ui.linenr.selected");



                        for (doc_line, y_pos) in doc_line_positions {
                            // Check if this is a phantom line (empty lines at EOF with trailing newline)
                            let line_start = text.line_to_char(doc_line);
                            let line_end = if doc_line + 1 < text.len_lines() {
                                text.line_to_char(doc_line + 1)
                            } else {
                                text.len_chars()
                            };
                            let is_phantom_line = line_start >= line_end;

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

                            let gutter_color = gutter_style.fg.and_then(crate::utils::color_to_hsla).unwrap_or(default_gutter_color);
                            let gutter_selected_color = gutter_selected_style.fg.and_then(crate::utils::color_to_hsla).unwrap_or(default_gutter_color);


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

                            let shaped = window.text_system()
                                .shape_line(line_num_str.into(), self.style.font_size.to_pixels(px(16.0)), &[run], None);

                            let _ = shaped.paint(point(gutter_origin.x, y), after_layout.line_height, window, cx);

                            // Paint a small diagnostic marker in the gutter if this line has diagnostics
                            if let Some(sev) = diag_line_severity.get(&doc_line).copied()
                                && let Some(color) = Self::severity_color(cx.helix_theme(), sev) {
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
                                                origin: point(marker_x + marker_size * 0.18, marker_y + marker_size * 0.18),
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
                                                origin: point(marker_x + marker_size * 0.22, marker_y + marker_size * 0.18),
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
                                        helix_core::diagnostic::Severity::Info | helix_core::diagnostic::Severity::Hint => {
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

                                        // Don't show visible characters for newlines in cursor
                                        let char_str = if char_str == "\n" || char_str == "\r\n" || char_str == "\r" {
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

                                // Get cursor style
                                let mode = editor.mode();
                                let base_cursor_style = cx.theme_style("ui.cursor");
                                let base_primary_cursor_style = cx.theme_style("ui.cursor.primary");
                                // Important: we need to patch styles to combine colors with modifiers
                                let cursor_style = match mode {
                                    helix_view::document::Mode::Insert => {
                                        let style = cx.theme_style("ui.cursor.primary.insert");
                                        if style.fg.is_some() || style.bg.is_some() {
                                            // Patch with base cursor to get modifiers
                                            base_cursor_style.patch(style)
                                        } else {
                                            base_cursor_style.patch(base_primary_cursor_style)
                                        }
                                    }
                                    helix_view::document::Mode::Select => {
                                        let style = cx.theme_style("ui.cursor.primary.select");
                                        if style.fg.is_some() || style.bg.is_some() {
                                            // Patch with base cursor to get modifiers
                                            base_cursor_style.patch(style)
                                        } else {
                                            base_cursor_style.patch(base_primary_cursor_style)
                                        }
                                    }
                                    helix_view::document::Mode::Normal => {
                                        let style = cx.theme_style("ui.cursor.primary.normal");
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
                                    &theme,
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

                        // Handle EOF phantom line case - if cursor wasn't found in formatter but is at EOF
                        if cursor_visual_line.is_none() && cursor_char_idx >= text.len_chars() {
                            // Cursor is at EOF phantom line - position at first tilde line
                            // Since phantom line layouts are skipped, the cursor should be at _visual_line + 1
                            // but the cursorline is showing at the right position, so let's match it
                            cursor_visual_line = Some(_visual_line);
                            cursor_visual_col = 0; // Phantom line starts at column 0
                        }


                        // If cursor is in viewport, render it
                        if let Some(cursor_line) = cursor_visual_line
                            && cursor_line >= view_offset.vertical_offset &&
                               cursor_line < view_offset.vertical_offset + viewport_height {
                                // Do not auto-scroll here; rely on Helix ensure_cursor_in_view.
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
                                // Account for horizontal scrolling when calculating cursor X position
                                let visual_col_in_viewport = cursor_visual_col as f32 - view_offset.horizontal_offset as f32;
                                let cursor_x = text_bounds.origin.x + (after_layout.cell_width * visual_col_in_viewport);

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

                                        let run = Self::make_cursor_text_run(
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
                                let mut cursor = Cursor {
                                    origin: point(px(0.0), px(0.0)),  // No offset needed, will be applied in paint
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
                                    let layout_info = cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
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
                    let _visual_lines_rendered = visual_line - view_offset.vertical_offset;
                    let viewport_height_in_lines = (bounds.size.height - px(2.0)) / after_layout.line_height;
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
                    let line_start_char = if line_idx < total_lines { text.line_to_char(line_idx) } else { text.len_chars() };
                    let line_end_char = if line_idx + 1 < total_lines {
                        text.line_to_char(line_idx + 1).saturating_sub(1)
                    } else {
                        text.len_chars()
                    };
                    let line_is_empty = line_start_char >= line_end_char;
                    let is_phantom_line = (cursor_at_end && file_ends_with_newline && line_idx == total_lines - 1) ||
                                         (line_idx >= total_lines) ||
                                         (line_idx == total_lines - 1 && line_is_empty && total_lines > 1);

                    // Skip phantom lines entirely - they shouldn't take up visual space
                    if is_phantom_line {
                        debug!("Skipping phantom line layout creation for line_idx={}", line_idx);
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

                        let diag_overlays = Self::diagnostics_overlays(document, cx.helix_theme());
                        let line_runs = Self::highlight_line_with_params(HighlightLineParams {
                            doc: document,
                            view,
                            cx,
                            editor_mode,
                            cursor_shape: &cursor_shape,
                            syn_loader: &syn_loader,
                            is_view_focused: self.is_focused,
                            line_start,
                            line_end,
                            fg_color,
                            font: self.style.font(),
                            diag_overlays,
                        });

                        (line_str, line_runs)
                    };

                    // Drop core before painting
                    // core goes out of scope here

                    let text_origin = point(text_origin_x, bounds.origin.y + px(1.) + y_offset);
                    // Defer painting of cursorline until after per-run backgrounds are drawn

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
                            // Do not overpaint the cursor row background with per-run backgrounds
                            if line_idx != cursor_line_num {
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
                            }
                            byte_offset += run.len;
                        }

                        // Paint cursorline background after per-run backgrounds so it applies across the row
                        if line_idx == cursor_line_num {
                            if let Some(cursorline_bg) = cursorline_style {
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
                    let layout = nucleotide_editor::LineLayout {
                        line_idx,
                        shaped_line,
                        origin: text_area_origin,
                        segment_char_offset: 0, // Non-wrapped lines always start at beginning
                        text_start_byte_offset: 0, // No wrap indicators in non-wrapped lines
                    };

                    // Debug: log line layout creation
                    debug!("💾 LINE LAYOUT CACHED: line_idx={}, y_offset={:?}, is_phantom={}",
                        line_idx, y_offset, false);

                    line_cache.push(layout);

                    y_offset += after_layout.line_height;
                }

                // Render tilde lines for empty viewport space (like Helix/Vim)
                // Calculate how many lines we've rendered vs viewport capacity
                // Since we skip phantom lines, we need to count actual rendered lines, not just last_row - first_row
                // Count lines by iterating through the range and checking which ones weren't skipped
                let mut _actual_lines_rendered = 0;
                for line_idx in first_row..last_row {
                    let line_start_char = if line_idx < total_lines { text.line_to_char(line_idx) } else { text.len_chars() };
                    let line_end_char = if line_idx + 1 < total_lines {
                        text.line_to_char(line_idx + 1).saturating_sub(1)
                    } else {
                        text.len_chars()
                    };
                    let line_is_empty = line_start_char >= line_end_char;
                    let is_phantom_line = (cursor_at_end && file_ends_with_newline && line_idx == total_lines - 1) ||
                                         (line_idx >= total_lines) ||
                                         (line_idx == total_lines - 1 && line_is_empty && total_lines > 1);
                    if !is_phantom_line {
                        _actual_lines_rendered += 1;
                    }
                }
                let viewport_height_in_lines = (bounds.size.height - px(2.0)) / after_layout.line_height;
                let _viewport_capacity = viewport_height_in_lines as usize;

                // Note: Tilde rendering is handled by the gutter for consistency with Helix
                // The gutter shows "~" for phantom lines in the line number area

                // draw cursor
                let element_focused = self.focus.is_focused(window);
                debug!("Cursor rendering check - is_focused: {}, element_focused: {}, cursor_pos: {:?}",
                    self.is_focused, element_focused, cursor_pos);

                // Debug: Log cursor position info
                {
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    if let Some(doc) = editor.document(self.doc_id)
                        && let Some(_view) = editor.tree.try_get(self.view_id) {
                            let sel = doc.selection(self.view_id);
                            let cursor_char = sel.primary().cursor(text);
                            debug!("Cursor char idx: {}, line: {}, selection: {:?}",
                                cursor_char, text.char_to_line(cursor_char), sel);
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
                        // Since we skip phantom lines, cursor at EOF should position on the last real line
                        let cursor_line = if cursor_at_end && file_ends_with_newline {
                            (total_lines - 1).min(text.len_lines().saturating_sub(1))  // Last real line, not phantom
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
                                    line_text.trim_end_matches(&['\n', '\r'][..]).chars().count()
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
                                let cursor_char_offset = cursor_char_offset.min(line_text.chars().count());

                                // Convert char offset to byte offset for GPUI's x_for_index
                                let cursor_byte_offset = line_text.chars().take(cursor_char_offset).map(char::len_utf8).sum::<usize>();

                                // Get the x position from the shaped line using byte offset
                                let cursor_x_relative_to_line = line_layout.shaped_line.x_for_index(cursor_byte_offset);

                                // Additional debug for x_for_index calculation

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
                                debug!("Line content: {:?}, cursor at char offset {} (byte offset {}), at_eof: {}",
                                    &line_text, cursor_char_offset, cursor_byte_offset, cursor_at_end && file_ends_with_newline);

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
                                cursor.paint(absolute_cursor_position, window, cx);
                            } else {
                                debug!("❌ CURSOR FAIL: Could not find line layout for cursor line {} (layout_line_idx={})", cursor_line, layout_line_idx);

                                // Special handling for EOF phantom line cursor
                                if cursor_at_end && file_ends_with_newline && cursor_char_idx >= text.len_chars() {

                                    // Calculate text bounds for phantom cursor positioning (same as normal cursor logic)
                                    let cell_width = after_layout.cell_width;
                                    let gutter_offset_u16 = gutter_width;
                                    let element_bounds = bounds;

                                    let phantom_text_bounds = {
                                        let gutter_width_px = Pixels::from(gutter_offset_u16 as f32 * cell_width.0);
                                        let right_padding = cell_width * 2.0;
                                        let top_padding = px(1.0);

                                        gpui::Bounds {
                                            origin: gpui::Point {
                                                x: element_bounds.origin.x + gutter_width_px,
                                                y: element_bounds.origin.y + top_padding,
                                            },
                                            size: gpui::Size {
                                                width: element_bounds.size.width - gutter_width_px - right_padding,
                                                height: element_bounds.size.height - top_padding,
                                            },
                                        }
                                    };

                                    // Calculate cursor position at the first tilde line
                                    // Use the y_offset from the main loop (where the next line would be)
                                    let cursor_x = phantom_text_bounds.origin.x; // Start of line
                                    let cursor_y = phantom_text_bounds.origin.y + y_offset; // At the phantom line position

                                    // Check if cursor has reversed modifier (same logic as normal cursor)
                                    let has_reversed = cursor_style.add_modifier.contains(helix_view::graphics::Modifier::REVERSED) &&
                                                       !cursor_style.sub_modifier.contains(helix_view::graphics::Modifier::REVERSED);

                                    // For reversed cursor, we need to get the text style at cursor position
                                    let cursor_color = if has_reversed {
                                        // Get the styled text color at cursor position
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

                                    // Use default cursor width
                                    let cursor_width = after_layout.cell_width;

                                    let mut cursor = Cursor {
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
                                        let layout_info = cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
                                        layout_info.cursor_position = Some(cursor_point);
                                        layout_info.cursor_size = Some(gpui::Size {
                                            width: cursor_width,
                                            height: after_layout.line_height,
                                        });
                                    }

                                    cursor.paint(cursor_point, window, cx);
                                } else {
                                    debug!("❌ CURSOR FAIL: Normal line layout missing for line {}", cursor_line);
                                }
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

                    // Build gutter lines and diagnostics map inside a limited borrow scope, then paint
                    let (gutter_lines, _diag_line_severity_nonwrap) = {
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

                        // Precompute per-line highest diagnostic severity for marker painting
                        let diag_map = {
                            let mut m: std::collections::BTreeMap<usize, helix_core::diagnostic::Severity> = std::collections::BTreeMap::new();
                            let text = document.text();
                            for d in document.diagnostics().iter() {
                                if let Some(sev) = d.severity {
                                    let start_line = text.char_to_line(d.range.start);
                                    let end_char = d.range.end.min(text.len_chars());
                                    let end_line = text.char_to_line(end_char);
                                    for line in start_line..=end_line {
                                        m.entry(line)
                                            .and_modify(|s| {
                                                if matches!((sev, *s), (helix_core::diagnostic::Severity::Error, _)
                                                    | (helix_core::diagnostic::Severity::Warning, helix_core::diagnostic::Severity::Info | helix_core::diagnostic::Severity::Hint)
                                                    | (helix_core::diagnostic::Severity::Info, helix_core::diagnostic::Severity::Hint)) {
                                                    *s = sev;
                                                }
                                            })
                                            .or_insert(sev);
                                    }
                                }
                            }
                            m
                        };

                        // Prepare gutter lines (no wrapping assumed here)
                        let text_system = window.text_system().clone();
                        let style = self.style.clone();
                        let gutter_last_row = last_row;
                        let mut lines = Vec::new();
                        for (current_visual_line, doc_line) in (first_row..gutter_last_row).enumerate() {
                            lines.push(LinePos {
                                first_visual_line: true,
                                doc_line,
                                visual_line: current_visual_line as u16,
                                start_char_idx: 0,
                            });
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
                        Gutter::init_gutter(editor, document, view, theme, is_focused, &mut gutters);
                        for line in lines {
                            for gut in &mut gutters {
                                gut(line, &mut gutter)
                            }
                        }
                        (gutter.lines, diag_map)
                    };

                    // Now paint the gutter lines
                    for (origin, line) in gutter_lines {
                        if let Err(e) = line.paint(origin, after_layout.line_height, window, cx) {
                            error!(error = ?e, "Failed to paint gutter line");
                        }
                    }

                    // Note: In non-soft-wrap mode, we rely on Helix's built-in sign gutters.
                    // Custom diagnostic indicators (circle/triangle/square) are only drawn in soft-wrap mode.
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

        let base_fg = style.fg.and_then(color_to_hsla).unwrap_or(white()); // Use white as a reasonable fallback for gutter text
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
            CursorKind::Hidden => Bounds {
                origin: self.origin + origin,
                size: size(px(0.0), px(0.0)),
            },
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    catch_unwind(AssertUnwindSafe(|| theme.highlight(highlight))).unwrap_or_default()
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

        // Guard against panics from upstream highlighter (tree-house). Process in-place.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (event, highlights) = highlighter.advance();
            // Collect highlights to detach borrow from highlighter
            let mut collected: Vec<syntax::Highlight> = Vec::new();
            for h in highlights {
                collected.push(h);
            }
            (event, collected)
        }));

        if let Ok((event, collected)) = result {
            let base = match event {
                HighlightEvent::Refresh => self.text_style,
                HighlightEvent::Push => self.style,
            };

            self.style = collected.into_iter().fold(base, |acc, highlight| {
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
        } else {
            // Disable syntax highlighting for this render cycle to avoid crashing the app
            self.inner = None;
            self.pos = usize::MAX;
        }
    }
}

struct OverlayHighlighter<'t> {
    inner: syntax::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: helix_view::graphics::Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: Vec<syntax::OverlayHighlights>, theme: &'t Theme) -> Self {
        let inner = syntax::OverlayHighlighter::new(overlays);
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
        // Guard against panics from upstream overlay highlighter. Process in-place.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (event, highlights) = self.inner.advance();
            let mut collected: Vec<syntax::Highlight> = Vec::new();
            for h in highlights {
                collected.push(h);
            }
            (event, collected)
        }));

        if let Ok((event, collected)) = result {
            let base = match event {
                HighlightEvent::Refresh => helix_view::graphics::Style::default(),
                HighlightEvent::Push => self.style,
            };

            self.style = collected.into_iter().fold(base, |acc, highlight| {
                let highlight_style = safe_highlight(self.theme, highlight);
                acc.patch(highlight_style)
            });
            self.update_pos();
        } else {
            // Disable overlay highlights on panic
            self.pos = usize::MAX;
        }
    }
}

// Removed DiagnosticView - diagnostics are now handled through events and document highlights

#[cfg(test)]
mod coordinate_transformation_tests {
    use super::*;
    use gpui::{Bounds, point, px, size};
    use helix_view::view::ViewPosition;

    fn create_test_text_bounds() -> Bounds<Pixels> {
        // Simulate text area: starts at (48, 24) with size 800x600
        Bounds {
            origin: point(px(48.0), px(24.0)), // After gutter (48px) and header (24px)
            size: size(px(800.0), px(600.0)),
        }
    }

    fn create_test_view_offset() -> ViewPosition {
        ViewPosition {
            anchor: 100, // Character position 100 in document
            horizontal_offset: 0,
            vertical_offset: 2, // 2 lines scrolled down
        }
    }

    #[test]
    fn test_window_to_text_area_coordinate_conversion() {
        let text_bounds = create_test_text_bounds();

        // Test basic coordinate conversion
        let window_pos = point(px(100.0), px(100.0));
        let text_area_pos = point(
            window_pos.x - text_bounds.origin.x,
            window_pos.y - text_bounds.origin.y,
        );

        // Expected: (100-48, 100-24) = (52, 76)
        assert_eq!(text_area_pos.x, px(52.0));
        assert_eq!(text_area_pos.y, px(76.0));

        // Test position at text area origin
        let origin_pos = text_bounds.origin;
        let converted = point(
            origin_pos.x - text_bounds.origin.x,
            origin_pos.y - text_bounds.origin.y,
        );
        assert_eq!(converted.x, px(0.0));
        assert_eq!(converted.y, px(0.0));

        // Test position outside text area (should handle negative coordinates)
        let outside_pos = point(px(20.0), px(10.0));
        let outside_converted = point(
            outside_pos.x - text_bounds.origin.x,
            outside_pos.y - text_bounds.origin.y,
        );
        assert_eq!(outside_converted.x, px(-28.0));
        assert_eq!(outside_converted.y, px(-14.0));
    }

    #[test]
    fn test_text_area_to_content_coordinate_conversion() {
        let view_offset = create_test_view_offset();
        let cell_width = px(8.0);
        let line_height = px(20.0);

        // Test conversion with scroll offset
        let text_area_pos = point(px(40.0), px(60.0));

        // Content position = text_area_position + scroll_offset
        let scroll_x = px(view_offset.horizontal_offset as f32 * cell_width.0);
        let scroll_y = px(view_offset.vertical_offset as f32 * line_height.0);

        let content_pos = point(text_area_pos.x + scroll_x, text_area_pos.y + scroll_y);

        // Expected: (40 + 0*8, 60 + 2*20) = (40, 100)
        assert_eq!(content_pos.x, px(40.0));
        assert_eq!(content_pos.y, px(100.0));

        // Test with horizontal scroll
        let view_offset_with_h_scroll = ViewPosition {
            anchor: 100,
            horizontal_offset: 5, // 5 characters scrolled right
            vertical_offset: 3,   // 3 lines scrolled down
        };

        let scroll_x_h = px(view_offset_with_h_scroll.horizontal_offset as f32 * cell_width.0);
        let scroll_y_h = px(view_offset_with_h_scroll.vertical_offset as f32 * line_height.0);

        let content_pos_h = point(text_area_pos.x + scroll_x_h, text_area_pos.y + scroll_y_h);

        // Expected: (40 + 5*8, 60 + 3*20) = (80, 120)
        assert_eq!(content_pos_h.x, px(80.0));
        assert_eq!(content_pos_h.y, px(120.0));
    }

    #[test]
    fn test_content_to_display_point_conversion() {
        let cell_width = px(8.0);
        let line_height = px(20.0);

        // Test basic pixel to display point conversion
        let content_pos = point(px(64.0), px(100.0));

        let display_col = (content_pos.x.0 / cell_width.0) as usize;
        let display_row = (content_pos.y.0 / line_height.0) as usize;

        // Expected: (64/8, 100/20) = (8, 5)
        assert_eq!(display_col, 8);
        assert_eq!(display_row, 5);

        // Test fractional positioning (should truncate)
        let fractional_pos = point(px(67.5), px(109.9));
        let frac_col = (fractional_pos.x.0 / cell_width.0) as usize;
        let frac_row = (fractional_pos.y.0 / line_height.0) as usize;

        // Expected: (67.5/8, 109.9/20) = (8, 5) (truncated)
        assert_eq!(frac_col, 8);
        assert_eq!(frac_row, 5);

        // Test zero position
        let zero_pos = point(px(0.0), px(0.0));
        let zero_col = (zero_pos.x.0 / cell_width.0) as usize;
        let zero_row = (zero_pos.y.0 / line_height.0) as usize;
        assert_eq!(zero_col, 0);
        assert_eq!(zero_row, 0);
    }

    #[test]
    fn test_complete_coordinate_transformation_chain() {
        // Test the complete chain: Window → TextArea → Content → DisplayPoint
        let text_bounds = create_test_text_bounds();
        let view_offset = create_test_view_offset();
        let cell_width = px(8.0);
        let line_height = px(20.0);

        // Start with a window coordinate
        let window_pos = point(px(144.0), px(84.0));

        // Step 1: Window → TextArea
        let text_area_pos = point(
            window_pos.x - text_bounds.origin.x,
            window_pos.y - text_bounds.origin.y,
        );
        // Expected: (144-48, 84-24) = (96, 60)
        assert_eq!(text_area_pos.x, px(96.0));
        assert_eq!(text_area_pos.y, px(60.0));

        // Step 2: TextArea → Content
        let scroll_x = px(view_offset.horizontal_offset as f32 * cell_width.0);
        let scroll_y = px(view_offset.vertical_offset as f32 * line_height.0);
        let content_pos = point(text_area_pos.x + scroll_x, text_area_pos.y + scroll_y);
        // Expected: (96 + 0*8, 60 + 2*20) = (96, 100)
        assert_eq!(content_pos.x, px(96.0));
        assert_eq!(content_pos.y, px(100.0));

        // Step 3: Content → DisplayPoint
        let display_col = (content_pos.x.0 / cell_width.0) as usize;
        let display_row = (content_pos.y.0 / line_height.0) as usize;
        // Expected: (96/8, 100/20) = (12, 5)
        assert_eq!(display_col, 12);
        assert_eq!(display_row, 5);
    }

    #[test]
    fn test_bounds_validation() {
        let text_bounds = create_test_text_bounds();

        // Test position inside bounds
        let inside_pos = point(px(100.0), px(100.0));
        assert!(text_bounds.contains(&inside_pos));

        // Test position outside bounds (left)
        let left_outside = point(px(20.0), px(100.0));
        assert!(!text_bounds.contains(&left_outside));

        // Test position outside bounds (top)
        let top_outside = point(px(100.0), px(10.0));
        assert!(!text_bounds.contains(&top_outside));

        // Test position outside bounds (right)
        let right_outside = point(px(900.0), px(100.0)); // text_bounds.origin.x + size.width = 48 + 800 = 848
        assert!(!text_bounds.contains(&right_outside));

        // Test position outside bounds (bottom)
        let bottom_outside = point(px(100.0), px(700.0)); // text_bounds.origin.y + size.height = 24 + 600 = 624
        assert!(!text_bounds.contains(&bottom_outside));

        // Test position exactly at bounds edges
        let top_left = text_bounds.origin;
        assert!(text_bounds.contains(&top_left));

        let bottom_right = point(
            text_bounds.origin.x + text_bounds.size.width - px(1.0),
            text_bounds.origin.y + text_bounds.size.height - px(1.0),
        );
        assert!(text_bounds.contains(&bottom_right));
    }

    #[test]
    fn test_scroll_position_clamping() {
        // Test scroll position clamping logic
        let max_scroll_x = px(200.0);
        let max_scroll_y = px(500.0);

        // Test normal position (should not be clamped)
        let normal_pos = point(px(100.0), px(250.0));
        let clamped_normal = point(
            normal_pos.x.max(px(0.0)).min(max_scroll_x),
            normal_pos.y.max(px(0.0)).min(max_scroll_y),
        );
        assert_eq!(clamped_normal, normal_pos);

        // Test negative position (should clamp to 0)
        let negative_pos = point(px(-50.0), px(-100.0));
        let clamped_negative = point(
            negative_pos.x.max(px(0.0)).min(max_scroll_x),
            negative_pos.y.max(px(0.0)).min(max_scroll_y),
        );
        assert_eq!(clamped_negative, point(px(0.0), px(0.0)));

        // Test position beyond maximum (should clamp to max)
        let beyond_max = point(px(300.0), px(600.0));
        let clamped_beyond = point(
            beyond_max.x.max(px(0.0)).min(max_scroll_x),
            beyond_max.y.max(px(0.0)).min(max_scroll_y),
        );
        assert_eq!(clamped_beyond, point(max_scroll_x, max_scroll_y));
    }

    #[test]
    fn test_x_overshoot_behavior() {
        // Test X-overshoot tracking for selections past end-of-line
        let line_width = px(80.0); // Line has 10 characters * 8px = 80px
        let cell_width = px(8.0);

        // Click within line bounds
        let within_bounds = px(64.0); // 8 characters in
        let within_col = (within_bounds.0 / cell_width.0) as usize;
        assert_eq!(within_col, 8);

        // Click past end of line (should allow overshoot)
        let past_end = px(120.0); // 15 characters in (past the 10-char line)
        let past_col = (past_end.0 / cell_width.0) as usize;
        assert_eq!(past_col, 15); // Should allow overshoot for selections

        // Verify overshoot distance
        let overshoot_distance = past_end - line_width;
        assert_eq!(overshoot_distance, px(40.0)); // 5 characters * 8px = 40px
    }

    #[test]
    fn test_edge_case_coordinates() {
        let cell_width = px(8.0);
        let line_height = px(20.0);

        // Test coordinates exactly at cell boundaries
        let exact_boundary = point(px(80.0), px(100.0));
        let boundary_col = (exact_boundary.x.0 / cell_width.0) as usize;
        let boundary_row = (exact_boundary.y.0 / line_height.0) as usize;
        assert_eq!(boundary_col, 10); // Exactly at character 10
        assert_eq!(boundary_row, 5); // Exactly at line 5

        // Test coordinates just before boundaries
        let before_boundary = point(px(79.9), px(99.9));
        let before_col = (before_boundary.x.0 / cell_width.0) as usize;
        let before_row = (before_boundary.y.0 / line_height.0) as usize;
        assert_eq!(before_col, 9); // Still character 9
        assert_eq!(before_row, 4); // Still line 4

        // Test coordinates just after boundaries
        let after_boundary = point(px(80.1), px(100.1));
        let after_col = (after_boundary.x.0 / cell_width.0) as usize;
        let after_row = (after_boundary.y.0 / line_height.0) as usize;
        assert_eq!(after_col, 10); // Now character 10
        assert_eq!(after_row, 5); // Now line 5
    }

    #[test]
    fn test_viewport_coordinate_calculations() {
        let text_bounds = create_test_text_bounds();
        let line_height = px(20.0);

        // Calculate visible lines in viewport
        let viewport_height = text_bounds.size.height;
        let lines_in_viewport = (viewport_height.0 / line_height.0) as usize;
        // Expected: 600 / 20 = 30 lines visible
        assert_eq!(lines_in_viewport, 30);

        // Test first/last visible line calculation with scroll
        let scroll_y = px(40.0); // Scrolled down 2 lines
        let first_visible_line = (scroll_y.0 / line_height.0) as usize;
        let last_visible_line = first_visible_line + lines_in_viewport;

        assert_eq!(first_visible_line, 2); // Line 2 is first visible
        assert_eq!(last_visible_line, 32); // Line 32 is last visible (2 + 30)

        // Test visibility check
        let test_line = 15;
        let is_visible = test_line >= first_visible_line && test_line < last_visible_line;
        assert!(is_visible); // Line 15 should be visible

        let out_of_view_line = 35;
        let is_out_of_view =
            out_of_view_line >= first_visible_line && out_of_view_line < last_visible_line;
        assert!(!is_out_of_view); // Line 35 should not be visible
    }

    #[test]
    fn test_coordinate_system_consistency() {
        // Verify that forward and reverse transformations are consistent
        let _text_bounds = create_test_text_bounds();
        let _view_offset = create_test_view_offset();
        let cell_width = px(8.0);
        let line_height = px(20.0);

        // Start with display coordinates
        let original_col = 12;
        let original_row = 5;

        // Convert to pixel coordinates
        let content_pos = point(
            px(original_col as f32 * cell_width.0),
            px(original_row as f32 * line_height.0),
        );

        // Convert back to display coordinates
        let recovered_col = (content_pos.x.0 / cell_width.0) as usize;
        let recovered_row = (content_pos.y.0 / line_height.0) as usize;

        // Should match original values
        assert_eq!(recovered_col, original_col);
        assert_eq!(recovered_row, original_row);

        // Test reverse transformation: pixel → display → pixel
        let original_pixel_pos = point(px(96.0), px(100.0));

        let display_col = (original_pixel_pos.x.0 / cell_width.0) as usize;
        let display_row = (original_pixel_pos.y.0 / line_height.0) as usize;

        let recovered_pixel_pos = point(
            px(display_col as f32 * cell_width.0),
            px(display_row as f32 * line_height.0),
        );

        // Should be at character boundary (may differ due to truncation)
        assert_eq!(recovered_pixel_pos.x, px(96.0)); // 12 * 8 = 96
        assert_eq!(recovered_pixel_pos.y, px(100.0)); // 5 * 20 = 100
    }
}
