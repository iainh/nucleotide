// ABOUTME: GPUI-native prompt component for text input with completion support
// ABOUTME: Replaces dependency on helix_term::ui::Prompt with a proper GPUI implementation

use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub text: SharedString,
    pub description: Option<SharedString>,
}

pub struct PromptView {
    // Core prompt state
    prompt: SharedString,
    input: SharedString,
    cursor_position: usize,
    
    // Completion state
    completions: Vec<CompletionItem>,
    completion_selection: usize,
    show_completions: bool,
    
    // UI state
    focus_handle: FocusHandle,
    
    // Callbacks
    on_submit: Option<Box<dyn FnMut(&str, &mut Context<Self>) + 'static>>,
    on_cancel: Option<Box<dyn FnMut(&mut Context<Self>) + 'static>>,
    on_change: Option<Box<dyn FnMut(&str, &mut Context<Self>) + 'static>>,
    
    // Styling
    style: PromptStyle,
}

#[derive(Clone)]
pub struct PromptStyle {
    pub background: Hsla,
    pub text: Hsla,
    pub prompt_text: Hsla,
    pub border: Hsla,
    pub completion_background: Hsla,
    pub completion_selected: Hsla,
}

impl Default for PromptStyle {
    fn default() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.1, 1.0),
            text: hsla(0.0, 0.0, 0.9, 1.0),
            prompt_text: hsla(220.0 / 360.0, 0.6, 0.7, 1.0),
            border: hsla(0.0, 0.0, 0.3, 1.0),
            completion_background: hsla(0.0, 0.0, 0.15, 1.0),
            completion_selected: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
        }
    }
}

impl PromptView {
    pub fn new(prompt: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            prompt: prompt.into(),
            input: SharedString::default(),
            cursor_position: 0,
            completions: Vec::new(),
            completion_selection: 0,
            show_completions: false,
            focus_handle: cx.focus_handle(),
            on_submit: None,
            on_cancel: None,
            on_change: None,
            style: PromptStyle::default(),
        }
    }
    
    pub fn with_style(mut self, style: PromptStyle) -> Self {
        self.style = style;
        self
    }
    
    pub fn on_submit(mut self, callback: impl FnMut(&str, &mut Context<Self>) + 'static) -> Self {
        self.on_submit = Some(Box::new(callback));
        self
    }
    
    pub fn on_cancel(mut self, callback: impl FnMut(&mut Context<Self>) + 'static) -> Self {
        self.on_cancel = Some(Box::new(callback));
        self
    }
    
    pub fn on_change(mut self, callback: impl FnMut(&str, &mut Context<Self>) + 'static) -> Self {
        self.on_change = Some(Box::new(callback));
        self
    }
    
    pub fn set_completions(&mut self, completions: Vec<CompletionItem>, cx: &mut Context<Self>) {
        self.completions = completions;
        self.completion_selection = 0;
        self.show_completions = !self.completions.is_empty();
        cx.notify();
    }
    
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input = SharedString::from(text.to_string());
        self.cursor_position = text.len();
        cx.notify();
    }
    
    fn insert_char(&mut self, ch: char, cx: &mut Context<Self>) {
        let mut input = self.input.to_string();
        input.insert(self.cursor_position, ch);
        self.input = input.into();
        self.cursor_position += ch.len_utf8();
        
        if let Some(on_change) = &mut self.on_change {
            on_change(&self.input, cx);
        }
        
        cx.notify();
    }
    
    fn delete_char(&mut self, cx: &mut Context<Self>) {
        if self.cursor_position > 0 {
            let mut input = self.input.to_string();
            let mut chars: Vec<char> = input.chars().collect();
            let char_pos = self.cursor_position.saturating_sub(1);
            if char_pos < chars.len() {
                chars.remove(char_pos);
                input = chars.into_iter().collect();
                self.input = input.into();
                self.cursor_position = char_pos;
                
                if let Some(on_change) = &mut self.on_change {
                    on_change(&self.input, cx);
                }
                
                cx.notify();
            }
        }
    }
    
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let input_len = self.input.chars().count();
        if delta > 0 {
            self.cursor_position = (self.cursor_position + delta as usize).min(input_len);
        } else {
            self.cursor_position = self.cursor_position.saturating_sub((-delta) as usize);
        }
        cx.notify();
    }
    
    fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.completions.is_empty() {
            return;
        }
        
        if delta > 0 {
            self.completion_selection = (self.completion_selection + delta as usize).min(self.completions.len() - 1);
        } else {
            self.completion_selection = self.completion_selection.saturating_sub((-delta) as usize);
        }
        cx.notify();
    }
    
    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        if self.show_completions && !self.completions.is_empty() {
            if let Some(completion) = self.completions.get(self.completion_selection) {
                self.input = completion.text.clone();
                self.cursor_position = self.input.chars().count();
                self.show_completions = false;
                
                if let Some(on_change) = &mut self.on_change {
                    on_change(&self.input, cx);
                }
                
                cx.notify();
            }
        }
    }
    
    fn submit(&mut self, cx: &mut Context<Self>) {
        if let Some(on_submit) = &mut self.on_submit {
            on_submit(&self.input, cx);
        }
    }
    
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(on_cancel) = &mut self.on_cancel {
            on_cancel(cx);
        }
    }
}

