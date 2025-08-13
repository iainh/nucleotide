use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
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
        let document_element = DocumentElement::with_scroll_manager(
            self.core.clone(),
            doc_id,
            self.view_id,
            self.style.clone(),
            &self.focus,
            self.is_focused,
            self.scroll_manager.clone(),
            self.scrollbar_state.clone(),
        );

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
                    .child(document_element),
            )
            .when_some(scrollbar_opt, |div, scrollbar| div.child(scrollbar));

        let diags = {
            let _theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

            self.get_diagnostics(cx).into_iter().map(move |_diag| {
                // TODO: Fix new_view API - DiagnosticView disabled for now
                div() // Placeholder
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
        let height = (text.len_lines() - text.char_to_line(anchor)) as u16;

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
            interactivity: Interactivity::default(),
            focus: focus.clone(),
            is_focused,
            scroll_manager,
            scrollbar_state,
        }
        .track_focus(focus)
    }

    pub fn with_scroll_manager(
        core: Entity<Core>,
        doc_id: DocumentId,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
        scroll_manager: ScrollManager,
        scrollbar_state: ScrollbarState,
    ) -> Self {
        Self {
            core,
            doc_id,
            view_id,
            style,
            interactivity: Interactivity::default(),
            focus: focus.clone(),
            is_focused,
            scroll_manager,
            scrollbar_state,
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
        document: &Document,
        view: &View,
        text_format: &TextFormat,
        viewport_height: usize,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        window: &mut Window,
        _cx: &mut App,
    ) -> Vec<nucleotide_editor::LineLayout> {
        let mut line_layouts = Vec::new();
        let text = document.text().slice(..);
        let view_offset = document.view_offset(self.view_id);

        // Create text annotations (empty for now, can be extended later)
        let annotations = TextAnnotations::default();

        // Create DocumentFormatter starting at the viewport anchor
        let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
            text,
            text_format,
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

        let text_origin_x = bounds.origin.x + (view.gutter_offset(document) as f32 * cell_width);
        let mut y_offset = px(0.0);

        // Render visible lines
        while visual_line < view_offset.vertical_offset + viewport_height {
            line_graphemes.clear();
            let line_y = bounds.origin.y + px(1.0) + y_offset;

            // Collect all graphemes for this visual line
            while let Some(grapheme) = formatter.next() {
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
            let shaped_line = window.text_system().shape_line(
                SharedString::from(line_str),
                self.style.font_size.to_pixels(px(16.0)),
                &[],
                None,
            );

            // Store line layout for interaction
            let layout = nucleotide_editor::LineLayout {
                line_idx: current_doc_line,
                shaped_line,
                origin: point(text_origin_x, line_y),
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
            y_offset += line_height;
        }

        line_layouts
    }

    /// Create a shaped line for a specific line, used for cursor positioning and mouse interaction
    #[allow(dead_code)]
    fn create_shaped_line(
        &self,
        line_idx: usize,
        text: RopeSlice,
        first_row: usize,
        end_char: usize,
        fg_color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<ShapedLine> {
        let line_start = text.line_to_char(line_idx);
        let line_end = if line_idx + 1 < text.len_lines() {
            text.line_to_char(line_idx + 1).saturating_sub(1) // Exclude newline
        } else {
            text.len_chars()
        };

        // Check if line is within our view
        let anchor_char = text.line_to_char(first_row);
        if line_start >= end_char || line_end < anchor_char {
            return None;
        }

        // Adjust line bounds to our view
        let line_start = line_start.max(anchor_char);
        let line_end = line_end.min(end_char);

        // For empty lines, line_start may equal line_end, which is valid
        if line_start > line_end {
            return None;
        }

        let line_slice = text.slice(line_start..line_end);
        let line_str: SharedString = RopeWrapper(line_slice).into();

        // Get highlights for this line (re-read core)
        let core = self.core.read(cx);
        let editor = &core.editor;
        let document = match editor.document(self.doc_id) {
            Some(doc) => doc,
            None => {
                // Document was closed, return empty line runs
                return None;
            }
        };
        let view = match editor.tree.try_get(self.view_id) {
            Some(v) => v,
            None => return None,
        };

        let theme = cx.global::<crate::ThemeManager>().helix_theme();
        let line_runs = Self::highlight_line_with_params(
            document,
            view,
            theme,
            editor.mode(),
            &editor.config().cursor_shape,
            &editor.syn_loader,
            self.is_focused,
            line_start,
            line_end,
            fg_color,
            self.style.font(),
        );

        // Create the shaped line
        let shaped_line = window.text_system().shape_line(
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
        debug!(range = ?range, "Creating highlighter for range");

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
        _mode: helix_view::document::Mode,
        _doc: &Document,
        _view: &View,
        _theme: &Theme,
        _cursor_shape_config: &helix_view::editor::CursorShapeConfig,
        _is_window_focused: bool,
    ) -> Vec<(usize, std::ops::Range<usize>)> {
        // TODO: Convert OverlayHighlights to Vec<(usize, Range<usize>)>
        // For now return empty to get compilation working
        Vec::new()
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

    fn highlight_line_with_params(
        doc: &Document,
        view: &View,
        theme: &Theme,
        editor_mode: helix_view::document::Mode,
        cursor_shape: &helix_view::editor::CursorShapeConfig,
        syn_loader: &std::sync::Arc<arc_swap::ArcSwap<helix_core::syntax::Loader>>,
        is_view_focused: bool,
        line_start: usize,
        line_end: usize,
        fg_color: Hsla,
        font: Font,
    ) -> Vec<TextRun> {
        let mut runs = vec![];
        let loader = syn_loader.load();

        // Get syntax highlighter for the entire document view
        let text = doc.text().slice(..);
        let anchor = doc.view_offset(view.id).anchor;
        let height = (text.len_lines() - text.char_to_line(anchor)) as u16;
        let syntax_highlighter = Self::doc_syntax_highlights(doc, anchor, height, theme, &loader);

        // Get overlay highlights
        let overlay_highlights = Self::overlay_highlights(
            editor_mode,
            doc,
            view,
            theme,
            cursor_shape,
            true,
            is_view_focused,
        );

        let default_style = theme.get("ui.text");
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: default_style.bg,
            ..Default::default()
        };

        // Create syntax and overlay highlighters
        let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);
        let mut overlay_hl = OverlayHighlighter::new(overlay_highlights, theme);

        // Get the line text slice to convert character positions to byte lengths
        let line_slice = text.slice(line_start..line_end);

        let mut position = line_start;
        while position < line_end {
            // Advance highlighters to current position
            while position >= syntax_hl.pos {
                syntax_hl.advance();
            }
            while position >= overlay_hl.pos {
                overlay_hl.advance();
            }

            // Calculate next position where style might change
            let next_pos = std::cmp::min(std::cmp::min(syntax_hl.pos, overlay_hl.pos), line_end);

            let char_len = next_pos - position;
            if char_len == 0 {
                break;
            }

            // Convert character length to byte length for this segment
            // Get the text slice for this run and measure its byte length
            let run_start_in_line = position - line_start;
            let run_end_in_line = next_pos - line_start;
            let run_slice = line_slice.slice(run_start_in_line..run_end_in_line);
            let byte_len = run_slice.len_bytes();

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            let fg = style.fg.and_then(color_to_hsla).unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            // Get default background color from theme for reversed modifier
            let default_bg = theme
                .get("ui.background")
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(black());

            let run =
                create_styled_text_run(byte_len, &font, &style, fg, bg, default_bg, underline);
            runs.push(run);
            position = next_pos;
        }

        runs
    }

    #[allow(dead_code)]
    fn highlight_line(
        editor: &Editor,
        doc: &Document,
        view: &View,
        theme: &Theme,
        is_view_focused: bool,
        line_start: usize,
        line_end: usize,
        fg_color: Hsla,
        font: Font,
    ) -> Vec<TextRun> {
        let mut runs = vec![];
        let loader = editor.syn_loader.load();

        // Get syntax highlighter for the entire document view
        let text = doc.text().slice(..);
        let anchor = doc.view_offset(view.id).anchor;
        let height = (text.len_lines() - text.char_to_line(anchor)) as u16;
        let syntax_highlighter = Self::doc_syntax_highlights(doc, anchor, height, theme, &loader);

        // Get overlay highlights
        let overlay_highlights = Self::overlay_highlights(
            editor.mode(),
            doc,
            view,
            theme,
            &editor.config().cursor_shape,
            true,
            is_view_focused,
        );

        let default_style = theme.get("ui.text");
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: default_style.bg,
            ..Default::default()
        };

        // Create syntax and overlay highlighters
        let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);
        let mut overlay_hl = OverlayHighlighter::new(overlay_highlights, theme);

        // Get the line text slice to convert character positions to byte lengths
        let line_slice = text.slice(line_start..line_end);

        let mut position = line_start;
        while position < line_end {
            // Advance highlighters to current position
            while position >= syntax_hl.pos {
                syntax_hl.advance();
            }
            while position >= overlay_hl.pos {
                overlay_hl.advance();
            }

            // Calculate next position where style might change
            let next_pos = std::cmp::min(std::cmp::min(syntax_hl.pos, overlay_hl.pos), line_end);

            let char_len = next_pos - position;
            if char_len == 0 {
                break;
            }

            // Convert character length to byte length for this segment
            let run_start_in_line = position - line_start;
            let run_end_in_line = next_pos - line_start;
            let run_slice = line_slice.slice(run_start_in_line..run_end_in_line);
            let byte_len = run_slice.len_bytes();

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            let fg = style.fg.and_then(color_to_hsla).unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            // Get default background color from theme for reversed modifier
            let default_bg = theme
                .get("ui.background")
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(black());

            let run =
                create_styled_text_run(byte_len, &font, &style, fg, bg, default_bg, underline);
            runs.push(run);
            position = next_pos;
        }

        runs
    }

    #[allow(dead_code)]
    fn highlight(
        editor: &Editor,
        doc: &Document,
        view: &View,
        theme: &Theme,
        is_view_focused: bool,
        anchor: usize,
        lines: u16,
        end_char: usize,
        fg_color: Hsla,
        font: Font,
    ) -> Vec<TextRun> {
        let mut runs = vec![];
        let loader = editor.syn_loader.load();

        // Get syntax highlighter
        let syntax_highlighter = Self::doc_syntax_highlights(doc, anchor, lines, theme, &loader);

        debug!(
            "Syntax highlighter created: {}",
            syntax_highlighter.is_some()
        );

        // Get overlay highlights
        let overlay_highlights = Self::overlay_highlights(
            editor.mode(),
            doc,
            view,
            theme,
            &editor.config().cursor_shape,
            true,
            is_view_focused,
        );

        let text = doc.text().slice(..);
        let default_style = theme.get("ui.text");
        let text_style = helix_view::graphics::Style {
            fg: default_style.fg,
            bg: default_style.bg,
            ..Default::default()
        };

        // Create syntax and overlay highlighters
        let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);
        let mut overlay_hl = OverlayHighlighter::new(overlay_highlights, theme);

        // Get the text slice to convert character positions to byte lengths
        let text_slice = text.slice(anchor..end_char);

        let mut position = anchor;
        while position < end_char {
            // Advance highlighters to current position
            while position >= syntax_hl.pos {
                syntax_hl.advance();
            }
            while position >= overlay_hl.pos {
                overlay_hl.advance();
            }

            // Calculate next position where style might change
            let next_pos = std::cmp::min(std::cmp::min(syntax_hl.pos, overlay_hl.pos), end_char);

            let char_len = next_pos - position;
            if char_len == 0 {
                break;
            }

            // Convert character length to byte length for this segment
            let run_start_in_text = position - anchor;
            let run_end_in_text = next_pos - anchor;
            let run_slice = text_slice.slice(run_start_in_text..run_end_in_text);
            let byte_len = run_slice.len_bytes();

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            // Debug log style changes
            if style.fg != text_style.fg {
                debug!(
                    "Style change at pos {}: {:?} -> {:?}",
                    position, text_style.fg, style.fg
                );
            }

            let fg = style.fg.and_then(color_to_hsla).unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            // Get default background color from theme for reversed modifier
            let default_bg = theme
                .get("ui.background")
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(black());

            let run =
                create_styled_text_run(byte_len, &font, &style, fg, bg, default_bg, underline);
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
        debug!(bounds = ?bounds, "Editor bounds");
        let _core = self.core.clone();
        self.interactivity.prepaint(
            _global_id,
            _inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, _window, cx| {
                // TODO: Content masking not available in new GPUI
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
        let _gutter_width_px = cell_width * gutter_width_cells as f32;

        // Check if soft wrap is enabled early for mouse handlers
        let soft_wrap_enabled = {
            let core = self.core.read(cx);
            let editor = &core.editor;
            if let Some(document) = editor.document(self.doc_id) {
                if let Some(view) = editor.tree.try_get(self.view_id) {
                    let gutter_offset = view.gutter_offset(document);
                    let theme = cx.global::<crate::ThemeManager>().helix_theme();

                    // Calculate viewport width accounting for gutter and some padding
                    let gutter_width_px = gutter_offset as f32 * after_layout.cell_width;
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
        let _soft_wrap_for_mouse = soft_wrap_enabled;
        self.interactivity
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                // Don't start selection if scrollbar is being dragged
                if scrollbar_state_mouse.is_dragging() {
                    return;
                }

                focus.focus(window);

                let mouse_pos = ev.position;

                // Find which line was clicked by checking line layouts
                let clicked_line = line_cache_mouse.find_line_at_position(
                    mouse_pos,
                    bounds.size.width,
                    line_height,
                );

                if let Some(line_layout) = clicked_line {
                    // Calculate x position relative to the line origin
                    let relative_x = mouse_pos.x - line_layout.origin.x;

                    // Find the character index at this x position using GPUI's method
                    // This is more accurate than cell-based calculation
                    let char_idx = line_layout.shaped_line.closest_index_for_x(relative_x);

                    // Update cursor position in the editor
                    core.update(cx, |core, cx| {
                        let editor = &mut core.editor;
                        let view = match editor.tree.try_get(view_id) {
                            Some(v) => v,
                            None => return,
                        };
                        let doc_id = view.doc;
                        let doc = match editor.document(doc_id) {
                            Some(doc) => doc,
                            None => {
                                // Document was closed during interaction
                                return;
                            }
                        };
                        let text = doc.text();

                        // Get the line text to convert between char and grapheme indices
                        let line_start = text.line_to_char(line_layout.line_idx);
                        let line_end = if line_layout.line_idx + 1 < text.len_lines() {
                            text.line_to_char(line_layout.line_idx + 1)
                        } else {
                            text.len_chars()
                        };
                        let line_text = text.slice(line_start..line_end).to_string();

                        // Convert GPUI byte index to grapheme index
                        let grapheme_idx = Self::byte_idx_to_grapheme_idx(&line_text, char_idx);

                        // Convert line index and grapheme offset to document position
                        let pos = line_start + grapheme_idx;

                        if pos <= text.len_chars() {
                            let doc = match editor.document_mut(doc_id) {
                                Some(doc) => doc,
                                None => return,
                            };
                            // Snap to grapheme boundary for proper cursor positioning
                            let text = doc.text().slice(..);
                            let pos = prev_grapheme_boundary(text, pos);

                            // Set selection to the clicked position
                            let selection = Selection::point(pos);
                            doc.set_selection(view_id, selection);
                        }

                        cx.notify();
                    });
                }
            });

        // Handle mouse drag for selection
        let core_drag = self.core.clone();
        let view_id_drag = self.view_id;
        let line_cache_drag = line_cache.clone();
        let scrollbar_state_drag = self.scrollbar_state.clone();

        self.interactivity.on_mouse_move(move |ev, _window, cx| {
            // Only process if dragging (mouse button held down)
            if !ev.dragging() {
                return;
            }

            // Don't select text if scrollbar is being dragged
            if scrollbar_state_drag.is_dragging() {
                return;
            }

            let mouse_pos = ev.position;

            // Find which line is under the mouse using line layouts
            let hovered_line =
                line_cache_drag.find_line_at_position(mouse_pos, bounds.size.width, line_height);

            if let Some(line_layout) = hovered_line {
                // Calculate x position relative to the line origin
                let relative_x = mouse_pos.x - line_layout.origin.x;

                // Find the character index at this x position
                let char_idx = line_layout.shaped_line.closest_index_for_x(relative_x);

                // Update selection end position in the editor
                core_drag.update(cx, |core, cx| {
                    let editor = &mut core.editor;
                    let view = match editor.tree.try_get(view_id_drag) {
                        Some(v) => v,
                        None => return,
                    };
                    let doc_id = view.doc;
                    let doc = match editor.document(doc_id) {
                        Some(doc) => doc,
                        None => {
                            // Document was closed during interaction
                            return;
                        }
                    };
                    let text = doc.text();

                    // Get the line text to convert between char and grapheme indices
                    let line_start = text.line_to_char(line_layout.line_idx);
                    let line_end = if line_layout.line_idx + 1 < text.len_lines() {
                        text.line_to_char(line_layout.line_idx + 1)
                    } else {
                        text.len_chars()
                    };
                    let line_text = text.slice(line_start..line_end).to_string();

                    // Convert GPUI byte index to grapheme index
                    let grapheme_idx = Self::byte_idx_to_grapheme_idx(&line_text, char_idx);

                    // Convert line index and grapheme offset to document position
                    let pos = line_start + grapheme_idx;

                    if pos <= text.len_chars() {
                        let doc = match editor.document_mut(doc_id) {
                            Some(doc) => doc,
                            None => return,
                        };
                        // Snap to grapheme boundary for proper selection
                        let text = doc.text().slice(..);
                        let pos = if pos > 0 {
                            prev_grapheme_boundary(text, pos)
                        } else {
                            pos
                        };

                        // Get current selection and extend it
                        let mut selection = doc.selection(view_id_drag).clone();
                        let range = selection.primary_mut();
                        range.head = pos;
                        doc.set_selection(view_id_drag, selection);
                    }

                    cx.notify();
                });
            }
        });

        // Handle scroll wheel events with optimized performance
        let scroll_manager = self.scroll_manager.clone();
        let core_scroll = self.core.clone();
        let view_id_scroll = self.view_id;
        let doc_id_scroll = self.doc_id;

        // Use a flag to batch scroll updates
        let last_scroll_time = Rc::new(Cell::new(std::time::Instant::now()));
        let pending_scroll_delta = Rc::new(Cell::new(Point::<Pixels>::default()));

        self.interactivity
            .on_scroll_wheel(move |event, _window, cx| {
                // Get the actual line height from scroll manager
                let line_height = scroll_manager.line_height.get();
                let delta = event.delta.pixel_delta(line_height);

                // Accumulate scroll delta for batching
                let current_pending = pending_scroll_delta.get();
                pending_scroll_delta.set(point(
                    current_pending.x + delta.x,
                    current_pending.y + delta.y,
                ));

                // Update scroll position immediately for visual feedback
                let current_offset = scroll_manager.scroll_offset();

                // Clamp the scroll offset to valid bounds
                let max_scroll = scroll_manager.max_scroll_offset();
                let new_offset = point(
                    (current_offset.x + delta.x)
                        .max(px(0.0))
                        .min(max_scroll.width),
                    (current_offset.y + delta.y)
                        .max(-max_scroll.height)
                        .min(px(0.0)),
                );

                // Only update if the offset actually changed
                if new_offset != current_offset {
                    scroll_manager.set_scroll_offset(new_offset);
                } else {
                    // We've hit the scroll bounds, clear pending delta
                    pending_scroll_delta.set(Point::default());
                    return;
                }

                // Only sync to Helix if enough time has passed or delta is significant
                let now = std::time::Instant::now();
                let time_since_last = now.duration_since(last_scroll_time.get());
                let accumulated_delta = pending_scroll_delta.get();

                // Sync to Helix less frequently (every 16ms ~60fps or when delta is large)
                if time_since_last > std::time::Duration::from_millis(16)
                    || accumulated_delta.y.abs() > px(60.0)
                {
                    last_scroll_time.set(now);
                    pending_scroll_delta.set(Point::default());

                    // Update Helix viewport to match the new scroll position
                    core_scroll.update(cx, |core, cx| {
                        let editor = &mut core.editor;

                        // Convert accumulated pixels to lines
                        let scroll_lines = (accumulated_delta.y.0 / line_height.0).round() as isize;
                        if scroll_lines != 0 {
                            // Store cursor position before scrolling to ensure it doesn't move
                            let cursor_pos = if let Some(doc) = editor.document(doc_id_scroll) {
                                let selection = doc.selection(view_id_scroll).clone();
                                Some(selection)
                            } else {
                                None
                            };

                            // Import the scroll command from helix
                            use helix_core::movement::Direction;
                            use helix_term::commands;

                            let count = scroll_lines.unsigned_abs();

                            // Create the correct context for the scroll command
                            let mut ctx = helix_term::commands::Context {
                                editor,
                                register: None,
                                count: None,
                                callback: Vec::new(),
                                on_next_key_callback: None,
                                jobs: &mut core.jobs,
                            };

                            // Call the appropriate scroll function with sync_cursor=false
                            // This ensures the cursor doesn't move with the viewport
                            if scroll_lines > 0 {
                                // Scroll up (content moves down)
                                commands::scroll(&mut ctx, count, Direction::Backward, false);
                            } else {
                                // Scroll down (content moves up)
                                commands::scroll(&mut ctx, count, Direction::Forward, false);
                            }

                            // Restore cursor position if it was changed (safeguard)
                            if let Some(saved_selection) = cursor_pos {
                                if let Some(doc) = ctx.editor.document_mut(doc_id_scroll) {
                                    let current_selection = doc.selection(view_id_scroll).clone();
                                    if current_selection != saved_selection {
                                        doc.set_selection(view_id_scroll, saved_selection);
                                    }
                                }
                            }
                        }

                        // Only notify for Helix sync, visual update already happened
                        cx.notify();
                    });
                }
            });

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
                let text_origin_x = bounds.origin.x + px(2.) + (after_layout.cell_width * gutter_width as f32);

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
                    let ruler_x = text_origin_x + (after_layout.cell_width * ((ruler_col - 1) as f32 - horizontal_offset));

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
                        let gutter_width_px = gutter_offset as f32 * after_layout.cell_width;
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

                    let text_origin_x = bounds.origin.x + (gutter_offset as f32 * after_layout.cell_width);
                    let mut y_offset = px(0.0);
                    let mut visual_line = 0;
                    let mut current_doc_line = text.char_to_line(view_offset.anchor);
                    let viewport_height = (bounds.size.height / after_layout.line_height) as usize;

                    // Skip lines before the viewport - need to consume all graphemes for skipped lines
                    let mut pending_grapheme = None;
                    while visual_line < view_offset.vertical_offset {
                        while let Some(grapheme) = formatter.next() {
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
                        while let Some(grapheme) = formatter.next() {
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
                                match &grapheme.raw {
                                    helix_core::graphemes::Grapheme::Other { g } => {
                                        wrap_indicator_len += g.len(); // Track byte length
                                        line_str.push_str(g);
                                    }
                                    _ => {}
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
                                .last()
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
                                    Self::highlight_line_with_params(
                                        document,
                                        view,
                                        &editor_theme,
                                        editor_mode,
                                        &cursor_shape,
                                        &syn_loader,
                                        self.is_focused,
                                        line_start,
                                        line_end,
                                        fg_color,
                                        self.style.font(),
                                    )
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
                            let layout = nucleotide_editor::LineLayout {
                                line_idx: current_doc_line,
                                shaped_line,
                                origin: point(text_origin_x, line_y),
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
                            while let Some(grapheme) = formatter.next() {
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
                            while let Some(grapheme) = formatter.next() {
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
                        let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
                            text,
                            &text_format,
                            &annotations,
                            view_offset.anchor,
                        );

                        let mut _visual_line = 0;
                        let mut cursor_visual_line = None;
                        let mut cursor_visual_col = 0;

                        // Iterate through graphemes to find cursor position (following Helix's approach)
                        while let Some(grapheme) = formatter.next() {
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
                                // Calculate cursor position
                                let relative_line = cursor_line - view_offset.vertical_offset;
                                let cursor_y = bounds.origin.y + px(1.0) + (after_layout.line_height * relative_line as f32);
                                let cursor_x = text_origin_x + (after_layout.cell_width * cursor_visual_col as f32);

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

                        let line_runs = Self::highlight_line_with_params(
                            document,
                            view,
                            &editor_theme,
                            editor_mode,
                            &cursor_shape,
                            &syn_loader,
                            self.is_focused,
                            line_start,
                            line_end,
                            fg_color,
                            self.style.font(),
                        );

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

                    // Always store the line layout for cursor positioning
                    let layout = nucleotide_editor::LineLayout {
                        line_idx,
                        shaped_line,
                        origin: text_origin,
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
                                    let cursor_byte_offset = line_text.chars().take(cursor_char_offset).map(|c| c.len_utf8()).sum::<usize>();

                                    (line_start, cursor_char_offset, cursor_byte_offset, line_text)
                                };

                                // Get the x position from the shaped line using byte offset
                                let cursor_x = line_layout.shaped_line.x_for_index(cursor_byte_offset);

                                // Debug logging
                                debug!("Cursor rendering - line: {cursor_line}, char_offset: {cursor_char_offset}, byte_offset: {cursor_byte_offset}, x: {cursor_x:?}, viewport_row: {viewport_row}");

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

                                // Cursor origin is relative to the line's origin
                                let cursor_origin = gpui::Point::new(
                                    cursor_x,
                                    px(0.0) // Relative to line origin
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

                                // Paint cursor at the line's origin
                                cursor.paint(line_layout.origin, window, cx);
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
                    let lines = (first_row..gutter_last_row)
                        .enumerate()
                        .map(|(visual_line, doc_line)| LinePos {
                            first_visual_line: true,
                            doc_line,
                            visual_line: visual_line as u16,
                            start_char_idx: 0,
                        });

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
            })
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
                // TODO handle softwrap in gutters
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
        let origin_y = self.origin.y + self.after_layout.line_height * y as f32;
        let origin_x = self.origin.x + self.after_layout.cell_width * x as f32;

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
        Err(_) => {
            debug!("Highlight index out of bounds for current theme, using default style");
            helix_view::graphics::Style::default()
        }
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
