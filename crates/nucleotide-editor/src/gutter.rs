// ABOUTME: Native GPUI gutter rendering for editor document views
// ABOUTME: Converts Helix gutter decorations into shaped GPUI lines

use std::sync::Arc;

use gpui::{
    App, Bounds, Hsla, Pixels, Point, Result, ShapedLine, TextAlign, TextStyle, Window,
    WindowTextSystem, black, fill, px, size, white,
};
use helix_view::{Document, Editor, Theme, View, editor::GutterType, graphics::Style};

use crate::{
    EditorLayout, SoftWrapVisualLine,
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub struct GutterLine {
    pub doc_line: usize,
    pub visual_line: u16,
    pub first_visual_line: bool,
    pub origin: Point<Pixels>,
    pub kind: GutterLineKind,
    pub color: Hsla,
    pub shaped_line: ShapedLine,
}

#[derive(Debug, Clone)]
pub struct GutterLinePlan {
    pub doc_line: usize,
    pub visual_line: u16,
    pub first_visual_line: bool,
    pub origin: Point<Pixels>,
    pub text: String,
    pub style: Style,
    pub kind: GutterLineKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterLineKind {
    Text,
    DiffBar(DiffGutterStyle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffGutterStyle {
    Added,
    Modified,
}

const DIFF_GUTTER_BAR_GLYPH: &str = "▍";
const DIFF_PLUS_GUTTER_SCOPE: &str = "diff.plus.gutter";
const DIFF_DELTA_GUTTER_SCOPE: &str = "diff.delta.gutter";

pub struct GutterLineParams<'a> {
    pub layout: &'a EditorLayout,
    pub text_system: Arc<WindowTextSystem>,
    pub text_style: TextStyle,
    pub origin: Point<Pixels>,
    pub positions: &'a [GutterLinePosition],
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

pub struct GutterLinePlanParams<'a> {
    pub layout: &'a EditorLayout,
    pub origin: Point<Pixels>,
    pub positions: &'a [GutterLinePosition],
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
    pub horizontal_offset: usize,
    pub first_row: usize,
    pub last_row: usize,
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

pub struct SoftWrapGutterLinePlanParams<'a> {
    pub layout: &'a EditorLayout,
    pub bounds: Bounds<Pixels>,
    pub scroll_line_offset: Pixels,
    pub visual_lines: &'a [SoftWrapVisualLine],
    pub vertical_offset: usize,
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub is_focused: bool,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct GutterLinePosition {
    pub first_visual_line: bool,
    pub doc_line: usize,
    pub visual_line: u16,
}

pub fn build_gutter_lines(params: GutterLineParams<'_>) -> Vec<GutterLine> {
    let font_size = params.layout.font_size;
    let text_system = params.text_system;
    let text_style = params.text_style;
    let plans = build_gutter_line_plans(GutterLinePlanParams {
        layout: params.layout,
        origin: params.origin,
        positions: params.positions,
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

    for line in params.positions.iter().copied() {
        for gutter_decoration in &mut gutters {
            gutter_decoration(line, &mut gutter);
        }
    }

    gutter.lines
}

pub fn build_unwrapped_gutter_line_plans(
    params: UnwrappedGutterLinePlanParams<'_>,
) -> Vec<GutterLinePlan> {
    let origin = gutter_origin(
        params.bounds,
        params.scroll_line_offset,
        params.layout.cell_width,
        params.horizontal_offset,
    );
    let positions = unwrapped_gutter_line_positions(params.first_row, params.last_row);

    build_gutter_line_plans(GutterLinePlanParams {
        layout: params.layout,
        origin,
        positions: &positions,
        editor: params.editor,
        document: params.document,
        view: params.view,
        theme: params.theme,
        is_focused: params.is_focused,
    })
}

pub fn build_soft_wrap_gutter_line_plans(
    params: SoftWrapGutterLinePlanParams<'_>,
) -> Vec<GutterLinePlan> {
    let origin = gutter_origin(
        params.bounds,
        params.scroll_line_offset,
        params.layout.cell_width,
        0,
    );
    let positions = soft_wrap_gutter_line_positions(params.visual_lines, params.vertical_offset);

    build_gutter_line_plans(GutterLinePlanParams {
        layout: params.layout,
        origin,
        positions: &positions,
        editor: params.editor,
        document: params.document,
        view: params.view,
        theme: params.theme,
        is_focused: params.is_focused,
    })
}

pub(crate) fn gutter_origin(
    bounds: Bounds<Pixels>,
    scroll_line_offset: Pixels,
    cell_width: Pixels,
    horizontal_offset: usize,
) -> Point<Pixels> {
    let mut origin = bounds.origin;
    origin.x += px(2.) - cell_width * horizontal_offset as f32;
    origin.y += px(1.) - scroll_line_offset;
    origin
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
                doc_line: plan.doc_line,
                visual_line: plan.visual_line,
                first_visual_line: plan.first_visual_line,
                origin: plan.origin,
                kind: plan.kind,
                color: base_fg,
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
    theme: &Theme,
    mut on_error: impl FnMut(Result<()>),
) {
    for line in lines {
        if let GutterLineKind::DiffBar(style) = line.kind {
            window.paint_quad(fill(
                diff_gutter_bar_bounds(line.origin, line_height),
                diff_gutter_bar_color(style, theme, line.color),
            ));
            continue;
        }

        let result =
            line.shaped_line
                .paint(line.origin, line_height, TextAlign::Left, None, window, cx);
        if result.is_err() {
            on_error(result);
        }
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

        for &gutter_type in view.gutters() {
            let mut gutter =
                gutter_decoration_provider(gutter_type, editor, doc, view, theme, is_focused);
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

                if let Some(decoration) =
                    gutter(pos.doc_line, selected, pos.first_visual_line, &mut text)
                {
                    renderer.render(
                        pos,
                        x,
                        y,
                        gutter_style.patch(decoration.style),
                        decoration.kind,
                        Some(&text),
                    );
                } else {
                    renderer.render(pos, x, y, gutter_style, GutterLineKind::Text, None);
                }
                text.clear();
            };
            gutters.push(Box::new(gutter_decoration));

            offset += width as u16;
        }
    }
}

impl GutterRenderer for GutterPlan<'_> {
    fn render(
        &mut self,
        pos: GutterLinePosition,
        x: u16,
        y: u16,
        style: helix_view::graphics::Style,
        kind: GutterLineKind,
        text: Option<&str>,
    ) {
        let origin_y = self.origin.y + self.layout.line_height * f32::from(y);
        let origin_x = self.origin.x + self.layout.cell_width * f32::from(x);

        let Some(text) = text else {
            return;
        };

        self.lines.push(GutterLinePlan {
            doc_line: pos.doc_line,
            visual_line: pos.visual_line,
            first_visual_line: pos.first_visual_line,
            origin: Point {
                x: origin_x,
                y: origin_y,
            },
            text: text.to_string(),
            style,
            kind,
        });
    }
}

type GutterDecoration<'a, T> = Box<dyn FnMut(GutterLinePosition, &mut T) + 'a>;

trait GutterRenderer {
    fn render(
        &mut self,
        pos: GutterLinePosition,
        x: u16,
        y: u16,
        style: helix_view::graphics::Style,
        kind: GutterLineKind,
        text: Option<&str>,
    );
}

#[derive(Debug, Clone, Copy)]
struct GutterDecorationStyle {
    style: Style,
    kind: GutterLineKind,
}

type GutterDecorationProvider<'doc> =
    Box<dyn FnMut(usize, bool, bool, &mut String) -> Option<GutterDecorationStyle> + 'doc>;

fn gutter_decoration_provider<'doc>(
    gutter_type: GutterType,
    editor: &'doc Editor,
    doc: &'doc Document,
    view: &'doc View,
    theme: &Theme,
    is_focused: bool,
) -> GutterDecorationProvider<'doc> {
    if matches!(gutter_type, GutterType::Diff) {
        return diff_gutter_decoration(doc, theme);
    }

    let mut gutter = gutter_type.style(editor, doc, view, theme, is_focused);
    Box::new(move |line, selected, first_visual_line, out| {
        gutter(line, selected, first_visual_line, out).map(|style| GutterDecorationStyle {
            style,
            kind: GutterLineKind::Text,
        })
    })
}

fn diff_gutter_decoration<'doc>(
    doc: &'doc Document,
    theme: &Theme,
) -> GutterDecorationProvider<'doc> {
    let added = theme.get(DIFF_PLUS_GUTTER_SCOPE);
    let deleted = theme.get("diff.minus.gutter");
    let modified = theme.get(DIFF_DELTA_GUTTER_SCOPE);

    if let Some(diff_handle) = doc.diff_handle() {
        let hunks = diff_handle.load();
        let mut hunk_i = 0;
        let mut hunk = hunks.nth_hunk(hunk_i);

        Box::new(
            move |line: usize, _selected: bool, first_visual_line: bool, out: &mut String| {
                while hunk.after.end < line as u32
                    || !hunk.is_pure_removal() && line as u32 == hunk.after.end
                {
                    hunk_i += 1;
                    hunk = hunks.nth_hunk(hunk_i);
                }

                if hunk.after.start > line as u32 {
                    return None;
                }

                if hunk.is_pure_insertion() {
                    out.push_str(DIFF_GUTTER_BAR_GLYPH);
                    Some(GutterDecorationStyle {
                        style: added,
                        kind: GutterLineKind::DiffBar(DiffGutterStyle::Added),
                    })
                } else if hunk.is_pure_removal() {
                    if !first_visual_line {
                        return None;
                    }

                    out.push('▔');
                    Some(GutterDecorationStyle {
                        style: deleted,
                        kind: GutterLineKind::Text,
                    })
                } else {
                    out.push_str(DIFF_GUTTER_BAR_GLYPH);
                    Some(GutterDecorationStyle {
                        style: modified,
                        kind: GutterLineKind::DiffBar(DiffGutterStyle::Modified),
                    })
                }
            },
        )
    } else {
        Box::new(move |_, _, _, _| None)
    }
}

