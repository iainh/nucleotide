// ABOUTME: Breadcrumb bar showing a location as a chain of labelled, iconned segments
// ABOUTME: Used above the editor for the project path and the symbols around the cursor

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::{Theme, icon};

type SegmentClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One step of a breadcrumb trail, such as a folder, a file or a symbol.
pub struct BreadcrumbSegment {
    label: SharedString,
    icon: Option<AnyElement>,
    on_click: Option<SegmentClickHandler>,
}

impl BreadcrumbSegment {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

/// A single-row trail of segments separated by chevrons. The last segment is
/// the current location and reads strongest; trailing segments are clipped
/// when the row is too narrow.
#[derive(IntoElement)]
pub struct Breadcrumbs {
    id: ElementId,
    segments: Vec<BreadcrumbSegment>,
}

impl Breadcrumbs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
        }
    }

    pub fn segment(mut self, segment: BreadcrumbSegment) -> Self {
        self.segments.push(segment);
        self
    }

    pub fn segments(mut self, segments: impl IntoIterator<Item = BreadcrumbSegment>) -> Self {
        self.segments.extend(segments);
        self
    }
}

impl RenderOnce for Breadcrumbs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.global::<Theme>().tokens;
        let last = self.segments.len().saturating_sub(1);
        let separator_color = tokens.editor.text_secondary.opacity(0.6);
        let id = self.id.clone();

        div()
            .id(self.id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .w_full()
            .min_w(px(0.0))
            .h(tokens.sizes.space_7)
            .px(tokens.sizes.space_3)
            .gap(tokens.sizes.space_1)
            .overflow_hidden()
            .whitespace_nowrap()
            .bg(tokens.editor.background)
            .border_b_1()
            .border_color(tokens.chrome.border_muted)
            .text_size(tokens.sizes.text_sm)
            .text_color(tokens.editor.text_secondary)
            .children(
                self.segments
                    .into_iter()
                    .enumerate()
                    .map(|(index, segment)| {
                        let is_current = index == last;
                        let separator = (index > 0).then(|| {
                            icon()
                                .path("icons/chevron-right.svg")
                                .size(px(12.0))
                                .flex_shrink_0()
                                .text_color(separator_color)
                        });
                        let crumb = div()
                            .id(ElementId::NamedInteger(
                                SharedString::from(format!("{id:?}-segment")),
                                index as u64,
                            ))
                            .flex()
                            .flex_row()
                            .flex_shrink_0()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .px(tokens.sizes.space_1)
                            .rounded(tokens.sizes.radius_sm)
                            .when(is_current, |crumb| {
                                crumb.text_color(tokens.editor.text_primary)
                            })
                            .children(segment.icon)
                            .child(segment.label)
                            .when_some(segment.on_click, |crumb, on_click| {
                                crumb
                                    .cursor_pointer()
                                    .hover(|style| style.bg(tokens.chrome.surface_hover))
                                    .on_click(move |event, window, cx| {
                                        cx.stop_propagation();
                                        on_click(event, window, cx);
                                    })
                            });
                        div()
                            .flex()
                            .flex_row()
                            .flex_shrink_0()
                            .items_center()
                            .gap(tokens.sizes.space_1)
                            .children(separator)
                            .child(crumb)
                    }),
            )
    }
}
