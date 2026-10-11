// ABOUTME: Connected selection contours following Zigonaut's SelectionShape.swift
// ABOUTME: Rounds exterior corners and concave steps without seams between rows

use gpui::{Bounds, Path, PathBuilder, Pixels, Point, point, px};

/// Each selection contributes at most one interval per visual row. Adjacent
/// rows connect only when their horizontal intervals positively overlap.
pub(crate) fn selection_paths(
    rows: &[Bounds<Pixels>],
    cell_width: Pixels,
    line_height: Pixels,
    viewport: Bounds<Pixels>,
) -> Vec<Path<Pixels>> {
    let radius = (line_height * 0.15).min(cell_width * 0.25).max(px(0.0));
    let rows: Vec<_> = rows
        .iter()
        .map(|row| row.intersect(&viewport))
        .filter(|row| row.size.width > px(0.0) && row.size.height > px(0.0))
        .collect();
    let mut paths = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        let mut end = start + 1;
        while end < rows.len() {
            let previous = rows[end - 1];
            let next = rows[end];
            if (previous.bottom() - next.top()).abs() > px(0.01)
                || previous.left().max(next.left()) >= previous.right().min(next.right())
            {
                break;
            }
            end += 1;
        }
        let component = &rows[start..end];
        let first = component[0];
        let last = component[component.len() - 1];
        let mut contour = vec![first.origin, point(first.right(), first.top())];
        for pair in component.windows(2) {
            if pair[0].right() != pair[1].right() {
                contour.push(point(pair[0].right(), pair[1].top()));
                contour.push(point(pair[1].right(), pair[1].top()));
            }
        }
        contour.push(point(last.right(), last.bottom()));
        contour.push(point(last.left(), last.bottom()));
        for pair in component.windows(2).rev() {
            if pair[0].left() != pair[1].left() {
                contour.push(point(pair[1].left(), pair[1].top()));
                contour.push(point(pair[0].left(), pair[1].top()));
            }
        }
        let mut builder = PathBuilder::fill();
        for i in 0..contour.len() {
            let corner = contour[i];
            let inset = |neighbor: Point<Pixels>| {
                let delta = neighbor - corner;
                let distance = delta.x.abs() + delta.y.abs();
                let r = if corner.y == viewport.top() || corner.y == viewport.bottom() {
                    px(0.0)
                } else {
                    radius.min(distance * 0.5)
                };
                corner + delta * (f32::from(r) / f32::from(distance))
            };
            let before = inset(contour[(i + contour.len() - 1) % contour.len()]);
            let after = inset(contour[(i + 1) % contour.len()]);
            if i == 0 {
                builder.move_to(before);
            } else {
                builder.line_to(before);
            }
            builder.curve_to(after, corner);
        }
        builder.close();
        if let Ok(path) = builder.build() {
            paths.push(path);
        }
        start = end;
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    fn paths(rows: &[Bounds<Pixels>]) -> Vec<Path<Pixels>> {
        selection_paths(rows, px(8.0), px(20.0), rect(0.0, 0.0, 200.0, 200.0))
    }

    fn covers(paths: &[Path<Pixels>], p: (f32, f32)) -> bool {
        paths.iter().any(|path| {
            path.vertices.chunks_exact(3).any(|triangle| {
                let cross = |a: usize, b: usize| {
                    let a = triangle[a].xy_position;
                    let b = triangle[b].xy_position;
                    f32::from(b.x - a.x) * (p.1 - f32::from(a.y))
                        - f32::from(b.y - a.y) * (p.0 - f32::from(a.x))
                };
                let signs = [cross(0, 1), cross(1, 2), cross(2, 0)];
                signs.iter().all(|s| *s >= 0.0) || signs.iter().all(|s| *s <= 0.0)
            })
        })
    }

    #[test]
    fn joins_only_adjacent_positive_overlaps() {
        let joined = paths(&[rect(10.0, 20.0, 30.0, 20.0), rect(20.0, 40.0, 30.0, 20.0)]);
        assert_eq!(joined.len(), 1);
        assert!(covers(&joined, (25.0, 40.0)));
        for second in [
            rect(40.0, 40.0, 20.0, 20.0),
            rect(50.0, 40.0, 20.0, 20.0),
            rect(10.0, 60.0, 30.0, 20.0),
        ] {
            assert_eq!(paths(&[rect(10.0, 20.0, 30.0, 20.0), second]).len(), 2);
        }
    }

    #[test]
    fn concave_steps_and_narrow_endpoints() {
        let shape = paths(&[rect(10.0, 20.0, 30.0, 20.0), rect(20.0, 40.0, 21.0, 20.0)]);
        assert!(covers(&shape, (19.75, 40.25)));
        assert!(!covers(&shape, (19.0, 45.0)));
        assert!(covers(&shape, (40.25, 45.0)));
        assert!(!covers(&shape, (10.1, 20.1)));
        let narrow = paths(&[rect(50.0, 20.0, 1.0, 20.0)]);
        assert!(covers(&narrow, (50.5, 30.0)));
        assert!(!covers(&narrow, (49.9, 30.0)));
    }

    #[test]
    fn clipped_ends_are_square_and_fractional_rows_connect() {
        let shape = selection_paths(
            &[rect(10.0, -10.0, 30.0, 220.0)],
            px(8.0),
            px(20.0),
            rect(0.0, 0.0, 200.0, 200.0),
        );
        assert!(covers(&shape, (10.1, 0.1)));
        assert!(covers(&shape, (39.9, 199.9)));
        assert_eq!(shape[0].bounds, rect(10.0, 0.0, 30.0, 200.0));
        assert!(paths(&[]).is_empty());
        assert_eq!(
            paths(&[
                rect(10.0, 20.0, 30.0, 20.3),
                rect(10.0, 40.300003, 30.0, 20.3)
            ])
            .len(),
            1
        );
    }
}
