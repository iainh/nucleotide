// ABOUTME: Shared semantic icon renderer with native platform sources and bundled fallbacks.
// ABOUTME: UI icon names stay independent of SF Symbols, Windows glyphs, and SVG artwork.

use gpui::{
    App, Div, InteractiveElement, Interactivity, IntoElement, ParentElement, RenderOnce,
    SharedString, StyleRefinement, Styled, Transformation, Window, div,
};

#[cfg(target_os = "macos")]
mod macos;
mod symbols;
#[cfg(target_os = "windows")]
mod windows;

/// An application icon. Paths identify semantic icons, not the rendering format.
/// Unknown names and unavailable system symbols use the embedded SVG unchanged.
#[derive(IntoElement)]
pub struct Icon {
    path: SharedString,
    element: Div,
    transformation: Option<Transformation>,
}

pub fn icon() -> Icon {
    Icon {
        path: SharedString::default(),
        element: div(),
        transformation: None,
    }
}

impl Icon {
    pub fn path(mut self, path: impl Into<SharedString>) -> Self {
        self.path = path.into();
        self
    }

    /// Animated artwork retains the SVG renderer's exact transformation semantics.
    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.transformation = Some(transformation);
        self
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl InteractiveElement for Icon {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if self.transformation.is_none()
            && let Some(symbol) = symbols::lookup(&self.path)
        {
            let path = self.path;
            return self
                .element
                .relative()
                .child(
                    gpui::canvas(
                        |_, _, _| (),
                        move |bounds, (), window, cx| {
                            let color = window.text_style().color;
                            #[cfg(target_os = "macos")]
                            let painted = macos::paint(symbol.macos, bounds, color, window);
                            #[cfg(target_os = "windows")]
                            let painted = windows::paint(symbol.windows, bounds, color, window, cx);
                            if !painted {
                                let _ = window.paint_svg(
                                    bounds,
                                    path,
                                    None,
                                    Default::default(),
                                    color,
                                    cx,
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .into_any_element();
        }

        let mut fallback = gpui::svg().path(self.path).absolute().inset_0().size_full();
        if let Some(transformation) = self.transformation {
            fallback = fallback.with_transformation(transformation);
        }
        self.element.relative().child(fallback).into_any_element()
    }
}