fn diff_gutter_bar_bounds(origin: Point<Pixels>, line_height: Pixels) -> Bounds<Pixels> {
    let width = (line_height * 0.24).max(px(3.0)).min(px(6.0));
    Bounds::new(origin, size(width, line_height))
}

fn diff_gutter_bar_color(style: DiffGutterStyle, theme: &Theme, fallback: Hsla) -> Hsla {
    diff_gutter_bar_color_from_style(theme.get(style.theme_scope()), fallback)
}

fn diff_gutter_bar_color_from_style(style: Style, fallback: Hsla) -> Hsla {
    style.fg.and_then(helix_color_to_hsla).unwrap_or(fallback)
}

impl DiffGutterStyle {
    fn theme_scope(self) -> &'static str {
        match self {
            Self::Added => DIFF_PLUS_GUTTER_SCOPE,
            Self::Modified => DIFF_DELTA_GUTTER_SCOPE,
        }
    }
}

pub fn unwrapped_gutter_line_positions(
    first_row: usize,
    last_row: usize,
) -> Vec<GutterLinePosition> {
    (first_row..last_row)
        .enumerate()
        .map(|(current_visual_line, doc_line)| GutterLinePosition {
            first_visual_line: true,
            doc_line,
            visual_line: u16::try_from(current_visual_line).unwrap_or(u16::MAX),
        })
        .collect()
}

