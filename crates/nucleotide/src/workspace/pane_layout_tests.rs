use super::*;

fn image(id: u64) -> TabId {
    TabId::Image(crate::workspace::ImageId(id))
}
fn document() -> TabId {
    TabId::Document(Default::default())
}
fn leaf(tab: TabId) -> InitialLayout {
    InitialLayout::Pane {
        tabs: vec![tab],
        active: Some(tab),
    }
}
fn branch(axis: Axis, children: Vec<(f32, InitialLayout)>) -> InitialLayout {
    InitialLayout::Split { axis, children }
}
fn bounds() -> Rect {
    Rect {
        x: 13.0,
        y: 29.0,
        width: 1000.0,
        height: 600.0,
    }
}
fn split_id(node: Node) -> SplitId {
    match node {
        Node::Split(id) => id,
        _ => panic!("expected split"),
    }
}
fn rect(layout: &PaneLayout, pane: PaneId) -> Rect {
    layout
        .layout(bounds(), PaneSize::default())
        .unwrap()
        .into_iter()
        .find(|entry| entry.pane == pane)
        .unwrap()
        .rect
}
fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
}
fn all_tabs(layout: &PaneLayout) -> HashSet<TabId> {
    layout
        .panes()
        .iter()
        .flat_map(|id| layout.pane(*id).unwrap().tabs().iter().copied())
        .collect()
}

#[test]
fn import_preserves_asymmetric_mixed_axis_shape_weights_and_active_tabs() {
    let layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (1.0, leaf(document())),
            (
                3.0,
                branch(
                    Axis::Vertical,
                    vec![
                        (2.0, leaf(image(2))),
                        (
                            1.0,
                            branch(
                                Axis::Horizontal,
                                vec![(1.0, leaf(image(3))), (4.0, leaf(image(4)))],
                            ),
                        ),
                    ],
                ),
            ),
        ],
    ))
    .unwrap();
    let panes = layout.main_panes();
    assert_eq!(panes.len(), 4);
    assert_eq!(
        layout.pane(panes[0]).unwrap().active_tab(),
        Some(document())
    );
    assert_eq!(layout.pane(panes[3]).unwrap().active_tab(), Some(image(4)));
    assert_eq!(
        rect(&layout, panes[0]),
        Rect {
            width: 250.0,
            ..bounds()
        }
    );
    assert_eq!(
        rect(&layout, panes[1]),
        Rect {
            x: 263.0,
            y: 29.0,
            width: 750.0,
            height: 400.0
        }
    );
    assert_eq!(
        rect(&layout, panes[2]),
        Rect {
            x: 263.0,
            y: 429.0,
            width: 150.0,
            height: 200.0
        }
    );
    assert_eq!(
        rect(&layout, panes[3]),
        Rect {
            x: 413.0,
            y: 429.0,
            width: 600.0,
            height: 200.0
        }
    );
    let root = layout.split(split_id(layout.root())).unwrap();
    assert_eq!(root.children()[1].weight, 3.0);
    assert_eq!(root.children().len(), 2);
    assert_eq!(layout.active_pane(), panes[0]);
    layout.validate().unwrap();
}

#[test]
fn all_four_edge_splits_only_bisect_the_target() {
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        let mut layout = PaneLayout::from_topology(branch(
            Axis::Horizontal,
            vec![(1.0, leaf(document())), (3.0, leaf(image(2)))],
        ))
        .unwrap();
        let panes = layout.main_panes();
        let root = layout.root();
        let original = rect(&layout, panes[1]);
        let unrelated = rect(&layout, panes[0]);
        let new = layout
            .split_pane(panes[1], edge, SplitMode::TargetOnly, vec![image(3)])
            .unwrap();
        assert_eq!(layout.root(), root);
        assert_eq!(layout.split(split_id(root)).unwrap().children().len(), 2);
        assert_eq!(rect(&layout, panes[0]), unrelated);
        let old = rect(&layout, panes[1]);
        let new_rect = rect(&layout, new);
        match edge {
            Edge::Left => {
                near(new_rect.x, original.x);
                near(old.x, original.x + original.width / 2.0);
            }
            Edge::Right => {
                near(old.x, original.x);
                near(new_rect.x, original.x + original.width / 2.0);
            }
            Edge::Top => {
                near(new_rect.y, original.y);
                near(old.y, original.y + original.height / 2.0);
            }
            Edge::Bottom => {
                near(old.y, original.y);
                near(new_rect.y, original.y + original.height / 2.0);
            }
        }
        if edge.axis() == Axis::Horizontal {
            near(old.width, 375.0);
            near(new_rect.width, 375.0);
            near(old.height, 600.0);
            near(new_rect.height, 600.0);
        } else {
            near(old.height, 300.0);
            near(new_rect.height, 300.0);
            near(old.width, 750.0);
            near(new_rect.width, 750.0);
        }
        layout.validate().unwrap();
    }
}

