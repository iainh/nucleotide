// ABOUTME: Paint helpers for frame-owned native editor render state
// ABOUTME: Converts EditorDocumentFrame plans into GPUI paint calls

use std::time::Duration;

use gpui::{
    App, Bounds, CursorStyle, FocusHandle, Hsla, Pixels, SharedString, TextStyle,
    TransformationMatrix, Window,
};
use helix_core::{Rope, RopeSlice};
use helix_view::{
    Document, Editor, Theme, View, ViewId,
    document::Mode,
    editor::CursorShapeConfig,
    graphics::{CursorKind, Style},
};
use nucleotide_logging::{PerfTimer, error, trace};

use crate::{
    CursorOverlayPlan, DiagnosticGutterMarkersPaintParams, EditorCursorTextPaintParams,
    EditorDocumentFrame, EditorDocumentFrameParams, EditorLayout, EditorLineBackgroundStyle,
    EditorSurfaceGeometry, EditorViewFrameState, EditorViewState, EditorViewportSurfaceLayout,
    GutterLine, GutterLinePlan, LineLayoutCache, SoftWrapCursorPaintPlanParams,
    SoftWrapEditorLinePaintParams, SoftWrapGutterLinePlanParams, UnwrappedCursorPaintPlanParams,
    UnwrappedEditorLinePaintParams, UnwrappedGutterLinePlanParams, build_gutter_lines_from_plans,
    build_soft_wrap_gutter_line_plans, build_unwrapped_gutter_line_plans, cursor_style_for_mode,
    diagnostics::DiagnosticSeverityIconColors, editor_document_frame, gutter::gutter_origin,
    highlight::gpui_hsla_to_helix_color, paint_diagnostic_gutter_markers, paint_editor_background,
    paint_gutter_lines, paint_soft_wrap_editor_line, paint_unwrapped_editor_line,
    paint_visible_rulers, run_gutter_button_bounds, run_gutter_icon_bounds,
    shape_and_paint_editor_cursor, soft_wrap_cursor_paint_plan, style::helix_color_to_hsla,
    unwrapped_cursor_paint_plan,
};

pub struct DocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub default_bg: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub is_focused: bool,
    pub element_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

#[derive(Clone, Copy)]
pub struct NativeEditorFramePaintStyle {
    pub fg_color: Hsla,
    pub bg_color: Hsla,
    pub default_text_style: Style,
    pub cursor_style: Style,
    pub cursorline_color: Option<Hsla>,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub gutter_color: Hsla,
    pub gutter_selected_color: Hsla,
    pub diagnostic_highlight_base: Hsla,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
    pub wrap_indicator_color: Option<Hsla>,
    pub ruler_color: Hsla,
    pub run_button_color: Hsla,
}

#[derive(Clone, Copy)]
pub struct NativeEditorFramePalette {
    pub fg_color: Hsla,
    pub bg_color: Hsla,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub fallback_gutter_color: Hsla,
    pub diagnostic_highlight_base: Hsla,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub fallback_ruler_color: Hsla,
    pub run_button_color: Hsla,
}

#[derive(Clone, Copy, Default)]
pub struct NativeEditorFrameThemeStyles {
    pub cursor: Style,
    pub cursor_primary: Style,
    pub cursor_primary_insert: Style,
    pub cursor_primary_normal: Style,
    pub cursor_primary_select: Style,
    pub virtual_wrap: Style,
    pub virtual_ruler: Style,
    pub cursorline_primary: Style,
    pub line_number: Style,
    pub line_number_selected: Style,
    pub gutter: Style,
}

impl NativeEditorFrameThemeStyles {
    pub fn from_style_fn(mut style_for_key: impl FnMut(&str) -> Style) -> Self {
        Self {
            cursor: style_for_key("ui.cursor"),
            cursor_primary: style_for_key("ui.cursor.primary"),
            cursor_primary_insert: style_for_key("ui.cursor.primary.insert"),
            cursor_primary_normal: style_for_key("ui.cursor.primary.normal"),
            cursor_primary_select: style_for_key("ui.cursor.primary.select"),
            virtual_wrap: style_for_key("ui.virtual.wrap"),
            virtual_ruler: style_for_key("ui.virtual.ruler"),
            cursorline_primary: style_for_key("ui.cursorline.primary"),
            line_number: style_for_key("ui.linenr"),
            line_number_selected: style_for_key("ui.linenr.selected"),
            gutter: style_for_key("ui.gutter"),
        }
    }

    fn style_for_key(&self, key: &str) -> Style {
        match key {
            "ui.cursor" => self.cursor,
            "ui.cursor.primary" => self.cursor_primary,
            "ui.cursor.primary.insert" => self.cursor_primary_insert,
            "ui.cursor.primary.normal" => self.cursor_primary_normal,
            "ui.cursor.primary.select" => self.cursor_primary_select,
            "ui.virtual.wrap" => self.virtual_wrap,
            "ui.virtual.ruler" => self.virtual_ruler,
            "ui.cursorline.primary" => self.cursorline_primary,
            "ui.linenr" => self.line_number,
            "ui.linenr.selected" => self.line_number_selected,
            "ui.gutter" => self.gutter,
            _ => Style::default(),
        }
    }
}

pub struct NativeEditorFramePaintStyleParams {
    pub editor_mode: Mode,
    pub theme_styles: NativeEditorFrameThemeStyles,
    pub palette: NativeEditorFramePalette,
}

