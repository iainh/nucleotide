// ABOUTME: Native GPUI gutter rendering for editor document views
// ABOUTME: Converts Helix gutter decorations into shaped GPUI lines

use std::sync::Arc;

use gpui::{
    App, Bounds, Hsla, Pixels, Point, Result, ShapedLine, TextAlign, TextRun, TextStyle, Window,
    WindowTextSystem, black, point, px, white,
};
use helix_view::{Document, Editor, Theme, View, graphics::Style};

use crate::{
    EditorLayout, SoftWrapVisualLine,
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub struct GutterLine {
    pub origin: Point<Pixels>,
    pub shaped_line: ShapedLine,
}

#[derive(Debug, Clone)]
pub struct GutterLinePlan {
    pub origin: Point<Pixels>,
    pub text: String,
    pub style: Style,
}

pub struct SoftWrapGutterLine {
    pub doc_line: usize,
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

pub struct GutterLinePlanParams<'a> {
    pub layout: &'a EditorLayout,
    pub origin: Point<Pixels>,
    pub first_row: usize,
    pub last_row: usize,
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

pub struct UnwrappedGutterLinePlanParams<'a> {
    pub layout: &'a EditorLayout,
    pub bounds: Bounds<Pixels>,
    pub scroll_line_offset: Pixels,
    pub first_row: usize,
    pub last_row: usize,
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoftWrapGutterLinePlan {
    pub doc_line: usize,
    pub is_phantom_line: bool,
    pub y_offset: Pixels,
    pub text: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoftWrapGutterLinePaintPlan<'a> {
    pub line: &'a SoftWrapGutterLinePlan,
    pub origin: Point<Pixels>,
    pub color: Hsla,
}

pub struct SoftWrapGutterPaintParams<'a> {
    pub text_system: Arc<WindowTextSystem>,
    pub text_style: &'a TextStyle,
    pub font_size: Pixels,
    pub visual_lines: &'a [SoftWrapVisualLine],
    pub vertical_offset: usize,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub cursor_lines: &'a [usize],
    pub origin: Point<Pixels>,
    pub gutter_color: Hsla,
    pub gutter_selected_color: Hsla,
}

pub fn build_gutter_lines(params: GutterLineParams<'_>) -> Vec<GutterLine> {
    let font_size = params.layout.font_size;
    let text_system = params.text_system;
    let text_style = params.text_style;
    let plans = build_gutter_line_plans(GutterLinePlanParams {
        layout: params.layout,
        origin: params.origin,
        first_row: params.first_row,
        last_row: params.last_row,
        editor: params.editor,
        document: params.document,
        view: params.view,
        theme: params.theme,
        is_focused: params.is_focused,
    });

    build_gutter_lines_from_plans(text_system, &text_style, font_size, &plans)
}

pub fn build_gutter_line_plans(params: GutterLinePlanParams<'_>) -> Vec<GutterLinePlan> {
    let mut gutter = GutterPlan {
        layout: params.layout,
        lines: Vec::new(),
        origin: params.origin,
    };

    let mut gutters = Vec::new();
    GutterPlan::init_gutter(
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

pub fn build_unwrapped_gutter_line_plans(
    params: UnwrappedGutterLinePlanParams<'_>,
) -> Vec<GutterLinePlan> {
    let mut origin = params.bounds.origin;
    origin.x += px(2.);
    origin.y += px(1.) - params.scroll_line_offset;

    build_gutter_line_plans(GutterLinePlanParams {
        layout: params.layout,
        origin,
        first_row: params.first_row,
        last_row: params.last_row,
        editor: params.editor,
        document: params.document,
        view: params.view,
        theme: params.theme,
        is_focused: params.is_focused,
    })
}

pub fn build_gutter_lines_from_plans(
    text_system: Arc<WindowTextSystem>,
    text_style: &TextStyle,
    font_size: Pixels,
    plans: &[GutterLinePlan],
) -> Vec<GutterLine> {
    plans
        .iter()
        .map(|plan| {
            let base_fg = plan
                .style
                .fg
                .and_then(helix_color_to_hsla)
                .unwrap_or(white());
            let base_bg = plan.style.bg.and_then(helix_color_to_hsla);
            let base_font = text_style.font();
            let run = create_styled_text_run(
                plan.text.len(),
                &base_font,
                &plan.style,
                base_fg,
                base_bg,
                black(),
                None,
            );
            let shaped = text_system.shape_line(plan.text.clone().into(), font_size, &[run], None);

            GutterLine {
                origin: plan.origin,
                shaped_line: shaped,
            }
        })
        .collect()
}

pub fn paint_gutter_lines(
    window: &mut Window,
    cx: &mut App,
    lines: &[GutterLine],
    line_height: Pixels,
    mut on_error: impl FnMut(Result<()>),
) {
    for line in lines {
        let result =
            line.shaped_line
                .paint(line.origin, line_height, TextAlign::Left, None, window, cx);
        if result.is_err() {
            on_error(result);
        }
    }
}

pub fn soft_wrap_gutter_line_plans(
    visual_lines: &[SoftWrapVisualLine],
    vertical_offset: usize,
    line_height: Pixels,
    scroll_line_offset: Pixels,
    cursor_lines: &[usize],
) -> Vec<SoftWrapGutterLinePlan> {
    let mut plans = Vec::new();
    let mut last_doc_line = None;

    for visual in visual_lines {
        if last_doc_line == Some(visual.doc_line) {
            continue;
        }

        let y_offset =
            -scroll_line_offset + line_height * visual.relative_row(vertical_offset) as f32;
        plans.push(SoftWrapGutterLinePlan {
            doc_line: visual.doc_line,
            is_phantom_line: visual.is_phantom_line,
            y_offset,
            text: soft_wrap_gutter_label(visual.doc_line, visual.is_phantom_line),
            selected: !visual.is_phantom_line && cursor_lines.contains(&visual.doc_line),
        });
        last_doc_line = Some(visual.doc_line);
    }

    plans
}

pub fn soft_wrap_gutter_line_paint_plans<'a>(
    lines: &'a [SoftWrapGutterLinePlan],
    origin: Point<Pixels>,
    gutter_color: Hsla,
    gutter_selected_color: Hsla,
) -> Vec<SoftWrapGutterLinePaintPlan<'a>> {
    lines
        .iter()
        .map(|line| SoftWrapGutterLinePaintPlan {
            line,
            origin: point(origin.x, origin.y + line.y_offset),
            color: if !line.is_phantom_line && line.selected {
                gutter_selected_color
            } else {
                gutter_color
            },
        })
        .collect()
}

pub fn build_soft_wrap_gutter_lines(
    text_system: Arc<WindowTextSystem>,
    text_style: &TextStyle,
    font_size: Pixels,
    paint_plans: &[SoftWrapGutterLinePaintPlan<'_>],
) -> Vec<SoftWrapGutterLine> {
    paint_plans
        .iter()
        .map(|paint_plan| {
            let line = paint_plan.line;
            let run = TextRun {
                len: line.text.len(),
                font: text_style.font(),
                color: paint_plan.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped_line =
                text_system.shape_line(line.text.clone().into(), font_size, &[run], None);

            SoftWrapGutterLine {
                doc_line: line.doc_line,
                origin: paint_plan.origin,
                shaped_line,
            }
        })
        .collect()
}

pub fn build_soft_wrap_gutter_for_visual_lines(
    params: SoftWrapGutterPaintParams<'_>,
) -> Vec<SoftWrapGutterLine> {
    let gutter_plans = soft_wrap_gutter_line_plans(
        params.visual_lines,
        params.vertical_offset,
        params.line_height,
        params.scroll_line_offset,
        params.cursor_lines,
    );
    let gutter_paint_plans = soft_wrap_gutter_line_paint_plans(
        &gutter_plans,
        params.origin,
        params.gutter_color,
        params.gutter_selected_color,
    );

    build_soft_wrap_gutter_lines(
        params.text_system,
        params.text_style,
        params.font_size,
        &gutter_paint_plans,
    )
}

pub fn paint_soft_wrap_gutter(
    window: &mut Window,
    cx: &mut App,
    params: SoftWrapGutterPaintParams<'_>,
    on_error: impl FnMut(Result<()>),
) -> Vec<SoftWrapGutterLine> {
    let line_height = params.line_height;
    let gutter_lines = build_soft_wrap_gutter_for_visual_lines(params);
    paint_soft_wrap_gutter_lines(window, cx, &gutter_lines, line_height, on_error);
    gutter_lines
}

pub fn paint_soft_wrap_gutter_lines(
    window: &mut Window,
    cx: &mut App,
    lines: &[SoftWrapGutterLine],
    line_height: Pixels,
    mut on_error: impl FnMut(Result<()>),
) {
    for line in lines {
        let result =
            line.shaped_line
                .paint(line.origin, line_height, TextAlign::Left, None, window, cx);
        if result.is_err() {
            on_error(result);
        }
    }
}

fn soft_wrap_gutter_label(doc_line: usize, is_phantom_line: bool) -> String {
    if is_phantom_line {
        "   ~ ".to_string()
    } else {
        format!("{:>4} ", doc_line + 1)
    }
}

struct GutterPlan<'a> {
    layout: &'a EditorLayout,
    lines: Vec<GutterLinePlan>,
    origin: Point<Pixels>,
}

impl<'a> GutterPlan<'a> {
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

impl GutterRenderer for GutterPlan<'_> {
    fn render(&mut self, x: u16, y: u16, style: helix_view::graphics::Style, text: Option<&str>) {
        let origin_y = self.origin.y + self.layout.line_height * f32::from(y);
        let origin_x = self.origin.x + self.layout.cell_width * f32::from(x);

        let Some(text) = text else {
            return;
        };

        self.lines.push(GutterLinePlan {
            origin: Point {
                x: origin_x,
                y: origin_y,
            },
            text: text.to_string(),
            style,
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
    use gpui::{point, px, rgb};

    use super::{
        GutterLinePosition, SoftWrapGutterLinePlan, gutter_line_positions,
        soft_wrap_gutter_line_paint_plans, soft_wrap_gutter_line_plans,
    };
    use crate::SoftWrapVisualLine;

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

    fn visual_line(
        visual_line: usize,
        doc_line: usize,
        is_phantom_line: bool,
    ) -> SoftWrapVisualLine {
        SoftWrapVisualLine {
            visual_line,
            doc_line,
            text: "".into(),
            line_start_col: 0,
            wrap_indicator_len: 0,
            line_start_char: None,
            line_end_char: None,
            segment_char_offset: 0,
            text_start_byte_offset: 0,
            is_phantom_line,
        }
    }

    #[test]
    fn soft_wrap_gutter_plans_deduplicate_wrapped_document_lines() {
        let visual_lines = vec![
            visual_line(2, 0, false),
            visual_line(3, 0, false),
            visual_line(4, 1, false),
        ];

        let plans = soft_wrap_gutter_line_plans(&visual_lines, 2, px(20.0), px(5.0), &[1]);

        assert_eq!(
            plans,
            vec![
                SoftWrapGutterLinePlan {
                    doc_line: 0,
                    is_phantom_line: false,
                    y_offset: px(-5.0),
                    text: "   1 ".to_string(),
                    selected: false,
                },
                SoftWrapGutterLinePlan {
                    doc_line: 1,
                    is_phantom_line: false,
                    y_offset: px(35.0),
                    text: "   2 ".to_string(),
                    selected: true,
                },
            ]
        );
    }

    #[test]
    fn soft_wrap_gutter_plans_use_tilde_for_phantom_lines() {
        let visual_lines = vec![visual_line(7, 3, true)];

        let plans = soft_wrap_gutter_line_plans(&visual_lines, 7, px(20.0), px(0.0), &[3]);

        assert_eq!(
            plans,
            vec![SoftWrapGutterLinePlan {
                doc_line: 3,
                is_phantom_line: true,
                y_offset: px(0.0),
                text: "   ~ ".to_string(),
                selected: false,
            }]
        );
    }

    #[test]
    fn soft_wrap_gutter_paint_plans_choose_origin_and_color() {
        let lines = vec![
            SoftWrapGutterLinePlan {
                doc_line: 0,
                is_phantom_line: false,
                y_offset: px(0.0),
                text: "   1 ".to_string(),
                selected: false,
            },
            SoftWrapGutterLinePlan {
                doc_line: 1,
                is_phantom_line: false,
                y_offset: px(20.0),
                text: "   2 ".to_string(),
                selected: true,
            },
            SoftWrapGutterLinePlan {
                doc_line: 2,
                is_phantom_line: true,
                y_offset: px(40.0),
                text: "   ~ ".to_string(),
                selected: true,
            },
        ];
        let gutter_color = rgb(0x667788).into();
        let gutter_selected_color = rgb(0xaabbcc).into();

        let plans = soft_wrap_gutter_line_paint_plans(
            &lines,
            point(px(8.0), px(12.0)),
            gutter_color,
            gutter_selected_color,
        );

        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].line, &lines[0]);
        assert_eq!(plans[0].origin, point(px(8.0), px(12.0)));
        assert_eq!(plans[0].color, gutter_color);
        assert_eq!(plans[1].line, &lines[1]);
        assert_eq!(plans[1].origin, point(px(8.0), px(32.0)));
        assert_eq!(plans[1].color, gutter_selected_color);
        assert_eq!(plans[2].line, &lines[2]);
        assert_eq!(plans[2].origin, point(px(8.0), px(52.0)));
        assert_eq!(plans[2].color, gutter_color);
    }
}
