// ABOUTME: Component gallery view for exercising nucleotide-ui primitives.
// ABOUTME: Provides an interactive storybook-style surface backed by real GPUI components.

use gpui::{
    App, AppContext, Context, DismissEvent, Element, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window, div, point, px,
};
use nucleotide_types::VcsStatus;

use crate::modal_layer::ModalView;
use crate::{
    AppShell, BottomPanel, Button, ButtonSize, ButtonVariant, Checkbox, CheckboxSize,
    ConfirmDialog, ConfirmDialogView, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
    EditorPaneGrid, FileIcon, IndeterminateProgressIndicator, ListItem, ListItemSpacing,
    ListItemState, ListItemVariant, Navigable, NavigableEntry, OverlaySurface, Panel, PanelVariant,
    PopupMenuSurface, StatusBar, TextInput, Toolbar, VcsIcon,
};

pub struct ComponentGallery {
    primary_focus: FocusHandle,
    secondary_focus: FocusHandle,
    danger_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    selected_list_index: usize,
    click_count: usize,
    checkbox_checked: bool,
    navigable_entries: Vec<NavigableEntry>,
    scroll_handle: ScrollHandle,
    text_input: Entity<TextInput>,
    confirm_dialog: Entity<ConfirmDialogView>,
}

impl ComponentGallery {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text_input = cx.new(|cx| {
            TextInput::new("gallery-text-input", cx)
                .placeholder("Type here while debugging")
                .ghost_suffix("  Cmd-K")
        });
        let confirm_dialog = cx.new(|cx| {
            ConfirmDialogView::new(
                ConfirmDialog::new(
                    "Confirm Dialog",
                    "A live modal dialog sample with focusable footer actions.",
                    "Confirm",
                )
                .cancel_label("Cancel")
                .confirm_variant(ButtonVariant::Primary),
                cx,
            )
        });

