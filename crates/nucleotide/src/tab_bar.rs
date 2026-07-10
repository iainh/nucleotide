// ABOUTME: Tab bar component that displays all open buffers as tabs
// ABOUTME: Provides a Zed-style scrollable tab strip with callbacks for tab interactions

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement, IntoElement, MouseDownEvent, ParentElement,
    Pixels, RenderOnce, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use helix_core::diagnostic::Severity as DiagnosticSeverity;
#[cfg(test)]
use helix_view::DocumentId;
use nucleotide_types::VcsStatus;
use nucleotide_ui::{ThemedContext, Tooltipped};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{TabCloseButtonVisibility, TabClosePosition, TabDiagnosticsVisibility};
use crate::tab::{Tab, TabId, TabPosition, tab_container_height};

/// Type alias for tab event handlers
type TabEventHandler = Arc<dyn Fn(TabId, &mut Window, &mut App) + 'static>;
type TabContextMenuHandler = Arc<dyn Fn(TabId, &MouseDownEvent, &mut Window, &mut App) + 'static>;
type EmptyTabBarClickHandler = Arc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type TabBarScrollWheelHandler = Arc<dyn Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static>;

const MAX_TAB_TITLE_LEN: usize = 24;

fn truncate_and_trailoff(text: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 5);

    if text.len() <= max_chars {
        return text.to_string();
    }

    match text.char_indices().map(|(index, _)| index).nth(max_chars) {
        Some(index) => text[..index].to_string() + "…",
        None => text.to_string(),
    }
}

fn tab_position(index: usize, tab_count: usize, active_index: Option<usize>) -> TabPosition {
    if index == 0 {
        TabPosition::First
    } else if index + 1 == tab_count {
        TabPosition::Last
    } else {
        TabPosition::Middle(
            active_index
                .map(|active_index| index.cmp(&active_index))
                .unwrap_or(Ordering::Less),
        )
    }
}

fn is_tab_double_click(click_count: usize) -> bool {
    click_count >= 2
}

fn should_render_pinned_scroll_separator(max_scroll_x: Pixels, scroll_offset_x: Pixels) -> bool {
    max_scroll_x > px(2.0) && scroll_offset_x < px(0.0)
}

fn tab_bar_control_gap(tokens: &nucleotide_ui::tokens::DesignTokens) -> Pixels {
    tokens.sizes.space_2
}

fn tab_bar_control_padding_x() -> Pixels {
    px(6.0)
}

fn end_drop_target_id(forced_pin_state: Option<bool>) -> &'static str {
    if forced_pin_state == Some(true) {
        "pinned_tabs_border"
    } else {
        "tab_bar_drop_target"
    }
}

fn end_drop_target_has_leading_border(forced_pin_state: Option<bool>) -> bool {
    forced_pin_state == Some(true)
}

fn compute_disambiguation_details<T, D>(
    items: &[T],
    get_description: impl Fn(&T, usize) -> D,
) -> Vec<usize>
where
    D: Clone + Eq + Hash,
{
    let mut details = vec![0usize; items.len()];
    let mut descriptions: HashMap<D, Vec<usize>> = HashMap::new();
    let mut current_descriptions = items
        .iter()
        .map(|item| get_description(item, 0))
        .collect::<Vec<_>>();

    loop {
        let mut any_collisions = false;

        for (index, (item, &detail)) in items.iter().zip(&details).enumerate() {
            if detail > 0 {
                let new_description = get_description(item, detail);
                if new_description == current_descriptions[index] {
                    continue;
                }
                current_descriptions[index] = new_description;
            }

            descriptions
                .entry(current_descriptions[index].clone())
                .or_default()
                .push(index);
        }

        for indices in descriptions.drain().map(|(_, indices)| indices) {
            if indices.len() > 1 {
                any_collisions = true;
                for index in indices {
                    details[index] += 1;
                }
            }
        }

        if !any_collisions {
            break;
        }
    }

    details
}