#[test]
fn sibling_splits_reuse_identity_and_equalize_matching_container() {
    let mut layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![(1.0, leaf(document())), (4.0, leaf(image(2)))],
    ))
    .unwrap();
    let panes = layout.main_panes();
    let root = layout.root();
    let third = layout
        .split_pane(panes[1], Edge::Left, SplitMode::Sibling, vec![image(3)])
        .unwrap();
    assert_eq!(layout.main_panes(), vec![panes[0], third, panes[1]]);
    let fourth = layout
        .split_pane(third, Edge::Right, SplitMode::Sibling, vec![image(4)])
        .unwrap();
    assert_eq!(layout.root(), root);
    assert_eq!(layout.main_panes(), vec![panes[0], third, fourth, panes[1]]);
    for pane in layout.main_panes() {
        near(rect(&layout, pane).width, 250.0);
    }
    assert!(
        layout
            .split(split_id(root))
            .unwrap()
            .children()
            .iter()
            .all(|child| child.weight == 1.0)
    );
    layout.validate().unwrap();
}

#[test]
fn tabs_have_local_dedup_explicit_order_and_selectable_removal_successor() {
    let mut layout = PaneLayout::new(vec![document(), image(2), image(3), image(2)]);
    let pane = layout.active_pane();
    assert_eq!(
        layout.pane(pane).unwrap().tabs(),
        &[document(), image(2), image(3)]
    );
    layout.activate_tab(pane, image(3)).unwrap();
    layout.activate_tab(pane, document()).unwrap();
    layout.reorder_tab(pane, image(3), 0).unwrap();
    layout.open_tab(pane, document(), true).unwrap();
    assert_eq!(
        layout.pane(pane).unwrap().tabs(),
        &[image(3), document(), image(2)]
    );
    let mut adjacent = layout.clone();
    layout.close_tab(pane, document()).unwrap();
    adjacent
        .close_tab_with_policy(pane, document(), TabSuccessor::Adjacent)
        .unwrap();
    assert_eq!(layout.pane(pane).unwrap().active_tab(), Some(image(3)));
    assert_eq!(adjacent.pane(pane).unwrap().active_tab(), Some(image(2)));
    adjacent
        .close_tab_with_policy(pane, image(2), TabSuccessor::Adjacent)
        .unwrap();
    assert_eq!(adjacent.pane(pane).unwrap().active_tab(), Some(image(3)));
    adjacent.close_tab(pane, image(3)).unwrap();
    assert_eq!(adjacent.pane(pane).unwrap().active_tab(), None);
    assert_eq!(adjacent.root(), Node::Pane(pane));
    layout.validate().unwrap();
    adjacent.validate().unwrap();
}

#[test]
fn transfer_dedups_target_preserves_unrelated_placements_and_collapses_final_source() {
    let mut layout = PaneLayout::new(vec![document(), image(2)]);
    let first = layout.active_pane();
    let second = layout
        .split_pane(
            first,
            Edge::Right,
            SplitMode::Sibling,
            vec![image(3), image(2)],
        )
        .unwrap();
    let third = layout
        .split_pane(second, Edge::Bottom, SplitMode::TargetOnly, vec![image(2)])
        .unwrap();
    layout.transfer_tab(first, image(2), second, 0).unwrap();
    assert_eq!(layout.pane(second).unwrap().tabs(), &[image(3), image(2)]);
    assert_eq!(layout.pane(third).unwrap().tabs(), &[image(2)]);
    assert_eq!(layout.pane(first).unwrap().tabs(), &[document()]);
    layout.transfer_tab(first, document(), second, 1).unwrap();
    assert!(layout.pane(first).is_none());
    assert_eq!(
        layout.pane(second).unwrap().tabs(),
        &[image(3), document(), image(2)]
    );
    assert_eq!(layout.active_pane(), second);
    assert_eq!(layout.pane(second).unwrap().active_tab(), Some(document()));
    assert_eq!(layout.main_panes(), vec![second, third]);
    layout
        .transfer_tab(second, document(), second, usize::MAX)
        .unwrap();
    assert_eq!(
        layout.pane(second).unwrap().tabs(),
        &[image(3), image(2), document()]
    );
    layout.validate().unwrap();
}