        Self {
            primary_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            secondary_focus: cx.focus_handle().tab_index(2).tab_stop(true),
            danger_focus: cx.focus_handle().tab_index(3).tab_stop(true),
            checkbox_focus: cx.focus_handle().tab_index(4).tab_stop(true),
            selected_list_index: 0,
            click_count: 0,
            checkbox_checked: true,
            navigable_entries: vec![
                NavigableEntry::from_focus_handle(cx.focus_handle().tab_index(5).tab_stop(true)),
                NavigableEntry::from_focus_handle(cx.focus_handle().tab_index(6).tab_stop(true)),
                NavigableEntry::from_focus_handle(cx.focus_handle().tab_index(7).tab_stop(true)),
            ],
            scroll_handle: ScrollHandle::new(),
            text_input,
            confirm_dialog,
        }
    }

    pub fn selected_list_index(&self) -> usize {
        self.selected_list_index
    }

    pub fn click_count(&self) -> usize {
        self.click_count
    }

    fn increment_clicks(&mut self, cx: &mut Context<Self>) {
        self.click_count += 1;
        cx.notify();
    }

    fn select_list_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_list_index = index;
        cx.notify();
    }

    fn set_checkbox_checked(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.checkbox_checked = checked;
        cx.notify();
    }

    fn render_buttons(
        &self,
        tokens: &crate::DesignTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let variants = [
            (ButtonVariant::Primary, "Primary"),
            (ButtonVariant::Secondary, "Secondary"),
            (ButtonVariant::Ghost, "Ghost"),
            (ButtonVariant::Danger, "Danger"),
            (ButtonVariant::Success, "Success"),
            (ButtonVariant::Warning, "Warning"),
            (ButtonVariant::Info, "Info"),
        ];

        div()
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .children(
                        variants
                            .into_iter()
                            .enumerate()
                            .map(|(index, (variant, label))| {
                                let focus = match variant {
                                    ButtonVariant::Primary => Some(self.primary_focus.clone()),
                                    ButtonVariant::Secondary => Some(self.secondary_focus.clone()),
                                    ButtonVariant::Danger => Some(self.danger_focus.clone()),
                                    _ => None,
                                };
                                let mut button = Button::new(("gallery-button", index), label)
                                    .variant(variant)
                                    .size(ButtonSize::Small)
                                    .activate_on_mouse_down()
                                    .on_click(cx.listener(|gallery, _, _, cx| {
                                        gallery.increment_clicks(cx);
                                    }));
                                if let Some(focus) = focus {
                                    button = button.focus_handle(focus);
                                }
                                button
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .child(
                        Button::new("gallery-icon-button", "Save")
                            .icon("icons/file.svg")
                            .activate_on_mouse_down()
                            .on_click(cx.listener(|gallery, _, _, cx| {
                                gallery.increment_clicks(cx);
                            })),
                    )
                    .child(
                        Button::icon_only("gallery-icon-only-button", "icons/search.svg")
                            .activate_on_mouse_down()
                            .on_click(cx.listener(|gallery, _, _, cx| {
                                gallery.increment_clicks(cx);
                            })),
                    )
                    .child(Button::new("gallery-loading-button", "Loading").loading(true))
                    .child(Button::new("gallery-disabled-button", "Disabled").disabled(true)),
            )
            .child(section_note(
                format!("Button interactions recorded: {}", self.click_count),
                tokens,
            ))
            .into_any()
    }

    fn render_inputs(
        &self,
        tokens: &crate::DesignTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let checked = self.checkbox_checked;
        let gallery = cx.entity().clone();
        let small_gallery = gallery.clone();
        div()
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3)
            .child(self.text_input.clone())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens.sizes.space_3)
                    .child(
                        Checkbox::new("gallery-checkbox", "Enable sample option")
                            .checked(checked)
                            .focus_handle(self.checkbox_focus.clone())
                            .on_change(move |checked, _window, cx| {
                                gallery.update(cx, |gallery, cx| {
                                    gallery.set_checkbox_checked(checked, cx);
                                });
                            }),
                    )
                    .child(
                        Checkbox::new("gallery-checkbox-small", "Small")
                            .checked(!checked)
                            .size(CheckboxSize::Small)
                            .on_change(move |checked, _window, cx| {
                                small_gallery.update(cx, |gallery, cx| {
                                    gallery.set_checkbox_checked(!checked, cx);
                                });
                            }),
                    )
                    .child(Checkbox::new("gallery-checkbox-disabled", "Disabled").disabled(true)),
            )
            .into_any()
    }

    fn render_lists(
        &self,
        tokens: &crate::DesignTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = [
            (
                ListItemVariant::Default,
                "Default row",
                "icons/file-text.svg",
            ),
            (ListItemVariant::Primary, "Primary row", "icons/plus.svg"),
            (
                ListItemVariant::Success,
                "Success row",
                "icons/square-check-big.svg",
            ),
            (
                ListItemVariant::Warning,
                "Warning row",
                "icons/triangle-alert.svg",
            ),
            (ListItemVariant::Danger, "Danger row", "icons/circle-x.svg"),
        ];

        let gallery = cx.entity().clone();
        let list_items = rows
            .into_iter()
            .enumerate()
            .map(|(index, (variant, label, icon))| {
                let gallery = gallery.clone();
                ListItem::new(("gallery-list-item", index))
                    .variant(variant)
                    .spacing(if index % 2 == 0 {
                        ListItemSpacing::Default
                    } else {
                        ListItemSpacing::Compact
                    })
                    .selected(self.selected_list_index == index)
                    .start_slot(gpui::svg().path(icon).size(tokens.sizes.text_md))
                    .end_slot(
                        div()
                            .text_size(tokens.sizes.text_xs)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(format!("#{}", index + 1)),
                    )
                    .child(label)
                    .with_listener(move |element: Stateful<gpui::Div>| {
                        element.on_click(move |_, _, cx| {
                            gallery.update(cx, |gallery, cx| {
                                gallery.select_list_index(index, cx);
                            });
                        })
                    })
            });

        let navigable_list = Navigable::new(
            div().flex().flex_col().gap(tokens.sizes.space_1).children(
                self.navigable_entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        div()
                            .id(("component-gallery-navigable-entry", index))
                            .track_focus(&entry.focus_handle)
                            .tab_stop(true)
                            .h(tokens.sizes.space_7)
                            .px(tokens.sizes.space_2)
                            .flex()
                            .items_center()
                            .rounded(tokens.sizes.radius_sm)
                            .bg(tokens.chrome.surface)
                            .focus(|this| {
                                this.bg(tokens.chrome.surface_elevated)
                                    .border_1()
                                    .border_color(tokens.editor.focus_ring)
                            })
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_on_chrome)
                            .child(format!("Focusable row {}", index + 1))
                    }),
            ),
        )
        .entries(self.navigable_entries.clone());

        div()
            .grid()
            .grid_cols(2)
            .gap(tokens.sizes.space_3)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens.sizes.space_1)
                    .children(list_items),
            )
            .child(navigable_list)
            .into_any()
    }

    fn render_icons(&self, tokens: &crate::DesignTokens) -> gpui::AnyElement {
        let icon_row = |label: &'static str, icon: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(tokens.sizes.space_2)
                .child(
                    div()
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon),
                )
                .child(
                    div()
                        .text_size(tokens.sizes.text_sm)
                        .text_color(tokens.chrome.text_on_chrome)
                        .child(label),
                )
        };

        div()
            .grid()
            .grid_cols(3)
            .gap(tokens.sizes.space_3)
            .child(icon_row(
                "Rust",
                FileIcon::from_extension(Some("rs"))
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .into_any_element(),
            ))
            .child(icon_row(
                "Markdown",
                FileIcon::from_extension(Some("md"))
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .into_any_element(),
            ))
            .child(icon_row(
                "Folder",
                FileIcon::directory(false)
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .into_any_element(),
            ))
            .child(icon_row(
                "Modified",
                VcsIcon::from_extension(Some("toml"))
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .with_vcs_status(VcsStatus::Modified)
                    .into_any_element(),
            ))
            .child(icon_row(
                "Added",
                VcsIcon::from_extension(Some("json"))
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .with_vcs_status(VcsStatus::Added)
                    .into_any_element(),
            ))
            .child(icon_row(
                "Conflict",
                VcsIcon::from_extension(Some("rs"))
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .with_vcs_status(VcsStatus::Conflicted)
                    .into_any_element(),
            ))
            .into_any()
    }

    fn render_component_index(&self, tokens: &crate::DesignTokens) -> gpui::AnyElement {
        div()
            .grid()
            .grid_cols(4)
            .gap(tokens.sizes.space_2)
            .children(crate::BUILT_IN_COMPONENTS.iter().map(|component| {
                div()
                    .px(tokens.sizes.space_2)
                    .py(tokens.sizes.space_1)
                    .rounded(tokens.sizes.radius_sm)
                    .border_1()
                    .border_color(tokens.chrome.border_muted)
                    .bg(tokens.chrome.surface_elevated)
                    .text_size(tokens.sizes.text_sm)
                    .text_color(tokens.chrome.text_on_chrome)
                    .child(*component)
            }))
            .into_any()
    }

    fn render_surfaces(&self, tokens: &crate::DesignTokens) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3)
            .child(
                div().h(px(74.0)).child(
                    EditorPaneGrid::new("component-gallery-editor-pane-grid").child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child("EditorPaneGrid"),
                    ),
                ),
            )
            .child(
                BottomPanel::new("component-gallery-bottom-panel")
                    .height(px(44.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .px(tokens.sizes.space_3)
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child("BottomPanel chrome"),
                    ),
            )
            .child(
                div().relative().h(px(96.0)).overflow_hidden().child(
                    PopupMenuSurface::new(
                        div()
                            .w(px(220.0))
                            .child(
                                ListItem::new("gallery-popup-item-1").child("Popup menu surface"),
                            )
                            .child(
                                ListItem::new("gallery-popup-item-2")
                                    .state(ListItemState::Selected)
                                    .child("Selected item"),
                            ),
                    )
                    .position(point(px(8.0), px(8.0))),
                ),
            )
            .child(
                div().relative().h(px(72.0)).overflow_hidden().child(
                    OverlaySurface::new()
                        .top(px(8.0))
                        .without_key_context()
                        .child(
                            div()
                                .p(tokens.sizes.space_2)
                                .rounded(tokens.sizes.radius_sm)
                                .bg(tokens.chrome.surface_elevated)
                                .text_size(tokens.sizes.text_sm)
                                .text_color(tokens.chrome.text_on_chrome)
                                .child("OverlaySurface content"),
                        ),
                ),
            )
            .into_any()
    }
}