pub fn native_editor_frame_paint_style(
    params: NativeEditorFramePaintStyleParams,
) -> NativeEditorFramePaintStyle {
    let default_text_style = Style {
        fg: gpui_hsla_to_helix_color(params.palette.fg_color),
        bg: gpui_hsla_to_helix_color(params.palette.bg_color),
        ..Default::default()
    };
    let cursor_style = cursor_style_for_mode(params.editor_mode, |key| {
        params.theme_styles.style_for_key(key)
    });
    let wrap_indicator_color = params
        .theme_styles
        .virtual_wrap
        .fg
        .and_then(helix_color_to_hsla);
    let ruler_color = params
        .theme_styles
        .virtual_ruler
        .bg
        .and_then(helix_color_to_hsla)
        .unwrap_or(params.palette.fallback_ruler_color);
    let cursorline_color = params
        .theme_styles
        .cursorline_primary
        .bg
        .and_then(helix_color_to_hsla);
    let gutter_color = params
        .theme_styles
        .line_number
        .fg
        .and_then(helix_color_to_hsla)
        .unwrap_or(params.palette.fallback_gutter_color);
    let gutter_selected_color = params
        .theme_styles
        .line_number_selected
        .fg
        .and_then(helix_color_to_hsla)
        .unwrap_or(params.palette.fallback_gutter_color);
    let gutter_bg = params.theme_styles.gutter.bg.and_then(helix_color_to_hsla);

    NativeEditorFramePaintStyle {
        fg_color: params.palette.fg_color,
        bg_color: params.palette.bg_color,
        default_text_style,
        cursor_style,
        cursorline_color,
        selection_primary: params.palette.selection_primary,
        selection_secondary: params.palette.selection_secondary,
        gutter_color,
        gutter_selected_color,
        diagnostic_highlight_base: params.palette.diagnostic_highlight_base,
        diagnostic_icon_colors: params.palette.diagnostic_icon_colors,
        gutter_bg,
        wrap_indicator_color,
        ruler_color,
        run_button_color: params.palette.run_button_color,
    }
}

pub struct NativeEditorFramePlanParams<'a> {
    pub editor: &'a Editor,
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub frame_state: &'a EditorViewFrameState,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub font_size: Pixels,
    pub is_focused: bool,
    pub soft_wrap_minimum_columns: u16,
    pub editor_mode: Mode,
    pub cursor_kind: CursorKind,
    pub cursor_shape: CursorShapeConfig,
    pub editor_rulers: Vec<u16>,
    pub cursorline_enabled: bool,
    pub style: NativeEditorFramePaintStyle,
}

pub struct NativeEditorFramePrepareParams<'a> {
    pub editor: &'a mut Editor,
    pub view_id: ViewId,
    pub editor_state: &'a mut EditorViewState,
    pub theme: &'a Theme,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a mut EditorLayout,
    pub text_style: &'a TextStyle,
    pub font_size: Pixels,
    pub is_focused: bool,
    pub soft_wrap_minimum_columns: u16,
    pub theme_styles: NativeEditorFrameThemeStyles,
    pub palette: NativeEditorFramePalette,
}

pub struct NativeEditorFramePaintPlan {
    frame: EditorDocumentFrame,
    text: Rope,
    bounds: Bounds<Pixels>,
    font_size: Pixels,
    is_focused: bool,
    style: NativeEditorFramePaintStyle,
}

pub struct NativeEditorFramePaintParams<'a> {
    pub editor_state: &'a mut EditorViewState,
    pub frame_state: &'a EditorViewFrameState,
    pub plan: &'a NativeEditorFramePaintPlan,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub diagnostic_theme: &'a Theme,
    pub element_focused: bool,
}

pub struct NativeEditorFrameRenderParams<'a> {
    pub editor: &'a mut Editor,
    pub view_id: ViewId,
    pub editor_state: &'a mut EditorViewState,
    pub theme: &'a Theme,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a mut EditorLayout,
    pub text_style: &'a TextStyle,
    pub font_size: Pixels,
    pub is_focused: bool,
    pub focus: &'a FocusHandle,
    pub soft_wrap_minimum_columns: u16,
    pub theme_styles: NativeEditorFrameThemeStyles,
    pub palette: NativeEditorFramePalette,
}

pub struct NativeEditorPreparedFrame {
    pub frame_state: EditorViewFrameState,
    pub paint_plan: NativeEditorFramePaintPlan,
}

struct UnwrappedDocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub default_bg: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub is_focused: bool,
    pub element_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

struct SoftWrapDocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub default_bg: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub is_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

struct DocumentFrameGutterPaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub font_size: Pixels,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

pub fn prepare_native_editor_frame(
    params: NativeEditorFramePrepareParams<'_>,
) -> Option<NativeEditorPreparedFrame> {
    let _timer = PerfTimer::new("prepare_native_editor_frame")
        .with_warn_threshold(Duration::from_millis(12));
    let doc_id = params.editor.tree.try_get(params.view_id)?.doc;
    let scrolloff = params.editor.config().scrolloff;
    let frame_state = params.editor_state.sync_frame_layout(
        params.editor,
        doc_id,
        params.view_id,
        EditorViewportSurfaceLayout::for_editor(
            Some(params.theme),
            params.bounds,
            params.layout.cell_width,
            params.layout.line_height,
            scrolloff,
            None,
        ),
    )?;
    let editor = &*params.editor;
    let view = editor.tree.try_get(params.view_id)?;
    let document = editor.document(doc_id)?;
    let syntax_loader = editor.syn_loader.load();
    let editor_config = editor.config();
    let editor_mode = editor.mode();
    let (_, cursor_kind) = editor.cursor();
    let cursor_shape = editor_config.cursor_shape.clone();
    let editor_rulers = editor_config.rulers.clone();
    let cursorline_enabled = editor_config.cursorline && params.is_focused;
    let paint_style = native_editor_frame_paint_style(NativeEditorFramePaintStyleParams {
        editor_mode,
        theme_styles: params.theme_styles,
        palette: params.palette,
    });
    let paint_plan = native_editor_frame_paint_plan(NativeEditorFramePlanParams {
        editor,
        document,
        view,
        view_id: params.view_id,
        theme: params.theme,
        syntax_loader: &syntax_loader,
        frame_state: &frame_state,
        bounds: params.bounds,
        layout: params.layout,
        text_style: params.text_style,
        font_size: params.font_size,
        is_focused: params.is_focused,
        soft_wrap_minimum_columns: params.soft_wrap_minimum_columns,
        editor_mode,
        cursor_kind,
        cursor_shape,
        editor_rulers,
        cursorline_enabled,
        style: paint_style,
    });

    Some(NativeEditorPreparedFrame {
        frame_state,
        paint_plan,
    })
}

