// ABOUTME: Native GPUI editor view component shell
// ABOUTME: Composes editor document painting with viewport input and scrollbars

use std::rc::Rc;

use gpui::{
    App, Bounds, Component, EntityId, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Pixels, RenderOnce, Styled as _, TextStyle, Window, div,
};

use crate::{
    CursorOverlayPlan, EditorDocumentElement, EditorLayout, EditorSurface,
    EditorSurfacePointerEvent, EditorViewState, EditorViewport, ViewportScrollUpdate,
    selection::EditorPointerSelectionPhase,
};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App)>;
type PointerSelectionCallback =
    Rc<dyn Fn(EditorPointerSelectionPhase, EditorSurfacePointerEvent, &mut App)>;
type CursorOverlayCallback = Rc<dyn Fn(Option<CursorOverlayPlan>, &mut App)>;
type KeyDownCallback = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) -> bool>;

pub struct NativeEditorView<P> {
    view_entity_id: EntityId,
    editor_state: EditorViewState,
    text_style: TextStyle,
    paint: P,
    focus: Option<FocusHandle>,
    on_scroll: Option<ScrollCallback>,
    on_key_down: Option<KeyDownCallback>,
    on_cursor_overlay: Option<CursorOverlayCallback>,
    on_pointer_selection: Option<PointerSelectionCallback>,
    on_mouse_down: Option<PointerCallback>,
    on_mouse_drag: Option<PointerCallback>,
    on_mouse_up: Option<PointerCallback>,
}

impl<P> NativeEditorView<P>
where
    P: FnMut(
            &mut EditorViewState,
            Bounds<Pixels>,
            &mut EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
{
    pub fn new(
        view_entity_id: EntityId,
        editor_state: EditorViewState,
        text_style: TextStyle,
        paint: P,
    ) -> Self {
        Self {
            view_entity_id,
            editor_state,
            text_style,
            paint,
            focus: None,
            on_scroll: None,
            on_key_down: None,
            on_cursor_overlay: None,
            on_pointer_selection: None,
            on_mouse_down: None,
            on_mouse_drag: None,
            on_mouse_up: None,
        }
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
        callback: impl Fn(EditorPointerSelectionPhase, EditorSurfacePointerEvent, &mut App) + 'static,
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

impl<P> IntoElement for NativeEditorView<P>
where
    P: FnMut(
            &mut EditorViewState,
            Bounds<Pixels>,
            &mut EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
{
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl<P> RenderOnce for NativeEditorView<P>
where
    P: FnMut(
            &mut EditorViewState,
            Bounds<Pixels>,
            &mut EditorLayout,
            &mut Window,
            &mut App,
        ) -> Option<CursorOverlayPlan>
        + 'static,
{
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let NativeEditorView {
            view_entity_id,
            editor_state,
            text_style,
            mut paint,
            focus,
            on_scroll,
            on_key_down,
            on_cursor_overlay,
            on_pointer_selection,
            on_mouse_down,
            on_mouse_drag,
            on_mouse_up,
        } = self;

        let root = div().id("editor-content").w_full().h_full().flex();

        let viewport = editor_state.viewport().clone();
        let surface_metrics = editor_state.surface_metrics().clone();
        let scrollbar_state = editor_state.scrollbar_state().clone();
        let mut paint_editor_state = editor_state;
        let document_element =
            EditorDocumentElement::new(text_style, move |bounds, after_layout, window, cx| {
                let overlay_plan = paint(&mut paint_editor_state, bounds, after_layout, window, cx);
                if let Some(on_cursor_overlay) = &on_cursor_overlay {
                    on_cursor_overlay(overlay_plan, cx);
                }
            });

        let mut editor_surface = EditorSurface::new(
            view_entity_id,
            viewport,
            surface_metrics,
            scrollbar_state,
            document_element,
        );

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
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    on_pointer_selection(EditorPointerSelectionPhase::Begin, event, cx);
                }
                if let Some(on_mouse_down) = &on_mouse_down {
                    on_mouse_down(event, cx);
                }
            });
        }

        if on_pointer_selection.is_some() || on_mouse_drag.is_some() {
            let on_pointer_selection = on_pointer_selection.clone();
            editor_surface = editor_surface.on_mouse_drag(move |event, cx| {
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    on_pointer_selection(EditorPointerSelectionPhase::Extend, event, cx);
                }
                if let Some(on_mouse_drag) = &on_mouse_drag {
                    on_mouse_drag(event, cx);
                }
            });
        }

        if on_pointer_selection.is_some() || on_mouse_up.is_some() {
            editor_surface = editor_surface.on_mouse_up(move |event, cx| {
                if let Some(on_pointer_selection) = &on_pointer_selection {
                    on_pointer_selection(EditorPointerSelectionPhase::End, event, cx);
                }
                if let Some(on_mouse_up) = &on_mouse_up {
                    on_mouse_up(event, cx);
                }
            });
        }

        let paint_area = div().id("editor-paint-area").w_full().h_full().flex_1();

        root.child(paint_area.child(editor_surface))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use gpui::{
        AppContext as _, Empty, Entity, FocusHandle, IntoElement as _, Keystroke, MouseButton,
        Render, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point, px, size,
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
                    move |phase, _, _| phases.borrow_mut().push(phase)
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
                EditorPointerSelectionPhase::End,
            ]
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