fn section_title(title: impl Into<SharedString>, tokens: &crate::DesignTokens) -> gpui::AnyElement {
    div()
        .text_size(tokens.sizes.text_md)
        .font_weight(FontWeight::SEMIBOLD)
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

fn gallery_panel(
    id: &'static str,
    title: &'static str,
    note: &'static str,
    tokens: &crate::DesignTokens,
    child: impl IntoElement,
) -> gpui::AnyElement {
    div()
        .flex_none()
        .child(
            Panel::new(id)
                .variant(PanelVariant::Surface)
                .child(section_title(title, tokens))
                .child(section_note(note, tokens))
                .child(div().mt(tokens.sizes.space_3).child(child)),
        )
        .into_any()
}

impl Render for ComponentGallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.global::<crate::Theme>().tokens.clone();

        div()
            .w(px(980.0))
            .max_w(px(980.0))
            .h(px(720.0))
            .max_h(px(720.0))
            .rounded(tokens.sizes.radius_lg)
            .border_1()
            .border_color(tokens.chrome.border_default)
            .shadow(vec![tokens.chrome.shadow_lg.to_box_shadow(false)])
            .overflow_hidden()
            .child(
                AppShell::new("component-gallery")
                    .header(
                        Toolbar::new("component-gallery-toolbar")
                            .label("Component Gallery")
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(tokens.sizes.text_sm)
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .child("Interactive nucleotide-ui debug surface"),
                            )
                            .child(IndeterminateProgressIndicator::new("gallery-toolbar-spinner")),
                    )
                    .child(
                        div()
                            .size_full()
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("component-gallery-scroll")
                                    .size_full()
                                    .min_h(px(0.0))
                                    .p(tokens.sizes.space_4)
                                    .overflow_y_scroll()
                                    .track_scroll(&self.scroll_handle)
                                    .flex()
                                    .flex_col()
                                    .gap(tokens.sizes.space_4)
                                    .child(gallery_panel(
                                        "component-gallery-index-panel",
                                        "Component Index",
                                        "Every built-in nucleotide-ui component registered by the crate.",
                                        &tokens,
                                        self.render_component_index(&tokens),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-input-panel",
                                        "Inputs",
                                        "TextInput and Checkbox components with live state.",
                                        &tokens,
                                        self.render_inputs(&tokens, cx),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-button-panel",
                                        "Buttons",
                                        "Button variants, sizes, icon states, disabled and loading states.",
                                        &tokens,
                                        self.render_buttons(&tokens, cx),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-list-panel",
                                        "Lists and Focus",
                                        "ListItem variants plus Navigable keyboard focus rows.",
                                        &tokens,
                                        self.render_lists(&tokens, cx),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-icon-panel",
                                        "File and VCS Icons",
                                        "FileIcon and VcsIcon status overlays.",
                                        &tokens,
                                        self.render_icons(&tokens),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-dialog-panel",
                                        "Dialogs",
                                        "Dialog header, description, footer, and ConfirmDialogView.",
                                        &tokens,
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(tokens.sizes.space_3)
                                            .child(
                                                Panel::new("component-gallery-dialog-parts")
                                                    .variant(PanelVariant::Elevated)
                                                    .child(
                                                        DialogHeader::new()
                                                            .child(DialogTitle::new().child("DialogTitle"))
                                                            .child(DialogDescription::new().child(
                                                                "DialogDescription text uses the same modal typography.",
                                                            )),
                                                    )
                                                    .child(
                                                        DialogFooter::new()
                                                            .child(Button::new("gallery-dialog-cancel", "Cancel"))
                                                            .child(Button::new("gallery-dialog-confirm", "Confirm").variant(ButtonVariant::Primary)),
                                                    ),
                                            )
                                            .child(self.confirm_dialog.clone()),
                                    ))
                                    .child(gallery_panel(
                                        "component-gallery-layout-panel",
                                        "Layout and Overlays",
                                        "AppShell, Toolbar, StatusBar, Panel, EditorPaneGrid, BottomPanel, PopupMenuSurface, and OverlaySurface.",
                                        &tokens,
                                        self.render_surfaces(&tokens),
                                    )),
                            ),
                    )
                    .footer(
                        StatusBar::new("component-gallery-status")
                            .child(format!("Selected list item: {}", self.selected_list_index + 1))
                            .child("Esc closes"),
                    ),
            )
    }
}

impl EventEmitter<DismissEvent> for ComponentGallery {}

impl Focusable for ComponentGallery {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_input.focus_handle(cx)
    }
}

impl ModalView for ComponentGallery {}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn init_gallery_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx, None);
            crate::text_input::init(cx);
            crate::confirm_dialog::init(cx);
            crate::modal_layer::init(cx);
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    #[gpui::test]
    fn component_gallery_renders(cx: &mut TestAppContext) {
        init_gallery_test(cx);
        let (gallery, _cx) = cx.add_window_view(|_window, cx| ComponentGallery::new(cx));

        gallery.read_with(cx, |gallery, _| {
            assert!(gallery.primary_focus.tab_stop);
            assert!(gallery.secondary_focus.tab_stop);
            assert!(gallery.danger_focus.tab_stop);
            assert!(gallery.checkbox_focus.tab_stop);
            assert_eq!(gallery.click_count(), 0);
            assert_eq!(gallery.selected_list_index(), 0);
        });
    }
}
