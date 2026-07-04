// ABOUTME: Semantic layout wrappers for common Nucleotide application structure.
// ABOUTME: Encodes token-backed shell, panel, toolbar, and status bar defaults.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, Styled, div, px,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelVariant {
    #[default]
    Surface,
    Elevated,
    Transparent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelLayout {
    current_px: f32,
    min_px: f32,
    max_px: f32,
    default_px: f32,
}

impl PanelLayout {
    pub fn new(current_px: f32, min_px: f32, max_px: f32, default_px: f32) -> Self {
        let min_px = finite_nonnegative_px(min_px);
        let max_px = finite_nonnegative_px(max_px).max(min_px);
        let current_px = finite_nonnegative_px(current_px).clamp(min_px, max_px);
        let default_px = finite_nonnegative_px(default_px).clamp(min_px, max_px);

        Self {
            current_px,
            min_px,
            max_px,
            default_px,
        }
    }

    pub fn current_px(&self) -> f32 {
        self.current_px
    }

    pub fn min_px(&self) -> f32 {
        self.min_px
    }

    pub fn max_px(&self) -> f32 {
        self.max_px
    }

    pub fn default_px(&self) -> f32 {
        self.default_px
    }

    pub fn clamp(&self, value_px: f32) -> f32 {
        finite_nonnegative_px(value_px).clamp(self.min_px, self.max_px)
    }

    pub fn reset_px(&self) -> f32 {
        self.default_px
    }

    pub fn with_reserved_trailing_space(&self, available_px: f32, reserved_px: f32) -> Self {
        let available_px = finite_nonnegative_px(available_px);
        let reserved_px = finite_nonnegative_px(reserved_px);
        let max_px = (available_px - reserved_px)
            .max(self.min_px)
            .min(self.max_px);
        Self::new(self.current_px, self.min_px, max_px, self.default_px)
    }
}

fn finite_nonnegative_px(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[derive(IntoElement)]
pub struct AppShell {
    id: ElementId,
    header: Option<AnyElement>,
    footer: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl AppShell {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            header: None,
            footer: None,
            children: Vec::new(),
        }
    }

    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }
}

impl ParentElement for AppShell {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for AppShell {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        let mut shell = div()
            .id(self.id)
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(tokens.editor.background)
            .text_color(tokens.chrome.text_on_chrome);

        if let Some(header) = self.header {
            shell = shell.child(header);
        }

        shell = shell.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .children(self.children),
        );

        if let Some(footer) = self.footer {
            shell = shell.child(footer);
        }

        shell
    }
}

#[derive(IntoElement)]
pub struct WorkspaceChrome {
    id: ElementId,
    children: Vec<AnyElement>,
}

impl WorkspaceChrome {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
        }
    }
}

impl ParentElement for WorkspaceChrome {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for WorkspaceChrome {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(tokens.editor.background)
            .text_color(tokens.chrome.text_on_chrome)
            .children(self.children)
    }
}

#[derive(IntoElement)]
pub struct Panel {
    id: ElementId,
    variant: PanelVariant,
    padding: Option<Pixels>,
    bordered: bool,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            variant: PanelVariant::Surface,
            padding: None,
            bordered: true,
            children: Vec::new(),
        }
    }

    pub fn variant(mut self, variant: PanelVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn padding(mut self, padding: impl Into<Pixels>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    pub fn border(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        let background = match self.variant {
            PanelVariant::Surface => tokens.chrome.surface,
            PanelVariant::Elevated => tokens.chrome.surface_elevated,
            PanelVariant::Transparent => tokens.chrome.surface.alpha(0.0),
        };

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(background)
            .text_color(tokens.chrome.text_on_chrome)
            .when(self.bordered, |this| {
                this.border_1().border_color(tokens.chrome.border_default)
            })
            .rounded(tokens.sizes.radius_md)
            .when(self.variant == PanelVariant::Elevated, |this| {
                this.shadow(vec![tokens.chrome.shadow_md.to_box_shadow(false)])
            })
            .p(self.padding.unwrap_or(tokens.sizes.space_3))
            .children(self.children)
    }
}

#[derive(IntoElement)]
pub struct BottomPanel {
    id: ElementId,
    height: Option<Pixels>,
    children: Vec<AnyElement>,
}

impl BottomPanel {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            height: None,
            children: Vec::new(),
        }
    }

    pub fn height(mut self, height: impl Into<Pixels>) -> Self {
        self.height = Some(height.into());
        self
    }
}