pub fn soft_wrap_gutter_line_positions(
    visual_lines: &[SoftWrapVisualLine],
    vertical_offset: usize,
) -> Vec<GutterLinePosition> {
    visual_lines
        .iter()
        .map(|visual| GutterLinePosition {
            first_visual_line: visual.segment_char_offset == 0,
            doc_line: visual.doc_line,
            visual_line: u16::try_from(visual.relative_row(vertical_offset)).unwrap_or(u16::MAX),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};
    use helix_view::graphics::{Color, Style};

    use super::{
        DIFF_DELTA_GUTTER_SCOPE, DIFF_PLUS_GUTTER_SCOPE, DiffGutterStyle, GutterLinePosition,
        diff_gutter_bar_bounds, diff_gutter_bar_color_from_style, gutter_origin,
        soft_wrap_gutter_line_positions, unwrapped_gutter_line_positions,
    };
    use crate::{SoftWrapVisualLine, line_text::DisplayTextMap, style::helix_color_to_hsla};

    #[test]
    fn gutter_positions_map_document_rows_to_visual_rows() {
        let positions = unwrapped_gutter_line_positions(3, 6);

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
        assert!(unwrapped_gutter_line_positions(4, 4).is_empty());
    }

    #[test]
    fn gutter_origin_accounts_for_scroll_and_horizontal_offset() {
        let origin = gutter_origin(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            px(5.0),
            px(8.0),
            3,
        );

        assert_eq!(origin, point(px(78.0), px(36.0)));
    }

    #[test]
    fn diff_gutter_styles_map_to_theme_scopes() {
        assert_eq!(DiffGutterStyle::Added.theme_scope(), DIFF_PLUS_GUTTER_SCOPE);
        assert_eq!(
            DiffGutterStyle::Modified.theme_scope(),
            DIFF_DELTA_GUTTER_SCOPE
        );
    }

    #[test]
    fn diff_gutter_bar_color_uses_themed_foreground() {
        let fallback = helix_color_to_hsla(Color::Rgb(1, 2, 3)).unwrap();
        let themed = helix_color_to_hsla(Color::Rgb(4, 5, 6)).unwrap();

        assert_eq!(
            diff_gutter_bar_color_from_style(Style::default().fg(Color::Rgb(4, 5, 6)), fallback),
            themed
        );
        assert_eq!(
            diff_gutter_bar_color_from_style(Style::default(), fallback),
            fallback
        );
    }

    #[test]
    fn diff_gutter_bar_bounds_touch_on_adjacent_rows() {
        let line_height = px(20.0);
        let first = diff_gutter_bar_bounds(point(px(12.0), px(40.0)), line_height);
        let second = diff_gutter_bar_bounds(point(px(12.0), px(60.0)), line_height);

        assert_eq!(first.origin.x, second.origin.x);
        assert_eq!(first.origin.y + first.size.height, second.origin.y);
        assert_eq!(first.size.height, line_height);
        assert_eq!(second.size.height, line_height);
    }

    fn visual_line(
        visual_line: usize,
        doc_line: usize,
        segment_char_offset: usize,
    ) -> SoftWrapVisualLine {
        SoftWrapVisualLine {
            visual_line,
            doc_line,
            text: "".into(),
            line_start_col: 0,
            wrap_indicator_len: 0,
            line_start_char: None,
            line_end_char: None,
            segment_char_offset,
            text_start_byte_offset: 0,
            is_phantom_line: false,
            display_map: DisplayTextMap::identity(0),
        }
    }

    #[test]
    fn soft_wrap_positions_preserve_visual_rows_and_continuations() {
        let visual_lines = vec![
            visual_line(2, 0, 0),
            visual_line(3, 0, 12),
            visual_line(4, 1, 0),
        ];

        let positions = soft_wrap_gutter_line_positions(&visual_lines, 2);

        assert_eq!(
            positions,
            vec![
                GutterLinePosition {
                    first_visual_line: true,
                    doc_line: 0,
                    visual_line: 0,
                },
                GutterLinePosition {
                    first_visual_line: false,
                    doc_line: 0,
                    visual_line: 1,
                },
                GutterLinePosition {
                    first_visual_line: true,
                    doc_line: 1,
                    visual_line: 2,
                },
            ]
        );
    }

    #[test]
    fn soft_wrap_positions_mark_viewport_start_continuations_as_virtual() {
        let visual_lines = vec![visual_line(7, 3, 24)];

        assert_eq!(
            soft_wrap_gutter_line_positions(&visual_lines, 7),
            vec![GutterLinePosition {
                first_visual_line: false,
                doc_line: 3,
                visual_line: 0,
            }]
        );
    }
}
