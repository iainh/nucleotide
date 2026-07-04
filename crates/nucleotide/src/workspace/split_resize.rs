use gpui::{Pixels, px};
use helix_view::{ViewId, graphics::Rect as HelixRect};

const SPLIT_PANE_MIN_WIDTH_CELLS: u16 = 8;
const SPLIT_PANE_MIN_HEIGHT_CELLS: u16 = 3;
const SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DocumentViewLayout {
    pub(super) view_id: ViewId,
    pub(super) area: HelixRect,
    pub(super) is_focused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitPaneResizeAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SplitPaneDivider {
    pub(super) axis: SplitPaneResizeAxis,
    pub(super) before_view_ids: Vec<ViewId>,
    pub(super) after_view_ids: Vec<ViewId>,
    pub(super) edge: u16,
    pub(super) start: u16,
    pub(super) span: u16,
    pub(super) gap: u16,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SplitPaneResizeViewState {
    pub(super) view_id: ViewId,
    pub(super) area: HelixRect,
}

#[derive(Clone, Debug)]
pub(super) struct SplitPaneResizeState {
    pub(super) axis: SplitPaneResizeAxis,
    pub(super) start_mouse_x: f32,
    pub(super) start_mouse_y: f32,
    pub(super) before_views: Vec<SplitPaneResizeViewState>,
    pub(super) after_views: Vec<SplitPaneResizeViewState>,
    pub(super) total_area: HelixRect,
    pub(super) editor_width_px: f32,
    pub(super) editor_height_px: f32,
}

pub(super) fn helix_rect_to_scaled_pixel_bounds(
    area: HelixRect,
    total_area: HelixRect,
    target_width: f32,
    target_height: f32,
) -> (Pixels, Pixels, Pixels, Pixels) {
    let total_width = f32::from(total_area.width).max(1.0);
    let total_height = f32::from(total_area.height).max(1.0);
    let target_width = target_width.max(1.0);
    let target_height = target_height.max(1.0);

    let relative_x = area.x.saturating_sub(total_area.x);
    let relative_y = area.y.saturating_sub(total_area.y);
    let left = f32::from(relative_x) / total_width * target_width;
    let top = f32::from(relative_y) / total_height * target_height;
    let width = (f32::from(area.width) / total_width * target_width).max(1.0);
    let height = (f32::from(area.height) / total_height * target_height).max(1.0);

    (px(left), px(top), px(width), px(height))
}

pub(super) fn split_pane_dividers(layouts: &[DocumentViewLayout]) -> Vec<SplitPaneDivider> {
    let mut dividers = Vec::new();

    for (index, first) in layouts.iter().enumerate() {
        for second in layouts.iter().skip(index + 1) {
            if let Some(divider) = split_pane_vertical_divider(*first, *second)
                .or_else(|| split_pane_vertical_divider(*second, *first))
            {
                push_or_merge_split_pane_divider(&mut dividers, divider);
            }

            if let Some(divider) = split_pane_horizontal_divider(*first, *second)
                .or_else(|| split_pane_horizontal_divider(*second, *first))
            {
                push_or_merge_split_pane_divider(&mut dividers, divider);
            }
        }
    }

    dividers
}

fn push_or_merge_split_pane_divider(
    dividers: &mut Vec<SplitPaneDivider>,
    mut divider: SplitPaneDivider,
) {
    let mut index = 0;
    while index < dividers.len() {
        if split_pane_dividers_can_merge(&dividers[index], &divider) {
            let existing = dividers.remove(index);
            divider = merge_split_pane_dividers(existing, divider);
            index = 0;
        } else {
            index += 1;
        }
    }

    dividers.push(divider);
}

fn split_pane_dividers_can_merge(first: &SplitPaneDivider, second: &SplitPaneDivider) -> bool {
    first.axis == second.axis
        && first.edge == second.edge
        && first.gap == second.gap
        && split_pane_ranges_can_merge(first.start, first.span, second.start, second.span)
}

fn split_pane_ranges_can_merge(
    first_start: u16,
    first_span: u16,
    second_start: u16,
    second_span: u16,
) -> bool {
    let first_end = first_start.saturating_add(first_span);
    let second_end = second_start.saturating_add(second_span);
    first_start <= second_end.saturating_add(SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS)
        && second_start <= first_end.saturating_add(SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS)
}

fn merge_split_pane_dividers(
    mut first: SplitPaneDivider,
    second: SplitPaneDivider,
) -> SplitPaneDivider {
    for view_id in second.before_view_ids {
        push_unique_view_id(&mut first.before_view_ids, view_id);
    }
    for view_id in second.after_view_ids {
        push_unique_view_id(&mut first.after_view_ids, view_id);
    }

    let start = first.start.min(second.start);
    let end = first
        .start
        .saturating_add(first.span)
        .max(second.start.saturating_add(second.span));
    first.start = start;
    first.span = end.saturating_sub(start);
    first.gap = first.gap.max(second.gap);
    first
}

fn push_unique_view_id(view_ids: &mut Vec<ViewId>, view_id: ViewId) {
    if !view_ids.contains(&view_id) {
        view_ids.push(view_id);
    }
}

pub(super) fn split_pane_resize_view_states(
    layouts: &[DocumentViewLayout],
    view_ids: &[ViewId],
) -> Vec<SplitPaneResizeViewState> {
    view_ids
        .iter()
        .filter_map(|view_id| {
            layouts
                .iter()
                .find(|layout| layout.view_id == *view_id)
                .map(|layout| SplitPaneResizeViewState {
                    view_id: *view_id,
                    area: layout.area,
                })
        })
        .collect()
}

fn split_pane_vertical_divider(
    before: DocumentViewLayout,
    after: DocumentViewLayout,
) -> Option<SplitPaneDivider> {
    let before_right = before.area.x.saturating_add(before.area.width);
    let gap = after.area.x.checked_sub(before_right)?;
    if gap > SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS {
        return None;
    }

    let start = before.area.y.max(after.area.y);
    let end = before
        .area
        .y
        .saturating_add(before.area.height)
        .min(after.area.y.saturating_add(after.area.height));
    if end <= start {
        return None;
    }

    Some(SplitPaneDivider {
        axis: SplitPaneResizeAxis::Vertical,
        before_view_ids: vec![before.view_id],
        after_view_ids: vec![after.view_id],
        edge: before_right.saturating_add(gap / 2),
        start,
        span: end - start,
        gap,
    })
}

fn split_pane_horizontal_divider(
    before: DocumentViewLayout,
    after: DocumentViewLayout,
) -> Option<SplitPaneDivider> {
    let before_bottom = before.area.y.saturating_add(before.area.height);
    let gap = after.area.y.checked_sub(before_bottom)?;
    if gap > SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS {
        return None;
    }

    let start = before.area.x.max(after.area.x);
    let end = before
        .area
        .x
        .saturating_add(before.area.width)
        .min(after.area.x.saturating_add(after.area.width));
    if end <= start {
        return None;
    }

    Some(SplitPaneDivider {
        axis: SplitPaneResizeAxis::Horizontal,
        before_view_ids: vec![before.view_id],
        after_view_ids: vec![after.view_id],
        edge: before_bottom.saturating_add(gap / 2),
        start,
        span: end - start,
        gap,
    })
}

pub(super) fn document_view_visual_area(
    layout: DocumentViewLayout,
    dividers: &[SplitPaneDivider],
) -> HelixRect {
    let mut area = layout.area;

    for divider in dividers {
        if divider.gap == 0 || !divider.after_view_ids.contains(&layout.view_id) {
            continue;
        }

        match divider.axis {
            SplitPaneResizeAxis::Vertical => {
                area.x = area.x.saturating_sub(divider.gap);
                area.width = area.width.saturating_add(divider.gap);
            }
            SplitPaneResizeAxis::Horizontal => {
                area.y = area.y.saturating_sub(divider.gap);
                area.height = area.height.saturating_add(divider.gap);
            }
        }
    }

    area
}

pub(super) fn split_pane_divider_visual_line(
    mut divider: SplitPaneDivider,
    dividers: &[SplitPaneDivider],
) -> SplitPaneDivider {
    for other in dividers {
        if divider.axis == other.axis || other.gap == 0 {
            continue;
        }

        let all_views_shift_with_other = divider
            .before_view_ids
            .iter()
            .chain(&divider.after_view_ids)
            .all(|view_id| other.after_view_ids.contains(view_id));
        if !all_views_shift_with_other {
            continue;
        }

        divider.start = divider.start.saturating_sub(other.gap);
        divider.span = divider.span.saturating_add(other.gap);
    }

    divider
}

pub(super) fn split_pane_resized_areas(
    state: &SplitPaneResizeState,
    mouse_x: f32,
    mouse_y: f32,
) -> Option<Vec<(ViewId, HelixRect)>> {
    match state.axis {
        SplitPaneResizeAxis::Vertical => {
            let cells_per_px =
                f32::from(state.total_area.width).max(1.0) / state.editor_width_px.max(1.0);
            let delta = ((mouse_x - state.start_mouse_x) * cells_per_px).round() as i32;
            resized_vertical_split_pane_view_areas(
                &state.before_views,
                &state.after_views,
                delta,
                SPLIT_PANE_MIN_WIDTH_CELLS,
            )
        }
        SplitPaneResizeAxis::Horizontal => {
            let cells_per_px =
                f32::from(state.total_area.height).max(1.0) / state.editor_height_px.max(1.0);
            let delta = ((mouse_y - state.start_mouse_y) * cells_per_px).round() as i32;
            resized_horizontal_split_pane_view_areas(
                &state.before_views,
                &state.after_views,
                delta,
                SPLIT_PANE_MIN_HEIGHT_CELLS,
            )
        }
    }
}

fn resized_vertical_split_pane_view_areas(
    before_views: &[SplitPaneResizeViewState],
    after_views: &[SplitPaneResizeViewState],
    delta_cells: i32,
    min_width: u16,
) -> Option<Vec<(ViewId, HelixRect)>> {
    let min_width = i32::from(min_width.max(1));
    let min_delta = before_views
        .iter()
        .map(|view| min_width - i32::from(view.area.width))
        .max()?;
    let max_delta = after_views
        .iter()
        .map(|view| i32::from(view.area.width) - min_width)
        .min()?;
    if min_delta > max_delta {
        return None;
    }

    let delta = delta_cells.clamp(min_delta, max_delta);
    let mut resized = Vec::with_capacity(before_views.len() + after_views.len());

    for view in before_views {
        let width = i32::from(view.area.width).checked_add(delta)?;
        let width = u16::try_from(width).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, view.area.y, width, view.area.height),
        ));
    }

    for view in after_views {
        let x = i32::from(view.area.x).checked_add(delta)?;
        let width = i32::from(view.area.width).checked_sub(delta)?;
        let x = u16::try_from(x).ok()?;
        let width = u16::try_from(width).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(x, view.area.y, width, view.area.height),
        ));
    }

    Some(resized)
}

