// ABOUTME: Background Mermaid rendering for the shared native Markdown component
// ABOUTME: Keeps one cancellable render per visible block, with readable source fallback

use super::{MarkdownStyle, MarkdownSyntaxLoader, render_code_block};
use anyhow::{Context as _, Result, anyhow, ensure};
use gpui::{
    App, Context, IntoElement, Render, RenderImage, RenderOnce, SharedString, Styled, TextRun,
    TextSystem, Window, div, font, img, prelude::*, px,
};
use merman::{
    Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment,
    SvgRequest,
    svg::{
        DeterministicTextMeasurer, MeasurementProfileId, RenderResourcePolicy, ResourceLimitId,
        RootBackgroundPostprocessor, SvgPipeline, TextMeasurementPolicy, TextMeasurementProfile,
        TextMeasurementProfileIdentity,
    },
};
use serde_json::{Value, json};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, LazyLock},
    time::Duration,
};

// These budgets also apply to Markdown supplied by language servers.
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_RASTER_PIXELS: f64 = 4_194_304.0;
const MAX_RASTER_EDGE: f64 = 4096.0;
static RENDER_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

#[derive(IntoElement)]
pub(super) struct MermaidBlock {
    source: SharedString,
    style: MarkdownStyle,
    id: String,
}

impl MermaidBlock {
    pub(super) fn new(source: String, style: MarkdownStyle, id: String) -> Self {
        Self {
            source: source.into(),
            style,
            id,
        }
    }
}

impl RenderOnce for MermaidBlock {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let view = window.use_keyed_state(self.id.clone(), cx, |_, cx| {
            cx.on_release(|view: &mut MermaidView, cx| {
                if let Some(Ok(image)) = view.result.take() {
                    // Release after the current window has returned to App.windows.
                    cx.defer(move |cx| cx.drop_image(image, None));
                }
            })
            .detach();
            MermaidView {
                source: SharedString::default(),
                config: Value::Null,
                style: self.style.clone(),
                id: self.id.clone(),
                result: None,
                control: OperationControl::new(),
                task: None,
            }
        });
        view.update(cx, |view, cx| {
            let config = theme_config(&self.style);
            let changed = view.source != self.source || view.config != config;
            let style_changed = view.style != self.style;
            view.style = self.style;
            if changed {
                if let Some(Ok(image)) = view.result.take() {
                    cx.drop_image(image, Some(window));
                }
                view.source = self.source;
                view.config = config;
                view.start_render(cx);
            }
            if changed || style_changed {
                cx.notify();
            }
        });
        view
    }
}

struct MermaidView {
    source: SharedString,
    config: Value,
    style: MarkdownStyle,
    id: String,
    result: Option<Result<Arc<RenderImage>>>,
    control: OperationControl,
    task: Option<gpui::Task<()>>,
}

impl Drop for MermaidView {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

impl MermaidView {
    fn start_render(&mut self, cx: &mut Context<Self>) {
        self.control.cancel();
        self.task = None;
        self.result = None;
        // Start the deadline on the worker, not while waiting for a render slot.
        self.control = OperationControl::new();
        let control = self.control.clone();
        let source = self.source.clone();
        let config = self.config.clone();
        let renderer = cx.svg_renderer();
        let text_system = cx.text_system().clone();
        let font_family = self.style.code_font_family.clone();
        self.task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move {
                    let _slot = RENDER_SLOTS.clone().acquire_owned().await?;
                    control.set_deadline(Duration::from_secs(5));
                    catch_unwind(AssertUnwindSafe(|| {
                        let svg = render_svg(&source, config, control, text_system, font_family)?;
                        check_raster_budget(&svg)?;
                        renderer
                            .render_single_frame(svg.as_bytes(), 1.0)
                            .map_err(|error| anyhow!("{error}"))
                    }))
                    .unwrap_or_else(|_| Err(anyhow!("Mermaid renderer panicked")))
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.result = Some(result);
                cx.notify();
            });
        }));
    }
}

impl Render for MermaidView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.result {
            Some(Ok(image)) => {
                // As a direct Markdown flex item, img's intrinsic aspect ratio
                // follows its constrained width without percentage-sized wrappers.
                return img(image.clone())
                    .debug_selector(|| format!("{}-diagram", self.id))
                    .max_w_full()
                    .min_w(px(0.0))
                    .flex_none()
                    .when(self.style.preview, |this| this.mt(px(8.0)).mb(px(14.0)))
                    .into_any_element();
            }
            result => {
                let helix_theme = cx
                    .try_global::<crate::theme_manager::ThemeManager>()
                    .map(|manager| manager.helix_theme());
                let loader = cx
                    .try_global::<MarkdownSyntaxLoader>()
                    .map(MarkdownSyntaxLoader::loader);
                let message = match result {
                    Some(Err(error)) => format!("Mermaid: {error:#}"),
                    _ => "Rendering Mermaid diagram…".to_string(),
                };
                div()
                    .debug_selector(|| format!("{}-fallback", self.id))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.style.secondary_color)
                            .child(message),
                    )
                    .child(render_code_block(
                        self.source.to_string(),
                        Some("mermaid".to_string()),
                        &self.style,
                        helix_theme,
                        loader,
                        &format!("{}-source", self.id),
                    ))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .when(self.style.preview, |this| this.mt(px(8.0)).mb(px(14.0)))
            .child(content)
            .into_any_element()
    }
}

