// ABOUTME: GPUI-native completion component for displaying LSP completion suggestions
// ABOUTME: Provides autocomplete functionality with keyboard navigation and insertion

use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: SharedString,
    pub kind: CompletionItemKind,
    pub detail: Option<SharedString>,
    pub documentation: Option<SharedString>,
    pub insert_text: Option<SharedString>,
    pub filter_text: Option<SharedString>,
    pub sort_text: Option<SharedString>,
    pub preselect: bool,
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

impl CompletionItemKind {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Text => "📝",
            Self::Method => "🔧",
            Self::Function => "ƒ",
            Self::Constructor => "🏗️",
            Self::Field => "🏷️",
            Self::Variable => "📦",
            Self::Class => "🏛️",
            Self::Interface => "🔌",
            Self::Module => "📚",
            Self::Property => "🔧",
            Self::Unit => "📏",
            Self::Value => "💎",
            Self::Enum => "🔢",
            Self::Keyword => "🔑",
            Self::Snippet => "✂️",
            Self::Color => "🎨",
            Self::File => "📄",
            Self::Reference => "🔗",
            Self::Folder => "📁",
            Self::EnumMember => "🔸",
            Self::Constant => "🔒",
            Self::Struct => "🏗️",
            Self::Event => "⚡",
            Self::Operator => "➕",
            Self::TypeParameter => "🏷️",
        }
    }
}

pub struct CompletionView {
    // Core completion state
    items: Vec<CompletionItem>,
    filtered_items: Vec<usize>,
    selected_index: usize,
    trigger_offset: usize,
    
    // Filtering
    filter_text: SharedString,
    
    // UI state
    focus_handle: FocusHandle,
    max_visible_items: usize,
    scroll_offset: usize,
    
    // Positioning
    anchor_position: Point<Pixels>,
    
    // Callbacks
    on_select: Option<Box<dyn FnMut(&CompletionItem, &mut Context<Self>) + 'static>>,
    on_dismiss: Option<Box<dyn FnMut(&mut Context<Self>) + 'static>>,
    
    // Styling
    style: CompletionStyle,
}

#[derive(Clone)]
pub struct CompletionStyle {
    pub background: Hsla,
    pub text: Hsla,
    pub selected_background: Hsla,
    pub selected_text: Hsla,
    pub border: Hsla,
    pub detail_text: Hsla,
    pub kind_text: Hsla,
}

impl Default for CompletionStyle {
    fn default() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.1, 0.95),
            text: hsla(0.0, 0.0, 0.9, 1.0),
            selected_background: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            selected_text: hsla(0.0, 0.0, 1.0, 1.0),
            border: hsla(0.0, 0.0, 0.3, 1.0),
            detail_text: hsla(0.0, 0.0, 0.6, 1.0),
            kind_text: hsla(120.0 / 360.0, 0.5, 0.7, 1.0),
        }
    }
}

impl CompletionView {
    pub fn new(items: Vec<CompletionItem>, anchor_position: Point<Pixels>, cx: &mut Context<Self>) -> Self {
        let filtered_items: Vec<usize> = (0..items.len()).collect();
        
        Self {
            items,
            filtered_items,
            selected_index: 0,
            trigger_offset: 0,
            filter_text: SharedString::default(),
            focus_handle: cx.focus_handle(),
            max_visible_items: 10,
            scroll_offset: 0,
            anchor_position,
            on_select: None,
            on_dismiss: None,
            style: CompletionStyle::default(),
        }
    }
    
    pub fn with_style(mut self, style: CompletionStyle) -> Self {
        self.style = style;
        self
    }
    
    pub fn with_filter_text(mut self, filter_text: impl Into<SharedString>) -> Self {
        self.filter_text = filter_text.into();
        self.apply_filter();
        self
    }
    
    pub fn on_select(mut self, callback: impl FnMut(&CompletionItem, &mut Context<Self>) + 'static) -> Self {
        self.on_select = Some(Box::new(callback));
        self
    }
    
    pub fn on_dismiss(mut self, callback: impl FnMut(&mut Context<Self>) + 'static) -> Self {
        self.on_dismiss = Some(Box::new(callback));
        self
    }
    
    fn apply_filter(&mut self) {
        if self.filter_text.is_empty() {
            self.filtered_items = (0..self.items.len()).collect();
        } else {
            let filter_lower = self.filter_text.to_lowercase();
            self.filtered_items = self.items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let search_text = item.filter_text.as_ref().unwrap_or(&item.label);
                    search_text.to_lowercase().contains(&filter_lower)
                })
                .map(|(idx, _)| idx)
                .collect();
        }
        
        // Reset selection to first item
        self.selected_index = 0;
        self.scroll_offset = 0;
    }
    
    pub fn update_filter(&mut self, filter_text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.filter_text = filter_text.into();
        self.apply_filter();
        cx.notify();
    }
    
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered_items.is_empty() {
            return;
        }
        
        let old_index = self.selected_index;
        let new_index = if delta > 0 {
            (self.selected_index + delta as usize).min(self.filtered_items.len() - 1)
        } else {
            self.selected_index.saturating_sub((-delta) as usize)
        };
        
        self.selected_index = new_index;
        println!("🎯 CompletionView selection moved from {} to {} (delta: {})", old_index, new_index, delta);
        
        // Adjust scroll to keep selection visible
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible_items {
            self.scroll_offset = self.selected_index - self.max_visible_items + 1;
        }
        
        cx.notify();
    }
    
    fn move_to_first(&mut self, cx: &mut Context<Self>) {
        if self.filtered_items.is_empty() {
            return;
        }
        
        self.selected_index = 0;
        self.scroll_offset = 0;
        println!("🎯 CompletionView moved to first item");
        cx.notify();
    }
    
    fn move_to_last(&mut self, cx: &mut Context<Self>) {
        if self.filtered_items.is_empty() {
            return;
        }
        
        self.selected_index = self.filtered_items.len() - 1;
        // Adjust scroll to show the last item
        if self.filtered_items.len() > self.max_visible_items {
            self.scroll_offset = self.filtered_items.len() - self.max_visible_items;
        } else {
            self.scroll_offset = 0;
        }
        println!("🎯 CompletionView moved to last item");
        cx.notify();
    }
    
    fn select_current(&mut self, cx: &mut Context<Self>) {
        if let Some(item_idx) = self.filtered_items.get(self.selected_index) {
            if let Some(item) = self.items.get(*item_idx) {
                if let Some(on_select) = &mut self.on_select {
                    on_select(item, cx);
                }
            }
        }
    }
    
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(on_dismiss) = &mut self.on_dismiss {
            on_dismiss(cx);
        }
    }
    
    pub fn is_empty(&self) -> bool {
        self.filtered_items.is_empty()
    }
    
    pub fn item_count(&self) -> usize {
        self.filtered_items.len()
    }
}