/// Information about a document for tab display
#[derive(Clone)]
pub struct DocumentInfo {
    pub id: TabId,
    pub path: Option<PathBuf>,
    pub is_modified: bool,
    pub is_readonly: bool,
    pub is_deleted: bool,
    pub is_pinned: bool,
    pub is_preview: bool,
    pub focused_at: std::time::Instant,
    pub order: usize, // Tracks the order documents were opened
    pub git_status: Option<VcsStatus>,
    pub diagnostic_severity: Option<DiagnosticSeverity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DocumentTabLabel {
    title: String,
    detail: Option<String>,
}

struct TabStripOptions {
    id: &'static str,
    tabs: Vec<Tab>,
    scroll_handle: Option<ScrollHandle>,
    on_empty_double_click: Option<EmptyTabBarClickHandler>,
    on_scroll_wheel: Option<TabBarScrollWheelHandler>,
    forced_pin_state: Option<bool>,
    tokens: nucleotide_ui::tokens::DesignTokens,
    border_color: gpui::Hsla,
}

/// Tab bar that displays all open documents
#[derive(IntoElement)]
pub struct TabBar {
    /// Document information for all open documents
    documents: Arc<[DocumentInfo]>,
    /// Currently active document ID
    active_doc_id: Option<TabId>,
    /// Project directory for relative paths
    project_directory: Option<PathBuf>,
    /// Callback when a tab is clicked
    on_tab_click: TabEventHandler,
    /// Callback when a tab close button is clicked
    on_tab_close: TabEventHandler,
    /// Callback when a pinned tab's pin button is clicked
    on_tab_toggle_pin: Option<TabEventHandler>,
    /// Callback when a read-only tab's lock icon is clicked
    on_tab_toggle_readonly: Option<TabEventHandler>,
    /// Callback when a tab is double-clicked
    on_tab_double_click: Option<TabEventHandler>,
    /// Callback when a tab context menu is requested
    on_tab_context_menu: Option<TabContextMenuHandler>,
    /// Callback when empty tab strip space is double-clicked
    on_empty_double_click: Option<EmptyTabBarClickHandler>,
    /// Callback when the scrollable unpinned tab strip is manually scrolled
    on_scroll_wheel: Option<TabBarScrollWheelHandler>,
    /// Render pinned tabs in a separate row when both pinned and unpinned tabs exist
    show_pinned_tabs_in_separate_row: bool,
    /// Controls close button visibility for unpinned tabs
    show_close_button: TabCloseButtonVisibility,
    /// Controls close or pin button placement within tabs
    close_position: TabClosePosition,
    /// Controls whether file icons are rendered in tabs
    file_icons: bool,
    /// Controls whether git status decorations are rendered in tabs
    git_status: bool,
    /// Controls whether diagnostic decorations are rendered in tabs
    show_diagnostics: TabDiagnosticsVisibility,
    /// Whether tab labels should be deemphasized because the editor pane is not focused
    deemphasized: bool,
    /// Scroll handle used to keep the active tab visible and preserve user scroll state
    scroll_handle: Option<ScrollHandle>,
    /// Controls rendered before the scrollable tab strip
    start_children: Vec<AnyElement>,
    /// Controls rendered after the scrollable tab strip
    end_children: Vec<AnyElement>,
}

impl TabBar {
    pub fn new(
        mut documents: Vec<DocumentInfo>,
        active_doc_id: Option<TabId>,
        project_directory: Option<PathBuf>,
        on_tab_click: impl Fn(TabId, &mut Window, &mut App) + 'static,
        on_tab_close: impl Fn(TabId, &mut Window, &mut App) + 'static,
    ) -> Self {
        documents.sort_by_key(|doc| (!doc.is_pinned, doc.order));
        Self::new_shared(
            Arc::from(documents),
            active_doc_id,
            project_directory,
            on_tab_click,
            on_tab_close,
        )
    }

    pub fn new_shared(
        documents: Arc<[DocumentInfo]>,
        active_doc_id: Option<TabId>,
        project_directory: Option<PathBuf>,
        on_tab_click: impl Fn(TabId, &mut Window, &mut App) + 'static,
        on_tab_close: impl Fn(TabId, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            documents,
            active_doc_id,
            project_directory,
            on_tab_click: Arc::new(on_tab_click),
            on_tab_close: Arc::new(on_tab_close),
            on_tab_toggle_pin: None,
            on_tab_toggle_readonly: None,
            on_tab_double_click: None,
            on_tab_context_menu: None,
            on_empty_double_click: None,
            on_scroll_wheel: None,
            show_pinned_tabs_in_separate_row: false,
            show_close_button: TabCloseButtonVisibility::default(),
            close_position: TabClosePosition::default(),
            file_icons: true,
            git_status: false,
            show_diagnostics: TabDiagnosticsVisibility::default(),
            deemphasized: false,
            scroll_handle: None,
            start_children: Vec::new(),
            end_children: Vec::new(),
        }
    }

    pub fn track_scroll(mut self, scroll_handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(scroll_handle.clone());
        self
    }

