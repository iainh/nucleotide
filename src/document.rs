use std::borrow::Cow;
use std::rc::Rc;

use gpui::*;
use gpui::{point, size, TextRun};
use gpui::prelude::FluentBuilder;
use helix_core::{
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    ropey::RopeSlice,
    syntax::{self, HighlightEvent},
    Selection, Uri,
};
use helix_lsp::lsp::Diagnostic;
use helix_term::ui::EditorView;
// Import helix's syntax highlighting system
use helix_view::{graphics::CursorKind, Document, DocumentId, Editor, Theme, View, ViewId};
use log::debug;

use crate::line_cache::LineLayoutCache;
use crate::scroll_manager::{ScrollManager, ViewOffset};
use crate::ui::scrollbar::{Scrollbar, ScrollbarState, ScrollableHandle};
use crate::utils::color_to_hsla;
use crate::{Core, Input};
use helix_stdx::rope::RopeSliceExt;

/// Custom scroll handle for DocumentView that integrates with ScrollManager
#[derive(Clone)]
pub struct DocumentScrollHandle {
    scroll_manager: ScrollManager,
    on_change: Option<Rc<dyn Fn()>>,
}

impl std::fmt::Debug for DocumentScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentScrollHandle")
            .field("scroll_manager", &self.scroll_manager)
            .field("on_change", &self.on_change.is_some())
            .finish()
    }
}

impl DocumentScrollHandle {
    pub fn new(scroll_manager: ScrollManager) -> Self {
        Self { 
            scroll_manager,
            on_change: None,
        }
    }
    
    pub fn with_callback(scroll_manager: ScrollManager, on_change: impl Fn() + 'static) -> Self {
        Self {
            scroll_manager,
            on_change: Some(Rc::new(on_change)),
        }
    }
}

impl ScrollableHandle for DocumentScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        self.scroll_manager.max_scroll_offset()
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.scroll_manager.set_scroll_offset(point);
        
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
    input: Entity<Input>,
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
        input: Entity<Input>,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
    ) -> Self {
        // Create scroll manager with placeholder doc_id (will be updated in render)
        let line_height = px(20.0); // Default, will be updated
        let scroll_manager = ScrollManager::new(DocumentId::default(), view_id, line_height);
        
        // Create custom scroll handle that wraps our scroll manager
        let scroll_handle = DocumentScrollHandle::new(scroll_manager.clone());
        let scrollbar_state = ScrollbarState::new(scroll_handle);
        
        Self {
            core,
            input,
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

    /// Convert a Helix anchor (character position) to scroll pixels
    fn anchor_to_scroll_px(&self, anchor_char: usize, document: &helix_view::Document) -> Pixels {
        let row = document.text().char_to_line(anchor_char);
        px(row as f32 * self.line_height.0)
    }

    /// Convert scroll pixels to a Helix anchor (character position)
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
            let view = editor.tree.get(self.view_id);
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
            let view = editor.tree.get(self.view_id);
            view.doc
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
                self.scroll_manager.set_viewport_size(size(px(800.0), viewport_height));
                
                // Don't recreate scrollbar state - it's already using our scroll manager
                
                debug!("Document has {} lines, viewport shows ~30 lines", total_lines);
            }
        }

        // Create the DocumentElement that will handle the actual rendering
        // Pass the same scroll manager to ensure state is shared
        let document_element = DocumentElement::with_scroll_manager(
            self.core.clone(),
            doc_id,
            self.view_id,
            self.style.clone(),
            &self.focus,
            self.is_focused,
            self.scroll_manager.clone(),
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
                    .child(document_element)
            )
            .when_some(
                scrollbar_opt,
                |div, scrollbar| div.child(scrollbar)
            );

        let mut status = crate::statusline::StatusLine::new(
            self.core.clone(),
            doc_id,
            self.view_id,
            self.is_focused,
            self.style.clone(),
        );
        
        // Add LSP state if available
        if let Some(lsp_state) = self.core.read(cx).lsp_state.as_ref() {
            status = status.with_lsp_state(lsp_state.clone());
        }

        let diags = {
            let _theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme().clone();

            self.get_diagnostics(cx).into_iter().map(move |_diag| {
                // TODO: Fix new_view API - DiagnosticView disabled for now
                div() // Placeholder
            })
        };

        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .child(editor_content)
            .child(status)
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
}

