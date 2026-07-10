// ABOUTME: Consistent presentation for loading, empty, informational, and error states
// ABOUTME: Keeps transient UI states aligned with shared tokens and motion preferences

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, div, px, svg,
};

use crate::{IndeterminateProgressIndicator, Theme};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StateViewTone {
    #[default]
    Neutral,
    Info,
    Warning,
    Error,
}

#[derive(IntoElement)]
pub struct StateView {
    id: ElementId,
    title: SharedString,
    detail: Option<SharedString>,
    icon_path: Option<SharedString>,
    action: Option<AnyElement>,
    tone: StateViewTone,
    loading: bool,
    compact: bool,
}

impl StateView {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            icon_path: None,
            action: None,
            tone: StateViewTone::Neutral,
            loading: false,
            compact: false,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(path.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    pub fn tone(mut self, tone: StateViewTone) -> Self {
        self.tone = tone;
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }
}

impl RenderOnce for StateView {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.global::<Theme>().tokens;
        let accent = match self.tone {
            StateViewTone::Neutral => tokens.chrome.text_chrome_secondary,
            StateViewTone::Info => tokens.editor.info,
            StateViewTone::Warning => tokens.editor.warning,
            StateViewTone::Error => tokens.editor.error,
        };
        let icon_size = if self.compact { 16.0 } else { 20.0 };

        div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w_full()
            .min_h(if self.compact { px(64.0) } else { px(96.0) })
            .px(tokens.sizes.space_4)
            .py(if self.compact {
                tokens.sizes.space_3
            } else {
                tokens.sizes.space_5
            })
            .gap(tokens.sizes.space_2)
            .text_center()
            .when(self.loading, |view| {
                view.child(
                    IndeterminateProgressIndicator::new(self.id)
                        .size(icon_size)
                        .text_color(accent),
                )
            })
            .when(!self.loading, |view| {
                view.when_some(self.icon_path, |view, icon_path| {
                    view.child(
                        svg()
                            .path(icon_path)
                            .size(px(icon_size))
                            .flex_shrink_0()
                            .text_color(accent),
                    )
                })
            })
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.chrome.text_on_chrome)
                    .child(self.title),
            )
            .when_some(self.detail, |view, detail| {
                view.child(
                    div()
                        .max_w(px(360.0))
                        .text_xs()
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(detail),
                )
            })
            .when_some(self.action, |view, action| {
                view.mt(tokens.sizes.space_1).child(action)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_neutral_non_loading_state() {
        let view = StateView::new("empty", "Nothing here");

        assert_eq!(view.tone, StateViewTone::Neutral);
        assert!(!view.loading);
        assert!(!view.compact);
        assert!(view.detail.is_none());
        assert!(view.icon_path.is_none());
        assert!(view.action.is_none());
    }

    #[test]
    fn builders_capture_transient_state_presentation() {
        let view = StateView::new("failed", "Could not load")
            .detail("Try again later")
            .icon("icons/triangle-alert.svg")
            .tone(StateViewTone::Error)
            .loading(true)
            .compact(true);

        assert_eq!(view.tone, StateViewTone::Error);
        assert!(view.loading);
        assert!(view.compact);
        assert!(view.detail.is_some());
        assert!(view.icon_path.is_some());
    }
}