fn resized_horizontal_split_pane_view_areas(
    before_views: &[SplitPaneResizeViewState],
    after_views: &[SplitPaneResizeViewState],
    delta_cells: i32,
    min_height: u16,
) -> Option<Vec<(ViewId, HelixRect)>> {
    let min_height = i32::from(min_height.max(1));
    let min_delta = before_views
        .iter()
        .map(|view| min_height - i32::from(view.area.height))
        .max()?;
    let max_delta = after_views
        .iter()
        .map(|view| i32::from(view.area.height) - min_height)
        .min()?;
    if min_delta > max_delta {
        return None;
    }

    let delta = delta_cells.clamp(min_delta, max_delta);
    let mut resized = Vec::with_capacity(before_views.len() + after_views.len());

    for view in before_views {
        let height = i32::from(view.area.height).checked_add(delta)?;
        let height = u16::try_from(height).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, view.area.y, view.area.width, height),
        ));
    }

    for view in after_views {
        let y = i32::from(view.area.y).checked_add(delta)?;
        let height = i32::from(view.area.height).checked_sub(delta)?;
        let y = u16::try_from(y).ok()?;
        let height = u16::try_from(height).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, y, view.area.width, height),
        ));
    }

    Some(resized)
}

#[cfg(test)]
fn resized_vertical_split_pane_areas(
    before: HelixRect,
    after: HelixRect,
    delta_cells: i32,
    min_width: u16,
) -> Option<(HelixRect, HelixRect)> {
    let before_right = before.x.checked_add(before.width)?;
    let outer_left = before.x;
    let outer_right = after.x.checked_add(after.width)?;
    let gap = after.x.checked_sub(before_right)?;
    let usable = outer_right.checked_sub(outer_left)?.checked_sub(gap)?;
    let min_width = min_width.min(usable.saturating_sub(1)).max(1);
    let max_before = usable.saturating_sub(min_width);
    if max_before < min_width {
        return None;
    }

    let target_before = (i32::from(before.width) + delta_cells)
        .clamp(i32::from(min_width), i32::from(max_before)) as u16;
    let after_x = outer_left.checked_add(target_before)?.checked_add(gap)?;
    let after_width = outer_right.checked_sub(after_x)?;

    Some((
        HelixRect::new(before.x, before.y, target_before, before.height),
        HelixRect::new(after_x, after.y, after_width, after.height),
    ))
}