pub fn render_native_editor_frame(
    window: &mut Window,
    cx: &mut App,
    params: NativeEditorFrameRenderParams<'_>,
) -> Option<CursorOverlayPlan> {
    let _timer =
        PerfTimer::new("render_native_editor_frame").with_warn_threshold(Duration::from_millis(16));
    let NativeEditorFrameRenderParams {
        editor,
        view_id,
        editor_state,
        theme,
        bounds,
        layout,
        text_style,
        font_size,
        is_focused,
        focus,
        soft_wrap_minimum_columns,
        theme_styles,
        palette,
    } = params;

    let Some(prepared_frame) = prepare_native_editor_frame(NativeEditorFramePrepareParams {
        editor,
        view_id,
        editor_state: &mut *editor_state,
        theme,
        bounds,
        layout: &mut *layout,
        text_style,
        font_size,
        is_focused,
        soft_wrap_minimum_columns,
        theme_styles,
        palette,
    }) else {
        editor_state.clear_gutter_run_button_hits();
        return None;
    };

    paint_native_editor_frame(
        window,
        cx,
        NativeEditorFramePaintParams {
            editor_state,
            frame_state: &prepared_frame.frame_state,
            plan: &prepared_frame.paint_plan,
            layout,
            text_style,
            diagnostic_theme: theme,
            element_focused: focus.is_focused(window),
        },
    )
}

