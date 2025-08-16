// ABOUTME: Individual tab component for the tab bar with close button
// ABOUTME: Displays buffer name, modified indicator, and handles click events

use gpui::prelude::FluentBuilder;
use gpui::prelude::*;
use gpui::{
    div, px, App, CursorStyle, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseUpEvent, ParentElement, RenderOnce, SharedString, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::theme_manager::ThemedContext;
use nucleotide_ui::{
    compute_component_state, Button, ButtonSize, ButtonVariant, Component, ComponentFactory,
    ComponentState, Interactive, StyleVariant, Styled as UIStyled,
    ThemedContext as UIThemedContext, Tooltipped, VcsIndicator, VcsStatus,
};

/// Type alias for mouse event handlers in tabs
type MouseEventHandler = Box<dyn Fn(&MouseUpEvent, &mut Window, &mut App) + 'static>;

/// Tab variant for different tab states
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TabVariant {
    #[default]
    Default,
    Active,
    Modified,
    Pinned,
}

impl From<TabVariant> for StyleVariant {
    fn from(variant: TabVariant) -> Self {
        match variant {
            TabVariant::Default => StyleVariant::Secondary,
            TabVariant::Active => StyleVariant::Primary,
            TabVariant::Modified => StyleVariant::Warning,
            TabVariant::Pinned => StyleVariant::Info,
        }
    }
}

/// Tab size for different layouts
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TabSize {
    #[default]
    Medium,
    Small,
    Large,
}

/// A single tab in the tab bar
#[derive(IntoElement)]
pub struct Tab {
    /// Component identifier
    id: ElementId,
    /// Document ID this tab represents
    pub doc_id: DocumentId,
    /// Display label for the tab
    pub label: String,
    /// File path for determining icon
    pub file_path: Option<std::path::PathBuf>,
    /// Whether the document has unsaved changes
    pub is_modified: bool,
    /// Git status for VCS indicator
    pub git_status: Option<VcsStatus>,
    /// Whether this tab is currently active
    pub is_active: bool,
    /// Component variant
    variant: TabVariant,
    /// Component size
    size: TabSize,
    /// Disabled state
    disabled: bool,
    /// Tooltip text
    tooltip: Option<SharedString>,
    /// Callback when tab is clicked
    pub on_click: MouseEventHandler,
    /// Callback when close button is clicked
    pub on_close: MouseEventHandler,
}

impl Tab {
    pub fn new(
        doc_id: DocumentId,
        label: String,
        file_path: Option<std::path::PathBuf>,
        is_modified: bool,
        git_status: Option<VcsStatus>,
        is_active: bool,
        on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
        on_close: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let id = ElementId::from(SharedString::from(format!("tab-{}", doc_id)));
        let variant = if is_active {
            TabVariant::Active
        } else if is_modified {
            TabVariant::Modified
        } else {
            TabVariant::Default
        };

        Self {
            id,
            doc_id,
            label,
            file_path,
            is_modified,
            git_status,
            is_active,
            variant,
            size: TabSize::Medium,
            disabled: false,
            tooltip: None,
            on_click: Box::new(on_click),
            on_close: Box::new(on_close),
        }
    }

    /// Get the component state based on current flags
    fn component_state(&self) -> ComponentState {
        compute_component_state(
            self.disabled,
            false, // loading
            false, // focused (handled by GPUI)
            false, // hovered (handled by GPUI)
            self.is_active,
        )
    }
}

// Implement nucleotide-ui Component trait
impl Component for Tab {
    fn id(&self) -> &ElementId {
        &self.id
    }

    fn with_id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    fn is_disabled(&self) -> bool {
        self.disabled
    }

    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

// Implement nucleotide-ui Styled trait
impl UIStyled for Tab {
    type Variant = TabVariant;
    type Size = TabSize;

    fn variant(&self) -> &Self::Variant {
        &self.variant
    }

    fn with_variant(mut self, variant: Self::Variant) -> Self {
        self.variant = variant;
        self
    }

    fn size(&self) -> &Self::Size {
        &self.size
    }

    fn with_size(mut self, size: Self::Size) -> Self {
        self.size = size;
        self
    }
}

// Implement nucleotide-ui Interactive trait
impl Interactive for Tab {
    type ClickHandler = MouseEventHandler;

    fn on_click(mut self, handler: Self::ClickHandler) -> Self {
        self.on_click = handler;
        self
    }

    fn on_secondary_click(self, _handler: Self::ClickHandler) -> Self {
        // Not implemented for tabs
        self
    }

    fn is_focusable(&self) -> bool {
        !self.disabled
    }
}

// Implement nucleotide-ui Tooltipped trait
impl Tooltipped for Tab {
    fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    fn get_tooltip(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }
}

// Implement ComponentFactory trait
impl ComponentFactory for Tab {
    fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            doc_id: DocumentId::default(),
            label: String::new(),
            file_path: None,
            is_modified: false,
            git_status: None,
            is_active: false,
            variant: TabVariant::Default,
            size: TabSize::Medium,
            disabled: false,
            tooltip: None,
            on_click: Box::new(|_, _, _| {}),
            on_close: Box::new(|_, _, _| {}),
        }
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use ThemedContext trait for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use provider hooks to get configuration for animations
        let enable_animations = nucleotide_ui::providers::use_provider::<
            nucleotide_ui::providers::ConfigurationProvider,
        >()
        .map(|config| config.ui_config.animation_config.enable_animations)
        .unwrap_or(true);

        // Compute component styles using nucleotide-ui styling system
        let component_state = self.component_state();
        let style_variant: StyleVariant = self.variant.into();

        // Use design tokens for consistent theming
        let (bg_color, text_color, hover_bg, border_color) = match component_state {
            ComponentState::Active => (
                tokens.colors.background, // Active tab matches editor background
                tokens.colors.text_primary,
                tokens.colors.background,
                tokens.colors.border_default,
            ),
            ComponentState::Disabled => (
                tokens.colors.surface_disabled,
                tokens.colors.text_disabled,
                tokens.colors.surface_disabled,
                tokens.colors.border_muted,
            ),
            _ => {
                let bg = if self.is_modified {
                    tokens.colors.surface_selected
                } else {
                    tokens.colors.surface
                };
                let hover_bg = tokens.colors.surface_hover;
                (
                    bg,
                    tokens.colors.text_primary,
                    hover_bg,
                    tokens.colors.border_default,
                )
            }
        };

        // Extract values we need before moving self
        let git_status = self.git_status.clone();
        let height = match self.size {
            TabSize::Small => tokens.sizes.button_height_sm,
            TabSize::Medium => px(32.0),
            TabSize::Large => tokens.sizes.button_height_lg,
        };
        let padding = tokens.sizes.space_4;

        // Build the tab using design tokens
        div()
            .id(self.id.clone())
            .flex()
            .flex_none() // Don't grow or shrink
            .items_center()
            .pl(padding)
            .pr(tokens.sizes.space_1)
            .h(height)
            .min_w(px(120.0)) // Minimum width to ensure readability
            .bg(bg_color)
            .when(enable_animations && !self.disabled, |tab| {
                tab.hover(|style| style.bg(hover_bg))
            })
            .when(!self.disabled, |tab| tab.cursor(CursorStyle::PointingHand))
            .border_r_1()
            .border_color(border_color)
            .when(self.is_active, |this| {
                // Active tabs: no bottom border for seamless integration with editor
                this
            })
            .when(!self.is_active, |this| {
                // Inactive tabs get bottom border to separate from editor/active content
                this.border_b_1().border_color(border_color)
            })
            .when(!self.disabled, |tab| {
                tab.on_mouse_up(MouseButton::Left, {
                    let on_click = self.on_click;
                    move |event, window, cx| {
                        on_click(event, window, cx);
                        cx.stop_propagation();
                    }
                })
            })
            // TODO: Implement tooltip when GPUI tooltip API is clarified
            // .when_some(self.tooltip.as_ref(), |tab, tooltip| {
            //     tab.tooltip(...)
            // })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens.sizes.space_1)
                    .child(
                        // File icon with VCS overlay
                        div()
                            .relative() // Needed for absolute positioning of the overlay
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(ref path) = self.file_path {
                                nucleotide_ui::FileIcon::from_path(path, false)
                                    .size(16.0)
                                    .text_color(text_color)
                            } else {
                                nucleotide_ui::FileIcon::scratch()
                                    .size(16.0)
                                    .text_color(text_color)
                            })
                            .when_some(git_status.as_ref(), |div, status| {
                                let indicator =
                                    VcsIndicator::new(status.clone()).size(8.0).overlay();
                                div.child(indicator)
                            }),
                    )
                    .child(
                        // Tab label
                        div()
                            .text_color(text_color)
                            .text_size(tokens.sizes.text_md)
                            .when(self.is_active, |this| {
                                // Active tab labels are slightly bolder/more prominent
                                this.font_weight(gpui::FontWeight::MEDIUM)
                            })
                            .when(self.is_modified, |this| {
                                // Modified files show with underline
                                this.underline()
                            })
                            .child(self.label.clone()),
                    )
                    .child(
                        // Close button
                        div().ml(tokens.sizes.space_1).child(
                            Button::icon_only("tab-close", "icons/close.svg")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Small)
                                .disabled(self.disabled)
                                .on_click({
                                    let on_close = self.on_close;
                                    move |event, window, cx| {
                                        on_close(event, window, cx);
                                        cx.stop_propagation();
                                    }
                                }),
                        ),
                    ),
            )
    }
}