#[cfg(test)]
fn resized_horizontal_split_pane_areas(
    before: HelixRect,
    after: HelixRect,
    delta_cells: i32,
    min_height: u16,
) -> Option<(HelixRect, HelixRect)> {
    let before_bottom = before.y.checked_add(before.height)?;
    let outer_top = before.y;
    let outer_bottom = after.y.checked_add(after.height)?;
    let gap = after.y.checked_sub(before_bottom)?;
    let usable = outer_bottom.checked_sub(outer_top)?.checked_sub(gap)?;
    let min_height = min_height.min(usable.saturating_sub(1)).max(1);
    let max_before = usable.saturating_sub(min_height);
    if max_before < min_height {
        return None;
    }

    let target_before = (i32::from(before.height) + delta_cells)
        .clamp(i32::from(min_height), i32::from(max_before)) as u16;
    let after_y = outer_top.checked_add(target_before)?.checked_add(gap)?;
    let after_height = outer_bottom.checked_sub(after_y)?;

    Some((
        HelixRect::new(before.x, before.y, before.width, target_before),
        HelixRect::new(after.x, after_y, after.width, after_height),
    ))
}

pub(super) fn document_view_layout_bounds(layouts: &[DocumentViewLayout]) -> Option<HelixRect> {
    let first = layouts.first()?;
    let mut min_x = first.area.x;
    let mut min_y = first.area.y;
    let mut max_x = first.area.x.saturating_add(first.area.width);
    let mut max_y = first.area.y.saturating_add(first.area.height);

    for layout in &layouts[1..] {
        min_x = min_x.min(layout.area.x);
        min_y = min_y.min(layout.area.y);
        max_x = max_x.max(layout.area.x.saturating_add(layout.area.width));
        max_y = max_y.max(layout.area.y.saturating_add(layout.area.height));
    }

    Some(HelixRect::new(
        min_x,
        min_y,
        max_x.saturating_sub(min_x).max(1),
        max_y.saturating_sub(min_y).max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::KeyData;

    fn test_view_id(index: u64) -> ViewId {
        ViewId::from(KeyData::from_ffi((1_u64 << 32) | index))
    }

    #[test]
    fn helix_rect_to_scaled_pixel_bounds_fills_target_for_single_view() {
        let (left, top, width, height) = helix_rect_to_scaled_pixel_bounds(
            HelixRect::new(0, 0, 80, 24),
            HelixRect::new(0, 0, 80, 24),
            800.0,
            240.0,
        );

        assert_eq!(f32::from(left), 0.0);
        assert_eq!(f32::from(top), 0.0);
        assert_eq!(f32::from(width), 800.0);
        assert_eq!(f32::from(height), 240.0);
    }

    #[test]
    fn helix_rect_to_scaled_pixel_bounds_maps_split_ratios_to_target() {
        let (left, top, width, height) = helix_rect_to_scaled_pixel_bounds(
            HelixRect::new(40, 12, 40, 12),
            HelixRect::new(0, 0, 80, 24),
            800.0,
            240.0,
        );

        assert_eq!(f32::from(left), 400.0);
        assert_eq!(f32::from(top), 120.0);
        assert_eq!(f32::from(width), 400.0);
        assert_eq!(f32::from(height), 120.0);
    }

    #[test]
    fn document_view_layout_bounds_covers_all_view_rects() {
        let layouts = vec![
            DocumentViewLayout {
                view_id: test_view_id(1),
                area: HelixRect::new(20, 5, 30, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: test_view_id(2),
                area: HelixRect::new(0, 0, 20, 15),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: test_view_id(3),
                area: HelixRect::new(50, 5, 10, 20),
                is_focused: false,
            },
        ];

        assert_eq!(
            document_view_layout_bounds(&layouts),
            Some(HelixRect::new(0, 0, 60, 25))
        );
        assert_eq!(document_view_layout_bounds(&[]), None);
    }

    #[test]
    fn split_pane_dividers_detect_vertical_shared_edge() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(40, 0, 40, 20),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Vertical);
        assert_eq!(dividers[0].edge, 40);
        assert_eq!(dividers[0].start, 0);
        assert_eq!(dividers[0].span, 20);
        assert_eq!(dividers[0].gap, 0);
        assert_eq!(dividers[0].before_view_ids, vec![before_id]);
        assert_eq!(dividers[0].after_view_ids, vec![after_id]);
    }

    #[test]
    fn split_pane_dividers_detect_horizontal_shared_edge() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 80, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(0, 10, 80, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Horizontal);
        assert_eq!(dividers[0].edge, 10);
        assert_eq!(dividers[0].start, 0);
        assert_eq!(dividers[0].span, 80);
        assert_eq!(dividers[0].gap, 0);
        assert_eq!(dividers[0].before_view_ids, vec![before_id]);
        assert_eq!(dividers[0].after_view_ids, vec![after_id]);
    }

    #[test]
    fn document_view_visual_area_expands_after_vertical_separator_cell() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(41, 0, 40, 20),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Vertical);
        assert_eq!(dividers[0].edge, 40);
        assert_eq!(dividers[0].gap, 1);
        assert_eq!(
            document_view_visual_area(layouts[0], &dividers),
            HelixRect::new(0, 0, 40, 20)
        );
        assert_eq!(
            document_view_visual_area(layouts[1], &dividers),
            HelixRect::new(40, 0, 41, 20)
        );
    }

    #[test]
    fn document_view_visual_area_expands_after_horizontal_separator_cell() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 80, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(0, 11, 80, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Horizontal);
        assert_eq!(dividers[0].edge, 10);
        assert_eq!(dividers[0].gap, 1);
        assert_eq!(
            document_view_visual_area(layouts[0], &dividers),
            HelixRect::new(0, 0, 80, 10)
        );
        assert_eq!(
            document_view_visual_area(layouts[1], &dividers),
            HelixRect::new(0, 10, 80, 11)
        );
    }

    #[test]
    fn split_pane_dividers_merge_horizontal_segments_across_vertical_separator_cell() {
        let top_id = test_view_id(1);
        let bottom_left_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: top_id,
                area: HelixRect::new(0, 0, 81, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: bottom_left_id,
                area: HelixRect::new(0, 11, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let horizontal = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Horizontal)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(horizontal.edge, 10);
        assert_eq!(horizontal.start, 0);
        assert_eq!(horizontal.span, 81);
        assert_eq!(horizontal.gap, 1);
        assert_eq!(horizontal.before_view_ids, vec![top_id]);
        assert_eq!(
            horizontal.after_view_ids,
            vec![bottom_left_id, bottom_right_id]
        );
    }

    #[test]
    fn split_pane_dividers_merge_vertical_segments_across_horizontal_separator_cell() {
        let left_id = test_view_id(1);
        let right_top_id = test_view_id(2);
        let right_bottom_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: left_id,
                area: HelixRect::new(0, 0, 40, 21),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: right_top_id,
                area: HelixRect::new(41, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: right_bottom_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 0);
        assert_eq!(vertical.span, 21);
        assert_eq!(vertical.gap, 1);
        assert_eq!(vertical.before_view_ids, vec![left_id]);
        assert_eq!(vertical.after_view_ids, vec![right_top_id, right_bottom_id]);
    }

    #[test]
    fn split_pane_divider_visual_line_expands_horizontal_inside_after_vertical_group() {
        let middle_id = test_view_id(1);
        let right_top_id = test_view_id(2);
        let right_bottom_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: middle_id,
                area: HelixRect::new(0, 0, 40, 21),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: right_top_id,
                area: HelixRect::new(41, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: right_bottom_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let horizontal = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Horizontal)
            .unwrap();

        assert_eq!(horizontal.edge, 10);
        assert_eq!(horizontal.start, 41);
        assert_eq!(horizontal.span, 40);

        let visual = split_pane_divider_visual_line(horizontal.clone(), &dividers);

        assert_eq!(visual.edge, 10);
        assert_eq!(visual.start, 40);
        assert_eq!(visual.span, 41);
    }

    #[test]
    fn split_pane_divider_visual_line_expands_vertical_inside_after_horizontal_group() {
        let top_id = test_view_id(1);
        let bottom_left_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: top_id,
                area: HelixRect::new(0, 0, 81, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: bottom_left_id,
                area: HelixRect::new(0, 11, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 11);
        assert_eq!(vertical.span, 10);

        let visual = split_pane_divider_visual_line(vertical.clone(), &dividers);

        assert_eq!(visual.edge, 40);
        assert_eq!(visual.start, 10);
        assert_eq!(visual.span, 11);
    }

    #[test]
    fn split_pane_dividers_merge_nested_leaf_segments() {
        let left_id = test_view_id(1);
        let top_right_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: left_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: top_right_id,
                area: HelixRect::new(40, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(40, 10, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 0);
        assert_eq!(vertical.span, 20);
        assert_eq!(vertical.gap, 0);
        assert_eq!(vertical.before_view_ids, vec![left_id]);
        assert_eq!(vertical.after_view_ids, vec![top_right_id, bottom_right_id]);
    }

    #[test]
    fn resized_vertical_split_pane_areas_clamp_to_min_width() {
        let before = HelixRect::new(0, 0, 40, 20);
        let after = HelixRect::new(40, 0, 40, 20);

        assert_eq!(
            resized_vertical_split_pane_areas(before, after, 10, SPLIT_PANE_MIN_WIDTH_CELLS),
            Some((HelixRect::new(0, 0, 50, 20), HelixRect::new(50, 0, 30, 20)))
        );
        assert_eq!(
            resized_vertical_split_pane_areas(before, after, -100, SPLIT_PANE_MIN_WIDTH_CELLS),
            Some((HelixRect::new(0, 0, 8, 20), HelixRect::new(8, 0, 72, 20)))
        );
    }

    #[test]
    fn resized_horizontal_split_pane_areas_clamp_to_min_height() {
        let before = HelixRect::new(0, 0, 80, 10);
        let after = HelixRect::new(0, 10, 80, 10);

        assert_eq!(
            resized_horizontal_split_pane_areas(before, after, 4, SPLIT_PANE_MIN_HEIGHT_CELLS),
            Some((HelixRect::new(0, 0, 80, 14), HelixRect::new(0, 14, 80, 6)))
        );
        assert_eq!(
            resized_horizontal_split_pane_areas(before, after, -100, SPLIT_PANE_MIN_HEIGHT_CELLS),
            Some((HelixRect::new(0, 0, 80, 3), HelixRect::new(0, 3, 80, 17)))
        );
    }

    #[test]
    fn split_pane_resized_areas_convert_mouse_delta_to_cells() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let state = SplitPaneResizeState {
            axis: SplitPaneResizeAxis::Vertical,
            start_mouse_x: 200.0,
            start_mouse_y: 0.0,
            before_views: vec![SplitPaneResizeViewState {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
            }],
            after_views: vec![SplitPaneResizeViewState {
                view_id: after_id,
                area: HelixRect::new(40, 0, 40, 20),
            }],
            total_area: HelixRect::new(0, 0, 80, 20),
            editor_width_px: 800.0,
            editor_height_px: 200.0,
        };

        assert_eq!(
            split_pane_resized_areas(&state, 300.0, 0.0),
            Some(vec![
                (before_id, HelixRect::new(0, 0, 50, 20)),
                (after_id, HelixRect::new(50, 0, 30, 20)),
            ])
        );
    }

    #[test]
    fn split_pane_resized_areas_resize_grouped_panes_together() {
        let before_id = test_view_id(1);
        let top_after_id = test_view_id(2);
        let bottom_after_id = test_view_id(3);
        let state = SplitPaneResizeState {
            axis: SplitPaneResizeAxis::Vertical,
            start_mouse_x: 200.0,
            start_mouse_y: 0.0,
            before_views: vec![SplitPaneResizeViewState {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
            }],
            after_views: vec![
                SplitPaneResizeViewState {
                    view_id: top_after_id,
                    area: HelixRect::new(40, 0, 40, 10),
                },
                SplitPaneResizeViewState {
                    view_id: bottom_after_id,
                    area: HelixRect::new(40, 10, 40, 10),
                },
            ],
            total_area: HelixRect::new(0, 0, 80, 20),
            editor_width_px: 800.0,
            editor_height_px: 200.0,
        };

        assert_eq!(
            split_pane_resized_areas(&state, 300.0, 0.0),
            Some(vec![
                (before_id, HelixRect::new(0, 0, 50, 20)),
                (top_after_id, HelixRect::new(50, 0, 30, 10)),
                (bottom_after_id, HelixRect::new(50, 10, 30, 10)),
            ])
        );
    }
}