fn theme_config(style: &MarkdownStyle) -> Value {
    let color = |color: gpui::Hsla| format!("#{:06x}", u32::from(gpui::Rgba::from(color)) >> 8);
    let background = color(style.code_background);
    let text = color(style.body_color);
    let border = color(style.code_border);
    let accent = color(style.link_color);
    let mut variables: serde_json::Map<String, Value> = [
        ("background", &background),
        ("mainBkg", &background),
        ("primaryColor", &background),
        ("primaryTextColor", &text),
        ("primaryBorderColor", &accent),
        ("secondaryColor", &background),
        ("secondaryTextColor", &text),
        ("secondaryBorderColor", &border),
        ("tertiaryColor", &background),
        ("tertiaryTextColor", &text),
        ("tertiaryBorderColor", &border),
        ("textColor", &text),
        ("nodeTextColor", &text),
        ("lineColor", &text),
        ("edgeLabelBackground", &background),
        ("clusterBkg", &background),
        ("clusterBorder", &border),
        ("actorBkg", &background),
        ("actorBorder", &accent),
        ("actorTextColor", &text),
        ("actorLineColor", &border),
        ("signalColor", &text),
        ("signalTextColor", &text),
        ("labelBoxBkgColor", &background),
        ("labelBoxBorderColor", &accent),
        ("labelTextColor", &text),
        ("loopTextColor", &text),
        ("noteBkgColor", &background),
        ("noteBorderColor", &accent),
        ("noteTextColor", &text),
        ("activationBkgColor", &background),
        ("activationBorderColor", &accent),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), Value::String(value.clone())))
    .collect();
    variables.insert("darkMode".into(), json!(style.code_background.l < 0.5));
    variables.insert("fontFamily".into(), json!(style.code_font_family.as_ref()));
    variables.insert(
        "fontSize".into(),
        json!(format!("{}px", f32::from(style.body_font_size))),
    );
    json!({
        "theme": "base",
        "layout": "dagre",
        "look": "classic",
        "fontFamily": style.code_font_family.as_ref(),
        "fontSize": f32::from(style.body_font_size),
        "themeVariables": variables,
    })
}

fn render_svg(
    source: &str,
    config: Value,
    control: OperationControl,
    text_system: Arc<TextSystem>,
    font_family: SharedString,
) -> Result<String> {
    let text_system = gpui::WindowTextSystem::new(text_system);
    let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
        let mut font = font(font_family.clone());
        font.weight = gpui::FontWeight(match style.font_weight.as_deref() {
            Some("bold" | "bolder") => 700.0,
            Some(weight) => weight.parse().unwrap_or(400.0),
            None => 400.0,
        });
        if style
            .font_style
            .as_deref()
            .is_some_and(|style| style == "italic" || style == "oblique")
        {
            font.style = gpui::FontStyle::Italic;
        }
        text.split('\n')
            .map(|line| {
                let run = TextRun {
                    len: line.len(),
                    font: font.clone(),
                    color: gpui::Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                f64::from(f32::from(
                    text_system
                        .layout_line(line, px(style.font_size as f32), &[run], None)
                        .width,
                ))
            })
            .fold(0.0, f64::max)
    });
    let identity =
        TextMeasurementProfileIdentity::new(MeasurementProfileId::new("nucleotide.gpui")?, "1")?;
    let limits = RenderResourcePolicy::interactive()
        .with_limit(ResourceLimitId::MaxSourceBytes, MAX_SOURCE_BYTES)?
        .with_limit(ResourceLimitId::MaxModelItems, 1000)?
        .with_limit(ResourceLimitId::MaxSvgBytes, 2 * 1024 * 1024)?;
    let request = SvgRequest {
        environment: SvgEnvironment::deterministic()
            .with_resource_policy(limits)
            .with_text_measurement_policy(TextMeasurementPolicy::uniform(
                TextMeasurementProfile::new(identity, measurer),
            )),
        // A token-matched opaque canvas avoids alpha-filtering seams around labels.
        pipeline: Some(SvgPipeline::resvg_safe().with_postprocessor(
            RootBackgroundPostprocessor::new(
                config["themeVariables"]["background"].as_str().unwrap(),
            ),
        )),
        ..Default::default()
    };
    let output = Renderer::new()
        .with_engine(Engine::new().with_site_config(MermaidConfig::from_value(config)))
        .with_parse_options(merman::ParseOptions::strict())
        .render(RenderRequest::svg(source, control, request))
        .context("could not render diagram")?;
    let RenderOutput::Svg(Some(svg)) = output else {
        return Err(anyhow!("no supported Mermaid diagram found"));
    };
    Ok(svg.into_parts().0)
}

fn check_raster_budget(svg: &str) -> Result<()> {
    let document = roxmltree::Document::parse(svg)?;
    // No image resources are needed by our supported diagram families. In
    // particular, don't send compressed data-URL image bombs to GPUI's decoder.
    ensure!(
        !document
            .descendants()
            .any(|node| node.has_tag_name("image") || node.has_tag_name("feImage")),
        "embedded images are not supported in Mermaid previews"
    );
    let root = document.root_element();
    let viewbox: Vec<f64> = root
        .attribute("viewBox")
        .unwrap_or_default()
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(viewbox.len() == 4, "diagram has no valid viewBox");
    let dimension = |name, fallback| -> Result<f64> {
        match root.attribute(name) {
            Some(value) if !value.ends_with('%') => Ok(value.trim_end_matches("px").parse()?),
            _ => Ok(fallback),
        }
    };
    let width = dimension("width", viewbox[2])? * f64::from(gpui::SMOOTH_SVG_SCALE_FACTOR);
    let height = dimension("height", viewbox[3])? * f64::from(gpui::SMOOTH_SVG_SCALE_FACTOR);
    ensure!(
        width.is_finite()
            && height.is_finite()
            && width >= 1.0
            && height >= 1.0
            && width <= MAX_RASTER_EDGE
            && height <= MAX_RASTER_EDGE
            && width * height <= MAX_RASTER_PIXELS,
        "diagram is too large for a preview; shorten or split it"
    );
    Ok(())
}