pub fn native_editor_frame_paint_plan(
    params: NativeEditorFramePlanParams<'_>,
) -> NativeEditorFramePaintPlan {
    let mut frame = editor_document_frame(EditorDocumentFrameParams {
        document: params.document,
        view: params.view,
        view_id: params.view_id,
        theme: params.theme,
        syntax_loader: params.syntax_loader,
        first_row: params.frame_state.first_row,
        last_row_from_scroll: params.frame_state.last_row_from_scroll,
        view_position: params
            .frame_state
            .viewport_update
            .view_position_plan
            .view_position,
        soft_wrap_enabled: params.frame_state.viewport_update.soft_wrap,
        gutter_line_plans: Vec::new(),
        bounds: params.bounds,
        gutter_columns: params.frame_state.viewport_update.gutter_columns,
        cell_width: params.layout.cell_width,
        line_height: params.layout.line_height,
        scroll_line_offset: params.frame_state.scroll_line_offset,
        soft_wrap_minimum_columns: params.soft_wrap_minimum_columns,
        fg_color: params.style.fg_color,
        font: params.text_style.font(),
        default_text_style: params.style.default_text_style,
        default_bg: params.style.bg_color,
        wrap_indicator_color: params.style.wrap_indicator_color,
        ruler_color: params.style.ruler_color,
        editor_mode: params.editor_mode,
        cursor_kind: params.cursor_kind,
        cursor_style: params.style.cursor_style,
        cursor_shape: params.cursor_shape.clone(),
        editor_rulers: params.editor_rulers.clone(),
        cursorline_enabled: params.cursorline_enabled,
        is_focused: params.is_focused,
    });
    frame.gutter_line_plans = native_editor_frame_gutter_line_plans(
        &params,
        &frame,
        params
            .frame_state
            .viewport_update
            .view_position_plan
            .view_position,
    );

    trace!(
        "Cursorline check - focused: {}, enabled: {}",
        params.is_focused, frame.cursorline_enabled
    );

    let text = params.document.text();
    let cursor_char_idx = frame.cursor_presentation.cursor_char_idx;
    let cursor_line_num = frame.render_snapshot.cursor_line;
    trace!(
        "Cursor position: line={}, char_idx={}",
        cursor_line_num, cursor_char_idx
    );
    trace!(
        "Cursor position - line: {}, col_in_line: {}, primary_idx: {}, gutter_width: {}",
        frame.primary_cursor_line,
        frame.primary_cursor_col,
        frame.primary_cursor_idx,
        frame.gutter_width
    );
    if frame.gutter_width != 0 {
        trace!("need to render gutter {}", frame.gutter_width);
    }

    let line_viewport = frame.render_snapshot.line_viewport;
    let last_row = frame.render_snapshot.last_row;
    let cursor_at_end = line_viewport.cursor_at_end;
    let file_ends_with_newline = line_viewport.file_ends_with_newline;

    trace!(
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

    if cursor_at_end && file_ends_with_newline {
        let cursor_line = text.char_to_line(cursor_char_idx.saturating_sub(1));
        trace!(
            "Cursor at EOF with newline - cursor_line: {cursor_line}, last_row: {last_row}, total_lines: {}",
            frame.total_lines
        );
    }

    NativeEditorFramePaintPlan {
        frame,
        text: text.clone(),
        bounds: params.bounds,
        font_size: params.font_size,
        is_focused: params.is_focused,
        style: params.style,
    }
}

fn native_editor_frame_gutter_line_plans(
    params: &NativeEditorFramePlanParams<'_>,
    frame: &EditorDocumentFrame,
    view_position: helix_view::view::ViewPosition,
) -> Vec<GutterLinePlan> {
    if let Some(soft_wrap_plan) = frame.soft_wrap_render_plan.as_ref() {
        return build_soft_wrap_gutter_line_plans(SoftWrapGutterLinePlanParams {
            layout: params.layout,
            bounds: params.bounds,
            scroll_line_offset: params.frame_state.scroll_line_offset,
            visual_lines: &soft_wrap_plan.visual_lines,
            vertical_offset: soft_wrap_plan.view_offset.vertical_offset,
            editor: params.editor,
            document: params.document,
            view: params.view,
            theme: params.theme,
            is_focused: params.is_focused,
        });
    }

    if frame.unwrapped_render_plan.is_some() {
        return build_unwrapped_gutter_line_plans(UnwrappedGutterLinePlanParams {
            layout: params.layout,
            bounds: params.bounds,
            scroll_line_offset: params.frame_state.scroll_line_offset,
            horizontal_offset: view_position.horizontal_offset,
            first_row: params.frame_state.first_row,
            last_row: frame.render_snapshot.last_row,
            editor: params.editor,
            document: params.document,
            view: params.view,
            theme: params.theme,
            is_focused: params.is_focused,
        });
    }

    Vec::new()
}

pub fn paint_native_editor_frame(
    window: &mut Window,
    cx: &mut App,
    params: NativeEditorFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let plan = params.plan;
    paint_editor_background(window, plan.bounds, plan.style.bg_color);

    let overlay_plan = paint_document_frame(
        window,
        cx,
        DocumentFramePaintParams {
            frame: &plan.frame,
            text: plan.text.slice(..),
            bounds: plan.bounds,
            layout: params.layout,
            text_style: params.text_style,
            line_cache: &params.frame_state.line_cache,
            font_size: plan.font_size,
            fg_color: plan.style.fg_color,
            default_bg: plan.style.bg_color,
            cursorline_color: plan
                .frame
                .cursorline_enabled
                .then_some(plan.style.cursorline_color)
                .flatten(),
            is_focused: plan.is_focused,
            element_focused: params.element_focused,
            selection_primary: plan.style.selection_primary,
            selection_secondary: plan.style.selection_secondary,
            diagnostic_theme: params.diagnostic_theme,
            diagnostic_icon_colors: plan.style.diagnostic_icon_colors,
            gutter_bg: plan.style.gutter_bg,
            scroll_line_offset: params.frame_state.scroll_line_offset,
        },
    );
    params.editor_state.apply_cursor_overlay_plan(overlay_plan);
    params
        .editor_state
        .set_gutter_line_anchors_from_plans(&plan.frame.gutter_line_plans, plan.bounds.origin);
    let run_button_hits = paint_gutter_run_buttons(window, cx, &params);
    params
        .editor_state
        .set_gutter_run_button_hits(run_button_hits);

    overlay_plan
}

fn paint_gutter_run_buttons(
    window: &mut Window,
    cx: &mut App,
    params: &NativeEditorFramePaintParams<'_>,
) -> Vec<crate::GutterRunButtonHit> {
    let run_button_lines = params.editor_state.gutter_run_button_lines();
    let extra_columns = params.editor_state.gutter_extra_columns();
    if run_button_lines.is_empty() || extra_columns == 0 {
        return Vec::new();
    }

    let frame = &params.plan.frame;
    let gutter_width = params.layout.cell_width * f32::from(frame.gutter_width);
    let reserved_width = params.layout.cell_width * f32::from(extra_columns);
    let icon_path = SharedString::from("icons/play.svg");
    let mut hits = Vec::new();

    for line in frame
        .gutter_line_plans
        .iter()
        .filter(|line| line.first_visual_line)
        .filter(|line| run_button_lines.binary_search(&line.doc_line).is_ok())
    {
        let button_bounds = run_gutter_button_bounds(
            params.plan.bounds.origin.x,
            line.origin.y,
            gutter_width,
            reserved_width,
            params.layout.line_height,
        );
        let icon_bounds = run_gutter_icon_bounds(button_bounds);
        if button_bounds.contains(&window.mouse_position()) {
            window.set_window_cursor_style(CursorStyle::PointingHand);
        }

        if let Err(e) = window.paint_svg(
            icon_bounds,
            icon_path.clone(),
            None,
            TransformationMatrix::default(),
            params.plan.style.run_button_color,
            cx,
        ) {
            error!(error = ?e, "Failed to paint run gutter icon");
        }

        hits.push(crate::GutterRunButtonHit {
            doc_line: line.doc_line,
            bounds: button_bounds,
        });
    }

    hits
}

pub fn paint_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: DocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    paint_visible_rulers(window, &params.frame.ruler_paint_plans);

    if params.frame.soft_wrap_render_plan.is_some() {
        return paint_soft_wrap_document_frame(
            window,
            cx,
            SoftWrapDocumentFramePaintParams {
                frame: params.frame,
                text: params.text,
                bounds: params.bounds,
                layout: params.layout,
                text_style: params.text_style,
                line_cache: params.line_cache,
                font_size: params.font_size,
                fg_color: params.fg_color,
                default_bg: params.default_bg,
                cursorline_color: params.cursorline_color,
                is_focused: params.is_focused,
                selection_primary: params.selection_primary,
                selection_secondary: params.selection_secondary,
                diagnostic_theme: params.diagnostic_theme,
                diagnostic_icon_colors: params.diagnostic_icon_colors,
                gutter_bg: params.gutter_bg,
                scroll_line_offset: params.scroll_line_offset,
            },
        );
    }

    paint_unwrapped_document_frame(
        window,
        cx,
        UnwrappedDocumentFramePaintParams {
            frame: params.frame,
            text: params.text,
            bounds: params.bounds,
            layout: params.layout,
            text_style: params.text_style,
            line_cache: params.line_cache,
            font_size: params.font_size,
            fg_color: params.fg_color,
            default_bg: params.default_bg,
            cursorline_color: params.cursorline_color,
            is_focused: params.is_focused,
            element_focused: params.element_focused,
            selection_primary: params.selection_primary,
            selection_secondary: params.selection_secondary,
            diagnostic_theme: params.diagnostic_theme,
            diagnostic_icon_colors: params.diagnostic_icon_colors,
            gutter_bg: params.gutter_bg,
            scroll_line_offset: params.scroll_line_offset,
        },
    )
}

