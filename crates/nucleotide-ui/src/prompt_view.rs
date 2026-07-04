// ABOUTME: GPUI-native prompt component for text input with completion support
// ABOUTME: Replaces the terminal prompt widget with a proper GPUI implementation

use crate::actions::prompt::{
    Cancel, Confirm, DeleteChar, MoveCursorLeft, MoveCursorRight, MoveToEnd, MoveToStart,
    NextCompletion, PrevCompletion,
};
use crate::common::ModalStyle;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render, SharedString, Styled, Task,
    Window, div, px, svg,
};

pub(crate) const PROMPT_CONTEXT: &str = "PromptView";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm, Some(PROMPT_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(PROMPT_CONTEXT)),
        KeyBinding::new("up", PrevCompletion, Some(PROMPT_CONTEXT)),
        KeyBinding::new("down", NextCompletion, Some(PROMPT_CONTEXT)),
        KeyBinding::new("left", MoveCursorLeft, Some(PROMPT_CONTEXT)),
        KeyBinding::new("right", MoveCursorRight, Some(PROMPT_CONTEXT)),
        KeyBinding::new("home", MoveToStart, Some(PROMPT_CONTEXT)),
        KeyBinding::new("end", MoveToEnd, Some(PROMPT_CONTEXT)),
        KeyBinding::new("backspace", DeleteChar, Some(PROMPT_CONTEXT)),
    ]);
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub text: SharedString,
    pub description: Option<SharedString>,
    /// Optional display text that's shown in the completion list but not inserted
    /// If None, the `text` field is used for both display and insertion
    pub display_text: Option<SharedString>,
}

// Type aliases for callbacks
type PromptSubmitCallback = Box<dyn FnMut(&str, &mut Context<PromptView>) + 'static>;
type PromptCancelCallback = Box<dyn FnMut(&mut Context<PromptView>) + 'static>;
type PromptChangeCallback = Box<dyn FnMut(&str, &mut Context<PromptView>) + 'static>;
type PromptCompletionFn = Box<dyn Fn(&str) -> Vec<CompletionItem> + 'static>;
type PromptCompletionTask = Task<Vec<CompletionItem>>;
type PromptCompletionTaskFn =
    Box<dyn Fn(&str, &mut Context<PromptView>) -> Option<PromptCompletionTask> + 'static>;

const ASYNC_COMPLETION_TASK_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);

pub struct PromptView {
    // Core prompt state
    prompt: SharedString,
    input: SharedString,
    cursor_position: usize,

    // Command history
    history: Vec<SharedString>,
    history_position: Option<usize>,

    // Completion state
    completions: Vec<CompletionItem>,
    completion_selection: usize,
    show_completions: bool,
    original_input: Option<SharedString>, // Store original input before showing completions
    completion_generation: u64,
    completion_task: Option<Task<()>>,

    // UI state
    focus_handle: FocusHandle,
    completion_scroll_offset: usize,

    // Callbacks
    on_submit: Option<PromptSubmitCallback>,
    on_cancel: Option<PromptCancelCallback>,
    on_change: Option<PromptChangeCallback>,
    completion_fn: Option<PromptCompletionFn>,
    completion_task_fn: Option<PromptCompletionTaskFn>,

    // Styling
    style: PromptStyle,
}

#[derive(Clone)]
pub struct PromptStyle {
    pub modal_style: ModalStyle,
    pub completion_background: Hsla,
}

impl Default for PromptStyle {
    fn default() -> Self {
        let dt = crate::DesignTokens::dark();
        Self {
            modal_style: ModalStyle::default(),
            completion_background: dt.chrome.menu_background,
        }
    }
}

