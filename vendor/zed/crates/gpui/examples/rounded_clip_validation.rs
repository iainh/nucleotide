//! Native GPU fixture: every coloured child is deliberately square.
//! Capture on Linux with grim; compare clipped corners against the grey canvas.
use gpui::{
    App, Bounds, ContentMask, Context, Corners, IntoElement, PathBuilder, Render,
    TransformationMatrix, Window, WindowBounds, WindowOptions, canvas, div, point, prelude::*, px,
    radians, rgb, size,
};
use gpui_platform::application;

struct RoundedClipValidation;

fn tile(label: &'static str, content: impl IntoElement) -> impl IntoElement {
    div()
        .w(px(160.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_size(px(12.)).child(label))
        .child(content)
}

fn clip(radius: f32) -> gpui::Div {
    div()
        .relative()
        .w(px(160.))
        .h(px(120.))
        .flex_shrink_0()
        .rounded(px(radius))
        .overflow_hidden()
}

fn path_fill(color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut path = PathBuilder::fill();
            path.move_to(bounds.origin);
            path.line_to(bounds.top_right());
            path.line_to(bounds.bottom_right());
            path.line_to(bounds.bottom_left());
            path.close();
            window.paint_path(path.build().unwrap(), rgb(color));
        },
    )
    .size_full()
}

fn rotated_glyph() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, cx| {
            let center = bounds.center().scale(window.scale_factor());
            let transform = TransformationMatrix::unit()
                .translate(center)
                .rotate(radians(0.45))
                .translate(center.map(|v| gpui::ScaledPixels(-v.0)));
            window.paint_svg(
                bounds,
                "validation-m-glyph".into(),
                Some(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 120"><path fill="white" d="M0 120V0H30L80 65 130 0H160V120H130V45L80 105 30 45V120Z"/></svg>"#),
                transform,
                rgb(0xffe060).into(),
                cx,
            ).unwrap();
        },
    ).size_full()
}

fn overlapping_paths() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            for (offset, radius, color) in [(0., 40., 0x30d080), (50., 20., 0xd060e0)] {
                let mask_bounds = Bounds::new(
                    bounds.origin + point(px(offset), px(0.)),
                    size(px(110.), px(120.)),
                );
                window.with_content_mask(
                    Some(ContentMask::rounded(mask_bounds, Corners::all(px(radius)))),
                    |window| {
                        let mut path = PathBuilder::fill();
                        path.move_to(bounds.origin);
                        path.line_to(bounds.top_right());
                        path.line_to(bounds.bottom_right());
                        path.line_to(bounds.bottom_left());
                        path.close();
                        window.paint_path(path.build().unwrap(), rgb(color));
                    },
                );
            }
        },
    )
    .size_full()
}

impl Render for RoundedClipValidation {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x202830))
            .text_color(rgb(0xffffff))
            .text_size(px(14.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(24.))
            .child("Rounded descendant clipping — native GPU")
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(tile(
                        "Square control",
                        clip(0.).child(div().size_full().bg(rgb(0xe85050))),
                    ))
                    .child(tile(
                        "Rounded / square child",
                        clip(40.).child(div().size_full().bg(rgb(0x30d080))),
                    ))
                    .child(tile(
                        "Nested offset clips",
                        clip(40.).bg(rgb(0x306080)).child(
                            clip(50.)
                                .absolute()
                                .left(px(35.))
                                .top(px(20.))
                                .child(div().size_full().bg(rgb(0xf0a030))),
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(tile(
                        "Glyphs + underlines",
                        clip(40.).child(
                            div()
                                .w(px(200.))
                                .text_size(px(48.))
                                .line_height(px(40.))
                                .underline()
                                .child("MMMMMM MMMMMM MMMMMM"),
                        ),
                    ))
                    .child(tile(
                        "Filled path / radius 40",
                        clip(40.).child(path_fill(0x4090f0)),
                    ))
                    .child(tile(
                        "Different path / radius 16",
                        clip(16.).child(path_fill(0xd060e0)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(tile(
                        "Rotated glyph / radius 40",
                        clip(40.).child(rotated_glyph()),
                    ))
                    .child(tile(
                        "Rotated glyph / square",
                        clip(0.).child(rotated_glyph()),
                    ))
                    .child(tile(
                        "Overlapping path clips",
                        clip(40.).child(overlapping_paths()),
                    )),
            )
    }
}

fn main() {
    env_logger::init();
    application().run(|cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(50.), px(50.)),
                    size(px(600.), px(630.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| RoundedClipValidation),
        )
        .unwrap();
        cx.activate(true);
    });
}
