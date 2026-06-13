// ABOUTME: Native GPUI gutter rendering for editor document views
// ABOUTME: Converts Helix gutter decorations into shaped GPUI lines

use std::sync::Arc;

use gpui::{Pixels, Point, ShapedLine, TextStyle, WindowTextSystem, black, white};
use helix_view::{Document, Editor, Theme, View};

use crate::{
    EditorLayout,
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub struct GutterLine {
    pub origin: Point<Pixels>,
    pub shaped_line: ShapedLine,
}

pub struct GutterLineParams<'a> {
    pub layout: &'a EditorLayout,
    pub text_system: Arc<WindowTextSystem>,
    pub text_style: TextStyle,
    pub origin: Point<Pixels>,
    pub first_row: usize,
    pub last_row: usize,
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

pub fn build_gutter_lines(params: GutterLineParams<'_>) -> Vec<GutterLine> {
    let mut gutter = Gutter {
        layout: params.layout,
        text_system: params.text_system,
        lines: Vec::new(),
        text_style: params.text_style,
        origin: params.origin,
    };

    let mut gutters = Vec::new();
    Gutter::init_gutter(
        params.editor,
        params.document,
        params.view,
        params.theme,
        params.is_focused,
        &mut gutters,
    );

    for line in gutter_line_positions(params.first_row, params.last_row) {
        for gutter_decoration in &mut gutters {
            gutter_decoration(line, &mut gutter);
        }
    }

    gutter.lines
}

struct Gutter<'a> {
    layout: &'a EditorLayout,
    text_system: Arc<WindowTextSystem>,
    lines: Vec<GutterLine>,
    text_style: TextStyle,
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
            let mut text = String::with_capacity(width);
            let cursors = cursors.clone();
            let gutter_decoration = move |pos: GutterLinePosition, renderer: &mut Self| {
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
                    renderer.render(x, y, gutter_style.patch(style), Some(&text));
                } else {
                    renderer.render(x, y, gutter_style, None);
                }
                text.clear();
            };
            gutters.push(Box::new(gutter_decoration));

            offset += width as u16;
        }
    }
}

impl GutterRenderer for Gutter<'_> {
    fn render(&mut self, x: u16, y: u16, style: helix_view::graphics::Style, text: Option<&str>) {
        let origin_y = self.origin.y + self.layout.line_height * f32::from(y);
        let origin_x = self.origin.x + self.layout.cell_width * f32::from(x);

        let Some(text) = text else {
            return;
        };

        let base_fg = style.fg.and_then(helix_color_to_hsla).unwrap_or(white());
        let base_bg = style.bg.and_then(helix_color_to_hsla);
        let base_font = self.text_style.font();
        let run = create_styled_text_run(
            text.len(),
            &base_font,
            &style,
            base_fg,
            base_bg,
            black(),
            None,
        );
        let shaped = self.text_system.shape_line(
            text.to_string().into(),
            self.layout.font_size,
            &[run],
            None,
        );

        self.lines.push(GutterLine {
            origin: Point {
                x: origin_x,
                y: origin_y,
            },
            shaped_line: shaped,
        });
    }
}

type GutterDecoration<'a, T> = Box<dyn FnMut(GutterLinePosition, &mut T) + 'a>;

trait GutterRenderer {
    fn render(&mut self, x: u16, y: u16, style: helix_view::graphics::Style, text: Option<&str>);
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
struct GutterLinePosition {
    first_visual_line: bool,
    doc_line: usize,
    visual_line: u16,
}

fn gutter_line_positions(
    first_row: usize,
    last_row: usize,
) -> impl Iterator<Item = GutterLinePosition> {
    (first_row..last_row)
        .enumerate()
        .map(|(current_visual_line, doc_line)| GutterLinePosition {
            first_visual_line: true,
            doc_line,
            visual_line: u16::try_from(current_visual_line).unwrap_or(u16::MAX),
        })
}

#[cfg(test)]
mod tests {
    use super::{GutterLinePosition, gutter_line_positions};

    #[test]
    fn gutter_positions_map_document_rows_to_visual_rows() {
        let positions: Vec<_> = gutter_line_positions(3, 6).collect();

        assert_eq!(
            positions,
            vec![
                GutterLinePosition {
                    first_visual_line: true,
                    doc_line: 3,
                    visual_line: 0,
                },
                GutterLinePosition {
                    first_visual_line: true,
                    doc_line: 4,
                    visual_line: 1,
                },
                GutterLinePosition {
                    first_visual_line: true,
                    doc_line: 5,
                    visual_line: 2,
                },
            ]
        );
    }

    #[test]
    fn empty_ranges_produce_no_positions() {
        assert_eq!(gutter_line_positions(4, 4).count(), 0);
    }
}
