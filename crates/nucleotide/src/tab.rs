// ABOUTME: Individual tab component for the tab bar with close button
// ABOUTME: Displays buffer name, modified indicator, and handles click events

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, Context, CursorStyle, Div, ElementId, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, RenderOnce, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window, div, px, svg,
};
use helix_core::diagnostic::Severity as DiagnosticSeverity;
use helix_view::DocumentId;
use nucleotide_types::VcsStatus;
use nucleotide_ui::ThemedContext;
use nucleotide_ui::{
    Component, ComponentFactory, ComponentState, Interactive, StyleVariant, Styled as UIStyled,
    Tooltipped, VcsIcon, click_event_from_mouse_down, compute_component_state,
};
use std::cmp::Ordering;
use std::sync::Arc;

use crate::config::{TabCloseButtonVisibility, TabClosePosition};

/// Type alias for mouse event handlers in tabs
type MouseEventHandler = Arc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type MouseDownEventHandler = Arc<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

struct TabTooltip {
    text: SharedString,
}

struct TabMetaTooltip {
    title: SharedString,
    detail: SharedString,
}

impl Render for TabTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().tokens;
        let tooltip_tokens = tokens.tooltip_tokens();

        div()
            .max_w(px(420.0))
            .px(tokens.sizes.space_2)
            .py(tokens.sizes.space_1)
            .rounded(tokens.sizes.radius_sm)
            .border_1()
            .border_color(tooltip_tokens.border)
            .bg(tooltip_tokens.background)
            .shadow(vec![tokens.chrome.shadow_md.to_box_shadow(false)])
            .text_size(tokens.sizes.text_sm)
            .text_color(tooltip_tokens.text)
            .child(self.text.clone())
    }
}

impl Render for TabMetaTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().tokens;
        let tooltip_tokens = tokens.tooltip_tokens();

        div()
            .max_w(px(420.0))
            .px(tokens.sizes.space_2)
            .py(tokens.sizes.space_1)
            .rounded(tokens.sizes.radius_sm)
            .border_1()
            .border_color(tooltip_tokens.border)
            .bg(tooltip_tokens.background)
            .shadow(vec![tokens.chrome.shadow_md.to_box_shadow(false)])
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_1)
            .child(
                div()
                    .text_size(tokens.sizes.text_sm)
                    .text_color(tooltip_tokens.text)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_size(tokens.sizes.text_xs)
                    .text_color(tokens.tab_bar_tokens().tab_text_inactive)
                    .child(self.detail.clone()),
            )
    }
}

struct TabEndButtonProps {
    doc_id: TabId,
    is_pinned: bool,
    disabled: bool,
    text_color: gpui::Hsla,
    tab_hover_group: SharedString,
    close_button_visibility: TabCloseButtonVisibility,
}

const START_TAB_SLOT_SIZE: f32 = 12.0;
const END_TAB_SLOT_SIZE: f32 = 14.0;
const TAB_SLOT_ICON_SIZE: f32 = 12.0;
const TAB_MIN_WIDTH: f32 = 112.0;
const TAB_MAX_WIDTH: f32 = 280.0;

pub(crate) fn tab_container_height(tokens: nucleotide_ui::tokens::DesignTokens) -> gpui::Pixels {
    // Zed tabs use DynamicSpacing::Base32 for the tab container height.
    tokens.sizes.space_8
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabContentIcon {
    File,
    Readonly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabId {
    Document(DocumentId),
    Image(u64),
}

impl From<DocumentId> for TabId {
    fn from(doc_id: DocumentId) -> Self {
        Self::Document(doc_id)
    }
}

impl PartialEq<DocumentId> for TabId {
    fn eq(&self, other: &DocumentId) -> bool {
        matches!(self, Self::Document(doc_id) if doc_id == other)
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Document(doc_id) => write!(f, "{doc_id}"),
            Self::Image(id) => write!(f, "image-{id}"),
        }
    }
}

/// Position of a tab relative to its siblings and the active tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TabPosition {
    #[default]
    First,
    Middle(Ordering),
    Last,
}

