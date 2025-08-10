// ABOUTME: List item component following Zed's pattern
// ABOUTME: Reusable component for consistent list rendering

use crate::ui::spacing;
use gpui::*;
use smallvec::SmallVec;

/// List item spacing options
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListItemSpacing {
    Compact,
    Default,
    Spacious,
}

impl ListItemSpacing {
    fn padding(&self) -> (Pixels, Pixels) {
        match self {
            Self::Compact => (spacing::XS, spacing::SM),
            Self::Default => (spacing::SM, spacing::MD),
            Self::Spacious => (spacing::MD, spacing::LG),
        }
    }
}

// Type alias for click handlers
type ListItemClickHandler = Box<dyn Fn(&MouseDownEvent, &mut App) + 'static>;

/// A reusable list item component following Zed's pattern
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    spacing: ListItemSpacing,
    selected: bool,
    disabled: bool,
    on_click: Option<ListItemClickHandler>,
    on_secondary_click: Option<ListItemClickHandler>,
    children: SmallVec<[AnyElement; 2]>,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    overflow: TextOverflow,
    tooltip: Option<SharedString>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            spacing: ListItemSpacing::Default,
            selected: false,
            disabled: false,
            on_click: None,
            on_secondary_click: None,
            children: SmallVec::new(),
            start_slot: None,
            end_slot: None,
            overflow: TextOverflow::Truncate("…".into()),
            tooltip: None,
        }
    }

    /// Set the spacing for this list item
    pub fn spacing(mut self, spacing: ListItemSpacing) -> Self {
        self.spacing = spacing;
        self
    }

    /// Mark this item as selected
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Mark this item as disabled
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set click handler
    pub fn on_click(mut self, handler: impl Fn(&MouseDownEvent, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Set secondary click handler
    pub fn on_secondary_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_click = Some(Box::new(handler));
        self
    }

    /// Add a child element
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    /// Add multiple children
    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }

    /// Set the start slot (icon, checkbox, etc.)
    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    /// Set the end slot (badge, action button, etc.)
    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }

    /// Set text overflow behavior
    pub fn overflow(mut self, overflow: TextOverflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Set tooltip text
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<crate::ui::Theme>();
        let (py, px) = self.spacing.padding();

        let mut base = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .py(py)
            .px(px)
            .text_overflow(self.overflow);

        // Apply styling based on state
        if self.disabled {
            base = base.text_color(theme.text_disabled).cursor_not_allowed();
        } else if self.selected {
            base = base.bg(theme.accent).text_color(white()).cursor_pointer();
        } else {
            base = base
                .text_color(theme.text)
                .cursor_pointer()
                .hover(|this| this.bg(theme.surface_hover));
        }

        // Add click handlers
        if let Some(on_click) = self.on_click {
            base = base.on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                on_click(ev, cx);
            });
        }

        if let Some(on_secondary_click) = self.on_secondary_click {
            base = base.on_mouse_down(MouseButton::Right, move |ev, _window, cx| {
                on_secondary_click(ev, cx);
            });
        }

        // TODO: Add tooltip support when GPUI API is stable
        // Tooltip implementation removed temporarily

        // Build content
        if let Some(start_slot) = self.start_slot {
            base = base.child(div().mr(spacing::SM).flex_shrink_0().child(start_slot));
        }

        base = base.child(
            div()
                .flex()
                .flex_1()
                .overflow_hidden()
                .children(self.children),
        );

        if let Some(end_slot) = self.end_slot {
            base = base.child(div().ml(spacing::SM).flex_shrink_0().child(end_slot));
        }

        base
    }
}