fn paint_soft_wrap_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: SoftWrapDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let _timer = PerfTimer::new("paint_soft_wrap_document_frame")
        .with_warn_threshold(Duration::from_millis(8));
    let frame = params.frame;
    let soft_wrap_render_plan = frame.soft_wrap_render_plan.as_ref()?;

    let soft_wrap_paint_plans = soft_wrap_render_plan.line_paint_plans(
        params.layout.line_height,
        params.scroll_line_offset,
        frame.render_snapshot.cursor_line,
    );

    for (line_plan, line_runs) in soft_wrap_paint_plans
        .into_iter()
        .zip(frame.soft_wrap_line_runs.iter())
    {
        match paint_soft_wrap_editor_line(
            window,
            cx,
            SoftWrapEditorLinePaintParams {
                plan: line_plan,
                line_runs,
                line_cache: params.line_cache,
                font_size: params.font_size,
                viewport_width: params.bounds.size.width,
                line_height: params.layout.line_height,
                cursorline_color: params.cursorline_color,
                background_style: EditorLineBackgroundStyle {
                    only_selection_backgrounds: line_plan.is_cursor_visual_line,
                    selection_primary: params.selection_primary,
                    selection_secondary: params.selection_secondary,
                },
            },
        ) {
            Ok(Some(layout)) => params.line_cache.push(layout),
            Ok(None) => {}
            Err(e) => {
                error!(error = ?e, "Failed to paint text");
            }
        }
    }

    paint_frame_gutter(
        window,
        cx,
        DocumentFrameGutterPaintParams {
            frame,
            bounds: params.bounds,
            layout: params.layout,
            text_style: params.text_style,
            font_size: params.font_size,
            diagnostic_theme: params.diagnostic_theme,
            diagnostic_icon_colors: params.diagnostic_icon_colors,
            gutter_bg: params.gutter_bg,
            scroll_line_offset: params.scroll_line_offset,
        },
    );

    paint_soft_wrap_cursor(window, cx, &params)
}

fn paint_unwrapped_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: UnwrappedDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let _timer = PerfTimer::new("paint_unwrapped_document_frame")
        .with_warn_threshold(Duration::from_millis(8));
    let frame = params.frame;
    let unwrapped_plan = frame.unwrapped_render_plan.as_ref()?;

    let unwrapped_paint_plans = unwrapped_plan.line_paint_plans();

    for (unwrapped_plan, highlighted_line) in unwrapped_paint_plans
        .into_iter()
        .zip(frame.unwrapped_highlighted_lines.iter())
    {
        let line_plan = unwrapped_plan.line;
        let line_idx = line_plan.line_idx;
        let y_offset = line_plan.y_offset;
        let line_text = highlighted_line.line_text.clone();
        let line_runs = &highlighted_line.line_runs;

        if unwrapped_plan.is_cursor_line && params.cursorline_color.is_some() {
            trace!(
                "Painting cursorline for line {} (cursor at line {})",
                line_idx, frame.render_snapshot.cursor_line
            );
        }

        let layout = match paint_unwrapped_editor_line(
            window,
            cx,
            UnwrappedEditorLinePaintParams {
                plan: unwrapped_plan,
                line_text,
                line_runs,
                line_cache: params.line_cache,
                font_size: params.font_size,
                viewport_width: params.bounds.size.width,
                line_height: params.layout.line_height,
                cursorline_color: params.cursorline_color,
                background_style: EditorLineBackgroundStyle {
                    only_selection_backgrounds: unwrapped_plan.is_cursor_line,
                    selection_primary: params.selection_primary,
                    selection_secondary: params.selection_secondary,
                },
            },
        ) {
            Ok(layout) => layout,
            Err(e) => {
                error!(error = ?e, "Failed to paint text");
                continue;
            }
        };

        trace!(
            "LINE LAYOUT CACHED: line_idx={}, y_offset={:?}, is_phantom={}",
            line_idx, y_offset, false
        );
        params.line_cache.push(layout);
    }

    let cursor_overlay = paint_unwrapped_cursor(window, cx, &params);
    paint_frame_gutter(
        window,
        cx,
        DocumentFrameGutterPaintParams {
            frame,
            bounds: params.bounds,
            layout: params.layout,
            text_style: params.text_style,
            font_size: params.font_size,
            diagnostic_theme: params.diagnostic_theme,
            diagnostic_icon_colors: params.diagnostic_icon_colors,
            gutter_bg: params.gutter_bg,
            scroll_line_offset: params.scroll_line_offset,
        },
    );

    cursor_overlay
}

fn paint_frame_gutter(
    window: &mut Window,
    cx: &mut App,
    params: DocumentFrameGutterPaintParams<'_>,
) -> Vec<GutterLine> {
    let gutter_lines = build_gutter_lines_from_plans(
        window.text_system().clone(),
        params.text_style,
        params.font_size,
        &params.frame.gutter_line_plans,
    );

    paint_gutter_lines(
        window,
        cx,
        &gutter_lines,
        params.layout.line_height,
        params.diagnostic_theme,
        |result| {
            let Err(e) = result else {
                return;
            };
            error!(error = ?e, "Failed to paint gutter line");
        },
    );

    let marker_origin = frame_gutter_origin(
        params.frame,
        params.bounds,
        params.layout,
        params.scroll_line_offset,
    );
    paint_diagnostic_gutter_markers(
        window,
        cx,
        DiagnosticGutterMarkersPaintParams {
            severity_by_line: &params.frame.diagnostic_severity_by_line,
            gutter_lines: &gutter_lines,
            gutter_origin: marker_origin,
            line_height: params.layout.line_height,
            icon_colors: params.diagnostic_icon_colors,
            gutter_bg: params.gutter_bg,
        },
    );

    gutter_lines
}