/// A single tab in the tab bar
#[derive(IntoElement)]
pub struct Tab {
    div: Stateful<Div>,
    /// Component identifier
    id: ElementId,
    /// Tab ID this tab represents
    pub doc_id: TabId,
    /// Display label for the tab
    pub label: String,
    /// Optional parent-path detail shown after the primary label
    pub label_detail: Option<String>,
    /// File path for determining icon
    pub file_path: Option<std::path::PathBuf>,
    /// Whether the document has unsaved changes
    pub is_modified: bool,
    /// Whether the backing document is read-only on disk
    pub is_readonly: bool,
    /// Whether the backing file was removed from disk after being opened
    pub is_deleted: bool,
    /// Whether the document is pinned in the tab strip
    pub is_pinned: bool,
    /// Whether the document is an active preview tab
    pub is_preview: bool,
    /// Git status for VCS indicator
    pub git_status: Option<VcsStatus>,
    /// Highest-priority diagnostic severity for the tab icon decoration
    pub diagnostic_severity: Option<DiagnosticSeverity>,
    /// Whether this tab is currently active
    pub is_active: bool,
    /// Component variant
    variant: TabVariant,
    /// Component size
    size: TabSize,
    /// Position in the tab strip for border treatment
    position: TabPosition,
    /// Close button visibility mode for unpinned tabs
    close_button_visibility: TabCloseButtonVisibility,
    /// Close or pin button position within the tab
    close_position: TabClosePosition,
    /// Whether to render file icons in the tab label area
    show_file_icons: bool,
    /// Whether tab text should be deemphasized because the editor pane is not focused
    deemphasized: bool,
    /// Disabled state
    disabled: bool,
    /// Tooltip text
    tooltip: Option<SharedString>,
    /// Callback when tab is clicked
    pub on_click: MouseEventHandler,
    /// Callback when close button is clicked
    pub on_close: MouseEventHandler,
    /// Callback when a pinned tab's pin button is clicked
    on_toggle_pin: Option<MouseEventHandler>,
    /// Callback when a read-only tab's lock icon is clicked
    on_toggle_readonly: Option<MouseEventHandler>,
    /// Callback when context menu is requested
    on_context_menu: Option<MouseDownEventHandler>,
}

