use std::borrow::Cow;

use gpui::{prelude::FluentBuilder, *};
use gpui::{point, size, TextRun};
use helix_core::{
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    ropey::RopeSlice,
    syntax::{self, HighlightEvent},
    Selection, Uri,
};
use helix_lsp::lsp::{Diagnostic, DiagnosticSeverity, NumberOrString};
use helix_term::ui::EditorView;
// Import helix's syntax highlighting system
use helix_view::{graphics::CursorKind, Document, DocumentId, Editor, Theme, View, ViewId};
use log::debug;

use crate::utils::color_to_hsla;
use crate::{Core, Input, InputEvent};
use helix_stdx::rope::RopeSliceExt;

pub struct DocumentView {
    core: Entity<Core>,
    input: Entity<Input>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
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
        Self {
            core,
            input,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) {
        self.is_focused = is_focused;
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
            let document = editor.document(doc_id).unwrap();
            let text = document.text();

            let primary_idx = document
                .selection(self.view_id)
                .primary()
                .cursor(text.slice(..));
            let cursor_pos = view.screen_coords_at_pos(document, text.slice(..), primary_idx);

            let anchor = document.view_offset(self.view_id).anchor;
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
        println!("{:?}: rendering document view", self.view_id);
        
        // Focus handlers should be set up once, not in render
        // The is_focused state is managed externally via set_focused()

        let doc_id = {
            let editor = &self.core.read(cx).editor;
            let view = editor.tree.get(self.view_id);
            view.doc
        };

        // Use DocumentElement for proper editor rendering
        let doc = DocumentElement::new(
            self.core.clone(),
            doc_id.clone(),
            self.view_id.clone(),
            self.style.clone(),
            &self.focus,
            self.is_focused,
        )
        .on_scroll_wheel(cx.listener(move |view, ev: &ScrollWheelEvent, _window, cx| {
            use helix_core::movement::Direction;
            let view_id = view.view_id;
            let line_height = px(20.0); // Approximate line height
            
            // Extract y delta from ScrollDelta enum
            let delta_y = match ev.delta {
                ScrollDelta::Pixels(point) => point.y,
                ScrollDelta::Lines(point) => px(point.y * 20.0), // Convert lines to pixels
            };
            
            if delta_y != px(0.) {
                let lines = delta_y / line_height;
                let direction = if lines > 0. {
                    Direction::Backward
                } else {
                    Direction::Forward
                };
                let line_count = 1 + lines.abs() as usize;

                view.input.update(cx, |_, cx| {
                    cx.emit(InputEvent::ScrollLines {
                        direction,
                        line_count,
                        view_id,
                    })
                });
            }
        }));

        let status = crate::statusline::StatusLine::new(
            self.core.clone(),
            doc_id.clone(),
            self.view_id,
            self.is_focused,
            self.style.clone(),
        );

        let diags = {
            let _theme = self.core.read(cx).editor.theme.clone();

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
            .child(doc)
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
        Self {
            core,
            doc_id,
            view_id,
            style,
            interactivity: Interactivity::default(),
            focus: focus.clone(),
            is_focused,
        }
        .track_focus(&focus)
    }
    
    /// Create a shaped line for a specific line, used for cursor positioning and mouse interaction
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
        let anchor = first_row;
        if line_start >= end_char || line_end < anchor {
            return None;
        }
        
        // Adjust line bounds to our view
        let line_start = line_start.max(anchor);
        let line_end = line_end.min(end_char);
        
        if line_start >= line_end {
            return None;
        }
        
        let line_slice = text.slice(line_start..line_end);
        let line_str: SharedString = RopeWrapper(line_slice).into();
        
        if line_str.is_empty() {
            return None;
        }
        
        // Get highlights for this line (re-read core)
        let core = self.core.read(cx);
        let editor = &core.editor;
        let document = editor.document(self.doc_id).unwrap();
        let view = editor.tree.get(self.view_id);
        
        let line_runs = Self::highlight_line_with_params(
            document,
            view,
            &editor.theme,
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
        debug!("Creating highlighter for range: {:?}", range);

        let highlighter = syntax.highlighter(text, syn_loader, range);
        Some(highlighter)
    }

    fn viewport_byte_range(text: RopeSlice, row: usize, height: u16) -> std::ops::Range<usize> {
        let start = text.line_to_byte(row);
        let end_row = (row + height as usize).min(text.len_lines());
        let end = text.line_to_byte(end_row);
        start..end
    }

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
                .and_then(|fg| color_to_hsla(fg))
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(|bg| color_to_hsla(bg));
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
                .and_then(|fg| color_to_hsla(fg))
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(|bg| color_to_hsla(bg));
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
                .and_then(|fg| color_to_hsla(fg))
                .unwrap_or(fg_color);
            let bg = style.bg.and_then(|bg| color_to_hsla(bg));
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

/// Stores layout information for a single line in the document
#[derive(Clone)]
struct LineLayout {
    line_idx: usize,
    shaped_line: ShapedLine,
    origin: gpui::Point<Pixels>,
}

struct RopeWrapper<'a>(RopeSlice<'a>);

impl<'a> Into<SharedString> for RopeWrapper<'a> {
    fn into(self) -> SharedString {
        let cow: Cow<'_, str> = self.0.into();
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
        debug!("editor bounds {:?}", bounds);
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
                        .unwrap()
                        .size
                        .width;
                    let cell_width = cx
                        .text_system()
                        .advance(font_id, font_size, 'm')
                        .unwrap()
                        .width;
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
        let gutter_width_cells = {
            let editor = &core.read(cx).editor;
            let view = editor.tree.get(view_id);
            let doc = editor.document(self.doc_id).unwrap();
            view.gutter_offset(doc)
        };
        let gutter_width_px = cell_width * gutter_width_cells as f32;
        
        // Store line layouts in element state for mouse interaction
        let line_layouts = std::rc::Rc::new(std::cell::RefCell::new(Vec::<LineLayout>::new()));
        let line_layouts_for_mouse = line_layouts.clone();
        let line_layouts_for_drag = line_layouts.clone();
        let line_layouts_for_paint = line_layouts.clone();
        
        self.interactivity
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                focus.focus(window);
                
                let mouse_pos = ev.position;
                
                // Find which line was clicked by checking line layouts
                let line_layouts = line_layouts_for_mouse.borrow();
                let clicked_line = line_layouts.iter().find(|layout| {
                    let line_bounds = Bounds {
                        origin: layout.origin,
                        size: size(bounds.size.width, line_height),
                    };
                    line_bounds.contains(&mouse_pos)
                });
                
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
                        let doc = editor.document(doc_id).unwrap();
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
                            let doc = editor.document_mut(doc_id).unwrap();
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
        
        self.interactivity
            .on_mouse_move(move |ev, _window, cx| {
                // Only process if dragging (mouse button held down)
                if !ev.dragging() {
                    return;
                }
                
                let mouse_pos = ev.position;
                
                // Find which line is under the mouse using line layouts
                let line_layouts = line_layouts_for_drag.borrow();
                let hovered_line = line_layouts.iter().find(|layout| {
                    let line_bounds = Bounds {
                        origin: layout.origin,
                        size: size(bounds.size.width, line_height),
                    };
                    line_bounds.contains(&mouse_pos)
                });
                
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
                        let doc = editor.document(doc_id).unwrap();
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
                            let doc = editor.document_mut(doc_id).unwrap();
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

        let is_focused = self.is_focused;

        self.interactivity
            .paint(_global_id, _inspector_id, bounds, after_layout.hitbox.as_ref(), window, cx, |_, window, cx| {
                let core = self.core.read(cx);
                let editor = &core.editor;

                let view = editor.tree.get(self.view_id);
                let _viewport = view.area;

                let theme = &editor.theme;
                let default_style = theme.get("ui.background");
                let bg_color = color_to_hsla(default_style.bg.unwrap()).unwrap_or(black());
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

                let document = editor.document(self.doc_id).unwrap();
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
                    debug!("Actual position - line: {}, col_in_line: {}, line_start: {}", 
                           line, col_in_line, line_start);
                } else {
                    debug!("Warning: screen_coords_at_pos returned None for cursor position {}", primary_idx);
                }
                let gutter_overflow = gutter_width == 0;
                if !gutter_overflow {
                    debug!("need to render gutter {}", gutter_width);
                }

                let _cursor_row = cursor_pos.map(|p| p.row);
                let anchor = document.view_offset(self.view_id).anchor;
                let total_lines = text.len_lines();
                let first_row = text.char_to_line(anchor.min(text.len_chars()));

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
                // println!("first row is {}", row);
                let last_row = (first_row + after_layout.rows + 1).min(total_lines);
                // println!("first row is {first_row} last row is {last_row}");
                let end_char = text.line_to_char(std::cmp::min(last_row, total_lines));

                // Render text line by line to avoid newline issues
                let mut y_offset = px(0.);
                let text_origin_x = bounds.origin.x + px(2.) + (after_layout.cell_width * gutter_width as f32);
                
                // Extract necessary values before the loop to avoid borrowing issues
                let editor_theme = editor.theme.clone();
                let editor_mode = editor.mode();
                let cursor_shape = editor.config().cursor_shape.clone();
                let syn_loader = editor.syn_loader.clone();
                
                // Clone text to avoid borrowing issues
                let doc_text = document.text().clone();
                
                // Also extract document_id and view_id for use in the loop
                let doc_id = self.doc_id;
                let view_id = self.view_id;
                
                // Extract cursor-related data before dropping core
                let cursor_char_idx = document
                    .selection(self.view_id)
                    .primary()
                    .cursor(text.slice(..));
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
                let mut line_layouts_local: Vec<LineLayout> = Vec::new();
                
                // Clear and update the shared line layouts
                line_layouts_for_paint.borrow_mut().clear();
                
                for line_idx in first_row..last_row {
                    let line_start = text.line_to_char(line_idx);
                    let line_end = if line_idx + 1 < total_lines {
                        text.line_to_char(line_idx + 1).saturating_sub(1) // Exclude newline
                    } else {
                        text.len_chars()
                    };
                    
                    // Skip empty lines or lines outside our view
                    if line_start >= end_char || line_end < anchor {
                        y_offset += after_layout.line_height;
                        continue;
                    }
                    
                    // Adjust line bounds to our view
                    let line_start = line_start.max(anchor);
                    let line_end = line_end.min(end_char);
                    
                    if line_start >= line_end {
                        y_offset += after_layout.line_height;
                        continue;
                    }
                    
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
                    let document = editor.document(doc_id).unwrap();
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
                        
                        // Shape and paint the text
                        let shaped = window.text_system()
                            .shape_line(line_str, self.style.font_size.to_pixels(px(16.0)), &line_runs, None);
                        
                        shaped.paint(text_origin, after_layout.line_height, window, cx).unwrap();
                        shaped
                    } else {
                        // Create an empty shaped line for cursor positioning
                        window.text_system()
                            .shape_line("".into(), self.style.font_size.to_pixels(px(16.0)), &[], None)
                    };
                    
                    // Always store the line layout for cursor positioning
                    let layout = LineLayout {
                        line_idx,
                        shaped_line,
                        origin: text_origin,
                    };
                    line_layouts_local.push(layout.clone());
                    line_layouts_for_paint.borrow_mut().push(layout);
                    
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
                        if let Some(view) = editor.tree.try_get(self.view_id) {
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
                        if cursor_line >= first_row && cursor_line < last_row {
                            let viewport_row = cursor_line.saturating_sub(first_row);
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
                        let cursor_line = text.char_to_line(cursor_char_idx);
                        
                        // Check if cursor line is in the rendered range
                        if cursor_line >= first_row && cursor_line < last_row {
                            // Find the line layout for the cursor line
                            if let Some(line_layout) = line_layouts_local.iter().find(|layout| layout.line_idx == cursor_line) {
                                // Calculate the cursor position within the line
                                let line_start = text.line_to_char(cursor_line);
                                let cursor_grapheme_offset = cursor_char_idx.saturating_sub(line_start);
                                
                                // Get the line text to convert grapheme offset to char offset
                                let line_end = if cursor_line + 1 < text.len_lines() {
                                    text.line_to_char(cursor_line + 1)
                                } else {
                                    text.len_chars()
                                };
                                let line_text = text.slice(line_start..line_end).to_string();
                                
                                // Convert grapheme offset to char offset for GPUI
                                let cursor_char_offset = Self::grapheme_idx_to_char_idx(&line_text, cursor_grapheme_offset);
                                
                                // Get the x position from the shaped line
                                let cursor_x = line_layout.shaped_line.x_for_index(cursor_char_offset);
                                
                                // Debug logging
                                debug!("Cursor rendering - line: {}, grapheme_offset: {}, char_offset: {}, x: {:?}, viewport_row: {}", 
                                    cursor_line, cursor_grapheme_offset, cursor_char_offset, cursor_x, viewport_row);
                                
                                // More debug info about the line content
                                let line_end = text.line_to_char(cursor_line + 1).saturating_sub(1);
                                let line_text = text.slice(line_start..line_end.min(text.len_chars()));
                                debug!("Line content: {:?}, cursor at grapheme offset {} (char offset {}) in line", 
                                    line_text.to_string(), cursor_grapheme_offset, cursor_char_offset);
                                
                                // Cursor origin is relative to the line's origin
                                let cursor_origin = gpui::Point::new(
                                    cursor_x,
                                    px(0.0) // Relative to line origin
                                );
                                
                                let cursor_color = cursor_style
                                    .fg
                                    .and_then(|fg| color_to_hsla(fg))
                                    .or_else(|| cursor_style.bg.and_then(|bg| color_to_hsla(bg)))
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
                                debug!("Warning: Could not find line layout for cursor line {}", cursor_line);
                            }
                        } else {
                            debug!("Cursor line {} is outside rendered range {}..{}", cursor_line, first_row, last_row);
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
                    let theme = &editor.theme;
                    let view = editor.tree.get(self.view_id);
                    let document = editor.document(self.doc_id).unwrap();
                    
                    // Clone necessary values before creating mutable references
                    let text_system = window.text_system().clone();
                    let style = self.style.clone();
                    
                    let lines = (first_row..last_row)
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
                        &editor,
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
                        line.paint(origin, after_layout.line_height, window, cx).unwrap();
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
            text.paint(self.origin + origin, self.line_height, window, cx)
                .unwrap();
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
            let patched = acc.patch(self.theme.highlight(highlight));
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
            acc.patch(self.theme.highlight(highlight))
        });
        self.update_pos();
    }
}

struct DiagnosticView {
    diagnostic: Diagnostic,
    theme: Theme,
}

impl Render for DiagnosticView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        debug!("rendering diag {:?}", self.diagnostic);

        fn color(style: helix_view::graphics::Style) -> Hsla {
            style.fg.and_then(color_to_hsla).unwrap_or(white())
        }

        let theme = &self.theme;
        let text_style = theme.get("ui.text.info");
        let popup_style = theme.get("ui.popup.info");
        let warning = theme.get("warning");
        let error = theme.get("error");
        let info = theme.get("info");
        let hint = theme.get("hint");

        let fg = text_style.fg.and_then(color_to_hsla).unwrap_or(white());
        let bg = popup_style.bg.and_then(color_to_hsla).unwrap_or(black());

        let title_color = match self.diagnostic.severity {
            Some(DiagnosticSeverity::WARNING) => color(warning),
            Some(DiagnosticSeverity::ERROR) => color(error),
            Some(DiagnosticSeverity::INFORMATION) => color(info),
            Some(DiagnosticSeverity::HINT) => color(hint),
            _ => fg,
        };

        let font = cx.global::<crate::FontSettings>().fixed_font.clone();
        let source_and_code = self.diagnostic.source.as_ref().and_then(|src| {
            let code = self.diagnostic.code.as_ref();
            let code_str = code.map(|code| match code {
                NumberOrString::Number(num) => num.to_string(),
                NumberOrString::String(str) => str.to_string(),
            });
            Some(format!("{}: {}", src, code_str.unwrap_or_default()))
        });

        div()
            .p_2()
            .gap_2()
            .shadow_sm()
            .rounded_sm()
            .bg(black())
            .flex()
            .flex_col()
            .font(font)
            .text_size(px(12.))
            .text_color(fg)
            .bg(bg)
            .child(
                div()
                    .flex()
                    .font_weight(FontWeight::BOLD)
                    .text_color(title_color)
                    .justify_center()
                    .items_center()
                    .when_some(source_and_code, |this, source| this.child(source.clone())),
            )
            .child(
                div()
                    .flex_col()
                    .children(
                        self.diagnostic.message
                            .lines()
                            .map(|line| div().child(line.to_string()))
                    )
            )
    }
}