fn frame_gutter_origin(
    frame: &EditorDocumentFrame,
    bounds: Bounds<Pixels>,
    layout: &EditorLayout,
    scroll_line_offset: Pixels,
) -> gpui::Point<Pixels> {
    let horizontal_offset = frame
        .unwrapped_render_plan
        .as_ref()
        .map_or(0, |plan| plan.horizontal_offset);

    gutter_origin(
        bounds,
        scroll_line_offset,
        layout.cell_width,
        horizontal_offset,
    )
}

fn paint_soft_wrap_cursor(
    window: &mut Window,
    cx: &mut App,
    params: &SoftWrapDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let frame = params.frame;
    let soft_wrap_render_plan = frame.soft_wrap_render_plan.as_ref()?;
    let cursor_paint_plan = soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
        text: params.text,
        text_format: &soft_wrap_render_plan.text_format,
        anchor: soft_wrap_render_plan.view_offset.anchor,
        cursor_char_idx: frame.cursor_presentation.cursor_char_idx,
        geometry: EditorSurfaceGeometry::new(
            params.bounds,
            frame.gutter_width,
            params.layout.cell_width,
        ),
        line_height: params.layout.line_height,
        cell_width: params.layout.cell_width,
        scroll_line_offset: params.scroll_line_offset,
        vertical_offset: soft_wrap_render_plan.view_offset.vertical_offset,
        viewport_height: soft_wrap_render_plan.viewport_height,
        horizontal_offset: soft_wrap_render_plan.view_offset.horizontal_offset,
    })?;

    let font = params.text_style.font();
    Some(shape_and_paint_editor_cursor(
        window,
        cx,
        EditorCursorTextPaintParams {
            paint_position: cursor_paint_plan.paint_position,
            kind: frame.cursor_presentation.kind,
            cursor_style: &frame.cursor_presentation.cursor_style,
            text_style_at_cursor: &frame.cursor_presentation.text_style_at_cursor,
            cursor_text: frame.cursor_presentation.block_text.clone(),
            font: &font,
            font_size: params.font_size,
            fallback_fg: params.fg_color,
            default_bg: params.default_bg,
            is_focused: params.is_focused,
            fallback_width: params.layout.cell_width,
            line_height: params.layout.line_height,
        },
    ))
}

