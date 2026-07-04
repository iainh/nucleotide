// ABOUTME: Component gallery view for exercising nucleotide-ui primitives.
// ABOUTME: Provides a small storybook-style surface backed by real GPUI components.

use gpui::{
    AppContext, Context, Element, Entity, FocusHandle, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};

use crate::{
    Button, ButtonSize, ButtonVariant, Panel, PanelVariant, StatusBar, TextInput, Toolbar,
    WorkspaceChrome,
};

pub struct ComponentGallery {
    primary_focus: FocusHandle,
    secondary_focus: FocusHandle,
    danger_focus: FocusHandle,
    text_input: Entity<TextInput>,
}

impl ComponentGallery {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text_input = cx.new(|cx| {
            TextInput::new("gallery-text-input", cx)
                .placeholder("Filter components")
                .ghost_suffix("  Cmd-K")
        });

        Self {
            primary_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            secondary_focus: cx.focus_handle().tab_index(2).tab_stop(true),
            danger_focus: cx.focus_handle().tab_index(3).tab_stop(true),
            text_input,
        }
    }
}

fn section_title(title: impl Into<SharedString>, tokens: &crate::DesignTokens) -> gpui::AnyElement {
    div()
        .text_size(tokens.sizes.text_md)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(tokens.chrome.text_on_chrome)
        .child(title.into())
        .into_any()
}

fn section_note(note: impl Into<SharedString>, tokens: &crate::DesignTokens) -> gpui::AnyElement {
    div()
        .text_size(tokens.sizes.text_sm)
        .text_color(tokens.chrome.text_chrome_secondary)
        .child(note.into())
        .into_any()
}

impl Render for ComponentGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = &cx.global::<crate::Theme>().tokens;

        let button_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens.sizes.space_2)
            .child(
                Button::new("gallery-primary-button", "Primary")
                    .variant(ButtonVariant::Primary)
                    .size(ButtonSize::Small)
                    .focus_handle(self.primary_focus.clone())
                    .on_click(|_, _, _| {}),
            )
            .child(
                Button::new("gallery-secondary-button", "Secondary")
                    .variant(ButtonVariant::Secondary)
                    .size(ButtonSize::Small)
                    .focus_handle(self.secondary_focus.clone())
                    .on_click(|_, _, _| {}),
            )
            .child(
                Button::new("gallery-danger-button", "Danger")
                    .variant(ButtonVariant::Danger)
                    .size(ButtonSize::Small)
                    .focus_handle(self.danger_focus.clone())
                    .on_click(|_, _, _| {}),
            );

        WorkspaceChrome::new("component-gallery")
            .child(
                Toolbar::new("component-gallery-toolbar")
                    .label("Component Gallery")
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child("nucleotide-ui"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .p(tokens.sizes.space_4)
                    .flex()
                    .flex_col()
                    .gap(tokens.sizes.space_4)
                    .child(
                        Panel::new("component-gallery-input-panel")
                            .variant(PanelVariant::Surface)
                            .child(section_title("Text Input", tokens))
                            .child(section_note(
                                "Shared editing, selection, clipboard, and IME path.",
                                tokens,
                            ))
                            .child(
                                div()
                                    .mt(tokens.sizes.space_3)
                                    .child(self.text_input.clone()),
                            ),
                    )
                    .child(
                        Panel::new("component-gallery-button-panel")
                            .variant(PanelVariant::Elevated)
                            .child(section_title("Buttons", tokens))
                            .child(section_note(
                                "Focusable, token-styled, keyboard-activatable controls.",
                                tokens,
                            ))
                            .child(div().mt(tokens.sizes.space_3).child(button_row)),
                    )
                    .child(
                        Panel::new("component-gallery-layout-panel")
                            .variant(PanelVariant::Transparent)
                            .border(false)
                            .child(section_title("Layout", tokens))
                            .child(section_note(
                                "WorkspaceChrome, Toolbar, Panel, and StatusBar wrappers.",
                                tokens,
                            )),
                    ),
            )
            .child(StatusBar::new("component-gallery-status").child("Ready"))
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn init_gallery_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::text_input::init(cx);
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    #[gpui::test]
    fn component_gallery_renders(cx: &mut TestAppContext) {
        init_gallery_test(cx);
        let (gallery, cx) = cx.add_window_view(|_window, cx| ComponentGallery::new(cx));

        gallery.read_with(cx, |gallery, _| {
            assert!(gallery.primary_focus.tab_stop);
            assert!(gallery.secondary_focus.tab_stop);
            assert!(gallery.danger_focus.tab_stop);
        });
    }
}
