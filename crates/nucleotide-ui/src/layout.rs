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
            WorkspaceChrome::new("workspace-chrome")
                .child(Toolbar::new("toolbar").label("Project"))
                .child(
                    Panel::new("panel")
                        .variant(PanelVariant::Elevated)
                        .child(div().child("Body")),
                )
                .child(StatusBar::new("status-bar").child("Ready"))
        }
    }

    fn init_layout_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    #[gpui::test]
    fn semantic_layout_wrappers_render(cx: &mut TestAppContext) {
        init_layout_test(cx);
        let (_harness, _cx) = cx.add_window_view(|_window, _cx| LayoutHarness);
    }
}
