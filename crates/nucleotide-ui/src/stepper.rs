// ABOUTME: Token-styled horizontal progress steps for guided dialogs
// ABOUTME: Shows completed, current and upcoming stages without allowing skipped steps

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use crate::ThemedContext;

/// An informational stepper. Selection is zero-based and controlled by its owner.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    items: Vec<SharedString>,
    selected_index: usize,
}

impl Stepper {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected_index: 0,
        }
    }

    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    pub fn selected_index(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.theme().tokens;
        let chrome = &tokens.chrome;
        let sizes = &tokens.sizes;
        let count = self.items.len();
        let mut stepper = div()
            .id(self.id)
            .role(gpui::accesskit::Role::List)
            .aria_label("Progress")
            .flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .gap(sizes.space_4)
            .text_size(sizes.text_lg)
            .px(sizes.space_5);

        for (index, label) in self.items.into_iter().enumerate() {
            let completed = index < self.selected_index;
            let current = index == self.selected_index;
            let reached = completed || current;
            let indicator = div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(sizes.space_8)
                .rounded_full()
                .border_1()
                .border_color(if reached {
                    chrome.primary
                } else {
                    chrome.border_default
                })
                .text_size(sizes.text_lg)
                .text_color(if reached {
                    chrome.primary_foreground
                } else {
                    chrome.text_chrome_secondary
                })
                .when(reached, |this| this.bg(chrome.primary))
                .when(completed, |this| {
                    this.child(
                        svg()
                            .path("icons/check.svg")
                            .size(sizes.space_5)
                            .text_color(chrome.primary_foreground),
                    )
                })
                .when(!completed, |this| this.child((index + 1).to_string()));

            stepper = stepper.child(
                div()
                    .id(("step", index))
                    .role(gpui::accesskit::Role::ListItem)
                    .aria_label(format!(
                        "Step {} of {}: {}, {}",
                        index + 1,
                        count,
                        label,
                        if completed {
                            "completed"
                        } else if current {
                            "current"
                        } else {
                            "upcoming"
                        }
                    ))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(sizes.space_3)
                    .text_color(if current {
                        chrome.text_on_chrome
                    } else {
                        chrome.text_chrome_secondary
                    })
                    .when(current, |this| this.font_weight(FontWeight::SEMIBOLD))
                    .child(indicator)
                    .child(label),
            );
            if index + 1 < count {
                stepper = stepper.child(
                    div()
                        .flex_1()
                        .min_w(sizes.space_5)
                        .max_w(sizes.space_10 + sizes.space_7)
                        .h(px(2.0))
                        .bg(if completed {
                            chrome.primary
                        } else {
                            chrome.border_default
                        }),
                );
            }
        }
        stepper
    }
}
