use gpui::prelude::FluentBuilder;
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
        self.prompt.is_none() && self.native_picker_view.is_none() && self.native_prompt_view.is_none() && self.completion_view.is_none()
    }
    
    pub fn clear(&mut self, cx: &mut Context<Self>) {
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
                println!("📝 OverlayView received prompt: {:?}", prompt);
                match prompt {
                    Prompt::Native { prompt: prompt_text, initial_input, on_submit, on_cancel } => {
                        println!("🎯 Creating native PromptView");
                        
                        let prompt_text = prompt_text.clone();
                        let initial_input = initial_input.clone();
                        let on_submit = on_submit.clone();
                        let on_cancel = on_cancel.clone();
                        
                        // Get theme from core for styling
                        let theme = self.core.upgrade()
                            .map(|core| core.read(cx).editor.theme.clone());
                        
                        let prompt_view = cx.new(|cx| {
                            let mut view = PromptView::new(prompt_text, cx);
                            
                            // Apply theme styling if available
                            if let Some(theme) = theme {
                                let style = crate::prompt_view::PromptStyle::from_helix_theme(&theme);
                                view = view.with_style(style);
                            }
                            
                            if !initial_input.is_empty() {
                                view.set_text(&initial_input, cx);
                            }
                            
                            // Set up completion function for command mode using helix's completions
                            let core_weak_completion = self.core.clone();
                            view = view.with_completion_fn(move |input| {
                                // Get completions from our Core
                                // Since we're in a closure without cx, we need to access core directly
                                // The completion function runs outside of the GPUI event loop
                                core_weak_completion.upgrade()
                                    .map(|_core| {
                                            // We need to access the Core directly since we don't have cx here
                                            // This is safe because we're just reading theme names
                                            use helix_term::commands::TYPABLE_COMMAND_LIST;
                                            use helix_core::fuzzy::fuzzy_match;
                                            
                                            // Split input to see if we're completing a command or arguments
                                            let parts: Vec<&str> = input.split_whitespace().collect();
                                            
                                            if parts.is_empty() || (parts.len() == 1 && !input.ends_with(' ')) {
                                                // Complete command names
                                                let pattern = if parts.is_empty() { "" } else { parts[0] };
                                                
                                                fuzzy_match(
                                                    pattern,
                                                    TYPABLE_COMMAND_LIST.iter().flat_map(|cmd| {
                                                        // Include both the command name and aliases
                                                        std::iter::once(cmd.name).chain(cmd.aliases.iter().copied())
                                                    }),
                                                    false,
                                                )
                                                .into_iter()
                                                .map(|(name, _score)| {
                                                    // Find the command to get its description
                                                    let desc = TYPABLE_COMMAND_LIST.iter()
                                                        .find(|cmd| cmd.name == name || cmd.aliases.contains(&name))
                                                        .map(|cmd| cmd.doc.to_string());
                                                    crate::prompt_view::CompletionItem {
                                                        text: name.to_string().into(),
                                                        description: desc.map(|d| d.into()),
                                                    }
                                                })
                                                .collect()
                                            } else if parts.len() >= 1 && parts[0] == "theme" {
                                                // Get available themes - inline the theme completion logic
                                                let theme_prefix = if parts.len() > 1 { parts[1] } else { "" };
                                                
                                                let mut names = helix_view::theme::Loader::read_names(&helix_loader::config_dir().join("themes"));
                                                for rt_dir in helix_loader::runtime_dirs() {
                                                    names.extend(helix_view::theme::Loader::read_names(&rt_dir.join("themes")));
                                                }
                                                names.push("default".into());
                                                names.push("base16_default".into());
                                                names.sort();
                                                names.dedup();
                                                
                                                fuzzy_match(theme_prefix, names, false)
                                                    .into_iter()
                                                    .map(|(name, _score)| crate::prompt_view::CompletionItem {
                                                        text: format!("theme {}", name).into(),
                                                        description: Some(format!("Switch to {} theme", name).into()),
                                                    })
                                                    .collect()
                                            } else {
                                                Vec::new()
                                            }
                                    })
                                    .unwrap_or_else(Vec::new)
                            });
                            
                            // Set up the submit callback with command execution
                            let core_weak_submit = self.core.clone();
                            view = view.on_submit(move |input: &str, cx| {
                                println!("📝 Prompt submitted: '{}'", input);
                                
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
                                    println!("📝 Prompt cancelled");
                                    (cancel_fn)();
                                    cx.emit(DismissEvent);
                                });
                            } else {
                                view = view.on_cancel(move |cx| {
                                    println!("📝 Prompt cancelled (default)");
                                    cx.emit(DismissEvent);
                                });
                            }
                            
                            view
                        });
                        
                        // Subscribe to dismiss events from the prompt view
                        cx.subscribe(&prompt_view, |this, _prompt_view, _event: &DismissEvent, cx| {
                            println!("🚨 DismissEvent received - clearing native_prompt_view");
                            this.native_prompt_view = None;
                            // Emit dismiss event to notify workspace
                            cx.emit(DismissEvent);
                            cx.notify();
                        }).detach();
                        
                        // Focus will be handled by the prompt view's render method
                        
                        self.native_prompt_view = Some(prompt_view);
                        println!("✅ Set native_prompt_view to Some() and focused it");
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
                println!("🔤 OverlayView received completion");
                
                // Subscribe to dismiss events from the completion view
                cx.subscribe(completion_view, |this, _completion_view, _event: &DismissEvent, cx| {
                    println!("🚨 DismissEvent received - clearing completion_view");
                    this.completion_view = None;
                    // Emit dismiss event to notify workspace
                    cx.emit(DismissEvent);
                    cx.notify();
                }).detach();
                
                self.completion_view = Some(completion_view.clone());
                println!("✅ Set completion_view to Some() and focused it");
                cx.notify();
            }
            crate::Update::Picker(picker) => {
                println!("🔍 OverlayView received picker: {:?}", picker);
                match picker {
                    Picker::Native { title: _, items, on_select } => {
                        println!("🎯 Creating native PickerView with {} items", items.len());
                        
                        let items = items.clone();
                        let on_select = on_select.clone();
                        let core_weak = self.core.clone();
                        let items_count = items.len();
                        
                        // Get theme outside the closure to avoid borrow conflict
                        let theme = core_weak.upgrade()
                            .map(|core| core.read(cx).editor.theme.clone());
                        
                        let picker_view = cx.new(|cx| {
                            // Use theme-aware constructor if theme is available
                            let mut view = if let Some(theme) = &theme {
                                PickerView::new_with_theme(theme, cx)
                            } else {
                                PickerView::new(cx)
                            };
                            let items_for_callback = items.clone();
                            view = view.with_core(core_weak.clone()).with_items(items);
                            
                            // Set up the selection callback
                            view = view.on_select(move |selected_item: &PickerItem, picker_cx| {
                                // Find the index of the selected item
                                if let Some(index) = items_for_callback.iter().position(|item| {
                                    std::ptr::eq(item, selected_item)
                                }) {
                                    // Call the original on_select callback with the index
                                    (on_select)(index);
                                }
                                
                                // Extract the file path from the selected item for opening
                                if let Some(path) = selected_item.data.downcast_ref::<std::path::PathBuf>() {
                                    // Emit OpenFile event to actually open the file
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |_core, core_cx| {
                                            core_cx.emit(crate::Update::OpenFile(path.clone()));
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
                        cx.subscribe(&picker_view, |this, _picker_view, _event: &DismissEvent, cx| {
                            // Clear the native picker when it emits a dismiss event
                            println!("🚨 DismissEvent received - clearing native_picker_view");
                            this.native_picker_view = None;
                            // Emit dismiss event to notify workspace
                            cx.emit(DismissEvent);
                            cx.notify();
                        }).detach();
                        
                        self.native_picker_view = Some(picker_view);
                        println!("✅ Set native_picker_view to Some() with {} items", items_count);
                    }
                }
                
                cx.notify();
            }
            crate::Update::Redraw => {
                // Don't clear native picker on redraw - let it persist until dismissed by user action
                println!("🎨 Redraw event (not clearing native picker)");
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
        println!(
            "🎨 rendering overlay - prompt: {}, native_prompt: {}, completion: {}, native_picker: {}", 
            self.prompt.is_some(),
            self.native_prompt_view.is_some(),
            self.completion_view.is_some(),
            self.native_picker_view.is_some()
        );
        
        div()
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
                    .items_start()  // Align to top instead of center
                    .pt_20()  // Add padding from top
                    .when_some(self.completion_view.clone(), |this, completion_view| {
                        println!("🎨 Rendering completion view");
                        // Completion view handles its own positioning (absolute)
                        this.child(completion_view)
                    })
                    .when_some(self.native_prompt_view.clone(), |this, prompt_view| {
                        println!("🎨 Rendering native prompt view");
                        // Use the actual PromptView component with full keyboard support
                        this.child(prompt_view)
                    })
                    .when_some(self.prompt.take(), |this, prompt| {
                        println!("🎨 Rendering legacy prompt");
                        // Fallback for legacy prompts
                        let handle = cx.focus_handle();
                        // Get theme from core
                        let theme = self.core.upgrade()
                            .map(|core| core.read(cx).editor.theme.clone());
                        let prompt = PromptElement {
                            prompt,
                            focus: handle.clone(),
                            theme,
                        };
                        // Focus is set through the focus handle when needed
                        this.child(prompt)
                    })
                    .when_some(self.native_picker_view.clone(), |this, picker_view| {
                        println!("🎨 Rendering native picker view");
                        // Use the actual PickerView component with full keyboard support
                        // Focus is handled by focus delegation in FocusableView
                        this.child(picker_view)
                    })
            )
    }
}
