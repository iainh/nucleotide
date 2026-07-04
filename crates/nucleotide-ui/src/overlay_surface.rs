// ABOUTME: Shared overlay surface for centred prompt, picker, and manager panels
// ABOUTME: Owns light-dismiss, occlusion, and click containment for app overlays

use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    Pixels, RenderOnce, Styled, Window, div, px,
};

type LightDismissHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// Full-window overlay wrapper for one centred, top-aligned surface.
#[derive(IntoElement)]
pub struct OverlaySurface {
    children: Vec<AnyElement>,
    top: Pixels,
    key_context: Option<&'static str>,
    on_light_dismiss: Option<LightDismissHandler>,
}

impl OverlaySurface {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            top: px(32.0),
            key_context: None,
            on_light_dismiss: None,
        }
    }

    pub fn top(mut self, top: impl Into<Pixels>) -> Self {
        self.top = top.into();
        self
    }

    pub fn key_context(mut self, key_context: &'static str) -> Self {
        self.key_context = Some(key_context);
        self
    }

    pub fn on_light_dismiss(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_light_dismiss = Some(Box::new(handler));
        self
    }
}

impl Default for OverlaySurface {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for OverlaySurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for OverlaySurface {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut root = div().absolute().size_full().bottom_0().left_0().occlude();

        if let Some(key_context) = self.key_context {
            root = root.key_context(key_context);
        }

        if let Some(on_light_dismiss) = self.on_light_dismiss {
            root = root.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                on_light_dismiss(event, window, cx);
                cx.stop_propagation();
            });
        }

        root.child(
            div()
                .flex()
                .size_full()
                .justify_center()
                .items_start()
                .pt(self.top)
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .children(self.children),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, ParentElement as _, Render, TestAppContext, Window, div, px};

    use super::*;

    struct OverlaySurfaceHarness;

    impl OverlaySurfaceHarness {
        fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
            Self
        }
    }

    impl Render for OverlaySurfaceHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                OverlaySurface::new()
                    .key_context("Overlay")
                    .top(px(16.0))
                    .child("overlay"),
            )
        }
    }

    #[gpui::test]
    fn overlay_surface_renders_in_test_harness(cx: &mut TestAppContext) {
        let (_harness, cx) = cx.add_window_view(OverlaySurfaceHarness::new);

        cx.update(|window, _cx| {
            assert!(window.viewport_size().width > px(0.0));
        });
    }
}