impl IntoElement for DocumentElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl DocumentElement {
    /// Convert a character index within a line to a grapheme index
    /// This is needed because GPUI's shaped line works with UTF-8 character indices
    /// but Helix works with grapheme cluster indices
    fn char_idx_to_grapheme_idx(line_text: &str, char_idx: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        
        let mut grapheme_idx = 0;
        let mut current_char_idx = 0;
        
        for grapheme in line_text.graphemes(true) {
            if current_char_idx >= char_idx {
                break;
            }
            current_char_idx += grapheme.len();
            grapheme_idx += 1;
        }
        
        grapheme_idx
    }
    
    /// Convert a grapheme index within a line to a character index for GPUI
    fn grapheme_idx_to_char_idx(line_text: &str, grapheme_idx: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        
        let mut char_idx = 0;
        for (idx, grapheme) in line_text.graphemes(true).enumerate() {
            if idx >= grapheme_idx {
                break;
            }
            char_idx += grapheme.len();
        }
        
        char_idx
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
        let scroll_manager = ScrollManager::new(doc_id, view_id, line_height);
        
        Self {
            core,
            doc_id,
            view_id,
            style,
            interactivity: Interactivity::default(),
            focus: focus.clone(),
            is_focused,
            scroll_manager,
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
        }
        .track_focus(focus)
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
        cx: &mut App
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
        let view = editor.tree.get(self.view_id);
        
        let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
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
        let shaped_line = window.text_system()
            .shape_line(line_str, self.style.font_size.to_pixels(px(16.0)), &line_runs, None);
            
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
        debug!("Document has syntax support for: {:?}", doc.language_name());

        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));
        let range = Self::viewport_byte_range(text, row, height);
        let range = range.start as u32..range.end as u32;
        debug!("Creating highlighter for range: {range:?}");

        let highlighter = syntax.highlighter(text, syn_loader, range);
        Some(highlighter)
    }

    fn viewport_byte_range(text: RopeSlice, row: usize, height: u16) -> std::ops::Range<usize> {
        let start = text.line_to_byte(row);
        let end_row = (row + height as usize).min(text.len_lines());
        let end = text.line_to_byte(end_row);
        start..end
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
        is_window_focused: bool,
        _is_view_focused: bool,
    ) -> helix_core::syntax::OverlayHighlights {
        // Get selection highlights from helix-term EditorView
        EditorView::doc_selection_highlights(
            mode,
            doc,
            view,
            theme,
            cursor_shape_config,
            is_window_focused,
        )
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

            let len = next_pos - position;
            if len == 0 {
                break;
            }

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            let fg = style
                .fg
                .and_then(color_to_hsla)
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            let underline = underline.map(|color| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: true,
            });

            let run = TextRun {
                len,
                font: font.clone(),
                color: fg,
                background_color: bg,
                underline,
                strikethrough: None,
            };
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

            let len = next_pos - position;
            if len == 0 {
                break;
            }

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            let fg = style
                .fg
                .and_then(color_to_hsla)
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            let underline = underline.map(|color| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: true,
            });

            let run = TextRun {
                len,
                font: font.clone(),
                color: fg,
                background_color: bg,
                underline,
                strikethrough: None,
            };
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

            let len = next_pos - position;
            if len == 0 {
                break;
            }

            // Combine syntax and overlay styles
            let style = syntax_hl.style.patch(overlay_hl.style);

            // Debug log style changes
            if style.fg != text_style.fg {
                debug!(
                    "Style change at pos {}: {:?} -> {:?}",
                    position, text_style.fg, style.fg
                );
            }

            let fg = style
                .fg
                .and_then(color_to_hsla)
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(color_to_hsla);
            let underline = style.underline_color.and_then(color_to_hsla);
            let underline = underline.map(|color| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: true,
            });

            let run = TextRun {
                len,
                font: font.clone(),
                color: fg,
                background_color: bg,
                underline,
                strikethrough: None,
            };
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
        debug!("editor bounds {bounds:?}");
        let _core = self.core.clone();
        self.interactivity
            .prepaint(_global_id, _inspector_id, bounds, bounds.size, window, cx, |_, _, hitbox, _window, cx| {
                // TODO: Content masking not available in new GPUI
                {
                    let font_id = cx.text_system().resolve_font(&self.style.font());
                    let font_size = self.style.font_size.to_pixels(px(16.0));
                    let line_height = self.style.line_height_in_pixels(px(16.0));
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
            })
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
        
        let gutter_width_cells = {
            let editor = &core.read(cx).editor;
            let view = editor.tree.get(view_id);
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
            self.scroll_manager.set_scroll_offset(point(px(0.0), -y));
            
            view.gutter_offset(doc)
        };
        let _gutter_width_px = cell_width * gutter_width_cells as f32;
        
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
        self.interactivity
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                focus.focus(window);
                
                let mouse_pos = ev.position;
                
                // Find which line was clicked by checking line layouts
                let clicked_line = line_cache_mouse.find_line_at_position(mouse_pos, bounds.size.width, line_height);
                
                if let Some(line_layout) = clicked_line {
                    // Calculate x position relative to the line origin
                    let relative_x = mouse_pos.x - line_layout.origin.x;
                    
                    // Find the character index at this x position using GPUI's method
                    // This is more accurate than cell-based calculation
                    let char_idx = line_layout.shaped_line.closest_index_for_x(relative_x);
                    
                    // Update cursor position in the editor
                    core.update(cx, |core, cx| {
                        let editor = &mut core.editor;
                        let view = editor.tree.get(view_id);
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
                        
                        // Convert GPUI char index to grapheme index
                        let grapheme_idx = Self::char_idx_to_grapheme_idx(&line_text, char_idx);
                        
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
        
        self.interactivity
            .on_mouse_move(move |ev, _window, cx| {
                // Only process if dragging (mouse button held down)
                if !ev.dragging() {
                    return;
                }
                
                let mouse_pos = ev.position;
                
                // Find which line is under the mouse using line layouts
                let hovered_line = line_cache_drag.find_line_at_position(mouse_pos, bounds.size.width, line_height);
                
                if let Some(line_layout) = hovered_line {
                    // Calculate x position relative to the line origin
                    let relative_x = mouse_pos.x - line_layout.origin.x;
                    
                    // Find the character index at this x position
                    let char_idx = line_layout.shaped_line.closest_index_for_x(relative_x);
                    
                    // Update selection end position in the editor
                    core_drag.update(cx, |core, cx| {
                        let editor = &mut core.editor;
                        let view = editor.tree.get(view_id_drag);
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
                        
                        // Convert GPUI char index to grapheme index
                        let grapheme_idx = Self::char_idx_to_grapheme_idx(&line_text, char_idx);
                        
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
        
        // Handle scroll wheel events
        let scroll_manager = self.scroll_manager.clone();
        let core_scroll = self.core.clone();
        let view_id_scroll = self.view_id;
        self.interactivity
            .on_scroll_wheel(move |event, _window, cx| {
                // Update scroll position based on wheel delta
                let current_offset = scroll_manager.scroll_offset();
                let delta = event.delta.pixel_delta(px(20.0)); // Use line height as scroll unit
                // GPUI convention: scrolling down makes offset more negative
                let new_offset = point(
                    current_offset.x + delta.x,
                    current_offset.y + delta.y,
                );
                
                scroll_manager.set_scroll_offset(new_offset);
                
                // Update Helix viewport to match the new scroll position
                core_scroll.update(cx, |core, cx| {
                    let editor = &mut core.editor;
                    
                    // Use Helix's scroll commands to properly update the view
                    let scroll_lines = (delta.y.0 / 20.0).round() as isize; // Convert pixels to lines
                    if scroll_lines != 0 {
                        // Import the scroll command from helix
                        use helix_term::commands;
                        use helix_core::movement::Direction;
                        
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
                        
                        // Call the appropriate scroll function
                        if scroll_lines > 0 {
                            // Scroll up (content moves down)
                            commands::scroll(&mut ctx, count, Direction::Backward, false);
                        } else {
                            // Scroll down (content moves up)
                            commands::scroll(&mut ctx, count, Direction::Forward, false);
                        }
                        
                        debug!("Scrolled by {} lines", scroll_lines);
                    }
                    
                    cx.notify();
                });
            });

        let is_focused = self.is_focused;

        self.interactivity
            .paint(_global_id, _inspector_id, bounds, after_layout.hitbox.as_ref(), window, cx, |_, window, cx| {
                let core = self.core.read(cx);
                let editor = &core.editor;

                let view = editor.tree.get(self.view_id);
                let _viewport = view.area;

                let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
                let default_style = theme.get("ui.background");
                let bg_color = default_style.bg
                    .and_then(color_to_hsla)
                    .unwrap_or(black());
                // Get mode-specific cursor theme like terminal version
                let mode = editor.mode();
                let _base_cursor_style = theme.get("ui.cursor");
                let base_primary_cursor_style = theme.get("ui.cursor.primary");
                
                // Try to get mode-specific cursor style, fallback to base
                let cursor_style = match mode {
                    helix_view::document::Mode::Insert => {
                        let style = theme.get("ui.cursor.primary.insert");
                        if style.fg.is_some() || style.bg.is_some() {
                            style
                        } else {
                            base_primary_cursor_style
                        }
                    }
                    helix_view::document::Mode::Select => {
                        let style = theme.get("ui.cursor.primary.select");
                        if style.fg.is_some() || style.bg.is_some() {
                            style
                        } else {
                            base_primary_cursor_style
                        }
                    }
                    helix_view::document::Mode::Normal => {
                        let style = theme.get("ui.cursor.primary.normal");
                        if style.fg.is_some() || style.bg.is_some() {
                            style
                        } else {
                            base_primary_cursor_style
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
                        let char_str = if char_str == "\n" {
                            "⏎".into() // Use return symbol for newlines
                        } else if char_str == "\r\n" {
                            "⏎".into()
                        } else if char_str == "\r" {
                            "⏎".into()
                        } else {
                            char_str
                        };
                        
                        if !char_str.is_empty() {
                                    // For block cursor, invert colors: use cursor background as text color
                                    // and render on transparent background (cursor will provide the bg)
                                    let text_color = if let Some(bg) = cursor_style.bg {
                                        color_to_hsla(bg).unwrap_or(black())
                                    } else {
                                        // If no cursor bg defined, use inverse of foreground
                                        black()
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
                
                // Extract necessary values before the loop to avoid borrowing issues
                let editor_theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme().clone();
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
                
                // Shape cursor text before dropping core borrow
                let cursor_text_shaped = cursor_text.map(|(char_str, text_color)| {
                    let run = TextRun {
                        len: char_str.len(),
                        font: self.style.font(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    
                    window.text_system()
                        .shape_line(char_str, self.style.font_size.to_pixels(px(16.0)), &[run], None)
                });
                
                // Drop the core borrow before the loop
                // core goes out of scope here
                
                let text = doc_text.slice(..);
                
                // Update the shared line layouts for mouse interaction
                
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
                        let view = editor.tree.get(view_id);
                        
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
                    
                    // Always create a shaped line, even for empty lines (needed for cursor positioning)
                    let shaped_line = if !line_str.is_empty() {
                        // First, paint any background highlights
                        let mut char_offset = 0;
                        for run in &line_runs {
                            if let Some(bg_color) = run.background_color {
                                let run_width = after_layout.cell_width * run.len as f32;
                                let bg_origin = point(
                                    text_origin_x + (after_layout.cell_width * char_offset as f32),
                                    bounds.origin.y + px(1.) + y_offset
                                );
                                let bg_bounds = Bounds {
                                    origin: bg_origin,
                                    size: size(run_width, after_layout.line_height),
                                };
                                window.paint_quad(fill(bg_bounds, bg_color));
                            }
                            char_offset += run.len;
                        }
                        
                        // Try to get cached shaped line first
                        let cache_key = crate::line_cache::ShapedLineKey {
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
                        
                        if let Err(e) = shaped.paint(text_origin, after_layout.line_height, window, cx) {
                            log::error!("Failed to paint text: {e:?}");
                        }
                        shaped
                    } else {
                        // Create an empty shaped line for cursor positioning
                        let cache_key = crate::line_cache::ShapedLineKey {
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
                    let layout = crate::line_cache::LineLayout {
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
                                
                                let (_line_start, cursor_grapheme_offset, line_text) = if is_phantom_line {
                                    // Phantom line: cursor is at end of file
                                    (text.len_chars(), 0, String::new())
                                } else {
                                    // Normal line
                                    let line_start = text.line_to_char(cursor_line);
                                    let cursor_grapheme_offset = cursor_char_idx.saturating_sub(line_start);
                                    
                                    // Get the line text to convert grapheme offset to char offset
                                    let line_end = if cursor_line + 1 < text.len_lines() {
                                        text.line_to_char(cursor_line + 1)
                                    } else {
                                        text.len_chars()
                                    };
                                    let line_text = text.slice(line_start..line_end).to_string();
                                    (line_start, cursor_grapheme_offset, line_text)
                                };
                                
                                // Convert grapheme offset to char offset for GPUI
                                let cursor_char_offset = Self::grapheme_idx_to_char_idx(&line_text, cursor_grapheme_offset);
                                
                                // Get the x position from the shaped line
                                let cursor_x = line_layout.shaped_line.x_for_index(cursor_char_offset);
                                
                                // Debug logging
                                debug!("Cursor rendering - line: {cursor_line}, grapheme_offset: {cursor_grapheme_offset}, char_offset: {cursor_char_offset}, x: {cursor_x:?}, viewport_row: {viewport_row}");
                                
                                // Debug info about the line content
                                debug!("Line content: {:?}, cursor at grapheme offset {} (char offset {}) in line, is_phantom: {}", 
                                    &line_text, cursor_grapheme_offset, cursor_char_offset, is_phantom_line);
                                
                                // Cursor origin is relative to the line's origin
                                let cursor_origin = gpui::Point::new(
                                    cursor_x,
                                    px(0.0) // Relative to line origin
                                );
                                
                                let cursor_color = cursor_style
                                    .fg
                                    .and_then(color_to_hsla)
                                    .or_else(|| cursor_style.bg.and_then(color_to_hsla))
                                    .unwrap_or(fg_color);

                                let mut cursor = Cursor {
                                    origin: cursor_origin,
                                    kind: cursor_kind,
                                    color: cursor_color,
                                    block_width: after_layout.cell_width,
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
                    let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
                    let view = editor.tree.get(self.view_id);
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
                            log::error!("Failed to paint gutter line: {e:?}");
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

        let fg_color = style
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0., 0., 1., 1.));
        if let Some(text) = text {
            let run = TextRun {
                len: text.len(),
                font: self.style.font(),
                color: fg_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = self
                .text_system
                .shape_line(text.to_string().into(), self.after_layout.font_size, &[run], None);
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

        // Paint the cursor quad
        window.paint_quad(fill(bounds, self.color));

        if let Some(text) = &self.text {
            if let Err(e) = text.paint(self.origin + origin, self.line_height, window, cx) {
                log::error!("Failed to paint cursor text: {e:?}");
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
            log::debug!("Highlight index out of bounds for current theme, using default style");
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