impl Tab {
    #[inline]
    fn element_id_for(doc_id: TabId) -> ElementId {
        ElementId::from(SharedString::from(format!("tab-{}", doc_id)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        doc_id: impl Into<TabId>,
        label: String,
        file_path: Option<std::path::PathBuf>,
        is_modified: bool,
        is_pinned: bool,
        git_status: Option<VcsStatus>,
        diagnostic_severity: Option<DiagnosticSeverity>,
        is_active: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let doc_id = doc_id.into();
        let id = Self::element_id_for(doc_id);
        let variant = if is_active {
            TabVariant::Active
        } else if is_pinned {
            TabVariant::Pinned
        } else if is_modified {
            TabVariant::Modified
        } else {
            TabVariant::Default
        };

        Self {
            div: div().id(id.clone()),
            id,
            doc_id,
            label,
            label_detail: None,
            file_path,
            is_modified,
            is_readonly: false,
            is_deleted: false,
            is_pinned,
            is_preview: false,
            git_status,
            diagnostic_severity,
            is_active,
            variant,
            size: TabSize::Medium,
            position: TabPosition::First,
            close_button_visibility: TabCloseButtonVisibility::default(),
            close_position: TabClosePosition::default(),
            show_file_icons: true,
            deemphasized: false,
            disabled: false,
            tooltip: None,
            on_click: Arc::new(on_click),
            on_close: Arc::new(on_close),
            on_toggle_pin: None,
            on_toggle_readonly: None,
            on_context_menu: None,
        }
    }

    pub fn with_position(mut self, position: TabPosition) -> Self {
        self.position = position;
        self
    }

    pub fn with_close_button_visibility(mut self, visibility: TabCloseButtonVisibility) -> Self {
        self.close_button_visibility = visibility;
        self
    }

    pub fn with_close_position(mut self, position: TabClosePosition) -> Self {
        self.close_position = position;
        self
    }

    pub fn show_file_icons(mut self, show: bool) -> Self {
        self.show_file_icons = show;
        self
    }

    pub fn detail(mut self, detail: Option<String>) -> Self {
        self.label_detail = detail;
        self
    }

    pub fn preview(mut self, preview: bool) -> Self {
        self.is_preview = preview;
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.is_readonly = readonly;
        self
    }

    pub fn deleted(mut self, deleted: bool) -> Self {
        self.is_deleted = deleted;
        self
    }

    pub fn deemphasized(mut self, deemphasized: bool) -> Self {
        self.deemphasized = deemphasized;
        self
    }

    #[cfg(test)]
    pub(crate) fn close_button_visibility(&self) -> TabCloseButtonVisibility {
        self.close_button_visibility
    }

    #[cfg(test)]
    pub(crate) fn close_position(&self) -> TabClosePosition {
        self.close_position
    }

    #[cfg(test)]
    pub(crate) fn file_icons_visible(&self) -> bool {
        self.show_file_icons
    }

    #[cfg(test)]
    pub(crate) fn is_preview(&self) -> bool {
        self.is_preview
    }

    #[cfg(test)]
    pub(crate) fn is_readonly(&self) -> bool {
        self.is_readonly
    }

    #[cfg(test)]
    pub(crate) fn has_readonly_toggle_handler(&self) -> bool {
        self.on_toggle_readonly.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    #[cfg(test)]
    pub(crate) fn tooltip_text(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn label_detail(&self) -> Option<&str> {
        self.label_detail.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn is_deemphasized(&self) -> bool {
        self.deemphasized
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_severity(&self) -> Option<DiagnosticSeverity> {
        self.diagnostic_severity
    }

    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Arc::new(handler));
        self
    }

    pub fn on_toggle_pin(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_pin = Some(Arc::new(handler));
        self
    }

    pub fn on_toggle_readonly(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_readonly = Some(Arc::new(handler));
        self
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
        let id = id.into();
        Self {
            div: div().id(id.clone()),
            id,
            doc_id: DocumentId::default().into(),
            label: String::new(),
            label_detail: None,
            file_path: None,
            is_modified: false,
            is_readonly: false,
            is_deleted: false,
            is_pinned: false,
            is_preview: false,
            git_status: None,
            diagnostic_severity: None,
            is_active: false,
            variant: TabVariant::Default,
            size: TabSize::Medium,
            position: TabPosition::First,
            close_button_visibility: TabCloseButtonVisibility::default(),
            close_position: TabClosePosition::default(),
            show_file_icons: true,
            deemphasized: false,
            disabled: false,
            tooltip: None,
            on_click: Arc::new(|_, _, _| {}),
            on_close: Arc::new(|_, _, _| {}),
            on_toggle_pin: None,
            on_toggle_readonly: None,
            on_context_menu: None,
        }
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use ThemedContext trait for consistent theme access
        let theme = cx.theme();
        let tokens = theme.tokens; // DesignTokens is Copy

        // Use provider hooks to get configuration for animations
        let enable_animations = nucleotide_ui::providers::use_provider::<
            nucleotide_ui::providers::ConfigurationProvider,
        >()
        .map(|config| config.ui_config.animation_config.enable_animations)
        .unwrap_or(true);

        // Compute component styles using nucleotide-ui styling system
        let component_state = self.component_state();
        let _style_variant: StyleVariant = self.variant.into();

        // Use TabBarTokens for consistent hybrid color theming
        let tab_tokens = tokens.tab_bar_tokens();
        let inactive_bg = Tab::inactive_background_color(tab_tokens);
        let inactive_hover_bg = Tab::inactive_hover_background_color(tab_tokens);
        let (bg_color, text_color, hover_bg, border_color) = match component_state {
            ComponentState::Active => (
                tab_tokens.tab_active_background,
                tab_tokens.tab_text_active,
                tab_tokens.tab_active_background, // No hover change for active tabs
                tab_tokens.tab_border,
            ),
            ComponentState::Disabled => (
                nucleotide_ui::styling::ColorTheory::with_alpha(inactive_bg, 0.5),
                nucleotide_ui::styling::ColorTheory::with_alpha(tab_tokens.tab_text_inactive, 0.5),
                inactive_bg, // No hover for disabled tabs
                tab_tokens.tab_border,
            ),
            _ => (
                inactive_bg,
                tab_tokens.tab_text_inactive,
                inactive_hover_bg,
                tab_tokens.tab_border,
            ),
        };

        // Extract values we need before moving self
        let height = match self.size {
            TabSize::Small => tokens.sizes.button_height_sm,
            TabSize::Medium => tab_container_height(tokens),
            TabSize::Large => tokens.sizes.button_height_lg,
        };
        let min_width = px(TAB_MIN_WIDTH);
        let max_width = px(TAB_MAX_WIDTH);
        let content_height = height - px(1.0);

        // Build the tab container using design tokens and delegate inner content
        let tab_hover_group = SharedString::from(format!("tab-hover-{}", self.doc_id));
        let on_close_handler = self.on_close.clone();
        let label_clone = self.label.clone();
        let label_detail = self.label_detail.clone();
        let file_path = self.file_path.clone();
        let is_active = self.is_active;
        let is_modified = self.is_modified;
        let is_readonly = self.is_readonly;
        let is_deleted = self.is_deleted;
        let is_pinned = self.is_pinned;
        let is_preview = self.is_preview;
        let deemphasized = self.deemphasized;
        let close_button_visibility = self.close_button_visibility;
        let close_position = self.close_position;
        let show_file_icons = self.show_file_icons;
        let disabled = self.disabled;
        let doc_id = self.doc_id;
        let on_context_menu = self.on_context_menu.clone();
        let on_toggle_pin = self.on_toggle_pin.clone();
        let on_toggle_readonly = self.on_toggle_readonly.clone();
        let root = self.div;
        let position = self.position;
        let tooltip = self.tooltip.clone();
        let git_status = self.git_status;
        let diagnostic_severity = self.diagnostic_severity;
        let text_color =
            Tab::deemphasized_text_color(text_color, is_active, disabled, deemphasized, tab_tokens);
        let content_row = Tab::build_content_row(
            doc_id,
            label_clone,
            label_detail,
            file_path,
            is_active,
            is_modified,
            is_readonly,
            is_deleted,
            is_pinned,
            is_preview,
            disabled,
            git_status,
            diagnostic_severity,
            text_color,
            tokens,
            on_close_handler,
            on_toggle_pin,
            on_toggle_readonly,
            tab_hover_group.clone(),
            close_button_visibility,
            close_position,
            show_file_icons,
            cx,
        );
        root.group(tab_hover_group)
            .flex()
            .flex_none() // Don't grow or shrink
            .items_center()
            .h(height)
            .min_w(min_width)
            .max_w(max_width)
            .bg(bg_color)
            .when(enable_animations && !disabled, |tab| {
                tab.hover(|style| style.bg(hover_bg))
            })
            .when(!disabled, |tab| tab.cursor(CursorStyle::PointingHand))
            .border_color(border_color)
            .map(|tab| match position {
                TabPosition::First => {
                    if is_active {
                        tab.pl(px(1.0)).border_r_1().pb(px(1.0))
                    } else {
                        tab.pl(px(1.0)).pr(px(1.0)).border_b_1()
                    }
                }
                TabPosition::Last => {
                    if is_active {
                        tab.border_l_1().border_r_1().pb(px(1.0))
                    } else {
                        tab.pl(px(1.0)).border_b_1().border_r_1()
                    }
                }
                TabPosition::Middle(Ordering::Equal) => tab.border_l_1().border_r_1().pb(px(1.0)),
                TabPosition::Middle(Ordering::Less) => tab.border_l_1().pr(px(1.0)).border_b_1(),
                TabPosition::Middle(Ordering::Greater) => tab.border_r_1().pl(px(1.0)).border_b_1(),
            })
            .when(!disabled, |tab| {
                tab.on_mouse_down(MouseButton::Left, {
                    let on_click = self.on_click.clone();
                    move |event, window, cx| {
                        let click_event = click_event_from_mouse_down(event);
                        window.prevent_default();
                        cx.stop_propagation();
                        on_click(&click_event, window, cx);
                    }
                })
            })
            .when(!disabled && !is_pinned, |tab| {
                tab.on_aux_click({
                    let on_close = self.on_close.clone();
                    move |event, window, cx| {
                        if event.is_middle_click() {
                            on_close(event, window, cx);
                            cx.stop_propagation();
                        }
                    }
                })
            })
            .when_some(on_context_menu, |tab, on_context_menu| {
                tab.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    on_context_menu(event, window, cx);
                    cx.stop_propagation();
                })
            })
            .when_some(tooltip, |tab, tooltip| {
                let readonly = is_readonly;
                tab.tooltip(move |_window, cx| {
                    if readonly {
                        cx.new(|_| TabMetaTooltip {
                            title: tooltip.clone(),
                            detail: SharedString::from(Tab::readonly_content_tooltip_detail()),
                        })
                        .into()
                    } else {
                        cx.new(|_| TabTooltip {
                            text: tooltip.clone(),
                        })
                        .into()
                    }
                })
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .relative()
                    .w_full()
                    .min_w(px(0.0))
                    .h(content_height)
                    .px(tokens.sizes.space_2)
                    .gap(tokens.sizes.space_2)
                    .text_color(text_color)
                    .child(content_row),
            )
    }
}

// Internal layout helpers to centralize tab content composition
impl Tab {
    fn inactive_background_color(tab_tokens: nucleotide_ui::tokens::TabBarTokens) -> gpui::Hsla {
        tab_tokens.tab_inactive_background
    }

    fn inactive_hover_background_color(
        tab_tokens: nucleotide_ui::tokens::TabBarTokens,
    ) -> gpui::Hsla {
        tab_tokens.tab_hover_background
    }

    fn deemphasized_text_color(
        text_color: gpui::Hsla,
        is_active: bool,
        disabled: bool,
        deemphasized: bool,
        tab_tokens: nucleotide_ui::tokens::TabBarTokens,
    ) -> gpui::Hsla {
        if !deemphasized || disabled {
            return text_color;
        }

        if is_active {
            tab_tokens.tab_text_inactive
        } else {
            nucleotide_ui::styling::ColorTheory::with_alpha(tab_tokens.tab_text_inactive, 0.5)
        }
    }

    fn build_start_indicator(
        is_modified: bool,
        tokens: nucleotide_ui::tokens::DesignTokens,
    ) -> gpui::AnyElement {
        div()
            .size(px(START_TAB_SLOT_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(is_modified, |slot| {
                slot.child(
                    div()
                        .size(px(6.0))
                        .rounded(px(3.0))
                        .bg(tokens.tab_bar_tokens().tab_modified_indicator),
                )
            })
            .into_any_element()
    }

    fn build_icon(
        file_path: Option<std::path::PathBuf>,
        diagnostic_severity: Option<DiagnosticSeverity>,
        tokens: nucleotide_ui::tokens::DesignTokens,
        cx: &mut App,
    ) -> gpui::AnyElement {
        let icon_color = Tab::content_icon_color(tokens);
        let icon = if let Some(ref path) = file_path {
            VcsIcon::from_path(path, false)
                .size(tokens.sizes.text_lg.into())
                .text_color(icon_color)
        } else {
            VcsIcon::scratch()
                .size(tokens.sizes.text_lg.into())
                .text_color(icon_color)
        };
        let theme = cx.global::<nucleotide_ui::Theme>();

        div()
            .relative()
            .size(tokens.sizes.text_lg)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(icon.render_with_theme(theme))
            .when_some(diagnostic_severity, |icon, severity| {
                icon.child(Tab::build_diagnostic_decoration(severity, tokens))
            })
            .into_any_element()
    }

    fn build_readonly_icon(
        tokens: nucleotide_ui::tokens::DesignTokens,
        diagnostic_severity: Option<DiagnosticSeverity>,
        on_toggle_readonly: Option<MouseEventHandler>,
    ) -> gpui::AnyElement {
        let is_toggleable = on_toggle_readonly.is_some();
        let button_tokens = tokens.button_tokens();
        let icon = div()
            .id("tab-readonly-lock")
            .relative()
            .size(tokens.sizes.text_lg)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/file-lock.svg")
                    .size(tokens.sizes.text_lg)
                    .text_color(Tab::content_icon_color(tokens)),
            )
            .when(is_toggleable, |icon| {
                icon.cursor(CursorStyle::PointingHand)
                    .hover(|icon| icon.bg(button_tokens.ghost_background_hover))
            })
            .when_some(diagnostic_severity, |icon, severity| {
                icon.child(Tab::build_diagnostic_decoration(severity, tokens))
            })
            .tooltip(move |_window, cx| {
                let title = Self::readonly_tooltip_title(is_toggleable);
                let detail = Self::readonly_tooltip_detail(is_toggleable);
                cx.new(|_| TabMetaTooltip {
                    title: SharedString::from(title),
                    detail: SharedString::from(detail),
                })
                .into()
            });

        if let Some(on_toggle_readonly) = on_toggle_readonly {
            icon.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let click_event = click_event_from_mouse_down(event);
                window.prevent_default();
                cx.stop_propagation();
                on_toggle_readonly(&click_event, window, cx);
            })
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .into_any_element()
        } else {
            icon.into_any_element()
        }
    }

    fn readonly_tooltip_title(is_toggleable: bool) -> &'static str {
        if is_toggleable {
            "Unlock File"
        } else {
            "Locked File"
        }
    }

    fn readonly_tooltip_detail(is_toggleable: bool) -> &'static str {
        if is_toggleable {
            "This will make this file editable"
        } else {
            "This file is read-only"
        }
    }

