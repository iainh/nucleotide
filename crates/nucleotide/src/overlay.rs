use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, Styled, Window, div,
};

use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::completion_v2::CompletionView;
use nucleotide_ui::picker::Picker;
use nucleotide_ui::picker_view::{PickerItem, PickerView};
use nucleotide_ui::prompt::{Prompt, PromptElement};
use nucleotide_ui::prompt_view::PromptView;
use nucleotide_ui::theme_manager::HelixThemedContext;

pub struct OverlayView {
    prompt: Option<Prompt>,
    native_picker_view: Option<Entity<PickerView>>,
    native_prompt_view: Option<Entity<PromptView>>,
    completion_view: Option<Entity<CompletionView>>,
    focus: FocusHandle,
    core: gpui::WeakEntity<crate::Core>,
}

impl OverlayView {
    pub fn new(focus: &FocusHandle, core: &Entity<crate::Core>) -> Self {
        Self {
            prompt: None,
            native_picker_view: None,
            native_prompt_view: None,
            completion_view: None,
            focus: focus.clone(),
            core: core.downgrade(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prompt.is_none()
            && self.native_picker_view.is_none()
            && self.native_prompt_view.is_none()
            && self.completion_view.is_none()
    }

    pub fn has_completion(&self) -> bool {
        self.completion_view.is_some()
    }

    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion_view.is_some() {
            eprintln!("DEBUG: Dismissing completion view");
            self.completion_view = None;
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        // Clean up picker before clearing
        if let Some(picker) = &self.native_picker_view {
            picker.update(cx, |picker, cx| {
                picker.cleanup(cx);
            });
        }
        // Clear all overlay components
        self.prompt = None;
        self.native_picker_view = None;
        self.native_prompt_view = None;
        self.completion_view = None;

        cx.notify();
    }

    pub fn subscribe(&self, editor: &Entity<crate::Core>, cx: &mut Context<Self>) {
        cx.subscribe(editor, |this, _, ev, cx| {
            this.handle_event(ev, cx);
        })
        .detach()
    }

    fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        match ev {
            crate::Update::Prompt(prompt) => {
                match prompt {
                    Prompt::Native {
                        prompt: prompt_text,
                        initial_input,
                        on_submit,
                        on_cancel,
                    } => {
                        let prompt_text = prompt_text.clone();
                        let initial_input = initial_input.clone();
                        let on_submit = on_submit.clone();
                        let on_cancel = on_cancel.clone();

                        let prompt_view = cx.new(|cx| {
                            let is_search_prompt = prompt_text.starts_with("search")
                                || prompt_text.starts_with("rsearch");

                            let mut view = PromptView::new(prompt_text.clone(), cx);

                            // Apply theme styling using testing-aware theme access
                            let style = Self::create_prompt_style_from_context(cx);
                            view = view.with_style(style);

                            if !initial_input.is_empty() {
                                view.set_text(&initial_input, cx);
                            }

                            // Only set up completion for command prompts, not search prompts
                            if !is_search_prompt {
                                // Set up completion function for command mode using our centralized completions module
                                // Try to get actual settings from the editor
                                let settings_cache = if let Some(core) = self.core.upgrade() {
                                    // Use the Helix setting completer to get all available settings
                                    // Pass an empty string to get all settings
                                    let all_settings = helix_term::ui::completers::setting(
                                        &core.read(cx).editor,
                                        "",
                                    );
                                    let settings: Vec<String> = all_settings
                                        .into_iter()
                                        .map(|(_, span)| span.content.to_string())
                                        .collect();
                                    settings
                                } else {
                                    // Fallback to hardcoded list
                                    crate::completions::SETTINGS_KEYS.to_vec()
                                };

                                view = view.with_completion_fn(move |input| {
                                    // Strip leading colon if present
                                    let input = input.strip_prefix(':').unwrap_or(input);

                                    // Use cached settings for better completions
                                    crate::completions::get_command_completions_with_cache(
                                        input,
                                        Some(settings_cache.to_vec()),
                                    )
                                });
                            }

                            // Set up the submit callback with command/search execution
                            let core_weak_submit = self.core.clone();
                            view = view.on_submit(move |input: &str, cx| {
                                use nucleotide_logging::info;
                                info!(input = %input, input_len = input.len(), is_search = is_search_prompt, "Overlay on_submit received input");
                                // Emit appropriate event based on prompt type
                                if let Some(core) = core_weak_submit.upgrade() {
                                    core.update(cx, |_core, cx| {
                                        if is_search_prompt {
                                            cx.emit(crate::Update::SearchSubmitted(
                                                input.to_string(),
                                            ));
                                        } else {
                                            info!(command = %input, "Emitting CommandSubmitted event");
                                            cx.emit(crate::Update::CommandSubmitted(
                                                input.to_string(),
                                            ));
                                        }
                                    });
                                }

                                // Also call the original callback
                                (on_submit)(input);

                                // Dismiss the prompt after submission
                                cx.emit(DismissEvent);
                            });

                            // Set up the cancel callback if provided
                            if let Some(cancel_fn) = on_cancel {
                                view = view.on_cancel(move |cx| {
                                    (cancel_fn)();
                                    cx.emit(DismissEvent);
                                });
                            } else {
                                view = view.on_cancel(move |cx| {
                                    cx.emit(DismissEvent);
                                });
                            }

                            view
                        });

                        // Subscribe to dismiss events from the prompt view
                        cx.subscribe(
                            &prompt_view,
                            |this, _prompt_view, _event: &DismissEvent, cx| {
                                this.native_prompt_view = None;
                                // Emit dismiss event to notify workspace
                                cx.emit(DismissEvent);
                                cx.notify();
                            },
                        )
                        .detach();

                        // Focus will be handled by the prompt view's render method

                        self.native_prompt_view = Some(prompt_view);
                    }
                    Prompt::Legacy(_) => {
                        // For legacy prompts, store them as-is
                        self.prompt = Some(prompt.clone());
                        self.native_prompt_view = None;
                    }
                }

                cx.notify();
            }
            crate::Update::Completion(completion_view) => {
                println!("COMP: OverlayView received completion view");
                // Subscribe to dismiss events from the completion view
                cx.subscribe(
                    completion_view,
                    |this, _completion_view, _event: &DismissEvent, cx| {
                        println!("COMP: CompletionView dismissed");
                        this.completion_view = None;
                        // Emit dismiss event to notify workspace
                        cx.emit(DismissEvent);
                        cx.notify();
                    },
                )
                .detach();

                // Subscribe to completion accepted events from the completion view
                cx.subscribe(
                    completion_view,
                    |_this,
                     _completion_view,
                     event: &nucleotide_ui::completion_v2::CompletionAcceptedEvent,
                     cx| {
                        println!(
                            "COMP: OverlayView received completion accepted event: {}",
                            event.text
                        );
                        // Forward the completion accepted event to the workspace
                        cx.emit(nucleotide_ui::completion_v2::CompletionAcceptedEvent {
                            text: event.text.clone(),
                        });
                    },
                )
                .detach();

                self.completion_view = Some(completion_view.clone());
                println!("COMP: OverlayView set completion_view and notifying");
                cx.notify();
            }
            crate::Update::Picker(picker) => {
                // Clean up any existing picker before creating a new one
                if let Some(existing_picker) = &self.native_picker_view {
                    existing_picker.update(cx, |picker, cx| {
                        picker.cleanup(cx);
                    });
                    self.native_picker_view = None;
                }

                match picker {
                    Picker::Native {
                        title: _,
                        items,
                        on_select,
                    } => {
                        let items = items.clone();
                        let on_select = on_select.clone();
                        let core_weak = self.core.clone();
                        let _items_count = items.len();

                        let picker_view = cx.new(|cx| {
                            // Use testing-aware theme constructor
                            let mut view = Self::create_picker_view_with_context(cx);
                            let items_for_callback = items.clone();

                            // Enable preview by default, especially for buffer picker
                            view = view.with_preview(true);

                            view = view.with_items(items);

                            // Set up the selection callback
                            view = view.on_select(move |selected_item: &PickerItem, picker_cx| {
                                // Find the index of the selected item
                                if let Some(index) = items_for_callback
                                    .iter()
                                    .position(|item| std::ptr::eq(item, selected_item))
                                {
                                    // Call the original on_select callback with the index
                                    (on_select)(index);
                                }

                                // Check if it's a buffer picker item (DocumentId, Option<PathBuf>)
                                if let Some((doc_id, _path)) = selected_item.data.downcast_ref::<(
                                    helix_view::DocumentId,
                                    Option<std::path::PathBuf>,
                                )>(
                                ) {
                                    // Switch to the selected buffer
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            core.editor.switch(
                                                *doc_id,
                                                helix_view::editor::Action::Replace,
                                            );
                                        });
                                    }
                                }
                                // Extract the file path from the selected item for opening
                                else if let Some(path) =
                                    selected_item.data.downcast_ref::<std::path::PathBuf>()
                                {
                                    // Emit OpenFile event to actually open the file
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |_core, core_cx| {
                                            core_cx.emit(crate::Update::OpenFile(path.clone()));
                                        });
                                    }
                                }
                                // Check if it's a document ID for buffer switching (legacy)
                                else if let Some(doc_id) =
                                    selected_item.data.downcast_ref::<helix_view::DocumentId>()
                                {
                                    // Switch to the selected buffer
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            // Switch to the selected document
                                            core.editor.switch(
                                                *doc_id,
                                                helix_view::editor::Action::Replace,
                                            );
                                        });
                                    }
                                }

                                // Dismiss the picker after selection
                                picker_cx.emit(gpui::DismissEvent);
                            });