impl Focusable for PromptView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PromptView {}

impl Render for PromptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = cx.global::<crate::FontSettings>().fixed_font.clone();
        let input_display = if self.input.is_empty() {
            "".to_string()
        } else {
            self.input.to_string()
        };
        
        // Create cursor indicator
        let cursor_indicator = if self.focus_handle.is_focused(window) {
            "│"
        } else {
            " "
        };
        
        div()
            .flex()
            .flex_col()
            .w(px(500.))
            .bg(self.style.background)
            .border_1()
            .border_color(self.style.border)
            .rounded_md()
            .shadow_lg()
            .font(font)
            .text_size(px(14.))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                println!("🔥 PromptView received key: {}", event.keystroke.key);
                match event.keystroke.key.as_str() {
                    "enter" => {
                        if this.show_completions && !this.completions.is_empty() {
                            this.accept_completion(cx);
                        } else {
                            this.submit(cx);
                        }
                    }
                    "escape" => {
                        if this.show_completions {
                            this.show_completions = false;
                            cx.notify();
                        } else {
                            this.cancel(cx);
                        }
                    }
                    "tab" => {
                        if this.show_completions && !this.completions.is_empty() {
                            this.accept_completion(cx);
                        }
                    }
                    "up" => {
                        if this.show_completions {
                            this.move_completion_selection(-1, cx);
                        }
                    }
                    "down" => {
                        if this.show_completions {
                            this.move_completion_selection(1, cx);
                        }
                    }
                    "left" => {
                        this.move_cursor(-1, cx);
                    }
                    "right" => {
                        this.move_cursor(1, cx);
                    }
                    "backspace" => {
                        this.delete_char(cx);
                    }
                    key if key.len() == 1 => {
                        if let Some(ch) = key.chars().next() {
                            if ch.is_alphanumeric() || ch.is_ascii_punctuation() || ch == ' ' {
                                this.insert_char(ch, cx);
                            }
                        }
                    }
                    _ => {
                        println!("🔥 Unhandled key: {}", event.keystroke.key);
                    }
                }
            }))
            .child(
                // Input line
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .text_color(self.style.prompt_text)
                            .child(self.prompt.clone())
                    )
                    .child(
                        div()
                            .flex_1()
                            .ml_2()
                            .text_color(self.style.text)
                            .child(format!("{}{}", input_display, cursor_indicator))
                    )
            )
            .when(self.show_completions && !self.completions.is_empty(), |this| {
                this.child(
                    div()
                        .border_t_1()
                        .border_color(self.style.border)
                        .bg(self.style.completion_background)
                        .max_h(px(200.))
                        .overflow_y_hidden()
                        .children(self.completions.iter().enumerate().map(|(idx, completion)| {
                            let is_selected = idx == self.completion_selection;
                            div()
                                .flex()
                                .flex_col()
                                .px_3()
                                .py_1()
                                .when(is_selected, |this| {
                                    this.bg(self.style.completion_selected)
                                })
                                .child(
                                    div()
                                        .text_color(if is_selected { 
                                            hsla(0.0, 0.0, 1.0, 1.0) 
                                        } else { 
                                            self.style.text 
                                        })
                                        .child(completion.text.clone())
                                )
                                .when_some(completion.description.as_ref(), |this, desc| {
                                    this.child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(if is_selected {
                                                hsla(0.0, 0.0, 0.8, 1.0)
                                            } else {
                                                self.style.prompt_text
                                            })
                                            .child(desc.clone())
                                    )
                                })
                        }))
                )
            })
    }
}