impl ParentElement for BottomPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for BottomPanel {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .when_some(self.height, |this, height| this.h(height))
            .bg(tokens.chrome.surface)
            .border_t_1()
            .border_color(tokens.chrome.separator_color)
            .text_color(tokens.chrome.text_on_chrome)
            .children(self.children)
    }
}

#[derive(IntoElement)]
pub struct Toolbar {
    id: ElementId,
    label: Option<SharedString>,
    compact: bool,
    children: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            compact: false,
            children: Vec::new(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }
}

impl ParentElement for Toolbar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Toolbar {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        let height = if self.compact {
            tokens.sizes.space_8
        } else {
            tokens.sizes.space_10
        };

        div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens.sizes.space_2)
            .h(height)
            .min_w(px(0.0))
            .px(tokens.sizes.space_3)
            .bg(tokens.chrome.surface)
            .border_b_1()
            .border_color(tokens.chrome.separator_color)
            .text_color(tokens.chrome.text_on_chrome)
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_size(tokens.sizes.text_sm)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(label),
                )
            })
            .children(self.children)
    }
}

#[derive(IntoElement)]
pub struct StatusBar {
    id: ElementId,
    active: bool,
    children: Vec<AnyElement>,
}

impl StatusBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            active: true,
            children: Vec::new(),
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

impl ParentElement for StatusBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;
        let status = tokens.status_bar_tokens();
        let background = if self.active {
            status.background_active
        } else {
            status.background_inactive
        };

        div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens.sizes.space_3)
            .h(tokens.sizes.statusbar_height)
            .min_w(px(0.0))
            .px(tokens.sizes.space_3)
            .bg(background)
            .border_t_1()
            .border_color(status.border)
            .text_size(tokens.sizes.text_sm)
            .text_color(status.text_primary)
            .children(self.children)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, ParentElement as _, Render, TestAppContext, div};

    use super::*;

    struct LayoutHarness;

    impl Render for LayoutHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut Context<Self>,
        ) -> impl IntoElement {
            AppShell::new("app-shell")
                .header(Toolbar::new("toolbar").label("Project"))
                .child(
                    Panel::new("panel")
                        .variant(PanelVariant::Elevated)
                        .child(div().child("Body")),
                )
                .child(BottomPanel::new("bottom-panel").height(gpui::px(48.0)))
                .footer(StatusBar::new("status-bar").child("Ready"))
        }
    }

    fn init_layout_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    #[test]
    fn panel_layout_clamps_current_default_and_bounds() {
        let layout = PanelLayout::new(720.0, 160.0, 640.0, 800.0);

        assert_eq!(layout.current_px(), 640.0);
        assert_eq!(layout.min_px(), 160.0);
        assert_eq!(layout.max_px(), 640.0);
        assert_eq!(layout.default_px(), 640.0);
        assert_eq!(layout.clamp(80.0), 160.0);
        assert_eq!(layout.clamp(320.0), 320.0);
        assert_eq!(layout.reset_px(), 640.0);
    }

    #[test]
    fn panel_layout_keeps_max_at_least_min() {
        let layout = PanelLayout::new(20.0, 120.0, 40.0, 60.0);

        assert_eq!(layout.current_px(), 120.0);
        assert_eq!(layout.min_px(), 120.0);
        assert_eq!(layout.max_px(), 120.0);
        assert_eq!(layout.reset_px(), 120.0);
    }

    #[test]
    fn panel_layout_reserves_trailing_space() {
        let layout =
            PanelLayout::new(500.0, 120.0, 700.0, 320.0).with_reserved_trailing_space(640.0, 240.0);

        assert_eq!(layout.current_px(), 400.0);
        assert_eq!(layout.min_px(), 120.0);
        assert_eq!(layout.max_px(), 400.0);
        assert_eq!(layout.reset_px(), 320.0);
    }

    #[gpui::test]
    fn semantic_layout_wrappers_render(cx: &mut TestAppContext) {
        init_layout_test(cx);
        let (_harness, _cx) = cx.add_window_view(|_window, _cx| LayoutHarness);
    }
}