impl Focusable for CompletionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for CompletionView {}

impl Render for CompletionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = cx.global::<crate::FontSettings>().fixed_font.clone();
        
        if self.filtered_items.is_empty() {
            return div().size_0().into_any_element();
        }
        
        let visible_items = self.filtered_items
            .iter()
            .skip(self.scroll_offset)
            .take(self.max_visible_items)
            .enumerate()
            .map(|(visible_idx, &item_idx)| {
                let item = &self.items[item_idx];
                let is_selected = visible_idx + self.scroll_offset == self.selected_index;
                
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .min_h(px(24.))
                    .when(is_selected, |this| {
                        this.bg(self.style.selected_background)
                            .text_color(self.style.selected_text)
                    })
                    .when(!is_selected, |this| {
                        this.text_color(self.style.text)
                    })
                    .child(
                        // Kind icon
                        div()
                            .w(px(20.))
                            .flex_shrink_0()
                            .text_color(self.style.kind_text)
                            .text_size(px(12.))
                            .child(item.kind.icon())
                    )
                    .child(
                        // Main content
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                // Label
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(item.label.clone())
                            )
                            .when_some(item.detail.as_ref(), |this, detail| {
                                this.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(if is_selected {
                                            hsla(0.0, 0.0, 0.8, 1.0)
                                        } else {
                                            self.style.detail_text
                                        })
                                        .child(detail.clone())
                                )
                            })
                    )
            })
            .collect::<Vec<_>>();
        
        // Note: Focus is handled automatically by the overlay view - don't manually focus here
        println!("🎯 CompletionView render: Rendering completion component");
        
        div()
            .absolute()
            .left(self.anchor_position.x)
            .top(self.anchor_position.y)
            .flex()
            .flex_col()
            .w(px(300.))
            .max_h(px(400.))
            .bg(self.style.background)
            .border_1()
            .border_color(self.style.border)
            .rounded_md()
            .shadow_lg()
            .font(font)
            .text_size(px(13.))
            .key_context("completion")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionSelectPrev, _window, cx| {
                println!("🔥 CompletionView received CompletionSelectPrev action");
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionSelectNext, _window, cx| {
                println!("🔥 CompletionView received CompletionSelectNext action");
                this.move_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionConfirm, _window, cx| {
                println!("🔥 CompletionView received CompletionConfirm action");
                this.select_current(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionDismiss, _window, cx| {
                println!("🔥 CompletionView received CompletionDismiss action");
                this.dismiss(cx);
                cx.emit(DismissEvent);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionSelectFirst, _window, cx| {
                println!("🔥 CompletionView received CompletionSelectFirst action");
                this.move_to_first(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::completion::CompletionSelectLast, _window, cx| {
                println!("🔥 CompletionView received CompletionSelectLast action");
                this.move_to_last(cx);
            }))
            .children(visible_items)
            .when(self.filtered_items.len() > self.max_visible_items, |this| {
                let scroll_indicator_height = 
                    (self.max_visible_items as f32 / self.filtered_items.len() as f32) * 200.0;
                let scroll_position = 
                    (self.scroll_offset as f32 / (self.filtered_items.len() - self.max_visible_items) as f32) * (200.0 - scroll_indicator_height);
                
                this.child(
                    div()
                        .absolute()
                        .right(px(2.))
                        .top(px(2.))
                        .w(px(2.))
                        .h(px(200.))
                        .bg(hsla(0.0, 0.0, 0.3, 0.3))
                        .child(
                            div()
                                .w_full()
                                .h(px(scroll_indicator_height))
                                .bg(self.style.border)
                                .rounded_sm()
                                .relative()
                                .top(px(scroll_position))
                        )
                )
            }).into_any_element()
    }
}

// Factory function for creating completion items from LSP data
impl CompletionItem {
    pub fn new(label: impl Into<SharedString>, kind: CompletionItemKind) -> Self {
        Self {
            label: label.into(),
            kind,
            detail: None,
            documentation: None,
            insert_text: None,
            filter_text: None,
            sort_text: None,
            preselect: false,
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
    
    pub fn with_filter_text(mut self, filter_text: impl Into<SharedString>) -> Self {
        self.filter_text = Some(filter_text.into());
        self
    }
    
    pub fn preselected(mut self) -> Self {
        self.preselect = true;
        self
    }
    
    pub fn get_insert_text(&self) -> &str {
        self.insert_text.as_ref().unwrap_or(&self.label)
    }
}