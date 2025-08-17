// ABOUTME: Completion view component for LSP completions
// ABOUTME: Displays and manages completion suggestions in a popup

use gpui::prelude::FluentBuilder;
use gpui::{
    div, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
};

/// LSP completion view component
pub struct CompletionView {
    focus_handle: FocusHandle,
    items: Vec<CompletionItem>,
    selected_index: usize,
    visible: bool,
}

impl EventEmitter<DismissEvent> for CompletionView {}

impl Focusable for CompletionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: SharedString,
    pub kind: CompletionItemKind,
    pub detail: Option<SharedString>,
    pub documentation: Option<SharedString>,
    pub insert_text: Option<SharedString>,
}

impl CompletionItem {
    pub fn new(label: impl Into<SharedString>, kind: CompletionItemKind) -> Self {
        Self {
            label: label.into(),
            kind,
            detail: None,
            documentation: None,
            insert_text: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_documentation(mut self, documentation: impl Into<SharedString>) -> Self {
        self.documentation = Some(documentation.into());
        self
    }

    pub fn with_insert_text(mut self, insert_text: impl Into<SharedString>) -> Self {
        self.insert_text = Some(insert_text.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompletionItemKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl CompletionView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            items: Vec::new(),
            selected_index: 0,
            visible: false,
        }
    }

    pub fn set_items(&mut self, items: Vec<CompletionItem>) {
        self.items = items;
        self.selected_index = 0;
        self.visible = !self.items.is_empty();
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.items.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.items.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.items.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected_index)
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.items.clear();
    }

    pub fn is_visible(&self) -> bool {
        self.visible && !self.items.is_empty()
    }
}

impl Render for CompletionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible() {
            return div().id("completion-hidden");
        }

        // Access theme from global state
        let theme = cx.global::<crate::Theme>();
        let tokens = &theme.tokens;

        div()
            .id("completion-popup")
            .key_context("CompletionView")
            .track_focus(&self.focus_handle)
            .bg(tokens.colors.popup_background)
            .border_1()
            .border_color(tokens.colors.popup_border)
            .rounded_md()
            .shadow_lg()
            .max_h_48()
            .overflow_y_scroll()
            .child(
                div().flex().flex_col().children(
                    self.items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let is_selected = index == self.selected_index;
                            div()
                                .px_2()
                                .py_1()
                                .when(is_selected, |div| div.bg(tokens.colors.selection_primary))
                                .when(!is_selected, |div| {
                                    div.hover(|style| style.bg(tokens.colors.selection_secondary))
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(tokens.colors.text_primary)
                                        .child(item.label.clone()),
                                )
                                .when_some(item.detail.as_ref(), |el, detail| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(tokens.colors.text_secondary)
                                            .child(detail.clone()),
                                    )
                                })
                        })
                        .collect::<Vec<_>>(),
                ),
            )
    }
}
