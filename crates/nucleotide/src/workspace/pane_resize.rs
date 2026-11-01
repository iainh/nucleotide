//! Logical-pixel dividers for document, image, and terminal panes alike.
use super::*;
use pane_layout::{Axis, Node, PaneGeometry, SplitId};

pub(super) const MINIMUM: PaneSize = PaneSize {
    width: 80.0,
    height: 60.0,
};

#[derive(Clone, Copy)]
pub(super) struct PaneDivider {
    split: Option<SplitId>,
    before: usize,
    axis: Axis,
    rect: PaneRect,
    extent: f32,
}

pub(super) struct PaneResize {
    divider: PaneDivider,
    last: Point<Pixels>,
}

impl Workspace {
    pub(super) fn render_pane_dividers(
        &self,
        geometry: &[PaneGeometry],
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        fn collect(
            layout: &pane_layout::PaneLayout,
            node: Node,
            geometry: &[PaneGeometry],
            dividers: &mut Vec<PaneDivider>,
        ) -> Option<PaneRect> {
            match node {
                Node::Pane(pane) => geometry
                    .iter()
                    .find(|entry| entry.pane == pane)
                    .map(|entry| entry.rect),
                Node::Split(id) => {
                    let split = layout.split(id)?;
                    let bounds: Vec<_> = split
                        .children()
                        .iter()
                        .filter_map(|child| collect(layout, child.node, geometry, dividers))
                        .collect();
                    let first = *bounds.first()?;
                    let last = *bounds.last()?;
                    let rect = PaneRect {
                        x: first.x,
                        y: first.y,
                        width: last.x + last.width - first.x,
                        height: last.y + last.height - first.y,
                    };
                    for (before, bounds) in bounds.iter().enumerate().take(bounds.len() - 1) {
                        let (divider, extent) = match split.axis() {
                            Axis::Horizontal => (
                                PaneRect {
                                    x: bounds.x + bounds.width,
                                    y: rect.y,
                                    width: 0.0,
                                    height: rect.height,
                                },
                                rect.width,
                            ),
                            Axis::Vertical => (
                                PaneRect {
                                    x: rect.x,
                                    y: bounds.y + bounds.height,
                                    width: rect.width,
                                    height: 0.0,
                                },
                                rect.height,
                            ),
                        };
                        dividers.push(PaneDivider {
                            split: Some(id),
                            before,
                            axis: split.axis(),
                            rect: divider,
                            extent,
                        });
                    }
                    Some(rect)
                }
            }
        }
        let mut dividers = Vec::new();
        let main = collect(
            &self.pane_host.layout,
            self.pane_host.layout.root(),
            geometry,
            &mut dividers,
        );
        if let Some(dock) = self.pane_host.layout.dock().filter(|dock| dock.visible()) {
            if let (Some(main), Some(dock)) = (
                main,
                collect(&self.pane_host.layout, dock.root(), geometry, &mut dividers),
            ) {
                dividers.push(PaneDivider {
                    split: None,
                    before: 0,
                    axis: Axis::Vertical,
                    rect: PaneRect {
                        x: dock.x,
                        y: dock.y,
                        width: dock.width,
                        height: 0.0,
                    },
                    extent: main.height + dock.height,
                });
            }
        }
        dividers
            .into_iter()
            .map(|divider| {
                let horizontal = divider.axis == Axis::Horizontal;
                let hit = SPLIT_PANE_HANDLE_HITBOX_PX;
                nucleotide_ui::resize_handle(
                    SharedString::from(format!(
                        "pane-divider-{:?}-{}",
                        divider.split, divider.before
                    )),
                    if horizontal {
                        nucleotide_ui::SplitterAxis::Vertical
                    } else {
                        nucleotide_ui::SplitterAxis::Horizontal
                    },
                    hit,
                    cx.listener(move |workspace, event: &MouseDownEvent, _, cx| {
                        workspace.pane_resize = Some(PaneResize {
                            divider,
                            last: event.position,
                        });
                        cx.stop_propagation();
                        cx.notify();
                    }),
                    cx.listener(|workspace, _: &MouseUpEvent, window, cx| {
                        workspace.finish_active_resize(window, cx);
                    }),
                    cx.listener(|workspace, _: &MouseUpEvent, window, cx| {
                        workspace.finish_active_resize(window, cx);
                    }),
                )
                .absolute()
                .left(px(divider.rect.x - if horizontal { hit / 2.0 } else { 0.0 }))
                .top(px(divider.rect.y - if horizontal { 0.0 } else { hit / 2.0 }))
                .w(px(if horizontal { hit } else { divider.rect.width }))
                .h(px(if horizontal { divider.rect.height } else { hit }))
                .child(
                    div()
                        .absolute()
                        .left(px(if horizontal { hit / 2.0 } else { 0.0 }))
                        .top(px(if horizontal { 0.0 } else { hit / 2.0 }))
                        .w(px(if horizontal { 1.0 } else { divider.rect.width }))
                        .h(px(if horizontal { divider.rect.height } else { 1.0 }))
                        .bg(cx.theme().tokens.chrome.separator_color),
                )
                .into_any_element()
            })
            .collect()
    }

    pub(super) fn capture_pane_resize(&mut self, mouse: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(state) = self.pane_resize.as_mut() else {
            return;
        };
        let horizontal = state.divider.axis == Axis::Horizontal;
        let delta = if horizontal {
            f32::from(mouse.x - state.last.x)
        } else {
            f32::from(mouse.y - state.last.y)
        };
        state.last = mouse;
        if let Some(split) = state.divider.split {
            let _ = self.pane_host.layout.resize_divider(
                split,
                state.divider.before,
                delta,
                state.divider.extent,
                MINIMUM,
            );
        } else if let Some(dock) = self.pane_host.layout.dock() {
            let size = (dock.size() - delta).clamp(
                MINIMUM.height,
                (state.divider.extent - MINIMUM.height).max(MINIMUM.height),
            );
            let _ = self.pane_host.layout.set_dock_size(size);
        }
        cx.notify();
    }
}
