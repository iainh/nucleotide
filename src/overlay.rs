use gpui::*;

use crate::completion::CompletionView;
use crate::picker::Picker;
use crate::picker_view::{PickerItem, PickerView};
use crate::prompt::{Prompt, PromptElement};
use crate::prompt_view::PromptView;

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

                        // Get theme from ThemeManager
                        let helix_theme = cx
                            .global::<crate::theme_manager::ThemeManager>()
                            .helix_theme()
                            .clone();

                        let prompt_view = cx.new(|cx| {
                            let mut view = PromptView::new(prompt_text, cx);

                            // Apply theme styling
                            let style =
                                crate::prompt_view::PromptStyle::from_helix_theme(&helix_theme);
                            view = view.with_style(style);

                            if !initial_input.is_empty() {
                                view.set_text(&initial_input, cx);
                            }

                            // Set up completion function for command mode using helix's completions
                            let core_weak_completion = self.core.clone();
                            view = view.with_completion_fn(move |input| {
                                // Get completions from our Core
                                // Since we're in a closure without cx, we need to access core directly
                                // The completion function runs outside of the GPUI event loop
                                core_weak_completion
                                    .upgrade()
                                    .map(|_core| {
                                        // We need to access the Core directly since we don't have cx here
                                        // This is safe because we're just reading theme names
                                        use helix_core::fuzzy::fuzzy_match;
                                        use helix_term::commands::TYPABLE_COMMAND_LIST;

                                        // Split input to see if we're completing a command or arguments
                                        let parts: Vec<&str> = input.split_whitespace().collect();

                                        if parts.is_empty()
                                            || (parts.len() == 1 && !input.ends_with(' '))
                                        {
                                            // Complete command names
                                            let pattern =
                                                if parts.is_empty() { "" } else { parts[0] };

                                            fuzzy_match(
                                                pattern,
                                                TYPABLE_COMMAND_LIST.iter().flat_map(|cmd| {
                                                    // Include both the command name and aliases
                                                    std::iter::once(cmd.name)
                                                        .chain(cmd.aliases.iter().copied())
                                                }),
                                                false,
                                            )
                                            .into_iter()
                                            .map(|(name, _score)| {
                                                // Find the command to get its description
                                                let desc = TYPABLE_COMMAND_LIST
                                                    .iter()
                                                    .find(|cmd| {
                                                        cmd.name == name
                                                            || cmd.aliases.contains(&name)
                                                    })
                                                    .map(|cmd| cmd.doc.to_string());
                                                crate::prompt_view::CompletionItem {
                                                    text: name.to_string().into(),
                                                    description: desc.map(|d| d.into()),
                                                }
                                            })
                                            .collect()
                                        } else if !parts.is_empty() && parts[0] == "theme" {
                                            // Get available themes - inline the theme completion logic
                                            let theme_prefix =
                                                if parts.len() > 1 { parts[1] } else { "" };

                                            let mut names = helix_view::theme::Loader::read_names(
                                                &helix_loader::config_dir().join("themes"),
                                            );

                                            for rt_dir in helix_loader::runtime_dirs() {
                                                let rt_names =
                                                    helix_view::theme::Loader::read_names(
                                                        &rt_dir.join("themes"),
                                                    );
                                                names.extend(rt_names);
                                            }
                                            names.push("default".into());
                                            names.push("base16_default".into());
                                            names.sort();
                                            names.dedup();

                                            fuzzy_match(theme_prefix, names, false)
                                                .into_iter()
                                                .map(|(name, _score)| {
                                                    crate::prompt_view::CompletionItem {
                                                        text: format!("theme {name}").into(),
                                                        description: Some(
                                                            format!("Switch to {name} theme")
                                                                .into(),
                                                        ),
                                                    }
                                                })
                                                .collect()
                                        } else {
                                            Vec::new()
                                        }
                                    })
                                    .unwrap_or_default()
                            });

                            // Set up the submit callback with command execution
                            let core_weak_submit = self.core.clone();
                            view = view.on_submit(move |input: &str, cx| {
                                // Emit CommandSubmitted event to be handled by workspace
                                if let Some(core) = core_weak_submit.upgrade() {
                                    core.update(cx, |_core, cx| {
                                        cx.emit(crate::Update::CommandSubmitted(input.to_string()));
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
                // Subscribe to dismiss events from the completion view
                cx.subscribe(
                    completion_view,
                    |this, _completion_view, _event: &DismissEvent, cx| {
                        this.completion_view = None;
                        // Emit dismiss event to notify workspace
                        cx.emit(DismissEvent);
                        cx.notify();
                    },
                )
                .detach();

                self.completion_view = Some(completion_view.clone());
                cx.notify();
            }
            crate::Update::Picker(picker) => {
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

                        // Get theme from ThemeManager
                        let helix_theme = cx
                            .global::<crate::theme_manager::ThemeManager>()
                            .helix_theme()
                            .clone();

                        let picker_view = cx.new(|cx| {
                            // Use theme-aware constructor
                            let mut view = PickerView::new_with_theme(&helix_theme, cx);
                            let items_for_callback = items.clone();

                            // Enable preview by default, especially for buffer picker
                            view = view.with_preview(true);

                            view = view.with_core(core_weak.clone()).with_items(items);

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
}

impl Focusable for OverlayView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate focus to the active native component
        if let Some(picker_view) = &self.native_picker_view {
            picker_view.focus_handle(cx)
        } else if let Some(prompt_view) = &self.native_prompt_view {
            prompt_view.focus_handle(cx)
        } else if let Some(completion_view) = &self.completion_view {
            completion_view.focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for OverlayView {}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check what type of overlay we should render
        if let Some(picker_view) = &self.native_picker_view {
            // For now, render picker directly until we update Overlay to work with entities
            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
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
                        .pt_20()
                        .child(picker_view.clone()),
                )
                .into_any_element();
        }

        if let Some(prompt_view) = &self.native_prompt_view {
            // For now, render prompt directly until we update Overlay to work with entities
            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
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
                        .pt_20()
                        .child(prompt_view.clone()),
                )
                .into_any_element();
        }

        if let Some(completion_view) = &self.completion_view {
            // Completion view handles its own positioning
            return completion_view.clone().into_any_element();
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
            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .occlude()
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt_20()
                        .child(prompt_elem),
                )
                .into_any_element();
        }

        // Empty overlay
        div().size_0().into_any_element()
    }
}