    pub fn with_context_menu_handler(
        mut self,
        on_context_menu: impl Fn(TabId, &MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_context_menu = Some(Arc::new(on_context_menu));
        self
    }

    pub fn with_empty_double_click_handler(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_empty_double_click = Some(Arc::new(handler));
        self
    }

    pub fn with_scroll_wheel_handler(
        mut self,
        handler: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_scroll_wheel = Some(Arc::new(handler));
        self
    }

    pub fn show_pinned_tabs_in_separate_row(mut self, show: bool) -> Self {
        self.show_pinned_tabs_in_separate_row = show;
        self
    }

    pub fn show_close_button(mut self, visibility: TabCloseButtonVisibility) -> Self {
        self.show_close_button = visibility;
        self
    }

    pub fn close_position(mut self, position: TabClosePosition) -> Self {
        self.close_position = position;
        self
    }

    pub fn file_icons(mut self, show: bool) -> Self {
        self.file_icons = show;
        self
    }

    pub fn git_status(mut self, show: bool) -> Self {
        self.git_status = show;
        self
    }

    pub fn show_diagnostics(mut self, visibility: TabDiagnosticsVisibility) -> Self {
        self.show_diagnostics = visibility;
        self
    }

    pub fn deemphasized(mut self, deemphasized: bool) -> Self {
        self.deemphasized = deemphasized;
        self
    }

    pub fn with_pin_toggle_handler(
        mut self,
        on_toggle_pin: impl Fn(TabId, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_toggle_pin = Some(Arc::new(on_toggle_pin));
        self
    }

    pub fn with_readonly_toggle_handler(
        mut self,
        on_toggle_readonly: impl Fn(TabId, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_toggle_readonly = Some(Arc::new(on_toggle_readonly));
        self
    }

    pub fn with_double_click_handler(
        mut self,
        on_double_click: impl Fn(TabId, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_double_click = Some(Arc::new(on_double_click));
        self
    }

    pub fn start_child(mut self, child: impl IntoElement) -> Self {
        self.start_children.push(child.into_any_element());
        self
    }

    pub fn start_children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.start_children
            .extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }

    pub fn end_child(mut self, child: impl IntoElement) -> Self {
        self.end_children.push(child.into_any_element());
        self
    }

    pub fn end_children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.end_children
            .extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }

    fn document_labels(&self, documents: &[DocumentInfo]) -> Vec<DocumentTabLabel> {
        let details = compute_disambiguation_details(documents, |doc, detail| {
            self.disambiguated_document_label(doc, detail)
        });

        documents
            .iter()
            .zip(details)
            .map(|(doc, detail)| DocumentTabLabel {
                title: truncate_and_trailoff(&self.base_document_label(doc), MAX_TAB_TITLE_LEN),
                detail: self
                    .document_label_detail(doc, detail)
                    .map(|detail| truncate_and_trailoff(&detail, MAX_TAB_TITLE_LEN)),
            })
            .collect()
    }

    fn base_document_label(&self, doc_info: &DocumentInfo) -> String {
        if let Some(path) = &doc_info.path {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| path.display().to_string())
        } else {
            "[scratch]".to_string()
        }
    }

    fn disambiguated_document_label(&self, doc_info: &DocumentInfo, detail: usize) -> String {
        let Some(components) = self.document_path_components(doc_info) else {
            return self.base_document_label(doc_info);
        };

        let detail = (detail + 1).min(components.len()).max(1);
        components[components.len() - detail..].join("/")
    }

    fn document_label_detail(&self, doc_info: &DocumentInfo, detail: usize) -> Option<String> {
        if detail == 0 {
            return None;
        }

        let components = self.document_path_components(doc_info)?;
        let parent_components = components.get(..components.len().saturating_sub(1))?;
        if parent_components.is_empty() {
            return None;
        }

        let detail = detail.min(parent_components.len());
        let path_detail = parent_components[parent_components.len() - detail..].join("/");
        (!path_detail.is_empty()).then_some(path_detail)
    }

    fn document_path_components(&self, doc_info: &DocumentInfo) -> Option<Vec<String>> {
        let path = doc_info.path.as_deref()?;
        let display_path = self.display_path(path);
        let components = display_path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .filter(|component| !component.is_empty())
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();

        if components.is_empty() {
            None
        } else {
            Some(components)
        }
    }

    fn display_path<'a>(&'a self, path: &'a Path) -> &'a Path {
        if let Some(project_dir) = &self.project_directory
            && let Ok(relative) = path.strip_prefix(project_dir)
        {
            return relative;
        }

        path
    }

    fn document_tooltip(&self, doc_info: &DocumentInfo, label: &DocumentTabLabel) -> SharedString {
        if let Some(path) = &doc_info.path {
            path.display().to_string().into()
        } else {
            label.title.clone().into()
        }
    }

    #[cfg(test)]
    fn ordered_documents(&self) -> &[DocumentInfo] {
        &self.documents
    }

    fn should_render_separate_pinned_row(&self, documents: &[DocumentInfo]) -> bool {
        let pinned_count = documents.iter().filter(|doc| doc.is_pinned).count();
        self.show_pinned_tabs_in_separate_row && pinned_count > 0 && pinned_count < documents.len()
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let documents = self.documents.clone();
        let labels = self.document_labels(&documents);

        let theme = cx.theme();
        let tokens = &theme.tokens;
        let tab_bar_tokens = tokens.tab_bar_tokens();

        let pinned_count = documents.iter().take_while(|doc| doc.is_pinned).count();
        let pinned_tabs = self.build_tabs(&documents[..pinned_count], &labels[..pinned_count], 0);
        let unpinned_tabs = self.build_tabs(
            &documents[pinned_count..],
            &labels[pinned_count..],
            pinned_count,
        );

        if self.should_render_separate_pinned_row(&documents) {
            self.render_two_row_tab_bar(pinned_tabs, unpinned_tabs, tokens, &tab_bar_tokens)
        } else {
            self.render_single_row_tab_bar(pinned_tabs, unpinned_tabs, tokens, &tab_bar_tokens)
                .into_any_element()
        }
    }
}

impl TabBar {
    fn build_tabs(
        &self,
        documents: &[DocumentInfo],
        labels: &[DocumentTabLabel],
        _global_start_index: usize,
    ) -> Vec<Tab> {
        let active_index = documents
            .iter()
            .position(|doc| self.active_doc_id == Some(doc.id));
        let tab_count = documents.len();

        documents
            .iter()
            .enumerate()
            .map(|(index, doc_info)| {
                let is_active = self.active_doc_id == Some(doc_info.id);
                let position = tab_position(index, tab_count, active_index);
                let on_tab_click = self.on_tab_click.clone();
                let on_tab_close = self.on_tab_close.clone();
                let on_tab_double_click = self.on_tab_double_click.clone();
                let doc_id = doc_info.id;
                let label = labels[index].clone();
                let diagnostic_severity = match (
                    self.file_icons,
                    self.show_diagnostics,
                    doc_info.diagnostic_severity,
                ) {
                    (true, TabDiagnosticsVisibility::All, Some(severity)) => Some(severity),
                    (true, TabDiagnosticsVisibility::Errors, Some(DiagnosticSeverity::Error)) => {
                        Some(DiagnosticSeverity::Error)
                    }
                    _ => None,
                };

                let mut tab = Tab::new(
                    doc_id,
                    label.title.clone(),
                    doc_info.path.clone(),
                    doc_info.is_modified,
                    doc_info.is_pinned,
                    self.git_status.then_some(doc_info.git_status).flatten(),
                    diagnostic_severity,
                    is_active,
                    move |event, window, cx| {
                        if is_tab_double_click(event.click_count())
                            && let Some(on_tab_double_click) = on_tab_double_click.clone()
                        {
                            on_tab_double_click(doc_id, window, cx);
                            return;
                        }

                        on_tab_click(doc_id, window, cx);
                    },
                    move |_event, window, cx| {
                        on_tab_close(doc_id, window, cx);
                    },
                )
                .with_position(position)
                .detail(label.detail.clone())
                .with_close_button_visibility(self.show_close_button)
                .with_close_position(self.close_position)
                .readonly(doc_info.is_readonly)
                .deleted(doc_info.is_deleted)
                .preview(doc_info.is_preview)
                .deemphasized(self.deemphasized)
                .show_file_icons(self.file_icons)
                .tooltip(self.document_tooltip(doc_info, &label));

                if let Some(on_tab_context_menu) = self.on_tab_context_menu.clone() {
                    tab = tab.on_context_menu(move |event, window, cx| {
                        on_tab_context_menu(doc_id, event, window, cx);
                    });
                }

                if let Some(on_tab_toggle_pin) = self.on_tab_toggle_pin.clone() {
                    tab = tab.on_toggle_pin(move |_event, window, cx| {
                        on_tab_toggle_pin(doc_id, window, cx);
                    });
                }

                if doc_info.is_readonly
                    && let Some(on_tab_toggle_readonly) = self.on_tab_toggle_readonly.clone()
                {
                    tab = tab.on_toggle_readonly(move |_event, window, cx| {
                        on_tab_toggle_readonly(doc_id, window, cx);
                    });
                }

                tab
            })
            .collect()
    }