    fn readonly_content_tooltip_detail() -> &'static str {
        "Read-Only File"
    }

    fn content_icon_color(tokens: nucleotide_ui::tokens::DesignTokens) -> gpui::Hsla {
        tokens.file_tree_tokens().icon_color
    }

    fn content_icon_kind(is_readonly: bool, show_file_icons: bool) -> Option<TabContentIcon> {
        if is_readonly {
            Some(TabContentIcon::Readonly)
        } else if show_file_icons {
            Some(TabContentIcon::File)
        } else {
            None
        }
    }

    fn build_diagnostic_decoration(
        severity: DiagnosticSeverity,
        tokens: nucleotide_ui::tokens::DesignTokens,
    ) -> gpui::AnyElement {
        let (path, color) = match severity {
            DiagnosticSeverity::Error => ("icons/circle-x.svg", tokens.editor.diagnostic_error),
            DiagnosticSeverity::Warning => {
                ("icons/triangle-alert.svg", tokens.editor.diagnostic_warning)
            }
            DiagnosticSeverity::Info | DiagnosticSeverity::Hint => {
                ("icons/triangle-alert.svg", tokens.editor.diagnostic_info)
            }
        };

        div()
            .absolute()
            .top(px(-2.0))
            .left(px(-2.0))
            .size(px(9.0))
            .flex()
            .items_center()
            .justify_center()
            .child(svg().path(path).size(px(9.0)).text_color(color))
            .into_any_element()
    }

    fn vcs_label_text_color(
        text_color: gpui::Hsla,
        git_status: Option<VcsStatus>,
        tokens: nucleotide_ui::tokens::DesignTokens,
    ) -> gpui::Hsla {
        match git_status {
            Some(VcsStatus::Added | VcsStatus::Untracked) => tokens.editor.vcs_added,
            Some(VcsStatus::Modified | VcsStatus::Renamed) => tokens.editor.vcs_modified,
            Some(VcsStatus::Deleted) => tokens.editor.vcs_deleted,
            Some(VcsStatus::Conflicted | VcsStatus::Unknown) => tokens.editor.error,
            Some(VcsStatus::Clean) | None => text_color,
        }
    }

    fn build_label(
        label_text: String,
        label_detail: Option<String>,
        is_active: bool,
        is_preview: bool,
        is_deleted: bool,
        text_color: gpui::Hsla,
        tokens: nucleotide_ui::tokens::DesignTokens,
    ) -> gpui::AnyElement {
        let mut label = div()
            .min_w(px(0.0))
            .max_w(px(150.0))
            .flex_shrink_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(text_color)
            .text_size(tokens.sizes.text_md);
        if is_active {
            label = label.font_weight(gpui::FontWeight::MEDIUM);
        }
        if is_preview {
            label = label.italic();
        }
        if is_deleted {
            label = label.line_through();
        }

        div()
            .flex()
            .items_center()
            .gap(tokens.sizes.space_1)
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(label.child(label_text))
            .when_some(label_detail, |row, detail| {
                row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(tokens.tab_bar_tokens().tab_text_inactive)
                        .text_size(tokens.sizes.text_sm)
                        .child(detail),
                )
            })
            .into_any_element()
    }

    fn build_end_button(
        props: TabEndButtonProps,
        tokens: nucleotide_ui::tokens::DesignTokens,
        on_close: MouseEventHandler,
        on_toggle_pin: Option<MouseEventHandler>,
    ) -> gpui::AnyElement {
        let button = if props.is_pinned {
            let tooltip = SharedString::from(Self::end_button_tooltip(true));
            let button = div()
                .id(format!("tab-unpin-{}", props.doc_id))
                .size(px(END_TAB_SLOT_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens.sizes.radius_sm)
                .text_color(props.text_color)
                .tooltip(move |_window, cx| {
                    cx.new(|_| TabTooltip {
                        text: tooltip.clone(),
                    })
                    .into()
                })
                .opacity(if props.disabled || on_toggle_pin.is_none() {
                    0.6
                } else {
                    1.0
                })
                .child(
                    svg()
                        .path("icons/pin.svg")
                        .size(px(TAB_SLOT_ICON_SIZE))
                        .text_color(props.text_color),
                );

            if let Some(on_toggle_pin) = on_toggle_pin {
                if props.disabled {
                    button
                } else {
                    button
                        .cursor(CursorStyle::PointingHand)
                        .hover(|button| button.bg(tokens.button_tokens().ghost_background_hover))
                        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                            let click_event = click_event_from_mouse_down(event);
                            window.prevent_default();
                            cx.stop_propagation();
                            on_toggle_pin(&click_event, window, cx);
                        })
                        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                }
            } else {
                button
            }
            .into_any_element()
        } else {
            if props.close_button_visibility == TabCloseButtonVisibility::Hidden {
                return div()
                    .size(px(END_TAB_SLOT_SIZE))
                    .flex_none()
                    .into_any_element();
            }

            let tooltip = SharedString::from(Self::end_button_tooltip(false));
            div()
                .id(format!("tab-close-{}", props.doc_id))
                .size(px(END_TAB_SLOT_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens.sizes.radius_sm)
                .text_color(props.text_color)
                .tooltip(move |_window, cx| {
                    cx.new(|_| TabTooltip {
                        text: tooltip.clone(),
                    })
                    .into()
                })
                .opacity(if props.disabled { 0.6 } else { 1.0 })
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(TAB_SLOT_ICON_SIZE))
                        .text_color(props.text_color),
                )
                .when(!props.disabled, |button| {
                    button
                        .cursor(CursorStyle::PointingHand)
                        .hover(|button| button.bg(tokens.button_tokens().ghost_background_hover))
                        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                            let click_event = click_event_from_mouse_down(event);
                            window.prevent_default();
                            cx.stop_propagation();
                            on_close(&click_event, window, cx);
                        })
                        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                })
                .into_any_element()
        };

        div()
            .size(px(END_TAB_SLOT_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_color(props.text_color)
            .when(
                !props.is_pinned
                    && props.close_button_visibility == TabCloseButtonVisibility::Hover,
                |slot| {
                    slot.invisible()
                        .group_hover(props.tab_hover_group, |style| style.visible())
                },
            )
            .child(button)
            .into_any_element()
    }

    fn end_button_tooltip(is_pinned: bool) -> &'static str {
        if is_pinned { "Unpin Tab" } else { "Close Tab" }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_content_row(
        doc_id: TabId,
        label: String,
        label_detail: Option<String>,
        file_path: Option<std::path::PathBuf>,
        is_active: bool,
        is_modified: bool,
        is_readonly: bool,
        is_deleted: bool,
        is_pinned: bool,
        is_preview: bool,
        disabled: bool,
        git_status: Option<VcsStatus>,
        diagnostic_severity: Option<DiagnosticSeverity>,
        text_color: gpui::Hsla,
        tokens: nucleotide_ui::tokens::DesignTokens,
        on_close: MouseEventHandler,
        on_toggle_pin: Option<MouseEventHandler>,
        on_toggle_readonly: Option<MouseEventHandler>,
        tab_hover_group: SharedString,
        close_button_visibility: TabCloseButtonVisibility,
        close_position: TabClosePosition,
        show_file_icons: bool,
        cx: &mut App,
    ) -> gpui::AnyElement {
        let start_slot = Tab::build_start_indicator(is_modified, tokens);
        let end_slot = Tab::build_end_button(
            TabEndButtonProps {
                doc_id,
                is_pinned,
                disabled,
                text_color,
                tab_hover_group,
                close_button_visibility,
            },
            tokens,
            on_close,
            on_toggle_pin,
        );

        let (leading_slot, trailing_slot) = match close_position {
            TabClosePosition::Left => (end_slot, start_slot),
            TabClosePosition::Right => (start_slot, end_slot),
        };
        let trailing_slot = div().flex_none().ml_auto().child(trailing_slot);
        let label_text_color = Tab::vcs_label_text_color(text_color, git_status, tokens);
        let content_icon = Tab::content_icon_kind(is_readonly, show_file_icons);
        let readonly_diagnostic_severity = show_file_icons.then_some(diagnostic_severity).flatten();

        div()
            .flex()
            .items_center()
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .gap(tokens.sizes.space_2)
            .child(leading_slot)
            .when_some(content_icon, |row, content_icon| match content_icon {
                TabContentIcon::File => {
                    row.child(Tab::build_icon(file_path, diagnostic_severity, tokens, cx))
                }
                TabContentIcon::Readonly => row.child(Tab::build_readonly_icon(
                    tokens,
                    readonly_diagnostic_severity,
                    on_toggle_readonly.clone(),
                )),
            })
            .child(Tab::build_label(
                label,
                label_detail,
                is_active,
                is_preview,
                is_deleted,
                label_text_color,
                tokens,
            ))
            .child(trailing_slot)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_container_height_matches_zed_base32() {
        let tokens = nucleotide_ui::DesignTokens::dark();

        assert_eq!(tab_container_height(tokens), px(32.0));
        assert_ne!(tab_container_height(tokens), tokens.sizes.button_height_md);
    }

    #[test]
    fn tab_slot_geometry_matches_zed() {
        assert_eq!(START_TAB_SLOT_SIZE, 12.0);
        assert_eq!(END_TAB_SLOT_SIZE, 14.0);
        assert_eq!(TAB_SLOT_ICON_SIZE, 12.0);
    }

    #[test]
    fn active_tab_background_matches_editor_background() {
        for tokens in [
            nucleotide_ui::DesignTokens::dark(),
            nucleotide_ui::DesignTokens::light(),
        ] {
            assert_eq!(
                tokens.tab_bar_tokens().tab_active_background,
                tokens.editor.background
            );
        }
    }

    #[test]
    fn inactive_tab_backgrounds_remain_distinct_from_active() {
        for tokens in [
            nucleotide_ui::DesignTokens::dark(),
            nucleotide_ui::DesignTokens::light(),
        ] {
            let tab_tokens = tokens.tab_bar_tokens();
            let active = tab_tokens.tab_active_background;
            let inactive = Tab::inactive_background_color(tab_tokens);
            let hover = Tab::inactive_hover_background_color(tab_tokens);

            assert_ne!(inactive, active);
            assert_ne!(hover, active);
            assert_ne!(inactive, hover);
        }
    }

    #[test]
    fn end_button_tooltips_match_zed() {
        assert_eq!(Tab::end_button_tooltip(false), "Close Tab");
        assert_eq!(Tab::end_button_tooltip(true), "Unpin Tab");
    }

    #[test]
    fn readonly_tooltip_matches_zed_non_toggleable_locked_file_copy() {
        assert_eq!(Tab::readonly_tooltip_title(false), "Locked File");
        assert_eq!(
            Tab::readonly_tooltip_detail(false),
            "This file is read-only"
        );
    }

    #[test]
    fn readonly_tooltip_matches_zed_toggleable_unlock_file_copy() {
        assert_eq!(Tab::readonly_tooltip_title(true), "Unlock File");
        assert_eq!(
            Tab::readonly_tooltip_detail(true),
            "This will make this file editable"
        );
    }

    #[test]
    fn readonly_content_tooltip_matches_zed_copy() {
        assert_eq!(Tab::readonly_content_tooltip_detail(), "Read-Only File");
    }

    #[test]
    fn readonly_tabs_replace_file_icon_slot() {
        assert_eq!(
            Tab::content_icon_kind(false, true),
            Some(TabContentIcon::File)
        );
        assert_eq!(
            Tab::content_icon_kind(true, true),
            Some(TabContentIcon::Readonly)
        );
        assert_eq!(
            Tab::content_icon_kind(true, false),
            Some(TabContentIcon::Readonly)
        );
        assert_eq!(Tab::content_icon_kind(false, false), None);
    }

    #[test]
    fn tab_content_icon_color_matches_treeview_icons() {
        let tokens = nucleotide_ui::DesignTokens::dark();

        assert_eq!(
            Tab::content_icon_color(tokens),
            tokens.file_tree_tokens().icon_color
        );
    }

    #[test]
    fn vcs_label_text_color_matches_zed_git_status_groups() {
        let tokens = nucleotide_ui::DesignTokens::dark();
        let text_color = tokens.tab_bar_tokens().tab_text_inactive;

        assert_eq!(
            Tab::vcs_label_text_color(text_color, None, tokens),
            text_color
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Clean), tokens),
            text_color
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Added), tokens),
            tokens.editor.vcs_added
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Untracked), tokens),
            tokens.editor.vcs_added
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Modified), tokens),
            tokens.editor.vcs_modified
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Renamed), tokens),
            tokens.editor.vcs_modified
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Deleted), tokens),
            tokens.editor.vcs_deleted
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Conflicted), tokens),
            tokens.editor.error
        );
        assert_eq!(
            Tab::vcs_label_text_color(text_color, Some(VcsStatus::Unknown), tokens),
            tokens.editor.error
        );
    }
}