                            // Set up the cancel callback
                            view = view.on_cancel(move |cx| {
                                // The PickerView will handle its own dismissal
                                cx.emit(DismissEvent);
                            });

                            view
                        });

                        // Subscribe to dismiss events from the picker view
                        cx.subscribe(
                            &picker_view,
                            |this, picker_view, _event: &DismissEvent, cx| {
                                // Clean up the picker before clearing it
                                picker_view.update(cx, |picker, cx| {
                                    picker.cleanup(cx);
                                });
                                this.native_picker_view = None;
                                // Emit dismiss event to notify workspace
                                cx.emit(DismissEvent);
                                cx.notify();
                            },
                        )
                        .detach();

                        self.native_picker_view = Some(picker_view);
                    }
                }

                cx.notify();
            }
            crate::Update::DirectoryPicker(_picker) => {
                // Use GPUI's native file dialog API
                let core_weak = self.core.clone();
                cx.spawn(async move |this, cx| {
                    // Configure the dialog to only allow directory selection
                    let options = gpui::PathPromptOptions {
                        files: false,      // Don't allow file selection
                        directories: true, // Allow directory selection
                        multiple: false,   // Single directory only
                    };

                    // Open the native directory picker
                    let result = cx.update(|cx| cx.prompt_for_paths(options)).ok();

                    if let Some(receiver) = result {
                        // Wait for the user to select a directory
                        if let Ok(Ok(Some(paths))) = receiver.await {
                            if let Some(path) = paths.first() {
                                // Emit the selected directory through the core entity
                                if let Some(core) = core_weak.upgrade() {
                                    cx.update(|cx| {
                                        core.update(cx, |_core, cx| {
                                            cx.emit(crate::Update::OpenDirectory(path.clone()));
                                        });
                                    })
                                    .ok();
                                }
                                // Dismiss the overlay
                                cx.update(|cx| {
                                    this.update(cx, |_this, cx| {
                                        cx.emit(DismissEvent);
                                    })
                                    .ok();
                                })
                                .ok();
                            }
                        } else {
                            // User cancelled - just dismiss
                            cx.update(|cx| {
                                this.update(cx, |_this, cx| {
                                    cx.emit(DismissEvent);
                                })
                                .ok();
                            })
                            .ok();
                        }
                    }
                })
                .detach();

                cx.notify();
            }
            crate::Update::Redraw => {
                // Don't clear native picker on redraw - let it persist until dismissed by user action
            }
            _ => {}
        }
    }

    /// Create PromptStyle using ThemedContext for consistent theme access
    fn create_prompt_style_from_context(cx: &App) -> nucleotide_ui::prompt_view::PromptStyle {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get ui.menu style with fallback to design tokens
        let ui_menu = cx.theme_style("ui.menu");

        nucleotide_ui::prompt_view::PromptStyle {
            modal_style,
            completion_background: ui_menu
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(tokens.colors.menu_background),
        }
    }

    /// Create ModalStyle using ThemedContext with design token fallbacks
    fn create_modal_style_from_context(cx: &App) -> nucleotide_ui::common::ModalStyle {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get theme styles for specific overrides
        let ui_popup = cx.theme_style("ui.popup");
        let ui_text = cx.theme_style("ui.text");
        let ui_menu_selected = cx.theme_style("ui.menu.selected");

        // Use design tokens as fallbacks - guaranteed to exist
        let background = ui_popup
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.popup_background);
        let text = ui_text
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.text_primary);
        let selected_background = ui_menu_selected
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.surface_selected);
        let selected_text = ui_menu_selected
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.text_primary);
        let border = ui_popup
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.border_default);
        let prompt_text = text;

        nucleotide_ui::common::ModalStyle {
            background,
            text,
            border,
            selected_background,
            selected_text,
            prompt_text,
        }
    }

    /// Create PickerView using ThemedContext with design token fallbacks
    fn create_picker_view_with_context(cx: &mut gpui::Context<PickerView>) -> PickerView {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get theme styles for specific overrides
        let ui_background_separator = cx.theme_style("ui.background.separator");
        let ui_cursor = cx.theme_style("ui.cursor");

        // Use design tokens as fallbacks - guaranteed to exist
        let preview_background = ui_background_separator
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.surface);
        let cursor = ui_cursor
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.primary);

        let picker_style = nucleotide_ui::picker_view::PickerStyle {
            modal_style,
            preview_background,
            cursor,
        };

        PickerView::new(cx).with_style(picker_style)
    }
}

