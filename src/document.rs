use std::borrow::Cow;

use gpui::{prelude::FluentBuilder, *};
use gpui::{point, size, StyledText, Overflow, TextRun, canvas};
use helix_core::{
    ropey::RopeSlice,
    syntax::{self, HighlightEvent},
    Uri,
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
            let theme = self.core.read(cx).editor.theme.clone();

            self.get_diagnostics(cx).into_iter().map(move |diag| {
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
        is_view_focused: bool,
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
        let core = self.core.clone();
        self.interactivity
            .prepaint(_global_id, _inspector_id, bounds, bounds.size, window, cx, |_, _, hitbox, window, cx| {
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
        self.interactivity
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                println!("MOUSE DOWN");
                focus.focus(_window);
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
                let cursor_style = theme.get("ui.cursor.primary");
                let bg = fill(bounds, bg_color);
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
                let gutter_overflow = gutter_width == 0;
                if !gutter_overflow {
                    debug!("need to render gutter {}", gutter_width);
                }

                let cursor_text = None; // TODO

                let _cursor_row = cursor_pos.map(|p| p.row);
                let anchor = document.view_offset(self.view_id).anchor;
                let total_lines = text.len_lines();
                let first_row = text.char_to_line(anchor.min(text.len_chars()));
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
                
                // Drop the core borrow before the loop
                drop(core);
                
                let text = doc_text.slice(..);
                
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
                    let line_str: SharedString = RopeWrapper(line_slice).into();
                    
                    // Get highlights for this specific line using the extracted values
                    // Re-read core for this iteration
                    let core = self.core.read(cx);
                    let editor = &core.editor;
                    let document = editor.document(self.doc_id).unwrap();
                    let view = editor.tree.get(self.view_id);
                    
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
                    drop(core);
                    
                    let text_origin = point(text_origin_x, bounds.origin.y + px(1.) + y_offset);
                    
                    if !line_str.is_empty() {
                        let shaped_line = window.text_system()
                            .shape_line(line_str, self.style.font_size.to_pixels(px(16.0)), &line_runs, None);
                        
                        shaped_line.paint(text_origin, after_layout.line_height, window, cx).unwrap();
                    }
                    
                    y_offset += after_layout.line_height;
                }
                // draw cursor
                if self.is_focused {
                    match (cursor_pos, cursor_kind) {
                        (Some(position), kind) => {
                            let helix_core::Position { row, col } = position;
                            let origin_y = after_layout.line_height * row as f32;
                            let origin_x =
                                after_layout.cell_width * ((col + gutter_width as usize) as f32);
                            let mut cursor_fg = cursor_style
                                .bg
                                .and_then(|fg| color_to_hsla(fg))
                                .unwrap_or(fg_color);
                            cursor_fg.a = 0.5;

                            let mut cursor = Cursor {
                                origin: gpui::Point::new(origin_x, origin_y),
                                kind,
                                color: cursor_fg,
                                block_width: after_layout.cell_width,
                                line_height: after_layout.line_height,
                                text: cursor_text,
                            };
                            let mut origin = bounds.origin;
                            origin.x += px(2.);
                            origin.y += px(1.);

                            cursor.paint(origin, window, cx);
                        }
                        (None, _) => {}
                    }
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
                    drop(core);
                    
                    // Now paint the gutter lines
                    for (origin, line) in gutter.lines {
                        line.paint(origin, after_layout.line_height, window, cx).unwrap();
                    }
                }
            });
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

        let cursor = fill(bounds, self.color);

        // Quad painting is handled differently in new GPUI
        // TODO: Use div with background color instead

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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().flex_col().child(self.diagnostic.message.clone()))
    }
}
