// ABOUTME: Panel component following Zed's patterns
// ABOUTME: Container component for organizing content

use crate::ui::{spacing, Theme};
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Panel padding options
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelPadding {
    None,
    Small,
    Medium,
    Large,
}

impl PanelPadding {
    fn padding(&self) -> Pixels {
        match self {
            Self::None => spacing::NONE,
            Self::Small => spacing::SM,
            Self::Medium => spacing::MD,
            Self::Large => spacing::LG,
        }
    }
}

/// A reusable panel component for grouping content
#[derive(IntoElement)]
pub struct Panel {
    id: Option<ElementId>,
    padding: PanelPadding,
    border: bool,
    background: bool,
    shadow: bool,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            id: None,
            padding: PanelPadding::Medium,
            border: true,
            background: true,
            shadow: false,
            children: Vec::new(),
        }
    }
    
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }
    
    pub fn padding(mut self, padding: PanelPadding) -> Self {
        self.padding = padding;
        self
    }
    
    pub fn border(mut self, border: bool) -> Self {
        self.border = border;
        self
    }
    
    pub fn background(mut self, background: bool) -> Self {
        self.background = background;
        self
    }
    
    pub fn shadow(mut self, shadow: bool) -> Self {
        self.shadow = shadow;
        self
    }
    
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
    
    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children.extend(children.into_iter().map(|c| c.into_any_element()));
        self
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let padding = self.padding.padding();
        
        let base = div()
            .flex()
            .flex_col()
            .rounded_md()
            .p(padding);
        
        let styled = if self.background {
            base.bg(theme.surface)
        } else {
            base
        };
        
        let styled = if self.border {
            styled.border_1().border_color(theme.border)
        } else {
            styled
        };
        
        let styled = if self.shadow {
            styled.shadow_sm()
        } else {
            styled
        };
        
        let styled = styled.children(self.children);
        
        match self.id {
            Some(id) => styled.id(id).into_any_element(),
            None => styled.into_any_element()
        }
    }
}