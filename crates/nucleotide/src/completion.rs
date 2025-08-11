// ABOUTME: Completion view component for LSP completions
// ABOUTME: Displays and manages completion suggestions in a popup

use gpui::prelude::FluentBuilder;
use gpui::*;

/// LSP completion view component
pub struct CompletionView {
    focus_handle: FocusHandle,
    items: Vec<CompletionItem>,
    selected_index: usize,
    visible: bool,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: SharedString,
    pub kind: CompletionItemKind,
    pub detail: Option<SharedString>,
    pub documentation: Option<SharedString>,
    pub insert_text: Option<SharedString>,
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
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible() {
            return div().id("completion-hidden");
        }

        div()
            .id("completion-popup")
            .key_context("CompletionView")
            .track_focus(&self.focus_handle)
            .bg(gpui::white())
            .border_1()
            .border_color(gpui::gray())
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
                                .when(is_selected, |div| div.bg(gpui::blue().with_alpha(0.2)))
                                .child(div().text_sm().child(item.label.clone()))
                                .when_some(item.detail.as_ref(), |div, detail| {
                                    div.child(
                                        div()
                                            .text_xs()
                                            .text_color(gpui::gray())
                                            .child(detail.clone()),
                                    )
                                })
                        })
                        .collect::<Vec<_>>(),
                ),
            )
    }
}
