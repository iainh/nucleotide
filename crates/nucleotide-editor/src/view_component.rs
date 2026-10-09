// ABOUTME: Native GPUI editor view component shell
// ABOUTME: Composes editor document painting with viewport input and scrollbars

use std::{marker::PhantomData, rc::Rc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, EntityId, FocusHandle,
    GlobalElementId, Hsla, InspectorElementId, InteractiveElement as _, IntoElement, KeyDownEvent,
    LayoutId, ParentElement as _, Pixels, Style, Styled as _, TextStyle, Window, div, relative,
};

use crate::{
    CursorOverlayPlan, EditorDocumentElement, EditorLayout, EditorScrollbarMarker, EditorSurface,
    EditorSurfacePointerEvent, EditorTextMetrics, EditorViewState, EditorViewport,
    ViewportScrollUpdate, selection::EditorPointerSelectionPhase,
};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App)>;
type PointerSelectionCallback =
    Rc<dyn Fn(EditorPointerSelectionPhase, EditorSurfacePointerEvent, &mut App) -> bool>;
type CursorOverlayCallback = Rc<dyn Fn(Option<CursorOverlayPlan>, &mut App)>;
type KeyDownCallback = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) -> bool>;

pub struct NativeEditorView<F, P, T> {
    view_entity_id: EntityId,
    editor_state: EditorViewState,
    text_style: TextStyle,
    prepare: F,
    paint: Option<P>,
    frame: PhantomData<T>,
    focus: Option<FocusHandle>,
    scrollbar_thumb_color: Option<Hsla>,
    scrollbar_markers: Vec<EditorScrollbarMarker>,
    on_scroll: Option<ScrollCallback>,
    on_key_down: Option<KeyDownCallback>,
    on_cursor_overlay: Option<CursorOverlayCallback>,
    on_pointer_selection: Option<PointerSelectionCallback>,
    on_mouse_down: Option<PointerCallback>,
    on_mouse_drag: Option<PointerCallback>,
    on_mouse_up: Option<PointerCallback>,
}