impl PromptStyle {
    pub fn from_helix_theme(theme: &helix_view::Theme) -> Self {
        // Prefer provider tokens for OKLab/OKLCH-driven colors
        if let Some(provider) = crate::providers::use_theme_provider() {
            let ui = provider.current_theme();
            let dt = ui.tokens;
            return Self {
                modal_style: ModalStyle::from_theme(theme),
                completion_background: dt.chrome.menu_background,
            };
        }

        let modal_style = ModalStyle::from_theme(theme);
        Self {
            completion_background: modal_style.background,
            modal_style,
        }
    }
}

// Helper function to create an icon element
fn create_icon(icon_path: String, size: f32, color: Option<Hsla>) -> impl IntoElement {
    let mut icon = svg().path(icon_path).size(gpui::px(size)).flex_shrink_0();

    if let Some(color) = color {
        icon = icon.text_color(color);
    }

    icon
}

impl PromptView {
    fn font_from_ui_config(ui_font: &nucleotide_types::UiFontConfig) -> gpui::Font {
        gpui::Font {
            family: ui_font.family.clone().into(),
            features: gpui::FontFeatures::default(),
            weight: ui_font.weight.into(),
            style: gpui::FontStyle::Normal,
            fallbacks: None,
        }
    }

    fn ui_font(cx: &mut Context<Self>) -> gpui::Font {
        let ui_font = cx.global::<nucleotide_types::UiFontConfig>();
        Self::font_from_ui_config(ui_font)
    }

    pub fn new(prompt: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        // Register prompt focus with the global coordinator if available
        if let Some(coord) = cx.try_global::<crate::FocusCoordinator>() {
            coord.set_prompt_focus(focus_handle.clone());
        }

        Self {
            prompt: prompt.into(),
            input: SharedString::default(),
            cursor_position: 0,
            history: Vec::new(),
            history_position: None,
            completions: Vec::new(),
            completion_selection: 0,
            show_completions: false,
            original_input: None,
            completion_generation: 0,
            completion_task: None,
            focus_handle,
            completion_scroll_offset: 0,
            on_submit: None,
            on_cancel: None,
            on_change: None,
            completion_fn: None,
            completion_task_fn: None,
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

    pub fn with_completion_fn(
        mut self,
        completion_fn: impl Fn(&str) -> Vec<CompletionItem> + 'static,
    ) -> Self {
        self.completion_fn = Some(Box::new(completion_fn));
        self
    }

    pub fn with_completion_task_fn(
        mut self,
        completion_task_fn: impl Fn(&str, &mut Context<PromptView>) -> Option<PromptCompletionTask>
        + 'static,
    ) -> Self {
        self.completion_task_fn = Some(Box::new(completion_task_fn));
        self
    }

    pub fn set_completions(&mut self, completions: Vec<CompletionItem>, cx: &mut Context<Self>) {
        self.apply_completions(completions, cx);
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input = SharedString::from(text.to_string());
        self.cursor_position = text.len();

        // Recalculate completions for the initial text
        self.recalculate_completions(cx);

        cx.notify();
    }

    fn insert_char(&mut self, ch: char, cx: &mut Context<Self>) {
        let mut input = self.input.to_string();
        let byte_pos = self.byte_position_from_char_position(&input, self.cursor_position);
        input.insert(byte_pos, ch);
        self.input = input.into();
        self.cursor_position += 1; // Move cursor by one character

        // Recalculate completions
        self.recalculate_completions(cx);

        if let Some(on_change) = &mut self.on_change {
            on_change(&self.input, cx);
        }

        cx.notify();
    }

    fn byte_position_from_char_position(&self, s: &str, char_pos: usize) -> usize {
        s.chars().take(char_pos).map(char::len_utf8).sum()
    }

    fn recalculate_completions(&mut self, cx: &mut Context<Self>) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.completion_task = None;

        let input = self.input.clone();
        if let Some(completion_fn) = &self.completion_fn {
            let completions = completion_fn(&input);
            self.apply_completions(completions, cx);
        }

        if self.completion_task_fn.is_none() {
            return;
        }

        let expected_input = input.clone();
        let generation = self.completion_generation;
        let executor = cx.background_executor().clone();
        self.completion_task = Some(cx.spawn(async move |view_weak, cx| {
            executor.timer(ASYNC_COMPLETION_TASK_DEBOUNCE).await;

            let task = view_weak
                .update(cx, |this, cx| {
                    if this.completion_generation != generation || this.input != expected_input {
                        return None;
                    }

                    this.completion_task_fn
                        .as_ref()
                        .and_then(|completion_task_fn| completion_task_fn(&expected_input, cx))
                })
                .unwrap_or_default();
            let Some(task) = task else {
                return;
            };

            let completions = task.await;
            _ = view_weak.update(cx, |this, cx| {
                if this.completion_generation != generation || this.input != expected_input {
                    return;
                }

                this.apply_completions(completions, cx);
            });
        }));
    }

    fn apply_completions(&mut self, completions: Vec<CompletionItem>, cx: &mut Context<Self>) {
        self.completions = completions;
        self.completion_selection = 0;
        self.completion_scroll_offset = 0;
        let will_show_completions = !self.completions.is_empty();

        if will_show_completions && !self.show_completions {
            self.original_input = Some(self.input.clone());
        } else if !will_show_completions && self.show_completions {
            self.original_input = None;
        }

        self.show_completions = will_show_completions;
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

                // Recalculate completions
                self.recalculate_completions(cx);

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
            let delta_usize = usize::try_from(delta).unwrap_or(0);
            self.cursor_position = (self.cursor_position + delta_usize).min(input_len);
        } else {
            let delta_usize = usize::try_from(-delta).unwrap_or(0);
            self.cursor_position = self.cursor_position.saturating_sub(delta_usize);
        }
        cx.notify();
    }

    fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.completions.is_empty() {
            return;
        }

        let max_visible = 4; // Must match the value in render
        let _old_selection = self.completion_selection;

        // Move selection
        if delta > 0 {
            self.completion_selection = {
                let delta_usize = usize::try_from(delta).unwrap_or(0);
                (self.completion_selection + delta_usize).min(self.completions.len() - 1)
            };
        } else {
            let delta_usize = usize::try_from(-delta).unwrap_or(0);
            self.completion_selection = self.completion_selection.saturating_sub(delta_usize);
        }

        // Adjust scroll offset based on selection movement
        if delta > 0 {
            // Moving down: scroll only if we moved past the last visible item
            let last_visible_index = self.completion_scroll_offset + max_visible - 1;
            if self.completion_selection > last_visible_index {
                // Selection moved beyond visible area, scroll down
                self.completion_scroll_offset = self.completion_selection + 1 - max_visible;
            }
        } else if delta < 0 {
            // Moving up: scroll only if we moved before the first visible item
            if self.completion_selection < self.completion_scroll_offset {
                // Selection moved before visible area, scroll up
                self.completion_scroll_offset = self.completion_selection;
            }
        }

        cx.notify();
    }

    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        if self.show_completions
            && !self.completions.is_empty()
            && let Some(completion) = self.completions.get(self.completion_selection)
        {
            self.input = completion.text.clone();
            self.cursor_position = self.input.chars().count();
            self.show_completions = false;
            self.original_input = None; // Clear original input since completion is accepted

            if let Some(on_change) = &mut self.on_change {
                on_change(&self.input, cx);
            }

            cx.notify();
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        use nucleotide_logging::info;
        info!(
            input_before = %self.input,
            show_completions = self.show_completions,
            completion_count = self.completions.len(),
            completion_selection = self.completion_selection,
            "PromptView submit called"
        );

        // Accept completion first if showing - but only if the user hasn't typed beyond the completion
        if self.show_completions
            && !self.completions.is_empty()
            && let Some(completion) = self.completions.get(self.completion_selection)
        {
            let input_str = self.input.to_string();
            let completion_str = completion.text.to_string();

            // Only replace input with completion if:
            // 1. The current input is a prefix of the completion, OR
            // 2. The completion is longer and starts with the current input
            let should_accept_completion =
                input_str.len() <= completion_str.len() && completion_str.starts_with(&input_str);

            if should_accept_completion {
                info!(completion_text = %completion.text, "Replacing input with completion");
                self.input = completion.text.clone();
                self.cursor_position = self.input.chars().count();
            } else {
                info!(
                    input_text = %input_str,
                    completion_text = %completion.text,
                    "Not accepting completion - user input is beyond completion"
                );
            }
        }

        info!(input_final = %self.input, "Final input being submitted");

        // Add to history if not empty
        if !self.input.is_empty() {
            self.history.push(self.input.clone());
        }

        if let Some(on_submit) = &mut self.on_submit {
            on_submit(&self.input, cx);
        }
    }

    fn navigate_history(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.history.is_empty() {
            return;
        }

        match self.history_position {
            None => {
                if up {
                    self.history_position = Some(self.history.len() - 1);
                    self.input = self.history[self.history.len() - 1].clone();
                    self.cursor_position = self.input.len();
                }
            }
            Some(pos) => {
                if up && pos > 0 {
                    self.history_position = Some(pos - 1);
                    self.input = self.history[pos - 1].clone();
                    self.cursor_position = self.input.len();
                } else if !up && pos < self.history.len() - 1 {
                    self.history_position = Some(pos + 1);
                    self.input = self.history[pos + 1].clone();
                    self.cursor_position = self.input.len();
                } else if !up {
                    // Going down from the last history item, clear input
                    self.history_position = None;
                    self.input = SharedString::default();
                    self.cursor_position = 0;
                }
            }
        }

        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(on_cancel) = &mut self.on_cancel {
            on_cancel(cx);
        }
    }

    fn dismiss_completions_or_cancel(&mut self, cx: &mut Context<Self>) {
        if self.show_completions {
            // Restore original input before hiding completions.
            if let Some(original) = &self.original_input {
                self.input = original.clone();

                if let Some(on_change) = &mut self.on_change {
                    on_change(&self.input, cx);
                }
            }
            self.show_completions = false;
            self.original_input = None;
            cx.notify();
        } else {
            self.cancel(cx);
        }
    }

    fn confirm_action(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
        cx.stop_propagation();
    }

    fn cancel_action(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_completions_or_cancel(cx);
        cx.stop_propagation();
    }

    fn prev_completion_action(
        &mut self,
        _: &PrevCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_completions {
            self.move_completion_selection(-1, cx);
        } else {
            self.navigate_history(true, cx);
        }
        cx.stop_propagation();
    }

    fn next_completion_action(
        &mut self,
        _: &NextCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_completions {
            self.move_completion_selection(1, cx);
        } else {
            self.navigate_history(false, cx);
        }
        cx.stop_propagation();
    }

    fn move_cursor_left_action(
        &mut self,
        _: &MoveCursorLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_cursor(-1, cx);
        cx.stop_propagation();
    }

    fn move_cursor_right_action(
        &mut self,
        _: &MoveCursorRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_cursor(1, cx);
        cx.stop_propagation();
    }

    fn move_to_start_action(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor_position = 0;
        cx.notify();
        cx.stop_propagation();
    }

    fn move_to_end_action(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor_position = self.input.chars().count();
        cx.notify();
        cx.stop_propagation();
    }

    fn delete_char_action(&mut self, _: &DeleteChar, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_char(cx);
        cx.stop_propagation();
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
        let font = Self::ui_font(cx);
        let ui_font_size = cx.global::<nucleotide_types::UiFontConfig>().size;
        let input_display = self.input.to_string();

        // Get the ghost text (completion suggestion after cursor)
        let ghost_text = if self.show_completions && !self.completions.is_empty() {
            if let Some(completion) = self.completions.get(self.completion_selection) {
                let completion_str = completion.text.as_ref();
                if completion_str.starts_with(&input_display) {
                    // Get the part of the completion that comes after the current input
                    Some(completion_str[input_display.len()..].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Focus ourselves if we're not already focused
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }
        let ui_theme = cx.global::<crate::Theme>();

        div()
            .key_context(PROMPT_CONTEXT)
            .flex()
            .flex_col()
            .w(px(500.))
            .bg(self.style.modal_style.background)
            .border_1()
            .border_color(self.style.modal_style.border)
            .rounded_md()
            .shadow(vec![
                ui_theme.tokens.chrome.shadow_lg.to_box_shadow(false),
                ui_theme.tokens.chrome.inset_highlight.to_box_shadow(true),
            ])
            .font(font.clone())
            .text_size(px(ui_font_size))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm_action))
            .on_action(cx.listener(Self::cancel_action))
            .on_action(cx.listener(Self::prev_completion_action))
            .on_action(cx.listener(Self::next_completion_action))
            .on_action(cx.listener(Self::move_cursor_left_action))
            .on_action(cx.listener(Self::move_cursor_right_action))
            .on_action(cx.listener(Self::move_to_start_action))
            .on_action(cx.listener(Self::move_to_end_action))
            .on_action(cx.listener(Self::delete_char_action))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "tab" => {
                        if this.show_completions && !this.completions.is_empty() {
                            this.accept_completion(cx);
                        }
                    }
                    "space" => {
                        this.insert_char(' ', cx);
                    }
                    key if key.len() == 1 => {
                        if let Some(ch) = key.chars().next()
                            && (ch.is_alphanumeric() || ch.is_ascii_punctuation() || ch == ' ')
                        {
                            this.insert_char(ch, cx);
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                // Input line
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .h(px(44.)) // Fixed height to prevent expansion
                    .gap_2()
                    .child(
                        div()
                            .font(font.clone())
                            .text_color(self.style.modal_style.prompt_text)
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(
                                // Show search icon for search prompts, otherwise show text
                                if self.prompt == "search:" || self.prompt == "rsearch:" {
                                    div().child(create_icon(
                                        "icons/search.svg".to_string(),
                                        16.0,
                                        Some(self.style.modal_style.prompt_text),
                                    ))
                                } else {
                                    div().font(font.clone()).child(self.prompt.clone())
                                },
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font(font.clone())
                            .text_color(self.style.modal_style.text)
                            .child(
                                // Split the input at cursor position for proper cursor rendering
                                div()
                                    .flex()
                                    .items_center()
                                    .child(if self.cursor_position > 0 {
                                        div().font(font.clone()).child(
                                            input_display
                                                .chars()
                                                .take(self.cursor_position)
                                                .collect::<String>(),
                                        )
                                    } else {
                                        div()
                                    })
                                    .child(
                                        div()
                                            .w(px(2.))
                                            .h(px(18.))
                                            .bg(self.style.modal_style.text)
                                            .when(!self.focus_handle.is_focused(window), |this| {
                                                this.opacity(0.5)
                                            }),
                                    )
                                    .child(if self.cursor_position < input_display.len() {
                                        div().font(font.clone()).child(
                                            input_display
                                                .chars()
                                                .skip(self.cursor_position)
                                                .collect::<String>(),
                                        )
                                    } else {
                                        div()
                                    })
                                    // Add ghost text after cursor
                                    .when_some(ghost_text.clone(), |this, ghost| {
                                        this.child(
                                            div()
                                                .font(font.clone())
                                                .text_color(self.style.modal_style.text)
                                                .opacity(0.5) // Make it faded
                                                .child(ghost),
                                        )
                                    }),
                            ),
                    ),
            )
            .when(
                self.show_completions && !self.completions.is_empty(),
                |this| {
                    this.child(
                        div()
                            .border_t_1()
                            .border_color(self.style.modal_style.border)
                            .bg(self.style.completion_background)
                            .max_h(px(200.))
                            .overflow_y_hidden()
                            .children({
                                // Use the tracked scroll offset to determine visible window
                                let max_visible = 4; // Maximum number of visible items (matches visual capacity)

                                let start_idx = self.completion_scroll_offset;
                                let end_idx = (start_idx + max_visible).min(self.completions.len());

                                self.completions[start_idx..end_idx]
                                    .iter()
                                    .enumerate()
                                    .map(|(visible_idx, completion)| {
                                        let actual_idx = start_idx + visible_idx;
                                        let is_selected = actual_idx == self.completion_selection;
                                        let row_font = font.clone();
                                        div()
                                            .id(("completion_item", actual_idx))
                                            .font(row_font.clone())
                                            .flex()
                                            .flex_col()
                                            .px_3()
                                            .py_1()
                                            .when(is_selected, |this| {
                                                this.bg(self.style.modal_style.selected_background)
                                            })
                                            .child(
                                                div()
                                                    .font(row_font.clone())
                                                    .text_color(if is_selected {
                                                        self.style.modal_style.selected_text
                                                    } else {
                                                        self.style.modal_style.text
                                                    })
                                                    .child(
                                                        completion
                                                            .display_text
                                                            .as_ref()
                                                            .unwrap_or(&completion.text)
                                                            .clone(),
                                                    ),
                                            )
                                            .when_some(
                                                completion.description.as_ref(),
                                                |this, desc| {
                                                    this.child(
                                                        div()
                                                            .font(row_font.clone())
                                                            .text_size(
                                                                cx.global::<crate::Theme>()
                                                                    .tokens
                                                                    .sizes
                                                                    .text_sm,
                                                            )
                                                            .text_color(if is_selected {
                                                                self.style.modal_style.selected_text
                                                            } else {
                                                                self.style.modal_style.prompt_text
                                                            })
                                                            .child(desc.clone()),
                                                    )
                                                },
                                            )
                                    })
                                    .collect::<Vec<_>>()
                            }),
                    )
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn init_prompt_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::prompt_view::init(cx);
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
            cx.set_global(nucleotide_types::UiFontConfig {
                family: "Test UI Font".to_string(),
                size: 13.0,
                weight: nucleotide_types::FontWeight::Normal,
            });
        });
    }

    #[test]
    fn prompt_font_comes_from_ui_font_config() {
        let ui_font = nucleotide_types::UiFontConfig {
            family: "Test UI Font".to_string(),
            size: 13.0,
            weight: nucleotide_types::FontWeight::Medium,
        };

        let font = PromptView::font_from_ui_config(&ui_font);

        assert_eq!(font.family, "Test UI Font");
        assert_eq!(font.weight, gpui::FontWeight::MEDIUM);
        assert_eq!(font.style, gpui::FontStyle::Normal);
    }

    #[test]
    fn async_completion_tasks_are_debounced_briefly() {
        assert_eq!(
            ASYNC_COMPLETION_TASK_DEBOUNCE,
            std::time::Duration::from_millis(100)
        );
    }

    #[gpui::test]
    fn prompt_actions_edit_focused_input(cx: &mut TestAppContext) {
        init_prompt_test(cx);
        let (prompt, cx) = cx.add_window_view(|_, cx| PromptView::new(":", cx));
        let focus = prompt.read_with(cx, |prompt, cx| prompt.focus_handle(cx));

        cx.update(|window, cx| {
            prompt.update(cx, |prompt, cx| {
                prompt.set_text("abc", cx);
            });
            window.focus(&focus, cx);
            focus.dispatch_action(&MoveCursorLeft, window, cx);
            focus.dispatch_action(&DeleteChar, window, cx);
        });

        prompt.read_with(cx, |prompt, _| {
            assert_eq!(prompt.input.as_ref(), "ac");
            assert_eq!(prompt.cursor_position, 1);
        });
    }
}
