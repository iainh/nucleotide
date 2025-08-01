use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;

use crate::completion::CompletionView;
use crate::picker::{Picker, PickerElement};
use crate::picker_view::{PickerItem, PickerView};
use crate::prompt::{Prompt, PromptElement};
use crate::prompt_view::PromptView;

pub struct OverlayView {
    prompt: Option<Prompt>,
    picker: Option<Picker>,
    native_picker_view: Option<View<PickerView>>,
    native_prompt_view: Option<View<PromptView>>,
    completion_view: Option<View<CompletionView>>,
    focus: FocusHandle,
    core: gpui::WeakModel<crate::Core>,
}

impl OverlayView {
    pub fn new(focus: &FocusHandle, core: &Model<crate::Core>) -> Self {
        Self {
            prompt: None,
            picker: None,
            native_picker_view: None,
            native_prompt_view: None,
            completion_view: None,
            focus: focus.clone(),
            core: core.downgrade(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prompt.is_none() && self.picker.is_none() && self.native_picker_view.is_none() && self.native_prompt_view.is_none() && self.completion_view.is_none()
    }

    pub fn subscribe(&self, editor: &Model<crate::Core>, cx: &mut ViewContext<Self>) {
        cx.subscribe(editor, |this, _, ev, cx| {
            this.handle_event(ev, cx);
        })
        .detach()
    }

    fn handle_event(&mut self, ev: &crate::Update, cx: &mut ViewContext<Self>) {
        match ev {
            crate::Update::Prompt(prompt) => {
                println!("📝 OverlayView received prompt: {:?}", prompt);
                match prompt {
                    Prompt::Native { prompt: prompt_text, initial_input: _, on_submit, on_cancel } => {
                        println!("🎯 Creating native PromptView");
                        
                        // Create a proper PromptView for native prompts
                        let prompt_view = cx.new_view(|cx| {
                            let mut view = PromptView::new(prompt_text.clone(), cx);
                            
                            // Set up the submit callback
                            let on_submit = on_submit.clone();
                            view = view.on_submit(move |input: &str, _cx| {
                                println!("📝 Prompt submitted: '{}'", input);
                                (on_submit)(input);
                            });
                            
                            // Set up the cancel callback if provided
                            if let Some(cancel_fn) = on_cancel {
                                let cancel_fn = cancel_fn.clone();
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
                            cx.notify();
                        }).detach();
                        
                        self.native_prompt_view = Some(prompt_view);
                        self.prompt = None; // Clear legacy prompt
                        println!("✅ Set native_prompt_view to Some()");
                    }
                    _ => {
                        // For legacy prompts
                        println!("📄 Setting legacy prompt and clearing native prompt");
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
                    cx.notify();
                }).detach();
                
                self.completion_view = Some(completion_view.clone());
                
                // Explicitly focus the completion view for keyboard input
                cx.focus_view(completion_view);
                println!("✅ Set completion_view to Some() and focused it");
                cx.notify();
            }
            crate::Update::Picker(picker) => {
                println!("🔍 OverlayView received picker: {:?}", picker);
                match picker {
                    Picker::Native { title, items, on_select } => {
                        println!("🎯 Creating native PickerView with {} items", items.len());
                        // Create a proper PickerView for native pickers
                        let _editor_handle = cx.model().clone();
                        let picker_view = cx.new_view(|cx| {
                            let mut view = PickerView::new(cx);
                            view = view.with_title(title.clone());
                            view = view.with_items(items.clone());
                            
                            // Set up the selection callback to emit OpenFile event instead of using the callback
                            let core_weak = self.core.clone();
                            view = view.on_select(move |selected_item: &PickerItem, picker_cx| {
                                // Extract the file path from the selected item
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
                            cx.notify();
                        }).detach();
                        
                        self.native_picker_view = Some(picker_view);
                        self.picker = None; // Clear legacy picker
                        println!("✅ Set native_picker_view to Some() with {} items", items.len());
                    }
                    _ => {
                        // For legacy pickers - but don't clear native picker if legacy picker is empty
                        if let Some(legacy_text) = picker.as_legacy() {
                            if legacy_text.is_empty() {
                                // Empty legacy picker - don't override native picker
                                println!("🚫 Ignoring empty legacy picker (keeping native picker)");
                                // Still notify to trigger re-render
                                cx.notify();
                                return;
                            }
                        }
                        
                        println!("📄 Setting legacy picker and clearing native picker");
                        self.picker = Some(picker.clone());
                        self.native_picker_view = None;
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

impl FocusableView for OverlayView {
    fn focus_handle(&self, cx: &AppContext) -> FocusHandle {
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
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        println!(
            "🎨 rendering overlay - prompt: {}, native_prompt: {}, completion: {}, native_picker: {}, legacy_picker: {}", 
            self.prompt.is_some(),
            self.native_prompt_view.is_some(),
            self.completion_view.is_some(),
            self.native_picker_view.is_some(),
            self.picker.is_some()
        );
        
        div().absolute().size_full().bottom_0().left_0()
            .child(
                div()
                    .flex()
                    .h_full()
                    .justify_center()
                    .items_center()
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
                        let prompt = PromptElement {
                            prompt,
                            focus: handle.clone(),
                        };
                        handle.focus(cx);
                        this.child(prompt)
                    })
                    .when_some(self.native_picker_view.clone(), |this, picker_view| {
                        println!("🎨 Rendering native picker view");
                        // Use the actual PickerView component with full keyboard support
                        // Focus is handled by focus delegation in FocusableView
                        this.child(picker_view)
                    })
                    .when_some(self.picker.take(), |this, picker| {
                        println!("🎨 Rendering legacy picker");
                        // Fallback for legacy pickers
                        let handle = cx.focus_handle();
                        let picker_element = PickerElement {
                            picker,
                            focus: handle.clone(),
                            selected_index: 0,
                        };
                        handle.focus(cx);
                        this.child(picker_element)
                    }),
            )
    }
}