impl<F, P, T> NativeEditorView<F, P, T>
where
    F: FnMut(&mut EditorViewState, Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) -> T
        + 'static,
    P: FnMut(
            &mut EditorViewState,
            &T,
            &EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
    T: 'static,
{
    pub fn new(
        view_entity_id: EntityId,
        editor_state: EditorViewState,
        text_style: TextStyle,
        prepare: F,
        paint: P,
    ) -> Self {
        Self {
            view_entity_id,
            editor_state,
            text_style,
            prepare,
            paint: Some(paint),
            frame: PhantomData,
            focus: None,
            scrollbar_thumb_color: None,
            scrollbar_markers: Vec::new(),
            on_scroll: None,
            on_key_down: None,
            on_cursor_overlay: None,
            on_pointer_selection: None,
            on_mouse_down: None,
            on_mouse_drag: None,
            on_mouse_up: None,
        }
    }

    pub fn scrollbar_thumb_color(mut self, color: Hsla) -> Self {
        self.scrollbar_thumb_color = Some(color);
        self
    }

    pub fn scrollbar_markers(mut self, markers: Vec<EditorScrollbarMarker>) -> Self {
        self.scrollbar_markers = markers;
        self
    }

    pub fn track_focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_key_down(
        mut self,
        callback: impl Fn(&KeyDownEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(callback));
        self
    }

    pub fn on_scroll(
        mut self,
        callback: impl Fn(&EditorViewport, ViewportScrollUpdate, &mut App) + 'static,
    ) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    pub fn on_cursor_overlay(
        mut self,
        callback: impl Fn(Option<CursorOverlayPlan>, &mut App) + 'static,
    ) -> Self {
        self.on_cursor_overlay = Some(Rc::new(callback));
        self
    }

    pub fn on_pointer_selection(
        mut self,
        callback: impl Fn(EditorPointerSelectionPhase, EditorSurfacePointerEvent, &mut App) -> bool
        + 'static,
    ) -> Self {
        self.on_pointer_selection = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_down(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_down = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_drag(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_drag = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_up(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_up = Some(Rc::new(callback));
        self
    }
}

impl<F, P, T> IntoElement for NativeEditorView<F, P, T>
where
    F: FnMut(&mut EditorViewState, Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) -> T
        + 'static,
    P: FnMut(
            &mut EditorViewState,
            &T,
            &EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
    T: 'static,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<F, P, T> Element for NativeEditorView<F, P, T>
where
    F: FnMut(&mut EditorViewState, Bounds<Pixels>, &mut EditorLayout, &mut Window, &mut App) -> T
        + 'static,
    P: FnMut(
            &mut EditorViewState,
            &T,
            &EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
    T: 'static,
{
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut layout = EditorTextMetrics::resolve(cx.text_system(), &self.text_style)
            .layout_for_bounds(bounds);
        // Prepare before constructing the surface: its scrollbars and input must
        // see the same geometry that the document will paint in this frame.
        let frame = (self.prepare)(&mut self.editor_state, bounds, &mut layout, window, cx);
        let view_entity_id = self.view_entity_id;
        let editor_state = self.editor_state.clone();
        let mut paint = self.paint.take().expect("editor element prepainted once");
        let focus = self.focus.clone();
        let scrollbar_thumb_color = self.scrollbar_thumb_color;
        let scrollbar_markers = self.scrollbar_markers.clone();
        let on_scroll = self.on_scroll.clone();
        let on_key_down = self.on_key_down.clone();
        let on_cursor_overlay = self.on_cursor_overlay.clone();
        let on_pointer_selection = self.on_pointer_selection.clone();
        let on_mouse_down = self.on_mouse_down.clone();
        let on_mouse_drag = self.on_mouse_drag.clone();
        let on_mouse_up = self.on_mouse_up.clone();

        let root = div().id("editor-content").w_full().h_full().flex();

        let viewport = editor_state.viewport().clone();
        let surface_metrics = editor_state.surface_metrics().clone();
        let vertical_scrollbar_state = editor_state.vertical_scrollbar_state().clone();
        let horizontal_scrollbar_state = editor_state.horizontal_scrollbar_state().clone();
        let mut paint_editor_state = editor_state;
        let document_element =
            EditorDocumentElement::new(layout, move |_bounds, layout, window, cx| {
                let overlay_plan = paint(&mut paint_editor_state, &frame, layout, window, cx);

                if let Some(on_cursor_overlay) = &on_cursor_overlay {
                    on_cursor_overlay(overlay_plan, cx);
                }
            });

        let mut editor_surface = EditorSurface::new(
            view_entity_id,
            viewport,
            surface_metrics,
            vertical_scrollbar_state,
            horizontal_scrollbar_state,
            document_element,
        );

        if let Some(scrollbar_thumb_color) = scrollbar_thumb_color {
            editor_surface = editor_surface.scrollbar_thumb_color(scrollbar_thumb_color);
        }
        editor_surface = editor_surface.scrollbar_markers(scrollbar_markers);

        if let Some(on_scroll) = on_scroll {
            editor_surface = editor_surface.on_scroll(move |viewport, update, cx| {
                on_scroll(viewport, update, cx);
            });
        }

        if let Some(focus) = focus {
            editor_surface = editor_surface.track_focus(focus);
        }

        if let Some(on_key_down) = on_key_down {
            editor_surface =
                editor_surface.on_key_down(move |event, window, cx| on_key_down(event, window, cx));
        }

        if on_pointer_selection.is_some() || on_mouse_down.is_some() {
            let on_pointer_selection = on_pointer_selection.clone();
            editor_surface = editor_surface.on_mouse_down(move |event, cx| {
                let mut changed = false;
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    changed |= on_pointer_selection(EditorPointerSelectionPhase::Begin, event, cx);
                }
                if let Some(on_mouse_down) = &on_mouse_down {
                    on_mouse_down(event, cx);
                    changed |= on_pointer_selection.is_none();
                }
                changed
            });
        }

        if on_pointer_selection.is_some() || on_mouse_drag.is_some() {
            let on_pointer_selection = on_pointer_selection.clone();
            editor_surface = editor_surface.on_mouse_drag(move |event, cx| {
                let mut changed = false;
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    changed |= on_pointer_selection(EditorPointerSelectionPhase::Extend, event, cx);
                }
                if let Some(on_mouse_drag) = &on_mouse_drag {
                    on_mouse_drag(event, cx);
                    changed = true;
                }
                changed
            });
        }

        if on_pointer_selection.is_some() || on_mouse_up.is_some() {
            editor_surface = editor_surface.on_mouse_up(move |event, cx| {
                let mut changed = false;
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    changed |= on_pointer_selection(EditorPointerSelectionPhase::End, event, cx);
                }
                if let Some(on_mouse_up) = &on_mouse_up {
                    on_mouse_up(event, cx);
                    changed = true;
                }
                changed
            });
        }

        let paint_area = div().id("editor-paint-area").w_full().h_full().flex_1();

        let mut surface = root
            .child(paint_area.child(editor_surface))
            .into_any_element();
        surface.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        surface
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        surface: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        surface.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use gpui::{
        AppContext as _, Empty, Entity, FocusHandle, Keystroke, MouseButton, Render, ScrollDelta,
        ScrollWheelEvent, TestAppContext, TouchPhase, point, px, size,
    };

    use super::*;

    #[gpui::test]
    fn native_editor_view_draws_and_dispatches_input(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });

        let mut editor_state = EditorViewState::new(px(20.0), px(8.0));
        editor_state
            .viewport_mut()
            .set_layout(px(20.0), size(px(100.0), px(200.0)), 50);

        let painted = Rc::new(Cell::new(false));
        let overlay_seen = Rc::new(Cell::new(None));
        let saw_scroll = Rc::new(Cell::new(false));
        let saw_down = Rc::new(Cell::new(false));
        let saw_drag = Rc::new(Cell::new(false));
        let saw_up = Rc::new(Cell::new(false));
        let phases = Rc::new(RefCell::new(Vec::new()));
        let overlay_plan = CursorOverlayPlan {
            cursor_position: point(px(12.0), px(24.0)),
            cursor_size: size(px(8.0), px(20.0)),
        };

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(112.0), px(200.0)),
            |_, _| {
                NativeEditorView::new(
                    view_entity_id,
                    editor_state.clone(),
                    TextStyle::default(),
                    |_, _, _, _, _| (),
                    {
                        let painted = Rc::clone(&painted);
                        move |_state, _bounds, _layout, _window, _cx| {
                            painted.set(true);
                            Some(overlay_plan)
                        }
                    },
                )
                .on_cursor_overlay({
                    let overlay_seen = Rc::clone(&overlay_seen);
                    move |overlay_plan, _| overlay_seen.set(overlay_plan)
                })
                .on_scroll({
                    let saw_scroll = Rc::clone(&saw_scroll);
                    move |_, _, _| saw_scroll.set(true)
                })
                .on_pointer_selection({
                    let phases = Rc::clone(&phases);
                    move |phase, _, _| {
                        phases.borrow_mut().push(phase);
                        true
                    }
                })
                .on_mouse_down({
                    let saw_down = Rc::clone(&saw_down);
                    move |_, _| saw_down.set(true)
                })
                .on_mouse_drag({
                    let saw_drag = Rc::clone(&saw_drag);
                    move |_, _| saw_drag.set(true)
                })
                .on_mouse_up({
                    let saw_up = Rc::clone(&saw_up);
                    move |_, _| saw_up.set(true)
                })
                .into_element()
            },
        );

        window.simulate_event(ScrollWheelEvent {
            position: point(px(10.0), px(10.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        window.simulate_mouse_down(
            point(px(10.0), px(10.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_move(
            point(px(10.0), px(30.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_up(
            point(px(10.0), px(30.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );

        assert!(painted.get());
        assert_eq!(overlay_seen.get(), Some(overlay_plan));
        assert!(saw_scroll.get());
        assert!(saw_down.get());
        assert!(saw_drag.get());
        assert!(saw_up.get());
        assert_eq!(
            phases.borrow().as_slice(),
            &[
                EditorPointerSelectionPhase::Begin,
                EditorPointerSelectionPhase::Extend,
                EditorPointerSelectionPhase::Extend,
                EditorPointerSelectionPhase::End,
            ]
        );
    }

    #[gpui::test]
    fn native_editor_view_scrolls_after_initial_prepaint_layout(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });
        let editor_state = EditorViewState::new(px(20.0), px(8.0));

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(112.0), px(200.0)),
            |_, _| {
                NativeEditorView::new(
                    view_entity_id,
                    editor_state.clone(),
                    TextStyle::default(),
                    move |state, bounds, _layout, _window, _cx| {
                        state.viewport_mut().set_layout(px(20.0), bounds.size, 50);
                    },
                    |_, _, _, _, _| None,
                )
                .on_mouse_down(|_, _| {})
                .into_element()
            },
        );

        assert!(editor_state.viewport().max_scroll_offset().height > px(0.0));

        // The scrollbar must already be interactive on the first draw, not
        // merely become available after a wheel event causes a repair frame.
        window.simulate_mouse_down(
            point(px(106.0), px(150.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_up(
            point(px(106.0), px(150.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        assert!(editor_state.viewport().scroll_position().y > px(0.0));

        window.simulate_event(ScrollWheelEvent {
            position: point(px(10.0), px(10.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });

        assert!(editor_state.viewport().scroll_position().y > px(0.0));
    }

    struct InitialLayoutRenderHost {
        editor_state: EditorViewState,
        render_count: Rc<Cell<usize>>,
    }

    impl Render for InitialLayoutRenderHost {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.render_count.set(self.render_count.get() + 1);
            NativeEditorView::new(
                cx.entity_id(),
                self.editor_state.clone(),
                TextStyle::default(),
                move |state, bounds, _layout, _window, _cx| {
                    state.viewport_mut().set_layout(px(20.0), bounds.size, 50);
                },
                |_, _, _, _, _| None,
            )
        }
    }

    #[gpui::test]
    fn native_editor_view_does_not_need_geometry_repair_render(cx: &mut TestAppContext) {
        let render_count = Rc::new(Cell::new(0));
        let render_count_clone = Rc::clone(&render_count);

        let (_host, cx) = cx.add_window_view(|_, _cx| InitialLayoutRenderHost {
            editor_state: EditorViewState::new(px(20.0), px(8.0)),
            render_count: render_count_clone,
        });

        cx.run_until_parked();

        assert_eq!(
            render_count.get(),
            1,
            "geometry must be ready in the first frame"
        );
    }

    struct KeyDispatchHost {
        view_entity_id: EntityId,
        editor_state: EditorViewState,
        focus: FocusHandle,
        saw_key: Rc<Cell<bool>>,
    }

    impl Render for KeyDispatchHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            NativeEditorView::new(
                self.view_entity_id,
                self.editor_state.clone(),
                TextStyle::default(),
                |_, _, _, _, _| (),
                |_state, _bounds, _layout, _window, _cx| None,
            )
            .track_focus(self.focus.clone())
            .on_key_down({
                let saw_key = Rc::clone(&self.saw_key);
                move |event, _, _| {
                    saw_key.set(event.keystroke.key == "a");
                    true
                }
            })
        }
    }

    #[gpui::test]
    fn native_editor_view_dispatches_key_events_from_focus(cx: &mut TestAppContext) {
        let saw_key = Rc::new(Cell::new(false));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let saw_key = Rc::clone(&saw_key);
                cx.new(|cx| KeyDispatchHost {
                    view_entity_id: cx.entity_id(),
                    editor_state: EditorViewState::new(px(20.0), px(8.0)),
                    focus: cx.focus_handle(),
                    saw_key,
                })
            })
            .unwrap()
        });

        window
            .update(cx, |host, window, cx| window.focus(&host.focus, cx))
            .unwrap();

        cx.dispatch_keystroke(*window, Keystroke::parse("a").unwrap());

        assert!(saw_key.get());
    }
}