#[test]
fn edge_drop_of_own_final_tab_is_identity_preserving_noop_in_main_nested_and_dock() {
    let main = PaneLayout::new(vec![document()]);
    let nested = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (1.0, leaf(document())),
            (
                3.0,
                branch(
                    Axis::Vertical,
                    vec![(2.0, leaf(image(2))), (1.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let mut docked = PaneLayout::new(vec![document()]);
    let dock = docked.ensure_dock(140.0).unwrap();
    docked.open_tab(dock, image(4), false).unwrap();
    docked.set_dock_visible(true);
    for (mut layout, index) in [(main, 0), (nested, 2), (docked, 1)] {
        let source = layout.panes()[index];
        let tab = layout.pane(source).unwrap().active_tab().unwrap();
        let before = layout.clone();
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            assert_eq!(layout.split_tab(source, tab, source, edge).unwrap(), source);
            assert_eq!(layout.root(), before.root());
            assert_eq!(layout.dock().map(Dock::root), before.dock().map(Dock::root));
            assert_eq!(layout.panes(), before.panes());
            assert_eq!(layout.pane_history(), before.pane_history());
            assert_eq!(
                layout.layout(bounds(), PaneSize::default()).unwrap(),
                before.layout(bounds(), PaneSize::default()).unwrap()
            );
            for pane in layout.panes() {
                assert_eq!(
                    layout.pane(pane).unwrap().history(),
                    before.pane(pane).unwrap().history()
                );
                assert_eq!(
                    layout.pane(pane).unwrap().tabs(),
                    before.pane(pane).unwrap().tabs()
                );
            }
        }
        layout.validate().unwrap();
    }
}

#[test]
fn close_and_keep_only_recover_orphan_tabs_in_stable_order_without_touching_dock() {
    let mut layout = PaneLayout::new(vec![document(), image(2)]);
    let first = layout.active_pane();
    let second = layout
        .split_pane(
            first,
            Edge::Right,
            SplitMode::Sibling,
            vec![image(3), image(2)],
        )
        .unwrap();
    let third = layout
        .split_pane(second, Edge::Top, SplitMode::TargetOnly, vec![image(4)])
        .unwrap();
    let dock = layout.ensure_dock(180.0).unwrap();
    layout.open_tab(dock, image(5), false).unwrap();
    assert_eq!(layout.close_pane(first, None).unwrap(), third);
    assert_eq!(
        layout.pane(third).unwrap().tabs(),
        &[image(4), document(), image(2)]
    );
    layout.keep_only(second).unwrap();
    assert_eq!(layout.main_panes(), vec![second]);
    assert_eq!(
        layout.pane(second).unwrap().tabs(),
        &[image(3), image(2), image(4), document()]
    );
    assert_eq!(layout.pane(second).unwrap().active_tab(), Some(image(3)));
    assert_eq!(layout.pane(dock).unwrap().tabs(), &[image(5)]);
    assert_eq!(layout.close_pane(second, None), Err(LayoutError::LastPane));
    layout
        .assign_unplaced_tabs(second, [image(5), image(6), image(6), document()])
        .unwrap();
    assert_eq!(layout.pane(second).unwrap().tabs().last(), Some(&image(6)));
    assert!(!layout.pane(second).unwrap().tabs().contains(&image(5)));
    layout.validate().unwrap();
}

#[test]
fn stale_pane_and_split_keys_never_alias_new_allocations() {
    let mut layout = PaneLayout::new(vec![document()]);
    let first = layout.active_pane();
    let removed = layout
        .split_pane(first, Edge::Right, SplitMode::TargetOnly, vec![image(2)])
        .unwrap();
    let removed_split = split_id(layout.root());
    layout.close_tab(removed, image(2)).unwrap();
    let added = layout
        .split_pane(first, Edge::Right, SplitMode::TargetOnly, vec![image(3)])
        .unwrap();
    let added_split = split_id(layout.root());
    assert_ne!(removed.key(), added.key());
    assert_ne!(removed_split.key(), added_split.key());
    assert!(layout.pane(removed).is_none());
    assert!(layout.split(removed_split).is_none());
    assert_eq!(
        layout.open_tab(removed, image(4), true),
        Err(LayoutError::MissingPane)
    );
    layout.validate().unwrap();
}

#[test]
fn hidden_dock_retains_identity_tabs_topology_and_remembered_size() {
    let mut layout = PaneLayout::new(vec![document()]);
    let main = layout.active_pane();
    let dock = layout.ensure_dock(170.0).unwrap();
    layout.open_tab(dock, image(2), false).unwrap();
    assert_eq!(
        layout.layout(bounds(), PaneSize::default()).unwrap().len(),
        1
    );
    assert_eq!(layout.activate_pane(dock), Err(LayoutError::HiddenPane));
    layout.set_dock_visible(true);
    let other = layout
        .split_pane(dock, Edge::Right, SplitMode::Sibling, vec![image(3)])
        .unwrap();
    let root = layout.dock().unwrap().root();
    layout.set_dock_size(210.0).unwrap();
    layout.activate_pane(other).unwrap();
    let geometry = layout.layout(bounds(), PaneSize::default()).unwrap();
    assert_eq!(geometry.len(), 3);
    near(rect(&layout, main).height, 390.0);
    near(rect(&layout, dock).y, 419.0);
    near(rect(&layout, dock).height, 210.0);
    layout.set_dock_visible(false);
    assert_eq!(layout.active_pane(), main);
    assert_eq!(layout.dock().unwrap().root(), root);
    assert_eq!(layout.ensure_dock(999.0).unwrap(), dock);
    assert_eq!(layout.dock().unwrap().size(), 210.0);
    layout.set_dock_visible(true);
    assert_eq!(
        layout.layout(bounds(), PaneSize::default()).unwrap(),
        geometry
    );
    assert_eq!(layout.pane(other).unwrap().active_tab(), Some(image(3)));
    // Closing the last dock tab keeps a reusable empty dock root.
    layout.close_pane(other, Some(dock)).unwrap();
    layout.close_tab(dock, image(3)).unwrap();
    layout.close_tab(dock, image(2)).unwrap();
    assert_eq!(layout.dock().unwrap().root(), Node::Pane(dock));
    layout.validate().unwrap();
}

#[test]
fn directional_ties_use_traversal_order_and_swap_transpose_retain_identity() {
    let mut layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (1.0, leaf(document())),
            (
                2.0,
                branch(
                    Axis::Vertical,
                    vec![(1.0, leaf(image(2))), (1.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let panes = layout.main_panes();
    let b = bounds();
    let m = PaneSize::default();
    assert_eq!(
        layout.neighbor(panes[0], Edge::Right, b, m).unwrap(),
        Some(panes[1])
    );
    assert_eq!(
        layout.neighbor(panes[1], Edge::Bottom, b, m).unwrap(),
        Some(panes[2])
    );
    assert_eq!(
        layout.neighbor(panes[2], Edge::Left, b, m).unwrap(),
        Some(panes[0])
    );
    assert_eq!(layout.neighbor(panes[0], Edge::Left, b, m).unwrap(), None);
    let first_rect = rect(&layout, panes[0]);
    let second_rect = rect(&layout, panes[1]);
    layout.activate_pane(panes[0]).unwrap();
    layout.swap_direction(panes[0], Edge::Right, b, m).unwrap();
    assert_eq!(rect(&layout, panes[0]), second_rect);
    assert_eq!(rect(&layout, panes[1]), first_rect);
    assert_eq!(layout.active_pane(), panes[0]);
    assert_eq!(layout.pane(panes[0]).unwrap().tabs(), &[document()]);
    let root = layout.root();
    let before = layout.layout(b, m).unwrap();
    layout.transpose(root).unwrap();
    assert_eq!(layout.split(split_id(root)).unwrap().axis(), Axis::Vertical);
    let after = rect(&layout, panes[1]);
    near(after.height, 200.0);
    near(after.width, 1000.0);
    layout.transpose(root).unwrap();
    assert_eq!(layout.layout(b, m).unwrap(), before);
    layout.validate().unwrap();
}

#[test]
fn swap_between_main_and_dock_roots_does_not_duplicate_or_orphan_nodes() {
    let mut layout = PaneLayout::new(vec![document()]);
    let main = layout.active_pane();
    let dock = layout.ensure_dock(200.0).unwrap();
    layout.open_tab(dock, image(2), false).unwrap();
    layout.swap_panes(main, dock).unwrap();
    assert_eq!(layout.root(), Node::Pane(dock));
    assert_eq!(layout.dock().unwrap().root(), Node::Pane(main));
    assert_eq!(layout.active_pane(), dock);
    layout.validate().unwrap();
}

#[test]
fn resize_clamps_both_directions_and_preserves_third_sibling() {
    let mut layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (2.0, leaf(document())),
            (3.0, leaf(image(2))),
            (5.0, leaf(image(3))),
        ],
    ))
    .unwrap();
    let root = split_id(layout.root());
    let panes = layout.main_panes();
    let m = PaneSize {
        width: 80.0,
        height: 40.0,
    };
    near(
        layout.resize_divider(root, 0, 900.0, 1000.0, m).unwrap(),
        220.0,
    );
    let geometry = layout.layout(bounds(), m).unwrap();
    near(geometry[0].rect.width, 420.0);
    near(geometry[1].rect.width, 80.0);
    near(rect(&layout, panes[2]).width, 500.0);
    near(
        layout.resize_divider(root, 0, -900.0, 1000.0, m).unwrap(),
        -340.0,
    );
    let geometry = layout.layout(bounds(), m).unwrap();
    near(geometry[0].rect.width, 80.0);
    near(geometry[1].rect.width, 420.0);
    near(geometry[2].rect.width, 500.0);
    let before = layout.split(root).unwrap().children().to_vec();
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            layout.set_weights(root, &[1.0, bad, 1.0]),
            Err(LayoutError::InvalidWeight)
        );
        assert_eq!(layout.split(root).unwrap().children(), &before);
    }
    assert_eq!(
        layout.resize_divider(root, 0, f32::NAN, 1000.0, m),
        Err(LayoutError::InvalidGeometry)
    );
    layout.validate().unwrap();
}

#[test]
fn subtree_minima_shrink_proportionally_and_geometry_remains_finite() {
    let mut layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (100.0, leaf(document())),
            (
                1.0,
                branch(
                    Axis::Horizontal,
                    vec![(1.0, leaf(image(2))), (1.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let minimum = PaneSize {
        width: 100.0,
        height: 80.0,
    };
    let geometry = layout.layout(bounds(), minimum).unwrap();
    near(geometry[0].rect.width, 800.0);
    near(geometry[1].rect.width, 100.0);
    near(geometry[2].rect.width, 100.0);
    let small = layout
        .layout(
            Rect {
                width: 150.0,
                height: 30.0,
                ..bounds()
            },
            minimum,
        )
        .unwrap();
    for entry in small {
        near(entry.rect.width, 50.0);
        near(entry.rect.height, 30.0);
    }
    let root = split_id(layout.root());
    layout.set_weights(root, &[1.0, 3.0]).unwrap();
    near(
        layout
            .resize_divider(root, 0, 1000.0, 1000.0, minimum)
            .unwrap(),
        550.0,
    );
    let geometry = layout.layout(bounds(), minimum).unwrap();
    near(geometry[0].rect.width, 800.0);
    near(geometry[1].rect.width, 100.0);
    near(geometry[2].rect.width, 100.0);
    near(
        layout
            .resize_divider(root, 0, 1000.0, 150.0, minimum)
            .unwrap(),
        0.0,
    );
    layout
        .set_weights(root, &[f32::from_bits(1), f32::MAX])
        .unwrap();
    for width in [0.0, f32::from_bits(1), 0.125, 1000.0] {
        layout
            .resize_divider(
                root,
                0,
                1.0,
                width.max(f32::MIN_POSITIVE),
                PaneSize::default(),
            )
            .unwrap();
        layout.validate().unwrap();
        for entry in layout
            .layout(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height: 7.0,
                },
                PaneSize::default(),
            )
            .unwrap()
        {
            assert!(entry.rect.width.is_finite() && entry.rect.width >= 0.0);
        }
    }
    let mut extreme = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (f32::from_bits(1), leaf(document())),
            (f32::from_bits(1), leaf(image(2))),
            (f32::MAX, leaf(image(3))),
        ],
    ))
    .unwrap();
    let root = split_id(extreme.root());
    extreme
        .resize_divider(root, 0, 1.0, 1000.0, PaneSize::default())
        .unwrap();
    extreme.validate().unwrap();
}

#[test]
fn invalid_commands_are_atomic() {
    let mut layout = PaneLayout::new(vec![document(), image(2)]);
    let main = layout.active_pane();
    let dock = layout.ensure_dock(100.0).unwrap();
    let before = format!("{layout:?}");
    assert_eq!(
        layout.transfer_tab(main, document(), dock, 0),
        Err(LayoutError::HiddenPane)
    );
    assert_eq!(
        layout.split_tab(main, image(99), main, Edge::Left),
        Err(LayoutError::MissingTab)
    );
    assert_eq!(
        layout.open_tab(dock, image(2), true),
        Err(LayoutError::HiddenPane)
    );
    assert_eq!(
        layout.close_pane(main, Some(main)),
        Err(LayoutError::SamePane)
    );
    assert_eq!(format!("{layout:?}"), before);
    assert!(PaneLayout::from_topology(branch(Axis::Vertical, vec![])).is_err());
    assert!(
        PaneLayout::from_topology(InitialLayout::Pane {
            tabs: vec![],
            active: Some(image(3))
        })
        .is_err()
    );
    layout.validate().unwrap();
}

#[test]
fn saturated_nested_resize_is_pair_local_on_both_axes_and_uses_local_extent() {
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let mut layout = PaneLayout::from_topology(branch(
            axis,
            vec![
                (1.0, leaf(document())),
                (
                    1.0,
                    branch(axis, vec![(1.0, leaf(image(2))), (1.0, leaf(image(3)))]),
                ),
                (1.0, leaf(image(4))),
            ],
        ))
        .unwrap();
        let root = split_id(layout.root());
        let nested = split_id(layout.split(root).unwrap().children()[1].node);
        let bounds = Rect {
            x: 13.0,
            y: 29.0,
            width: 600.0,
            height: 600.0,
        };
        let minimum = match axis {
            Axis::Horizontal => PaneSize {
                width: 100.0,
                height: 70.0,
            },
            Axis::Vertical => PaneSize {
                width: 70.0,
                height: 100.0,
            },
        };
        let extent = |rect: Rect| {
            if axis == Axis::Horizontal {
                rect.width
            } else {
                rect.height
            }
        };
        let before = layout.layout(bounds, minimum).unwrap();
        near(
            layout
                .resize_divider(root, 0, 100.0, 600.0, minimum)
                .unwrap(),
            0.0,
        );
        let after = layout.layout(bounds, minimum).unwrap();
        for (a, b) in after.iter().zip(&before) {
            near(extent(a.rect), extent(b.rect));
        }
        near(
            layout
                .resize_divider(root, 0, -80.0, 600.0, minimum)
                .unwrap(),
            -80.0,
        );
        let after = layout.layout(bounds, minimum).unwrap();
        for (entry, expected) in after.iter().zip([120.0, 140.0, 140.0, 200.0]) {
            near(extent(entry.rect), expected);
        }
        near(
            layout
                .resize_divider(nested, 0, 100.0, 280.0, minimum)
                .unwrap(),
            40.0,
        );
        let after = layout.layout(bounds, minimum).unwrap();
        for (entry, expected) in after.iter().zip([120.0, 180.0, 100.0, 200.0]) {
            near(extent(entry.rect), expected);
        }
        layout.validate().unwrap();
    }
}

#[test]
fn focused_transpose_only_toggles_immediate_parent() {
    let mut layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (1.0, leaf(document())),
            (
                3.0,
                branch(
                    Axis::Vertical,
                    vec![(1.0, leaf(image(2))), (4.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let panes = layout.main_panes();
    let root = split_id(layout.root());
    let nested = split_id(layout.split(root).unwrap().children()[1].node);
    let original_children = layout.split(root).unwrap().children().to_vec();
    let nested_children = layout.split(nested).unwrap().children().to_vec();
    layout.transpose(Node::Pane(panes[0])).unwrap();
    assert_eq!(layout.split(root).unwrap().axis(), Axis::Vertical);
    assert_eq!(layout.split(nested).unwrap().axis(), Axis::Vertical);
    layout.transpose(Node::Pane(panes[2])).unwrap();
    assert_eq!(layout.split(root).unwrap().axis(), Axis::Vertical);
    assert_eq!(layout.split(nested).unwrap().axis(), Axis::Horizontal);
    assert_eq!(layout.split(root).unwrap().children(), original_children);
    assert_eq!(layout.split(nested).unwrap().children(), nested_children);
    layout.validate().unwrap();
}

#[test]
fn navigation_uses_top_left_not_center_for_unequal_subtrees_and_stable_ties() {
    let layout = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (1.0, leaf(document())),
            (
                1.0,
                branch(
                    Axis::Vertical,
                    vec![(1.0, leaf(image(2))), (4.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let panes = layout.main_panes();
    assert_eq!(
        layout
            .neighbor(panes[0], Edge::Right, bounds(), PaneSize::default())
            .unwrap(),
        Some(panes[1])
    );
    let mut tie = PaneLayout::from_topology(branch(
        Axis::Horizontal,
        vec![
            (
                1.0,
                branch(
                    Axis::Vertical,
                    vec![(1.0, leaf(document())), (3.0, leaf(image(4)))],
                ),
            ),
            (
                1.0,
                branch(
                    Axis::Vertical,
                    vec![(1.0, leaf(image(2))), (1.0, leaf(image(3)))],
                ),
            ),
        ],
    ))
    .unwrap();
    let panes = tie.main_panes();
    // Source top=150; candidates top=0 and 300: exact tie chooses first child.
    assert_eq!(
        tie.neighbor(panes[1], Edge::Right, bounds(), PaneSize::default())
            .unwrap(),
        Some(panes[2])
    );
    let dock = tie.ensure_dock(150.0).unwrap();
    tie.set_dock_visible(true);
    assert_eq!(
        tie.neighbor(panes[1], Edge::Bottom, bounds(), PaneSize::default())
            .unwrap(),
        Some(dock)
    );
    tie.set_dock_visible(false);
    assert_eq!(
        tie.neighbor(panes[1], Edge::Bottom, bounds(), PaneSize::default())
            .unwrap(),
        None
    );
}

#[test]
fn left_adjacent_close_selects_previous_then_falls_back_right_without_affecting_inactive_close() {
    let mut layout = PaneLayout::new(vec![document(), image(2), image(3)]);
    let pane = layout.active_pane();
    layout.activate_tab(pane, image(2)).unwrap();
    layout
        .close_tab_with_policy(pane, image(2), TabSuccessor::LeftAdjacent)
        .unwrap();
    assert_eq!(layout.pane(pane).unwrap().active_tab(), Some(document()));
    layout
        .close_tab_with_policy(pane, document(), TabSuccessor::LeftAdjacent)
        .unwrap();
    assert_eq!(layout.pane(pane).unwrap().active_tab(), Some(image(3)));
    layout.open_tab(pane, image(4), false).unwrap();
    layout
        .close_tab_with_policy(pane, image(4), TabSuccessor::LeftAdjacent)
        .unwrap();
    assert_eq!(layout.pane(pane).unwrap().active_tab(), Some(image(3)));
    layout.validate().unwrap();
}

#[test]
fn generated_command_sequences_preserve_ownership_content_and_rectangular_partition() {
    // Deterministic state-machine generation through public commands, not random
    // rejected input. Each seed executes successful topology/content changes.
    for seed in 1..=32u64 {
        let mut random = seed;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            random
        };
        let mut layout = PaneLayout::new(vec![document()]);
        layout.ensure_dock(140.0).unwrap();
        let mut expected = HashSet::from([document()]);
        for step in 0..300 {
            let dock = layout.ensure_dock(140.0).unwrap();
            let main = layout.main_panes();
            let source = main[next() as usize % main.len()];
            let target = main[next() as usize % main.len()];
            let edge = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom][next() as usize % 4];
            let tab = image(1 + next() % 19);
            let minimum = PaneSize {
                width: 30.0 + (next() % 20) as f32,
                height: 20.0 + (next() % 30) as f32,
            };
            match next() % 16 {
                0 | 1 => {
                    layout.open_tab(source, tab, true).unwrap();
                    expected.insert(tab);
                }
                2 if main.len() < 10 => {
                    layout
                        .split_pane(target, edge, SplitMode::Sibling, vec![tab])
                        .unwrap();
                    expected.insert(tab);
                }
                3 if main.len() < 10 => {
                    layout
                        .split_pane(target, edge, SplitMode::TargetOnly, vec![tab])
                        .unwrap();
                    expected.insert(tab);
                }
                4 => {
                    if let Some(tab) = layout.pane(source).unwrap().active_tab() {
                        layout
                            .transfer_tab(source, tab, target, next() as usize % 5)
                            .unwrap();
                    }
                }
                5 if main.len() > 1 => {
                    layout.close_pane(source, None).unwrap();
                }
                6 => {
                    layout.keep_only(source).unwrap();
                }
                7 => {
                    layout.swap_panes(source, target).unwrap();
                }
                8 => {
                    layout.transpose(layout.root()).unwrap();
                }
                9 => {
                    layout.set_dock_visible(next() % 2 == 0);
                    layout.open_tab(dock, tab, false).unwrap();
                    expected.insert(tab);
                }
                10 => {
                    if let Some(tab) = layout.pane(source).unwrap().active_tab() {
                        let count = layout
                            .panes()
                            .iter()
                            .filter(|id| layout.pane(**id).unwrap().tabs().contains(&tab))
                            .count();
                        layout.close_tab(source, tab).unwrap();
                        if count == 1 {
                            expected.remove(&tab);
                        }
                    }
                }
                11 if main.len() < 10 => {
                    if let Some(tab) = layout.pane(source).unwrap().active_tab() {
                        layout.split_tab(source, tab, target, edge).unwrap();
                    }
                }
                12 => {
                    layout.set_dock_visible(true);
                    if let Some(tab) = layout.pane(source).unwrap().active_tab() {
                        layout
                            .transfer_tab(source, tab, dock, next() as usize % 5)
                            .unwrap();
                    }
                }
                13 => {
                    if let Some(tab) = layout.pane(dock).unwrap().active_tab() {
                        layout.transfer_tab(dock, tab, target, 0).unwrap();
                    }
                }
                14 => {
                    layout.set_dock_visible(true);
                    if layout.panes().len() < 12 {
                        layout
                            .split_pane(dock, edge, SplitMode::Sibling, vec![tab])
                            .unwrap();
                        expected.insert(tab);
                    }
                }
                _ => {
                    if let Node::Split(root) = layout.root() {
                        layout
                            .resize_divider(root, 0, (next() % 401) as f32 - 200.0, 1000.0, minimum)
                            .unwrap();
                    }
                }
            }
            layout
                .validate()
                .unwrap_or_else(|error| panic!("seed {seed} step {step}: {error:?}"));
            assert_eq!(all_tabs(&layout), expected, "seed {seed} step {step}");
            let geometry = layout.layout(bounds(), minimum).unwrap();
            let total_area: f64 = geometry
                .iter()
                .map(|entry| entry.rect.width as f64 * entry.rect.height as f64)
                .sum();
            assert!(
                (total_area - 600_000.0).abs() < 1.0,
                "seed {seed} step {step}: {total_area}"
            );
            for (index, entry) in geometry.iter().enumerate() {
                assert!(entry.rect.width >= 0.0 && entry.rect.height >= 0.0);
                for other in &geometry[index + 1..] {
                    let overlap_w = (entry.rect.x + entry.rect.width)
                        .min(other.rect.x + other.rect.width)
                        - entry.rect.x.max(other.rect.x);
                    let overlap_h = (entry.rect.y + entry.rect.height)
                        .min(other.rect.y + other.rect.height)
                        - entry.rect.y.max(other.rect.y);
                    assert!(
                        overlap_w < 0.001 || overlap_h < 0.001,
                        "seed {seed} step {step}: overlapping panes"
                    );
                }
            }
        }
    }
}
