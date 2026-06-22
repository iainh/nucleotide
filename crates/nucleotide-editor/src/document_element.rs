// ABOUTME: Native GPUI element shell for editor document painting
// ABOUTME: Owns document layout/prepaint while callers provide app-specific paint data

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style as GpuiStyle, TextStyle, Window, relative,
};

use crate::{EditorLayout, EditorTextMetrics};

pub struct EditorDocumentElement<P> {
    text_style: TextStyle,
    paint: P,
}

impl<P> EditorDocumentElement<P>
where
    P: FnMut(Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) + 'static,
{
    pub fn new(text_style: TextStyle, paint: P) -> Self {
        Self { text_style, paint }
    }
}

impl<P> IntoElement for EditorDocumentElement<P>
where
    P: FnMut(Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) + 'static,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<P> Element for EditorDocumentElement<P>
where
    P: FnMut(Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) + 'static,
{
    type RequestLayoutState = ();
    type PrepaintState = EditorLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = GpuiStyle::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();

        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        EditorTextMetrics::resolve(cx.text_system(), &self.text_style).layout_for_bounds(bounds)
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        after_layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        (self.paint)(bounds, after_layout, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{ParentElement as _, Styled as _, TestAppContext, div, point, px, size};

    use super::*;

    #[gpui::test]
    fn editor_document_element_dispatches_paint_with_layout(cx: &mut TestAppContext) {
        let painted = Rc::new(Cell::new(false));
        let line_height = Rc::new(Cell::new(px(0.0)));
        let painted_clone = Rc::clone(&painted);
        let line_height_clone = Rc::clone(&line_height);
        let text_style = TextStyle::default();

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(120.0), px(80.0)),
            |_, _| {
                div().size_full().child(
                    EditorDocumentElement::new(
                        text_style.clone(),
                        move |_bounds, layout, _window, _cx| {
                            painted_clone.set(true);
                            line_height_clone.set(layout.line_height);
                        },
                    )
                    .into_element(),
                )
            },
        );

        assert!(painted.get());
        assert!(line_height.get() > px(0.0));
    }
}
