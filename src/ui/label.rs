// ABOUTME: Label component following Zed's patterns  
// ABOUTME: Provides consistent text styling

use crate::ui::Theme;
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Label weight options
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabelWeight {
    Regular,
    Medium,
    Bold,
}

/// Label size options
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabelSize {
    XSmall,
    Small,
    Default,
    Large,
    XLarge,
}

impl LabelSize {
    fn text_size(&self) -> Pixels {
        match self {
            Self::XSmall => px(10.),
            Self::Small => px(12.),
            Self::Default => px(14.),
            Self::Large => px(16.),
            Self::XLarge => px(18.),
        }
    }
}

/// A reusable label component
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: LabelSize,
    weight: LabelWeight,
    color: Option<Hsla>,
    muted: bool,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: LabelSize::Default,
            weight: LabelWeight::Regular,
            color: None,
            muted: false,
        }
    }
    
    pub fn size(mut self, size: LabelSize) -> Self {
        self.size = size;
        self
    }
    
    pub fn weight(mut self, weight: LabelWeight) -> Self {
        self.weight = weight;
        self
    }
    
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
    
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let text_color = if let Some(color) = self.color {
            color
        } else if self.muted {
            theme.text_muted
        } else {
            theme.text
        };
        
        let font_weight = match self.weight {
            LabelWeight::Regular => FontWeight::NORMAL,
            LabelWeight::Medium => FontWeight::MEDIUM,
            LabelWeight::Bold => FontWeight::BOLD,
        };
        
        div()
            .text_size(self.size.text_size())
            .text_color(text_color)
            .font_weight(font_weight)
            .child(self.text)
    }
}