use super::*;

fn panes() -> (PaneId, PaneId) {
    use super::super::pane_layout::{Edge, PaneLayout, SplitMode};

    let mut layout = PaneLayout::new(vec![]);
    let first = layout.active_pane();
    let second = layout
        .split_pane(first, Edge::Right, SplitMode::TargetOnly, vec![])
        .unwrap();
    (first, second)
}

#[test]
fn shared_image_presentations_have_independent_zoom_and_scroll() {
    let (left, right) = panes();
    let image = ImageId(7);
    let other_image = ImageId(8);
    let mut presentations = ImagePresentations::default();
    presentations.ensure(left, image).set_zoom(2.25);
    presentations
        .ensure(left, image)
        .scroll_handle
        .set_offset(gpui::point(px(-47.0), px(-123.0)));
    presentations.ensure(right, image);
    presentations.ensure(left, other_image);

    let right_view = presentations.get_mut(right, image).unwrap();
    assert_eq!(right_view.zoom, 1.0);
    assert_eq!(
        right_view.scroll_handle.offset(),
        gpui::point(px(0.0), px(0.0))
    );
    right_view.set_zoom(0.75);
    right_view
        .vertical_scrollbar_state
        .scroll_handle()
        .set_offset(gpui::point(px(-13.0), px(-29.0)));
    assert_eq!(
        right_view.scroll_handle.offset(),
        gpui::point(px(-13.0), px(-29.0))
    );
    assert_eq!(
        right_view
            .horizontal_scrollbar_state
            .scroll_handle()
            .offset(),
        gpui::point(px(-13.0), px(-29.0))
    );

    // Reopening/ensuring an existing placement must not reset it.
    let left_view = presentations.ensure(left, image);
    assert_eq!(left_view.zoom, 2.25);
    assert_eq!(
        left_view.scroll_handle.offset(),
        gpui::point(px(-47.0), px(-123.0))
    );
    assert_eq!(presentations.get(right, image).unwrap().zoom, 0.75);
    assert_eq!(presentations.get(left, other_image).unwrap().zoom, 1.0);
    assert_eq!(
        presentations
            .get(left, other_image)
            .unwrap()
            .scroll_handle
            .offset(),
        gpui::point(px(0.0), px(0.0))
    );
}

#[test]
fn closing_a_placement_preserves_the_other_and_cached_bytes() {
    let (left, right) = panes();
    let directory = tempfile::tempdir().unwrap();
    let render_path = directory.path().join("cached-image.png");
    std::fs::write(&render_path, b"shared cached bytes").unwrap();
    let resource = ImageTab {
        id: ImageId(11),
        path: PathBuf::from("ssh://devbox/assets/image.png"),
        render_path,
        dimensions: Some((640, 480)),
        focused_at: std::time::Instant::now(),
    };
    let mut presentations = ImagePresentations::default();
    presentations.ensure(left, resource.id);
    presentations.ensure(right, resource.id).set_zoom(1.75);
    presentations
        .ensure(right, resource.id)
        .scroll_handle
        .set_offset(gpui::point(px(-31.0), px(-67.0)));

    presentations.remove(left, resource.id);
    assert!(presentations.get(left, resource.id).is_none());
    let survivor = presentations.get(right, resource.id).unwrap();
    assert_eq!(survivor.zoom, 1.75);
    assert_eq!(
        survivor.scroll_handle.offset(),
        gpui::point(px(-31.0), px(-67.0))
    );
    assert_eq!(
        std::fs::read(&resource.render_path).unwrap(),
        b"shared cached bytes"
    );

    // Placement teardown is not resource or cache eviction, even at zero mounts.
    presentations.remove(right, resource.id);
    assert_eq!(resource.dimensions, Some((640, 480)));
    assert!(resource.render_path.exists());
    assert_eq!(presentations.ensure(left, resource.id).zoom, 1.0);
}

#[test]
fn moving_a_presentation_retains_state_and_existing_destination_wins() {
    let (source, target) = panes();
    let image = ImageId(5);
    let mut presentations = ImagePresentations::default();
    let source_view = presentations.ensure(source, image);
    source_view.set_zoom(3.5);
    source_view
        .scroll_handle
        .set_offset(gpui::point(px(-71.0), px(-19.0)));
    presentations.transfer(source, target, image);

    assert!(presentations.get(source, image).is_none());
    let target_view = presentations.get(target, image).unwrap();
    assert_eq!(target_view.zoom, 3.5);
    assert_eq!(
        target_view.scroll_handle.offset(),
        gpui::point(px(-71.0), px(-19.0))
    );

    presentations.transfer(target, target, image);
    assert_eq!(presentations.get(target, image).unwrap().zoom, 3.5);
    presentations.ensure(source, image).set_zoom(0.5);
    presentations.transfer(source, target, image);
    assert!(presentations.get(source, image).is_none());
    assert_eq!(presentations.get(target, image).unwrap().zoom, 3.5);
    assert_eq!(
        presentations
            .get(target, image)
            .unwrap()
            .scroll_handle
            .offset(),
        gpui::point(px(-71.0), px(-19.0))
    );
}

#[test]
fn image_zoom_preserves_limits_and_percentage_labels() {
    let mut presentation = ImagePresentation::default();
    for (requested, expected, label) in [
        (0.09, 0.10, "10%"),
        (0.10, 0.10, "10%"),
        (0.11, 0.11, "11%"),
        (1.0, 1.0, "100%"),
        (7.99, 7.99, "799%"),
        (8.0, 8.0, "800%"),
        (8.01, 8.0, "800%"),
    ] {
        presentation.set_zoom(requested);
        assert_eq!(presentation.zoom, expected);
        assert_eq!(image_zoom_percent(presentation.zoom), label);
    }
}

#[test]
fn image_cache_reads_binary_bytes_and_dimensions_through_the_backend() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("binary image.png");
    image::RgbaImage::from_pixel(19, 7, image::Rgba([0, 127, 255, 0]))
        .save(&path)
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    // The cache reader uses the same backend contract for local and remote IO.
    let cached = futures_executor::block_on(cache_remote_image_file(
        nucleotide_workspace::local_workspace_backend(),
        path.clone(),
    ))
    .unwrap();
    let cached = tempfile::TempPath::try_from_path(cached).unwrap();
    assert_ne!(&*cached, path.as_path());
    assert_eq!(std::fs::read(&cached).unwrap(), bytes);
    assert_eq!(image_file_dimensions(&cached), Some((19, 7)));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn image_cache_rejects_truncated_reads_without_replacing_cached_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.png");
    std::fs::write(&path, b"previous image bytes").unwrap();
    let backend = nucleotide_workspace::local_workspace_backend();
    let cached =
        futures_executor::block_on(cache_remote_image_file(backend.clone(), path.clone())).unwrap();
    let cached = tempfile::TempPath::try_from_path(cached).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(128 * 1024 * 1024 + 1)
        .unwrap();

    let error = futures_executor::block_on(cache_remote_image_file(backend, path)).unwrap_err();
    assert!(error.to_string().contains("larger than 128 MiB"));
    assert_eq!(std::fs::read(&cached).unwrap(), b"previous image bytes");
}