fn paint_unwrapped_cursor(
    window: &mut Window,
    cx: &mut App,
    params: &UnwrappedDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let frame = params.frame;
    let cursor_viewport_pos = frame.render_snapshot.cursor_viewport_position;
    trace!(
        "Cursor rendering check - is_focused: {}, element_focused: {}, cursor_viewport_pos: {:?}",
        params.is_focused, params.element_focused, cursor_viewport_pos
    );
    trace!(
        "Cursor char idx: {}, line: {}",
        frame.render_snapshot.cursor_char_idx, frame.render_snapshot.cursor_line
    );

    let Some(cursor_viewport_pos) = cursor_viewport_pos else {
        trace!(
            "Cursor line {} is outside rendered range {}..{}",
            frame.render_snapshot.cursor_doc_line,
            frame.render_snapshot.line_viewport.first_row,
            frame.render_snapshot.last_row
        );
        return None;
    };

    let cursor_line = cursor_viewport_pos.line;
    trace!(
        "Looking for cursor line {cursor_line} in range {}..{}",
        frame.render_snapshot.line_viewport.first_row, frame.render_snapshot.last_row
    );

    let line_layout = params.line_cache.find_line_by_index(cursor_line);
    let line_viewport = frame.render_snapshot.line_viewport;
    let cursor_paint_plan = unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
        text: params.text.slice(..),
        geometry: EditorSurfaceGeometry::new(
            params.bounds,
            frame.gutter_width,
            params.layout.cell_width,
        ),
        cursor_char_idx: frame.cursor_presentation.cursor_char_idx,
        cursor_at_trailing_newline: line_viewport.cursor_at_end
            && line_viewport.file_ends_with_newline,
        cursor_viewport_position: Some(cursor_viewport_pos),
        line_layout: line_layout.as_ref(),
        line_height: params.layout.line_height,
        scroll_line_offset: params.scroll_line_offset,
    });

    let Some(cursor_paint_plan) = cursor_paint_plan else {
        trace!(
            "Cursor paint plan unavailable for visible line {} (at_eof: {})",
            cursor_line,
            line_viewport.cursor_at_end && line_viewport.file_ends_with_newline
        );
        return None;
    };

    let cursor_paint_position = cursor_paint_plan.paint_position;
    if let Some(line_position) = &cursor_paint_plan.line_position {
        trace!(
            "Cursor rendering - line: {}, char_offset: {}, byte_offset: {}, x_relative: {:?}, x_absolute: {:?}, viewport_row: {}",
            line_position.line,
            line_position.cursor_char_offset,
            line_position.cursor_byte_offset,
            cursor_paint_position.cursor_origin.x,
            cursor_paint_position.cursor_point().x,
            cursor_viewport_pos.viewport_row
        );
    } else {
        trace!(
            "Cursor rendering - source: {:?}, x_absolute: {:?}, viewport_row: {}",
            cursor_paint_plan.source,
            cursor_paint_position.cursor_point().x,
            cursor_viewport_pos.viewport_row
        );
    }
    trace!("Cursor paint plan selected: {:?}", cursor_paint_plan.source);

    let font = params.text_style.font();
    Some(shape_and_paint_editor_cursor(
        window,
        cx,
        EditorCursorTextPaintParams {
            paint_position: cursor_paint_position,
            kind: frame.cursor_presentation.kind,
            cursor_style: &frame.cursor_presentation.cursor_style,
            text_style_at_cursor: &frame.cursor_presentation.text_style_at_cursor,
            cursor_text: frame.cursor_presentation.block_text.clone(),
            font: &font,
            font_size: params.font_size,
            fallback_fg: params.fg_color,
            default_bg: params.default_bg,
            is_focused: params.is_focused,
            fallback_width: params.layout.cell_width,
            line_height: params.layout.line_height,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use gpui::{TextStyle, black, point, px, size, white};
    use helix_core::{Transaction, syntax};
    use helix_view::{
        DocumentId, Editor, ViewId,
        document::Mode,
        editor::{Action, Config},
        graphics::{Color, Modifier, Rect, Style},
        handlers::Handlers,
        theme,
    };

    use crate::{EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorViewState, EditorViewportSurfaceLayout};

    use super::*;

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor_with_text(text: &str) -> (Editor, DocumentId, ViewId) {
        let mut config = Config::default();
        config.cursorline = true;
        test_editor_with_config(text, config)
    }

    fn test_editor_with_config(text: &str, config: Config) -> (Editor, DocumentId, ViewId) {
        let config = Arc::new(ArcSwap::new(Arc::new(config)));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        (editor, doc_id, view_id)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frame_paint_style_resolves_theme_styles_and_fallbacks() {
        let fallback_gutter_color = helix_color_to_hsla(Color::Rgb(80, 90, 100)).unwrap();
        let fallback_ruler_color = helix_color_to_hsla(Color::Rgb(110, 120, 130)).unwrap();
        let selection_primary = helix_color_to_hsla(Color::Rgb(140, 150, 160)).unwrap();
        let selection_secondary = helix_color_to_hsla(Color::Rgb(170, 180, 190)).unwrap();
        let diagnostic_highlight_base = helix_color_to_hsla(Color::Rgb(200, 210, 220)).unwrap();
        let diagnostic_icon_colors = test_diagnostic_icon_colors();

        let style = native_editor_frame_paint_style(NativeEditorFramePaintStyleParams {
            editor_mode: Mode::Normal,
            theme_styles: NativeEditorFrameThemeStyles::from_style_fn(|key| match key {
                "ui.cursor" => Style::default().add_modifier(Modifier::BOLD),
                "ui.cursor.primary" => Style::default().bg(Color::Rgb(1, 2, 3)),
                "ui.virtual.wrap" => Style::default().fg(Color::Rgb(4, 5, 6)),
                "ui.virtual.ruler" => Style::default().bg(Color::Rgb(7, 8, 9)),
                "ui.cursorline.primary" => Style::default().bg(Color::Rgb(10, 11, 12)),
                "ui.linenr" => Style::default().fg(Color::Rgb(13, 14, 15)),
                "ui.gutter" => Style::default().bg(Color::Rgb(16, 17, 18)),
                _ => Style::default(),
            }),
            palette: NativeEditorFramePalette {
                fg_color: black(),
                bg_color: white(),
                selection_primary,
                selection_secondary,
                fallback_gutter_color,
                diagnostic_highlight_base,
                diagnostic_icon_colors,
                fallback_ruler_color,
                run_button_color: fallback_gutter_color,
            },
        });

        assert_eq!(style.fg_color, black());
        assert_eq!(style.bg_color, white());
        assert_eq!(
            style.default_text_style.fg,
            gpui_hsla_to_helix_color(black())
        );
        assert_eq!(
            style.default_text_style.bg,
            gpui_hsla_to_helix_color(white())
        );
        assert_eq!(style.cursor_style.bg, Some(Color::Rgb(1, 2, 3)));
        assert!(style.cursor_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            style.wrap_indicator_color,
            helix_color_to_hsla(Color::Rgb(4, 5, 6))
        );
        assert_eq!(
            style.ruler_color,
            helix_color_to_hsla(Color::Rgb(7, 8, 9)).unwrap()
        );
        assert_eq!(
            style.cursorline_color,
            helix_color_to_hsla(Color::Rgb(10, 11, 12))
        );
        assert_eq!(
            style.gutter_color,
            helix_color_to_hsla(Color::Rgb(13, 14, 15)).unwrap()
        );
        assert_eq!(style.gutter_selected_color, fallback_gutter_color);
        assert_eq!(style.gutter_bg, helix_color_to_hsla(Color::Rgb(16, 17, 18)));
        assert_eq!(style.selection_primary, selection_primary);
        assert_eq!(style.selection_secondary, selection_secondary);
        assert_eq!(style.diagnostic_highlight_base, diagnostic_highlight_base);
        assert_eq!(style.diagnostic_icon_colors, diagnostic_icon_colors);
        assert_eq!(style.run_button_color, fallback_gutter_color);
    }

    fn paint_palette() -> NativeEditorFramePalette {
        NativeEditorFramePalette {
            fg_color: black(),
            bg_color: white(),
            selection_primary: black(),
            selection_secondary: white(),
            fallback_gutter_color: black(),
            diagnostic_highlight_base: black(),
            diagnostic_icon_colors: test_diagnostic_icon_colors(),
            fallback_ruler_color: black(),
            run_button_color: black(),
        }
    }

    fn paint_style() -> NativeEditorFramePaintStyle {
        NativeEditorFramePaintStyle {
            fg_color: black(),
            bg_color: white(),
            default_text_style: Style::default(),
            cursor_style: Style::default(),
            cursorline_color: Some(black()),
            selection_primary: black(),
            selection_secondary: white(),
            gutter_color: black(),
            gutter_selected_color: white(),
            diagnostic_highlight_base: black(),
            diagnostic_icon_colors: test_diagnostic_icon_colors(),
            gutter_bg: None,
            wrap_indicator_color: None,
            ruler_color: black(),
            run_button_color: black(),
        }
    }

    fn test_diagnostic_icon_colors() -> DiagnosticSeverityIconColors {
        DiagnosticSeverityIconColors {
            error: black(),
            warning: white(),
            info: black(),
            hint: white(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frame_paint_plan_owns_frame_and_text() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\n");
        let theme = theme::Loader::new(&[]).default_theme(true);
        let bounds = gpui::Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0)));
        let scrolloff = editor.config().scrolloff;
        let frame_state = state
            .sync_frame_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout::for_editor(
                    Some(&theme),
                    bounds,
                    px(8.0),
                    px(20.0),
                    scrolloff,
                    None,
                ),
            )
            .unwrap();
        let document = editor.document(doc_id).unwrap();
        let view = editor.tree.try_get(view_id).unwrap();
        let syntax_loader = editor.syn_loader.load();
        let editor_config = editor.config();
        let editor_mode = editor.mode();
        let (_, cursor_kind) = editor.cursor();
        let text_style = TextStyle::default();
        let layout = crate::EditorLayout {
            rows: 6,
            columns: 30,
            line_height: px(20.0),
            font_size: px(16.0),
            cell_width: px(8.0),
        };

        let plan = native_editor_frame_paint_plan(NativeEditorFramePlanParams {
            editor: &editor,
            document,
            view,
            view_id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            frame_state: &frame_state,
            bounds,
            layout: &layout,
            text_style: &text_style,
            font_size: px(16.0),
            is_focused: true,
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            editor_mode,
            cursor_kind,
            cursor_shape: editor_config.cursor_shape.clone(),
            editor_rulers: editor_config.rulers.clone(),
            cursorline_enabled: editor_config.cursorline,
            style: paint_style(),
        });

        assert_eq!(plan.text.to_string(), document.text().to_string());
        assert_eq!(plan.frame.total_lines, document.text().len_lines());
        assert!(plan.frame.cursorline_enabled);
        assert!(!plan.frame.gutter_line_plans.is_empty());
        assert_eq!(plan.bounds, bounds);
        assert_eq!(plan.font_size, px(16.0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frame_paint_plan_builds_soft_wrap_gutter_from_shared_plan() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let mut config = Config::default();
        config.cursorline = true;
        config.soft_wrap.enable = Some(true);
        let (mut editor, doc_id, view_id) =
            test_editor_with_config("alpha beta gamma delta epsilon zeta eta theta\n", config);
        let theme = theme::Loader::new(&[]).default_theme(true);
        let bounds = gpui::Bounds::new(point(px(0.0), px(0.0)), size(px(120.0), px(120.0)));
        let scrolloff = editor.config().scrolloff;
        let mut frame_state = state
            .sync_frame_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout::for_editor(
                    Some(&theme),
                    bounds,
                    px(8.0),
                    px(20.0),
                    scrolloff,
                    None,
                ),
            )
            .unwrap();
        frame_state.viewport_update.soft_wrap = true;

        let document = editor.document(doc_id).unwrap();
        let view = editor.tree.try_get(view_id).unwrap();
        let syntax_loader = editor.syn_loader.load();
        let editor_config = editor.config();
        let editor_mode = editor.mode();
        let (_, cursor_kind) = editor.cursor();
        let text_style = TextStyle::default();
        let layout = crate::EditorLayout {
            rows: 6,
            columns: 15,
            line_height: px(20.0),
            font_size: px(16.0),
            cell_width: px(8.0),
        };

        let plan = native_editor_frame_paint_plan(NativeEditorFramePlanParams {
            editor: &editor,
            document,
            view,
            view_id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            frame_state: &frame_state,
            bounds,
            layout: &layout,
            text_style: &text_style,
            font_size: px(16.0),
            is_focused: true,
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            editor_mode,
            cursor_kind,
            cursor_shape: editor_config.cursor_shape.clone(),
            editor_rulers: editor_config.rulers.clone(),
            cursorline_enabled: editor_config.cursorline,
            style: paint_style(),
        });

        assert!(plan.frame.soft_wrap_render_plan.is_some());
        assert!(plan.frame.unwrapped_render_plan.is_none());
        let line_number_plan = plan
            .frame
            .gutter_line_plans
            .iter()
            .find(|plan| plan.doc_line == 0 && plan.first_visual_line && plan.text.trim() == "1")
            .expect("soft-wrap line number gutter plan");
        assert_ne!(line_number_plan.text, "   1 ");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prepare_native_editor_frame_syncs_layout_and_builds_plan() {
        let mut state = EditorViewState::new(px(20.0), px(8.0));
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\n");
        let theme = theme::Loader::new(&[]).default_theme(true);
        let bounds = gpui::Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0)));
        let text_style = TextStyle::default();
        let mut layout = crate::EditorLayout {
            rows: 6,
            columns: 30,
            line_height: px(20.0),
            font_size: px(16.0),
            cell_width: px(8.0),
        };

        let prepared = prepare_native_editor_frame(NativeEditorFramePrepareParams {
            editor: &mut editor,
            view_id,
            editor_state: &mut state,
            theme: &theme,
            bounds,
            layout: &mut layout,
            text_style: &text_style,
            font_size: px(16.0),
            is_focused: true,
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            theme_styles: NativeEditorFrameThemeStyles::default(),
            palette: paint_palette(),
        })
        .unwrap();

        let document = editor.document(doc_id).unwrap();
        assert_eq!(
            prepared.paint_plan.text.to_string(),
            document.text().to_string()
        );
        assert_eq!(
            prepared.paint_plan.frame.total_lines,
            document.text().len_lines()
        );
        assert_eq!(prepared.paint_plan.bounds, bounds);
        assert_eq!(prepared.paint_plan.font_size, px(16.0));
        assert!(prepared.frame_state.viewport_update.visual_rows >= 2);
        assert_eq!(
            prepared.frame_state.first_row,
            state.viewport().visible_visual_range().0
        );
        assert_eq!(
            prepared.frame_state.last_row_from_scroll,
            state.viewport().visible_visual_range().1
        );
        assert_eq!(
            prepared.frame_state.scroll_line_offset,
            state.viewport().offset_within_row()
        );
        assert_eq!(state.surface_metrics().get().cell_width, px(8.0));
        assert_eq!(state.surface_metrics().get().line_height, px(20.0));
    }
}
