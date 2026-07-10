use gpui::{Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div};
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, IndeterminateProgressIndicator, ThemedContext, Tooltipped,
};

use crate::actions::updates::Show;

use super::{CheckOrigin, UpdateController, UpdateState};

pub struct UpdateIndicator {
    controller: Entity<UpdateController>,
}

impl UpdateIndicator {
    pub fn new(controller: Entity<UpdateController>, cx: &mut Context<Self>) -> Self {
        cx.observe(&controller, |_, _, cx| cx.notify()).detach();
        Self { controller }
    }

    fn open_update_details(window: &mut Window, cx: &mut gpui::App) {
        window.prevent_default();
        window.dispatch_action(Box::new(Show), cx);
        cx.stop_propagation();
    }
}

impl Render for UpdateIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.controller.read(cx).state().clone();
        let titlebar_tokens = cx.theme().tokens.titlebar_tokens();

        let button = match state {
            UpdateState::Checking {
                origin: CheckOrigin::Manual,
            } => Some(
                Button::new("titlebar-update-checking", "")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .content(
                        IndeterminateProgressIndicator::new("titlebar-update-checking-spinner")
                            .size(13.0)
                            .text_color(titlebar_tokens.foreground),
                    )
                    .tooltip("Checking for Nucleotide updates")
                    .aria_label("Checking for Nucleotide updates")
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            UpdateState::Available(update) => Some(
                Button::icon_only("titlebar-update-available", "icons/info.svg")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .tooltip(format!("Nucleotide {} is available", update.version))
                    .aria_label(format!("Nucleotide {} is available", update.version))
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            UpdateState::Downloading { update, percent } => Some(
                Button::new("titlebar-update-downloading", "")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .content(
                        IndeterminateProgressIndicator::new("titlebar-update-downloading-spinner")
                            .size(13.0)
                            .text_color(titlebar_tokens.foreground),
                    )
                    .tooltip(format!(
                        "Downloading Nucleotide {}: {}%",
                        update.version, percent
                    ))
                    .aria_label(format!(
                        "Downloading Nucleotide {}: {}%",
                        update.version, percent
                    ))
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            UpdateState::ReadyToRestart(update) => Some(
                Button::icon_only("titlebar-update-ready", "icons/rotate-ccw.svg")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .tooltip(format!(
                        "Restart to update Nucleotide to {}",
                        update.version
                    ))
                    .aria_label(format!(
                        "Restart to update Nucleotide to {}",
                        update.version
                    ))
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            UpdateState::Applying(update) => Some(
                Button::new("titlebar-update-applying", "")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .content(
                        IndeterminateProgressIndicator::new("titlebar-update-applying-spinner")
                            .size(13.0)
                            .text_color(titlebar_tokens.foreground),
                    )
                    .tooltip(format!("Preparing Nucleotide {}", update.version))
                    .aria_label(format!("Preparing Nucleotide {}", update.version))
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            UpdateState::Failed { .. } => Some(
                Button::icon_only("titlebar-update-failed", "icons/triangle-alert.svg")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .tooltip("Nucleotide update failed; open to retry")
                    .aria_label("Nucleotide update failed; open to retry")
                    .activate_on_mouse_down()
                    .on_click(|_, window, cx| Self::open_update_details(window, cx)),
            ),
            _ => None,
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .children(button)
    }
}