impl Focusable for OverlayView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate focus to the active native component, but not completion views
        // Completion views should not steal focus - let the editor maintain focus
        if let Some(picker_view) = &self.native_picker_view {
            picker_view.focus_handle(cx)
        } else if let Some(prompt_view) = &self.native_prompt_view {
            prompt_view.focus_handle(cx)
        } else {
            // Don't delegate to completion_view - let editor keep focus
            self.focus.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for OverlayView {}
impl EventEmitter<nucleotide_ui::completion_v2::CompletionAcceptedEvent> for OverlayView {}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check what type of overlay we should render
        if let Some(picker_view) = &self.native_picker_view {
            // Render picker with design tokens and consistent overlay patterns
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .bg(tokens.colors.surface_overlay)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    // Prevent click-through to elements below
                })
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(picker_view.clone()),
                )
                .into_any_element();
        }

        if let Some(prompt_view) = &self.native_prompt_view {
            // Render prompt with design tokens and consistent overlay patterns
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .bg(tokens.colors.surface_overlay)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    // Prevent click-through to elements below
                })
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(prompt_view.clone()),
                )
                .into_any_element();
        }

        if let Some(completion_view) = &self.completion_view {
            // Render completion view positioned near cursor without full overlay
            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .child(completion_view.clone())
                .into_any_element();
        }

        // Legacy prompt fallback
        if let Some(prompt) = self.prompt.take() {
            let handle = cx.focus_handle();
            let theme = self
                .core
                .upgrade()
                .map(|core| core.read(cx).editor.theme.clone());
            let prompt_elem = PromptElement {
                prompt,
                focus: handle.clone(),
                theme,
            };
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .bg(tokens.colors.surface_overlay)
                .occlude()
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(prompt_elem),
                )
                .into_any_element();
        }

        // Empty overlay using design tokens
        div().size_0().into_any_element()
    }
}