    fn render_single_row_tab_bar(
        self,
        pinned_tabs: Vec<Tab>,
        unpinned_tabs: Vec<Tab>,
        tokens: &nucleotide_ui::tokens::DesignTokens,
        tab_bar_tokens: &nucleotide_ui::tokens::TabBarTokens,
    ) -> gpui::AnyElement {
        let tabbar_bg = tab_bar_tokens.container_background;
        let border_color = tab_bar_tokens.tab_border;
        let active_doc_id = self.active_doc_id;
        let has_active_unpinned_tab = self
            .documents
            .iter()
            .filter(|doc| !doc.is_pinned)
            .any(|doc| active_doc_id == Some(doc.id));
        let scroll_handle = self.scroll_handle;
        let on_empty_double_click = self.on_empty_double_click;
        let on_scroll_wheel = self.on_scroll_wheel;
        let start_children = self.start_children;
        let end_children = self.end_children;
        let pinned_scroll_separator = scroll_handle.as_ref().is_some_and(|scroll_handle| {
            should_render_pinned_scroll_separator(
                scroll_handle.max_offset().x,
                scroll_handle.offset().x,
            )
        });

        let tab_strip = Self::render_tab_strip(TabStripOptions {
            id: "tabs",
            tabs: unpinned_tabs,
            scroll_handle,
            on_empty_double_click,
            on_scroll_wheel,
            forced_pin_state: None,
            tokens: *tokens,
            border_color,
        });

        div()
            .id("tab-bar")
            .group("tab_bar")
            .flex()
            .flex_none()
            .w_full()
            .h(tab_container_height(*tokens))
            .bg(tabbar_bg)
            .when(!start_children.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h_full()
                        .gap(tab_bar_control_gap(tokens))
                        .px(tab_bar_control_padding_x())
                        .border_b_1()
                        .border_r_1()
                        .border_color(border_color)
                        .children(start_children),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_x_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .border_b_1()
                            .border_color(border_color),
                    )
                    .when(!pinned_tabs.is_empty(), |bar| {
                        bar.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .h_full()
                                .when(pinned_scroll_separator, |pinned| {
                                    let pinned = if has_active_unpinned_tab {
                                        pinned.border_r_2()
                                    } else {
                                        pinned.border_r_1()
                                    };
                                    pinned.border_color(border_color)
                                })
                                .children(pinned_tabs),
                        )
                    })
                    .child(tab_strip),
            )
            .when(!end_children.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h_full()
                        .gap(tab_bar_control_gap(tokens))
                        .px(tab_bar_control_padding_x())
                        .border_b_1()
                        .border_l_1()
                        .border_color(border_color)
                        .children(end_children),
                )
            })
            .into_any_element()
    }

    fn render_tab_bar_row(
        id: &'static str,
        tab_strip: gpui::AnyElement,
        start_children: Vec<AnyElement>,
        end_children: Vec<AnyElement>,
        tokens: &nucleotide_ui::tokens::DesignTokens,
        border_color: gpui::Hsla,
    ) -> gpui::AnyElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .w_full()
            .h(tab_container_height(*tokens))
            .when(!start_children.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h_full()
                        .gap(tab_bar_control_gap(tokens))
                        .px(tab_bar_control_padding_x())
                        .border_b_1()
                        .border_r_1()
                        .border_color(border_color)
                        .children(start_children),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_x_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .border_b_1()
                            .border_color(border_color),
                    )
                    .child(tab_strip),
            )
            .when(!end_children.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h_full()
                        .gap(tab_bar_control_gap(tokens))
                        .px(tab_bar_control_padding_x())
                        .border_b_1()
                        .border_l_1()
                        .border_color(border_color)
                        .children(end_children),
                )
            })
            .into_any_element()
    }

    fn render_two_row_tab_bar(
        self,
        pinned_tabs: Vec<Tab>,
        unpinned_tabs: Vec<Tab>,
        tokens: &nucleotide_ui::tokens::DesignTokens,
        tab_bar_tokens: &nucleotide_ui::tokens::TabBarTokens,
    ) -> gpui::AnyElement {
        let tabbar_bg = tab_bar_tokens.container_background;
        let border_color = tab_bar_tokens.tab_border;
        let scroll_handle = self.scroll_handle;
        let on_empty_double_click = self.on_empty_double_click;
        let on_scroll_wheel = self.on_scroll_wheel;
        let start_children = self.start_children;
        let end_children = self.end_children;
        let row_height = tab_container_height(*tokens);

        let pinned_strip = Self::render_tab_strip(TabStripOptions {
            id: "pinned-tabs",
            tabs: pinned_tabs,
            scroll_handle: None,
            on_empty_double_click: on_empty_double_click.clone(),
            on_scroll_wheel: None,
            forced_pin_state: Some(true),
            tokens: *tokens,
            border_color,
        });
        let unpinned_strip = Self::render_tab_strip(TabStripOptions {
            id: "unpinned-tabs",
            tabs: unpinned_tabs,
            scroll_handle,
            on_empty_double_click,
            on_scroll_wheel,
            forced_pin_state: Some(false),
            tokens: *tokens,
            border_color,
        });

        div()
            .id("tab-bar")
            .group("tab_bar")
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .h(row_height * 2.0)
            .bg(tabbar_bg)
            .child(Self::render_tab_bar_row(
                "pinned-tabs-row",
                pinned_strip,
                start_children,
                end_children,
                tokens,
                border_color,
            ))
            .child(Self::render_tab_bar_row(
                "unpinned-tabs-row",
                unpinned_strip,
                Vec::new(),
                Vec::new(),
                tokens,
                border_color,
            ))
            .into_any_element()
    }

    fn render_tab_strip(options: TabStripOptions) -> gpui::AnyElement {
        div()
            .id(options.id)
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .w_full()
            .overflow_x_scroll()
            .when_some(options.scroll_handle, |strip, scroll_handle| {
                strip.track_scroll(&scroll_handle)
            })
            .when_some(options.on_scroll_wheel, |strip, on_scroll_wheel| {
                strip.on_scroll_wheel(move |event, window, cx| {
                    on_scroll_wheel(event, window, cx);
                })
            })
            .children(options.tabs)
            .child(Self::render_end_drop_target(
                options.on_empty_double_click,
                options.forced_pin_state,
                options.tokens,
                options.border_color,
            ))
            .into_any_element()
    }

    fn render_end_drop_target(
        on_empty_double_click: Option<EmptyTabBarClickHandler>,
        forced_pin_state: Option<bool>,
        tokens: nucleotide_ui::tokens::DesignTokens,
        border_color: gpui::Hsla,
    ) -> gpui::AnyElement {
        let target = div()
            .id(end_drop_target_id(forced_pin_state))
            .min_w(px(24.0))
            .h_full()
            .flex_grow(1.0)
            .overflow_hidden()
            .bg(Self::empty_tab_bar_background(tokens))
            .shadow(Self::empty_tab_bar_inset_shadows(tokens))
            .child("")
            .when(
                end_drop_target_has_leading_border(forced_pin_state),
                |target| target.border_l_1().border_color(border_color),
            );

        target
            .when_some(on_empty_double_click, |target, handler| {
                target.on_click(move |event, window, cx| {
                    if event.click_count() >= 2 {
                        handler(event, window, cx);
                        cx.stop_propagation();
                    }
                })
            })
            .into_any_element()
    }

    fn empty_tab_bar_background(tokens: nucleotide_ui::tokens::DesignTokens) -> gpui::Hsla {
        tokens.tab_bar_tokens().container_background
    }

    fn empty_tab_bar_inset_shadows(
        tokens: nucleotide_ui::tokens::DesignTokens,
    ) -> Vec<gpui::BoxShadow> {
        vec![
            gpui::BoxShadow {
                color: nucleotide_ui::tokens::utils::with_alpha(tokens.chrome.border_shadow, 0.52),
                offset: gpui::point(px(1.0), px(1.0)),
                blur_radius: px(3.0),
                spread_radius: px(0.0),
                inset: true,
            },
            gpui::BoxShadow {
                color: nucleotide_ui::tokens::utils::with_alpha(
                    tokens.chrome.border_highlight,
                    0.20,
                ),
                offset: gpui::point(px(0.0), px(-1.0)),
                blur_radius: px(0.0),
                spread_radius: px(0.0),
                inset: true,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: Option<&str>, order: usize) -> DocumentInfo {
        DocumentInfo {
            id: DocumentId::default().into(),
            path: path.map(PathBuf::from),
            is_modified: false,
            is_readonly: false,
            is_deleted: false,
            is_pinned: false,
            is_preview: false,
            focused_at: std::time::Instant::now(),
            order,
            git_status: None,
            diagnostic_severity: None,
        }
    }

    fn pinned_doc(path: Option<&str>, order: usize) -> DocumentInfo {
        DocumentInfo {
            is_pinned: true,
            ..doc(path, order)
        }
    }

    fn label_parts(labels: &[DocumentTabLabel]) -> Vec<(&str, Option<&str>)> {
        labels
            .iter()
            .map(|label| (label.title.as_str(), label.detail.as_deref()))
            .collect()
    }

    #[test]
    fn labels_use_filenames_when_unique() {
        let tab_bar = TabBar::new(
            vec![],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        let labels = tab_bar.document_labels(&[
            doc(Some("/project/src/main.rs"), 0),
            doc(Some("/project/tests/tab.rs"), 1),
        ]);

        assert_eq!(
            label_parts(&labels),
            vec![("main.rs", None), ("tab.rs", None)]
        );
    }

    #[test]
    fn labels_truncate_long_titles_like_zed() {
        let tab_bar = TabBar::new(
            vec![],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        let labels =
            tab_bar.document_labels(&[doc(Some("/project/abcdefghijklmnopqrstuvwxyz.rs"), 0)]);

        assert_eq!(
            label_parts(&labels),
            vec![("abcdefghijklmnopqrstuvwx…", None)]
        );
    }

    #[test]
    fn labels_truncate_long_parent_details_like_zed() {
        let tab_bar = TabBar::new(
            vec![],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        let labels = tab_bar.document_labels(&[
            doc(Some("/project/12345678901234567890123a-long/mod.rs"), 0),
            doc(Some("/project/12345678901234567890123b-long/mod.rs"), 1),
        ]);

        assert_eq!(
            label_parts(&labels),
            vec![
                ("mod.rs", Some("12345678901234567890123a…")),
                ("mod.rs", Some("12345678901234567890123b…"))
            ]
        );
    }

    #[test]
    fn label_truncation_counts_characters_like_zed() {
        let input = "ééééééééééééééééééééééééé";

        assert_eq!(
            truncate_and_trailoff(input, MAX_TAB_TITLE_LEN),
            "éééééééééééééééééééééééé…"
        );
    }

    #[test]
    fn labels_add_zed_style_parent_detail_for_duplicates() {
        let tab_bar = TabBar::new(
            vec![],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        let labels = tab_bar.document_labels(&[
            doc(Some("/project/src/config/mod.rs"), 0),
            doc(Some("/project/tests/config/mod.rs"), 1),
        ]);

        assert_eq!(
            label_parts(&labels),
            vec![
                ("mod.rs", Some("src/config")),
                ("mod.rs", Some("tests/config"))
            ]
        );
    }

    #[test]
    fn build_tabs_preserve_zed_style_title_and_detail() {
        let documents = vec![
            doc(Some("/project/src/config/mod.rs"), 0),
            doc(Some("/project/tests/config/mod.rs"), 1),
        ];
        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs[0].label, "mod.rs");
        assert_eq!(tabs[0].label_detail(), Some("src/config"));
        assert_eq!(tabs[1].label, "mod.rs");
        assert_eq!(tabs[1].label_detail(), Some("tests/config"));
    }

    #[test]
    fn empty_tab_bar_area_uses_tabbar_background_and_inset_depth() {
        for tokens in [
            nucleotide_ui::DesignTokens::dark(),
            nucleotide_ui::DesignTokens::light(),
        ] {
            let shadows = TabBar::empty_tab_bar_inset_shadows(tokens);

            assert_eq!(
                TabBar::empty_tab_bar_background(tokens),
                tokens.tab_bar_tokens().container_background,
                "empty tabbar area should use the same background as the surrounding tabbar"
            );
            assert_eq!(shadows.len(), 2);
            assert!(
                shadows.iter().all(|shadow| shadow.inset),
                "empty tabbar depth should be inset, not cast outward"
            );
        }
    }

    #[test]
    fn builders_store_start_and_end_children() {
        let tab_bar = TabBar::new(vec![], None, None, |_, _, _| {}, |_, _, _| {})
            .start_child(div())
            .start_children([div()])
            .end_child(div())
            .end_children([div()])
            .show_pinned_tabs_in_separate_row(true)
            .show_close_button(TabCloseButtonVisibility::Hidden)
            .close_position(TabClosePosition::Left)
            .file_icons(true)
            .git_status(true)
            .show_diagnostics(TabDiagnosticsVisibility::All)
            .deemphasized(true)
            .with_empty_double_click_handler(|_, _, _| {})
            .with_pin_toggle_handler(|_, _, _| {})
            .with_readonly_toggle_handler(|_, _, _| {})
            .with_scroll_wheel_handler(|_, _, _| {})
            .with_double_click_handler(|_, _, _| {});

        assert_eq!(tab_bar.start_children.len(), 2);
        assert_eq!(tab_bar.end_children.len(), 2);
        assert!(tab_bar.show_pinned_tabs_in_separate_row);
        assert_eq!(tab_bar.show_close_button, TabCloseButtonVisibility::Hidden);
        assert_eq!(tab_bar.close_position, TabClosePosition::Left);
        assert!(tab_bar.file_icons);
        assert!(tab_bar.git_status);
        assert_eq!(tab_bar.show_diagnostics, TabDiagnosticsVisibility::All);
        assert!(tab_bar.deemphasized);
        assert!(tab_bar.on_tab_toggle_pin.is_some());
        assert!(tab_bar.on_tab_toggle_readonly.is_some());
        assert!(tab_bar.on_tab_double_click.is_some());
        assert!(tab_bar.on_empty_double_click.is_some());
        assert!(tab_bar.on_scroll_wheel.is_some());
    }

    #[test]
    fn build_tabs_propagates_close_button_visibility() {
        let tab_bar = TabBar::new(
            vec![doc(Some("/project/a.rs"), 0)],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .show_close_button(TabCloseButtonVisibility::Always);

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert_eq!(
            tabs[0].close_button_visibility(),
            TabCloseButtonVisibility::Always
        );
    }

    #[test]
    fn build_tabs_propagates_close_position() {
        let tab_bar = TabBar::new(
            vec![doc(Some("/project/a.rs"), 0)],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .close_position(TabClosePosition::Left);

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].close_position(), TabClosePosition::Left);
    }

    #[test]
    fn build_tabs_show_file_icons_by_default() {
        let tab_bar = TabBar::new(
            vec![doc(Some("/project/a.rs"), 0)],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].file_icons_visible());
    }

    #[test]
    fn build_tabs_allows_file_icons_to_be_disabled() {
        let tab_bar = TabBar::new(
            vec![doc(Some("/project/a.rs"), 0)],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .file_icons(false);

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(!tabs[0].file_icons_visible());
    }

    #[test]
    fn build_tabs_propagates_preview_state() {
        let tab_bar = TabBar::new(
            vec![DocumentInfo {
                is_preview: true,
                ..doc(Some("/project/a.rs"), 0)
            }],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].is_preview());
    }

    #[test]
    fn build_tabs_propagates_deemphasized_state() {
        let tab_bar = TabBar::new(
            vec![doc(Some("/project/a.rs"), 0)],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .deemphasized(true);

        let documents = tab_bar.ordered_documents();
        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].is_deemphasized());
    }

    #[test]
    fn build_tabs_propagates_readonly_state() {
        let documents = vec![DocumentInfo {
            is_readonly: true,
            ..doc(Some("/project/a.rs"), 0)
        }];

        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].is_readonly());
    }

    #[test]
    fn build_tabs_attaches_readonly_toggle_only_to_readonly_tabs() {
        let documents = vec![
            DocumentInfo {
                is_readonly: true,
                ..doc(Some("/project/locked.rs"), 0)
            },
            DocumentInfo {
                is_readonly: false,
                ..doc(Some("/project/editable.rs"), 1)
            },
        ];

        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .with_readonly_toggle_handler(|_, _, _| {});

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 2);
        assert!(tabs[0].has_readonly_toggle_handler());
        assert!(!tabs[1].has_readonly_toggle_handler());
    }

    #[test]
    fn build_tabs_propagates_deleted_state() {
        let documents = vec![DocumentInfo {
            is_deleted: true,
            ..doc(Some("/project/a.rs"), 0)
        }];

        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(tabs.len(), 1);
        assert!(tabs[0].is_deleted());
    }

    #[test]
    fn build_tabs_use_absolute_file_path_tooltips() {
        let documents = vec![doc(Some("/project/src/a.rs"), 0)];
        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(
            tabs[0].tooltip_text().map(|text| text.as_ref()),
            Some("/project/src/a.rs")
        );
    }

    #[test]
    fn build_tabs_use_label_tooltips_for_pathless_tabs() {
        let documents = vec![doc(None, 0)];
        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);

        assert_eq!(
            tabs[0].tooltip_text().map(|text| text.as_ref()),
            Some("[scratch]")
        );
    }

    #[test]
    fn build_tabs_gates_git_status_by_setting() {
        let documents = vec![DocumentInfo {
            git_status: Some(VcsStatus::Modified),
            ..doc(Some("/project/a.rs"), 0)
        }];

        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);
        assert_eq!(tabs[0].git_status, None);

        let tab_bar_with_status = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .git_status(true);

        let labels = tab_bar_with_status.document_labels(&documents);
        let tabs = tab_bar_with_status.build_tabs(&documents, &labels, 0);
        assert_eq!(tabs[0].git_status, Some(VcsStatus::Modified));
    }

    #[test]
    fn build_tabs_gates_diagnostics_by_setting_and_file_icons() {
        let documents = vec![DocumentInfo {
            diagnostic_severity: Some(DiagnosticSeverity::Warning),
            ..doc(Some("/project/a.rs"), 0)
        }];

        let default_tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        let labels = default_tab_bar.document_labels(&documents);
        let tabs = default_tab_bar.build_tabs(&documents, &labels, 0);
        assert_eq!(tabs[0].diagnostic_severity(), None);

        let all_tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .file_icons(false)
        .show_diagnostics(TabDiagnosticsVisibility::All);
        let labels = all_tab_bar.document_labels(&documents);
        let tabs = all_tab_bar.build_tabs(&documents, &labels, 0);
        assert_eq!(tabs[0].diagnostic_severity(), None);

        let all_tab_bar_with_icons = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .show_diagnostics(TabDiagnosticsVisibility::All);
        let labels = all_tab_bar_with_icons.document_labels(&documents);
        let tabs = all_tab_bar_with_icons.build_tabs(&documents, &labels, 0);
        assert_eq!(
            tabs[0].diagnostic_severity(),
            Some(DiagnosticSeverity::Warning)
        );

        let errors_only_tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .show_diagnostics(TabDiagnosticsVisibility::Errors);
        let labels = errors_only_tab_bar.document_labels(&documents);
        let tabs = errors_only_tab_bar.build_tabs(&documents, &labels, 0);
        assert_eq!(tabs[0].diagnostic_severity(), None);
    }

    #[test]
    fn build_tabs_shows_error_diagnostics_in_errors_mode() {
        let documents = vec![DocumentInfo {
            diagnostic_severity: Some(DiagnosticSeverity::Error),
            ..doc(Some("/project/a.rs"), 0)
        }];

        let tab_bar = TabBar::new(
            documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .show_diagnostics(TabDiagnosticsVisibility::Errors);

        let labels = tab_bar.document_labels(&documents);
        let tabs = tab_bar.build_tabs(&documents, &labels, 0);
        assert_eq!(
            tabs[0].diagnostic_severity(),
            Some(DiagnosticSeverity::Error)
        );
    }

    #[test]
    fn separate_pinned_row_requires_opt_in_and_mixed_tabs() {
        let mixed_documents = vec![
            pinned_doc(Some("/project/a.rs"), 0),
            doc(Some("/project/b.rs"), 1),
        ];
        let pinned_only_documents = vec![
            pinned_doc(Some("/project/a.rs"), 0),
            pinned_doc(Some("/project/b.rs"), 1),
        ];
        let unpinned_only_documents =
            vec![doc(Some("/project/a.rs"), 0), doc(Some("/project/b.rs"), 1)];

        let default_tab_bar = TabBar::new(
            mixed_documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );
        assert!(!default_tab_bar.should_render_separate_pinned_row(&mixed_documents));

        let split_tab_bar = TabBar::new(
            mixed_documents.clone(),
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        )
        .show_pinned_tabs_in_separate_row(true);

        assert!(split_tab_bar.should_render_separate_pinned_row(&mixed_documents));
        assert!(!split_tab_bar.should_render_separate_pinned_row(&pinned_only_documents));
        assert!(!split_tab_bar.should_render_separate_pinned_row(&unpinned_only_documents));
    }

    #[test]
    fn tab_position_is_relative_to_rendered_row() {
        assert_eq!(tab_position(0, 3, Some(2)), TabPosition::First);
        assert_eq!(
            tab_position(1, 3, Some(2)),
            TabPosition::Middle(Ordering::Less)
        );
        assert_eq!(tab_position(2, 3, Some(2)), TabPosition::Last);
        assert_eq!(
            tab_position(1, 4, Some(1)),
            TabPosition::Middle(Ordering::Equal)
        );
        assert_eq!(
            tab_position(2, 4, Some(1)),
            TabPosition::Middle(Ordering::Greater)
        );
    }

    #[test]
    fn tab_double_click_requires_at_least_two_clicks() {
        assert!(!is_tab_double_click(0));
        assert!(!is_tab_double_click(1));
        assert!(is_tab_double_click(2));
        assert!(is_tab_double_click(3));
    }

    #[test]
    fn pinned_scroll_separator_matches_zed_scroll_threshold() {
        assert!(!should_render_pinned_scroll_separator(
            gpui::px(0.0),
            gpui::px(-1.0)
        ));
        assert!(!should_render_pinned_scroll_separator(
            gpui::px(2.0),
            gpui::px(-1.0)
        ));
        assert!(should_render_pinned_scroll_separator(
            gpui::px(2.1),
            gpui::px(-1.0)
        ));
    }

    #[test]
    fn pinned_scroll_separator_requires_scrolled_unpinned_tabs() {
        assert!(!should_render_pinned_scroll_separator(
            gpui::px(8.0),
            gpui::px(0.0)
        ));
        assert!(!should_render_pinned_scroll_separator(
            gpui::px(8.0),
            gpui::px(1.0)
        ));
        assert!(should_render_pinned_scroll_separator(
            gpui::px(8.0),
            gpui::px(-0.5)
        ));
    }

    #[test]
    fn tab_bar_control_spacing_matches_zed() {
        let tokens = nucleotide_ui::DesignTokens::dark();

        assert_eq!(tab_bar_control_gap(&tokens), px(4.0));
        assert_eq!(tab_bar_control_padding_x(), px(6.0));
    }

    #[test]
    fn end_drop_target_identity_matches_zed_rows() {
        assert_eq!(end_drop_target_id(None), "tab_bar_drop_target");
        assert_eq!(end_drop_target_id(Some(false)), "tab_bar_drop_target");
        assert_eq!(end_drop_target_id(Some(true)), "pinned_tabs_border");
    }

    #[test]
    fn only_pinned_row_end_drop_target_gets_leading_border() {
        assert!(!end_drop_target_has_leading_border(None));
        assert!(!end_drop_target_has_leading_border(Some(false)));
        assert!(end_drop_target_has_leading_border(Some(true)));
    }

    #[test]
    fn ordered_documents_place_pinned_tabs_first() {
        let tab_bar = TabBar::new(
            vec![
                doc(Some("/project/a.rs"), 0),
                pinned_doc(Some("/project/b.rs"), 1),
                doc(Some("/project/c.rs"), 2),
                pinned_doc(Some("/project/d.rs"), 3),
            ],
            None,
            Some(PathBuf::from("/project")),
            |_, _, _| {},
            |_, _, _| {},
        );

        let labels = tab_bar.document_labels(&tab_bar.ordered_documents());

        assert_eq!(
            label_parts(&labels),
            vec![
                ("b.rs", None),
                ("d.rs", None),
                ("a.rs", None),
                ("c.rs", None)
            ]
        );
    }
}
