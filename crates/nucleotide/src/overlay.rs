use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, Styled, Window,
    div, px,
};
use helix_stdx::rope::RopeSliceExt;
use nucleotide_core::EventBus;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::completion_v2::CompletionView;
use nucleotide_ui::picker::Picker;
use nucleotide_ui::picker_view::{PickerItem, PickerView};
use nucleotide_ui::prompt::{Prompt, PromptElement};
use nucleotide_ui::prompt_view::PromptView;
use nucleotide_ui::theme_manager::HelixThemedContext; // bring dispatch_* trait methods into scope
use std::sync::{Arc, Mutex};

pub struct OverlayView {
    prompt: Option<Prompt>,
    native_picker_view: Option<Entity<PickerView>>,
    native_prompt_view: Option<Entity<PromptView>>,
    completion_view: Option<Entity<CompletionView>>,
    code_action_pairs: Option<
        Vec<(
            helix_lsp::lsp::CodeActionOrCommand,
            helix_core::diagnostic::LanguageServerId,
            helix_lsp::OffsetEncoding,
        )>,
    >,
    diagnostics_panel: Option<Entity<crate::DiagnosticsPanel>>,
    terminal_panel: Option<Entity<nucleotide_terminal_panel::TerminalPanel>>,
    // Dedicated focus handle for the terminal panel area
    terminal_focus: Option<FocusHandle>,
    // Resizable terminal panel height (pixels)
    terminal_height_px: f32,
    // Resize interaction state
    terminal_resizing: bool,
    resize_start_mouse_y: gpui::Pixels,
    resize_start_height: f32,
    // Shared state for window-level resize listeners
    resize_state: Arc<Mutex<ResizeStateInner>>,
    // Track last dispatched terminal size to avoid redundant resize events
    last_terminal_size: Option<(nucleotide_events::v2::terminal::TerminalId, u16, u16)>,
    focus: FocusHandle,
    core: gpui::WeakEntity<crate::Core>,
}

#[derive(Debug)]
struct ResizeStateInner {
    resizing: bool,
    start_mouse_y: f32,
    start_height: f32,
    height: f32,
}

impl OverlayView {
    pub fn new(focus: &FocusHandle, core: &Entity<crate::Core>) -> Self {
        Self {
            prompt: None,
            native_picker_view: None,
            native_prompt_view: None,
            completion_view: None,
            code_action_pairs: None,
            diagnostics_panel: None,
            terminal_panel: None,
            terminal_focus: None,
            terminal_height_px: 220.0,
            terminal_resizing: false,
            resize_start_mouse_y: px(0.0),
            resize_start_height: 220.0,
            resize_state: Arc::new(Mutex::new(ResizeStateInner {
                resizing: false,
                start_mouse_y: 0.0,
                start_height: 220.0,
                height: 220.0,
            })),
            last_terminal_size: None,
            focus: focus.clone(),
            core: core.downgrade(),
        }
    }

    pub fn is_empty(&self) -> bool {
        let empty = self.prompt.is_none()
            && self.native_picker_view.is_none()
            && self.native_prompt_view.is_none()
            && self.completion_view.is_none()
            && self.diagnostics_panel.is_none()
            && self.terminal_panel.is_none();

        if !empty && self.completion_view.is_some() {
            nucleotide_logging::debug!("COMP: Overlay not empty - has completion view");
        }

        empty
    }

    pub fn has_completion(&self) -> bool {
        self.completion_view.is_some()
    }

    /// Whether the current completion popup represents a Code Actions list
    pub fn has_code_actions(&self) -> bool {
        self.code_action_pairs.is_some()
    }

    pub fn has_picker(&self) -> bool {
        self.native_picker_view.is_some()
    }

    pub fn has_prompt(&self) -> bool {
        self.native_prompt_view.is_some() || self.prompt.is_some()
    }

    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion_view.is_some() {
            self.completion_view = None;
            cx.notify();
        }
    }

    /// Update the completion filter with a new prefix
    pub fn update_completion_filter(&self, new_prefix: String, cx: &mut Context<Self>) -> bool {
        if let Some(completion_view) = &self.completion_view {
            completion_view.update(cx, |view, cx| {
                nucleotide_logging::debug!(
                    prefix = %new_prefix,
                    "Updating completion view filter with new prefix"
                );
                view.update_filter(new_prefix, cx);
            });
            true
        } else {
            false
        }
    }

    pub fn handle_completion_arrow_key(&self, key: &str, cx: &mut Context<Self>) -> bool {
        if let Some(completion_view) = &self.completion_view {
            completion_view.update(cx, |view, cx| match key {
                "up" => view.select_prev(cx),
                "down" => view.select_next(cx),
                _ => {}
            });
            true // Key was handled
        } else {
            false // No completion view to handle the key
        }
    }

    pub fn handle_completion_tab_key(&self, cx: &mut Context<Self>) -> bool {
        if let Some(completion_view) = &self.completion_view {
            completion_view.update(cx, |view, cx| {
                // Accept the currently selected completion item
                if let Some(selected_index) = view.selected_index() {
                    nucleotide_logging::info!(
                        selected_index = selected_index,
                        "Tab key forwarded - accepting selected completion via Helix"
                    );
                    // Signal to Helix that it should accept the completion
                    // The overlay subscription will catch this and forward to workspace
                    cx.emit(nucleotide_ui::CompleteViaHelixEvent {
                        item_index: selected_index,
                    });
                    // Note: DismissEvent will be emitted by the overlay subscription
                } else {
                    nucleotide_logging::warn!("Tab key forwarded but no completion item selected");
                }
            });
            true // Key was handled
        } else {
            false // No completion view to handle the key
        }
    }

    /// Handle Enter key for accepting the highlighted item in completion/code-action popup
    pub fn handle_completion_enter_key(&self, cx: &mut Context<Self>) -> bool {
        // Reuse the same acceptance flow as Tab
        self.handle_completion_tab_key(cx)
    }

    pub fn get_completion_item(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<nucleotide_ui::CompletionItem> {
        if let Some(completion_view) = &self.completion_view {
            completion_view.read(cx).get_item_at_index(index).cloned()
        } else {
            None
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

    pub fn show_terminal_panel(
        &mut self,
        panel: Entity<nucleotide_terminal_panel::TerminalPanel>,
        cx: &mut Context<Self>,
    ) {
        self.terminal_panel = Some(panel);
        cx.notify();
    }

    pub fn hide_terminal_panel(&mut self, cx: &mut Context<Self>) {
        self.terminal_panel = None;
        cx.notify();
    }

    pub fn subscribe(&self, editor: &Entity<crate::Core>, cx: &mut Context<Self>) {
        cx.subscribe(editor, |this, _, ev, cx| {
            this.handle_event(ev, cx);
        })
        .detach()
    }

    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        match ev {
            crate::Update::Prompt(prompt) => {
                nucleotide_logging::info!("DIAG: Overlay Update::Prompt received");
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
                nucleotide_logging::info!("DIAG: Overlay Update::Completion received");
                nucleotide_logging::info!(
                    "🎨 OVERLAY RECEIVED COMPLETION VIEW: Setting up completion overlay"
                );

                // Set up completion view with event subscription
                self.completion_view = Some(completion_view.clone());

                // Subscribe to dismiss events from completion view
                cx.subscribe(
                    completion_view,
                    |this, _completion_view, _event: &DismissEvent, cx| {
                        this.completion_view = None;
                        cx.emit(DismissEvent);
                        cx.notify();
                    },
                )
                .detach();

                // Subscribe to the new completion acceptance event
                cx.subscribe(
                    completion_view,
                    |_this, _completion_view, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                        nucleotide_logging::info!(
                            item_index = event.item_index,
                            "Completion accepted via Helix transaction system - forwarding to workspace"
                        );
                        // Forward the completion acceptance event to the workspace FIRST
                        // (while completion_view is still available for item retrieval)
                        cx.emit(nucleotide_ui::CompleteViaHelixEvent {
                            item_index: event.item_index,
                        });

                        // IMPORTANT: Don't dismiss yet - let workspace handle completion first
                        // The workspace will call dismiss_completion() after successful text insertion
                        nucleotide_logging::info!("Completion acceptance forwarded - workspace will dismiss after processing");
                    },
                )
                .detach();

                cx.notify();
            }
            crate::Update::CodeActions(completion_view, pairs) => {
                nucleotide_logging::info!("🎨 OVERLAY RECEIVED CODE ACTIONS VIEW");
                self.completion_view = Some(completion_view.clone());
                self.code_action_pairs = Some(pairs.clone());
                // Dismiss subscription
                cx.subscribe(completion_view, |this, _cv, _ev: &DismissEvent, cx| {
                    this.completion_view = None;
                    this.code_action_pairs = None;
                    cx.emit(DismissEvent);
                    cx.notify();
                })
                .detach();

                // Accept subscription maps selection index to code action application
                let core_for_apply = self.core.clone();
                cx.subscribe(
                    completion_view,
                    move |this, _cv, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                        if let Some(pairs) = this.code_action_pairs.clone() {
                            if let Some((action, ls_id, offset)) = pairs.get(event.item_index)
                                && let Some(core) = core_for_apply.upgrade()
                            {
                                core.update(cx, |core, _| {
                                    if let Some(ls) = core.editor.language_server_by_id(*ls_id) {
                                        match action {
                                            helix_lsp::lsp::CodeActionOrCommand::Command(cmd) => {
                                                core.editor
                                                    .execute_lsp_command(cmd.clone(), *ls_id);
                                            }
                                            helix_lsp::lsp::CodeActionOrCommand::CodeAction(ca) => {
                                                let mut resolved = None;
                                                if (ca.edit.is_none() || ca.command.is_none())
                                                    && let Some(fut) = ls.resolve_code_action(ca)
                                                    && let Ok(c) = helix_lsp::block_on(fut)
                                                {
                                                    resolved = Some(c);
                                                }
                                                let action_ref = resolved.as_ref().unwrap_or(ca);
                                                if let Some(edit) = &action_ref.edit {
                                                    let _ = core
                                                        .editor
                                                        .apply_workspace_edit(*offset, edit);
                                                }
                                                if let Some(cmd) = &action_ref.command {
                                                    core.editor
                                                        .execute_lsp_command(cmd.clone(), *ls_id);
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                            this.completion_view = None;
                            this.code_action_pairs = None;
                            cx.notify();
                        } else {
                            // Fallback to standard completion handling if pairs are missing
                            cx.emit(nucleotide_ui::CompleteViaHelixEvent {
                                item_index: event.item_index,
                            });
                        }
                    },
                )
                .detach();

                cx.notify();
            }
            crate::Update::DiagnosticsPanel(panel) => {
                nucleotide_logging::info!("DIAG: Showing Diagnostics panel overlay");
                // Replace any existing diagnostics panel (do not clear other overlays here; overlay render ordering handles precedence)
                self.diagnostics_panel = Some(panel.clone());
                // Focus will be ensured by the diagnostics panel during render
                // Subscribe to dismiss from panel via global dismiss event
                cx.subscribe(panel, |this, _panel, _ev: &DismissEvent, cx| {
                    this.diagnostics_panel = None;
                    cx.emit(DismissEvent);
                    cx.notify();
                })
                .detach();

                // Subscribe to diagnostics navigation events
                let core_for_nav = self.core.clone();
                cx.subscribe(
                    panel,
                    move |_this,
                          _panel,
                          ev: &crate::diagnostics_panel::DiagnosticsJumpEvent,
                          cx| {
                        if let Some(core) = core_for_nav.upgrade() {
                            let path = ev.path.clone();
                            let offset = ev.offset;
                            nucleotide_logging::info!(
                                path = %path.display(),
                                offset = offset,
                                "DIAG: Diagnostics item selected - opening and jumping"
                            );
                            // Open the file, then move cursor after a short delay
                            core.update(cx, |_app, cx| {
                                cx.emit(crate::Update::OpenFile(path.clone()));
                            });
                            let core2 = core_for_nav.clone();
                            let path_for_lookup = path.clone();
                            cx.spawn(async move |_this, cx| {
                                // Small delay to allow file to open
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(20))
                                    .await;
                                if let Some(core) = core2.upgrade() {
                                    let _ = core.update(cx, |app, _cx| {
                                        // Find document by path
                                        if let Some(doc_id) =
                                            app.editor.documents.iter().find_map(|(id, doc)| {
                                                doc.path()
                                                    .filter(|p| *p == &path_for_lookup)
                                                    .map(|_| *id)
                                            })
                                        {
                                            // Set cursor selection to offset
                                            let view_id = app.editor.tree.focus;
                                            let selection = helix_core::Selection::point(offset);
                                            let doc = helix_view::doc_mut!(app.editor, &doc_id);
                                            doc.set_selection(view_id, selection);
                                            nucleotide_logging::info!(
                                                doc_id = ?doc_id,
                                                view_id = ?view_id,
                                                offset = offset,
                                                "DIAG: Cursor moved to diagnostic offset"
                                            );
                                        }
                                    });
                                }
                            })
                            .detach();
                        }
                    },
                )
                .detach();
                cx.notify();
            }
            crate::Update::TerminalPanel(panel) => {
                nucleotide_logging::info!("TERMINAL: Showing Terminal panel overlay");
                self.terminal_panel = Some(panel.clone());
                cx.notify();
            }
            crate::Update::Picker(picker) => {
                nucleotide_logging::info!("DIAG: Overlay Update::Picker received");
                // Clear any diagnostics panel if present to avoid overlay conflicts
                self.diagnostics_panel = None;
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

                            // Wire minimal preview open/close hooks
                            let open_core = core_weak.clone();
                            // Use a preview executor (event-based sketch). Currently no-op to avoid layout changes.
                            let preview_exec = crate::picker_capability::PickerPreviewExecutor::new_from_weak(open_core.clone());
                            // Keep a handle to the concrete capability to register DocumentView entities
                            let cap_for_open = core_weak.clone();
                            let cap_for_picker: Option<
                                std::sync::Arc<
                                    std::sync::RwLock<crate::picker_capability::HelixPickerCapability>,
                                >,
                            > = if let Some(core) = core_weak.upgrade() {
                                let cap_arc = crate::picker_capability::HelixPickerCapability::new(&core);
                                view = view.with_capability(
                                    cap_arc.clone()
                                        as std::sync::Arc<
                                            std::sync::RwLock<
                                                dyn nucleotide_core::capabilities::PickerCapability
                                                    + Send
                                                    + Sync,
                                            >,
                                        >,
                                );
                                Some(cap_arc)
                            } else {
                                None
                            };

                            // Clone capability handle for each closure to avoid moving the original
                            let cap_for_open_reg = cap_for_picker.clone();
                            let cap_for_close = cap_for_picker.clone();

                            view = view.with_preview_open_fn(move |path, picker_cx| {
                                let result = preview_exec.open(path, picker_cx);
                                if let Some((doc_id, view_id)) = result {
                                    // Mount a DocumentView entity and register with capability for rendering
                                    if let Some(core) = cap_for_open.upgrade() {
                                        let entity = picker_cx.new(|cx| {
                                            // Build theme-aware TextStyle similar to editor views
                                            let editor_font = cx.global::<crate::types::EditorFontConfig>();
                                            let theme = cx.theme();
                                            let tokens = &theme.tokens;
                                            let default_style = gpui::TextStyle {
                                                color: tokens.editor.text_primary,
                                                font_family: cx
                                                    .global::<crate::types::FontSettings>()
                                                    .fixed_font
                                                    .family
                                                    .clone()
                                                    .into(),
                                                font_features: gpui::FontFeatures::default(),
                                                font_fallbacks: None,
                                                font_size: gpui::px(editor_font.size).into(),
                                                line_height: gpui::phi(),
                                                font_weight: editor_font.weight.into(),
                                                font_style: gpui::FontStyle::Normal,
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                                white_space: gpui::WhiteSpace::Normal,
                                                text_overflow: None,
                                                text_align: gpui::TextAlign::default(),
                                                line_clamp: None,
                                            };
                                            let focus = cx.focus_handle();
                                            crate::document::DocumentView::new(core.clone(), view_id, default_style, &focus, false)
                                        });
                                        if let Some(cap_arc) = &cap_for_open_reg
                                            && let Ok(mut cap) = cap_arc.write() {
                                                cap.register_preview_entity(doc_id, view_id, entity);
                                            }
                                    }
                                }
                                result
                            });

                            let close_core = core_weak.clone();
                            let preview_exec_close = crate::picker_capability::PickerPreviewExecutor::new_from_weak(close_core.clone());
                            view = view.with_preview_close_fn(move |doc_id, view_id, picker_cx| {
                                // Unregister entity before closing
                                if let Some(cap_arc) = &cap_for_close
                                    && let Ok(mut cap) = cap_arc.write() {
                                        cap.unregister_preview_entity(doc_id, view_id);
                                    }
                                preview_exec_close.close(doc_id, view_id, picker_cx);
                            });

                            // Render the preview using current document content with simple styling
                            // Provide a lightweight syntax renderer: non-destructive, no Helix view needed.
                            let render_core = core_weak.clone();
                            view = view.with_preview_text_renderer_fn(move |text, path, cx| {
                                use gpui::{div, px, IntoElement, Styled};

                                // Resolve simple scope colors from the theme
                                let theme = cx.helix_theme();
                                let color_for = |scope: &str| -> Option<gpui::Hsla> {
                                    theme
                                        .get(scope)
                                        .fg
                                        .and_then(nucleotide_ui::theme_utils::color_to_hsla)
                                };
                                let kw_color = color_for("keyword");
                                let str_color = color_for("string");
                                let com_color = color_for("comment");
                                let num_color = color_for("constant.numeric").or_else(|| color_for("number"));

                                // Pick a basic keyword set by extension (fallback to Rust-like)
                                let ext = path.and_then(|p| p.extension().and_then(|e| e.to_str())).unwrap_or("");
                                let rust_like = matches!(ext, "rs"|"js"|"ts"|"jsx"|"tsx"|"c"|"cpp"|"cc"|"h"|"hpp"|"java"|"kt"|"swift");
                                let py_like = matches!(ext, "py");
                                let toml_like = matches!(ext, "toml"|"ini"|"cfg");

                                // Very lightweight tokenization: comments, strings, numbers, keywords
                                let keywords: &[&str] = if py_like {
                                    &[
                                        "def","class","return","if","elif","else","for","while","try","except","with","as","import","from","pass","break","continue","in","not","and","or","lambda","yield","global","nonlocal","assert","raise","del","is",
                                    ]
                                } else if toml_like {
                                    &["true","false"]
                                } else {
                                    // Rust/JS-like
                                    &[
                                        "fn","let","mut","const","pub","struct","enum","impl","trait","use","mod","crate","super","self","Self","match","if","else","for","while","loop","break","continue","return","async","await","move","ref","type","where","as","in","from","import","export","class","extends","new","this","static","final","const",
                                    ]
                                };

                                // Render line by line with simple highlighting
                                // Default text color from theme
                                let default_text = theme
                                    .get("ui.text")
                                    .fg
                                    .and_then(nucleotide_ui::theme_utils::color_to_hsla)
                                    .unwrap_or(gpui::white());

                                // Use the editor font and size for preview
                                let editor_font = cx.global::<nucleotide_types::FontSettings>().var_font.clone();
                                let editor_size = cx.global::<nucleotide_types::UiFontConfig>().size;

                                let mut container = div()
                                    .px_3()
                                    .py_2()
                                    .font(editor_font.into())
                                    .text_size(px(editor_size))
                                    .text_color(default_text)
                                    .overflow_y_hidden()
                                    .size_full();

                                // Try Helix loader-based highlighter for accurate colors
                                let mut used_loader = false;
                                if let Some(core) = render_core.upgrade() {
                                    let core_read = core.read(cx);
                                    let loader_arc = core_read.editor.syn_loader.load();
                                    let loader: &helix_core::syntax::Loader = &loader_arc;
                                    // Prepare rope
                                    let rope = helix_core::Rope::from(text);
                                    let slice = rope.slice(..);
                                    // Detect language by filename, shebang, or match
                                    let lang_opt = path
                                        .and_then(|p| loader.language_for_filename(p))
                                        .or_else(|| loader.language_for_shebang(slice))
                                        .or_else(|| loader.language_for_match(slice));

                                    if let Some(lang) = lang_opt
                                        && let Ok(syntax) = helix_core::syntax::Syntax::new(slice, lang, loader) {
                                            // Build highlighter for full range
                                            let mut hl = syntax.highlighter(
                                                slice,
                                                loader,
                                                0u32..(slice.len_bytes() as u32),
                                            );

                                            // Base colors
                                            let default_text = theme
                                                .get("ui.text")
                                                .fg
                                                .and_then(nucleotide_ui::theme_utils::color_to_hsla)
                                                .unwrap_or(gpui::white());

                                            // Build a single StyledText with highlight ranges
                                            use nucleotide_ui::text_utils::TextWithStyle;
                                            use gpui::{HighlightStyle, TextStyle};

                                            let full = String::from(slice);
                                            let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();

                                            let mut current_char = 0usize;
                                            let mut current_color = default_text;

                                            while hl.next_event_offset() != u32::MAX {
                                                let next_byte: u32 = hl.next_event_offset();
                                                let next_char = slice.byte_to_char(slice.ceil_char_boundary(next_byte as usize));

                                                if next_char > current_char && current_color != default_text {
                                                    highlights.push((current_char..next_char, HighlightStyle::color(current_color)));
                                                }

                                                // Advance style stack
                                                let (event, iter) = hl.advance();
                                                match event {
                                                    helix_core::syntax::HighlightEvent::Refresh => {
                                                        current_color = default_text;
                                                    }
                                                    helix_core::syntax::HighlightEvent::Push => {
                                                        for h in iter {
                                                            let style = theme.highlight(h);
                                                            if let Some(fg) = style.fg.and_then(nucleotide_ui::theme_utils::color_to_hsla) {
                                                                current_color = fg;
                                                            }
                                                        }
                                                    }
                                                }
                                                current_char = next_char;
                                            }

                                            // Tail
                                            if current_char < slice.len_chars() && current_color != default_text {
                                                highlights.push((current_char..slice.len_chars(), HighlightStyle::color(current_color)));
                                            }

                                            let default_style = TextStyle {
                                                font_family: cx
                                                    .global::<crate::types::FontSettings>()
                                                    .fixed_font
                                                    .family
                                                    .clone()
                                                    .into(),
                                                font_size: px(cx.global::<nucleotide_types::UiFontConfig>().size).into(),
                                                ..Default::default()
                                            };

                                            let styled = TextWithStyle::new(full).with_highlights(highlights).into_styled_text(&default_style);
                                            container = container.child(styled);
                                            used_loader = true;
                                        }
                                }

                                if !used_loader {
                                    for line in text.lines() {
                                        // Handle line comments (// or # for py/toml)
                                    // Handle line comments (// or # for py/toml)
                                    let (code_part, comment_part): (&str, Option<&str>) = if rust_like {
                                        if let Some(idx) = line.find("//") { (&line[..idx], Some(&line[idx..])) } else { (line, None) }
                                    } else if py_like || toml_like {
                                        if let Some(idx) = line.find('#') { (&line[..idx], Some(&line[idx..])) } else { (line, None) }
                                    } else {
                                        (line, None)
                                    };

                                    // Tokenize strings in code_part (simple double-quoted)
                                    let mut row = div();
                                    let mut rest = code_part;
                                    while let Some(start) = rest.find('"') {
                                        let (before, after_start) = rest.split_at(start);
                                        // Find closing quote
                                        if let Some(end_rel) = after_start[1..].find('"') {
                                            let end = 1 + end_rel; // position of closing quote relative to after_start
                                            // before
                                            if !before.is_empty() {
                                                row = row.child(before.to_string());
                                            }
                                            // string token
                                            let tok = &after_start[..=end];
                                            let mut s = div().child(tok.to_string());
                                            if let Some(c) = str_color { s = s.text_color(c); }
                                            row = row.child(s);
                                            // advance
                                            rest = &after_start[end+1..];
                                        } else {
                                            // no closing quote; push remainder and break
                                            row = row.child(rest.to_string());
                                            rest = "";
                                            break;
                                        }
                                    }
                                    if !rest.is_empty() {
                                        row = row.child(rest.to_string());
                                    }

                                    // Apply simple keyword and number coloring on the assembled row children if plain text
                                    // For simplicity, re-render a plain span with coarse keyword/number highlighting when no strings were found
                                    if !code_part.contains('"') {
                                        // Split by whitespace and punctuation
                                        let mut buf = String::new();
                                        let mut styled_row = div();
                                        for ch in code_part.chars() {
                                            if ch.is_alphanumeric() || ch == '_' {
                                                buf.push(ch);
                                            } else {
                                                // flush word with style if keyword/number
                                                if !buf.is_empty() {
                                                    let word = buf.clone();
                                                    if keywords.contains(&word.as_str()) {
                                                        let mut span = div().child(word);
                                                        if let Some(c) = kw_color { span = span.text_color(c); }
                                                        styled_row = styled_row.child(span);
                                                        buf.clear();
                                                    } else if word.chars().all(|c| c.is_ascii_digit()) {
                                                        let mut span = div().child(word);
                                                        if let Some(c) = num_color { span = span.text_color(c); }
                                                        styled_row = styled_row.child(span);
                                                        buf.clear();
                                                    } else {
                                                        styled_row = styled_row.child(std::mem::take(&mut buf));
                                                    }
                                                }
                                                // punctuation
                                                styled_row = styled_row.child(ch.to_string());
                                            }
                                        }
                                        if !buf.is_empty() {
                                            let word = std::mem::take(&mut buf);
                                            if keywords.contains(&word.as_str()) {
                                                let mut span = div().child(word);
                                                if let Some(c) = kw_color { span = span.text_color(c); }
                                                styled_row = styled_row.child(span);
                                            } else if word.chars().all(|c| c.is_ascii_digit()) {
                                                let mut span = div().child(word);
                                                if let Some(c) = num_color { span = span.text_color(c); }
                                                styled_row = styled_row.child(span);
                                            } else {
                                                styled_row = styled_row.child(word);
                                            }
                                        }
                                        row = styled_row;
                                    }

                                    // Append comment part with comment color
                                    if let Some(cmt) = comment_part {
                                        let mut cspan = div().child(cmt.to_string());
                                        if let Some(c) = com_color { cspan = cspan.text_color(c); }
                                        row = row.child(cspan);
                                    }

                                    container = container.child(row);
                                    }
                                }

                                container.into_any_element()
                            });
                            // Provide a preview text provider for buffer items
                            let text_core = core_weak.clone();
                            view = view.with_preview_text_provider_fn(move |item, cx| {
                                if let Some((doc_id, path_opt)) = item
                                    .data
                                    .downcast_ref::<(helix_view::DocumentId, Option<std::path::PathBuf>)>()
                                {
                                    if let Some(core) = text_core.upgrade() {
                                        let core_read = core.read(cx);
                                        if let Some(doc) = core_read.editor.documents.get(doc_id) {
                                            let content = String::from(doc.text().slice(..));
                                            return Some((content, path_opt.clone()));
                                        }
                                    }
                                } else if let Some(doc_id) = item.data.downcast_ref::<helix_view::DocumentId>()
                                    && let Some(core) = text_core.upgrade() {
                                        let core_read = core.read(cx);
                                        if let Some(doc) = core_read.editor.documents.get(doc_id) {
                                            let content = String::from(doc.text().slice(..));
                                            let path_opt = doc.path().cloned();
                                            return Some((content, path_opt));
                                        }
                                    }
                                None
                            });

                            // Preview capability integration can be wired here in the future

                            // Set up the selection callback
                            let core_for_on_select = core_weak.clone();
                            view = view.on_select(move |selected_item: &PickerItem, picker_cx| {
                                // Find the index of the selected item
                                if let Some(index) = items_for_callback
                                    .iter()
                                    .position(|item| std::ptr::eq(item, selected_item))
                                {
                                    // Call the original on_select callback with the index
                                    (on_select)(index);
                                }

                                // Proactively dismiss the picker first so focus can restore immediately
                                picker_cx.emit(gpui::DismissEvent);

                                // Handle LSP code action items (action, server id, offset)
                                if let Some((action, ls_id, offset)) = selected_item
                                    .data
                                    .downcast_ref::<(
                                        helix_lsp::lsp::CodeActionOrCommand,
                                        helix_core::diagnostic::LanguageServerId,
                                        helix_lsp::OffsetEncoding,
                                    )>()
                                    && let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            if let Some(ls) = core.editor.language_server_by_id(*ls_id)
                                            {
                                                match action {
                                                    helix_lsp::lsp::CodeActionOrCommand::Command(
                                                        cmd,
                                                    ) => {
                                                        core.editor.execute_lsp_command(
                                                            cmd.clone(),
                                                            *ls_id,
                                                        );
                                                    }
                                                    helix_lsp::lsp::CodeActionOrCommand::CodeAction(
                                                        ca,
                                                    ) => {
                                                        let mut resolved: Option<
                                                            helix_lsp::lsp::CodeAction,
                                                        > = None;
                                                        if (ca.edit.is_none() || ca.command.is_none())
                                                            && let Some(fut) =
                                                                ls.resolve_code_action(ca)
                                                                && let Ok(c) =
                                                                    helix_lsp::block_on(fut)
                                                                {
                                                                    resolved = Some(c);
                                                                }
                                                        let action_ref =
                                                            resolved.as_ref().unwrap_or(ca);
                                                        if let Some(edit) = &action_ref.edit {
                                                            let _ = core.editor.apply_workspace_edit(
                                                                *offset,
                                                                edit,
                                                            );
                                                        }
                                                        if let Some(cmd) = &action_ref.command {
                                                            core.editor.execute_lsp_command(
                                                                cmd.clone(),
                                                                *ls_id,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        });
                                    }
                                // Check if it's a buffer picker item (DocumentId, Option<PathBuf>)
                                if let Some((doc_id, _path)) = selected_item.data.downcast_ref::<(
                                    helix_view::DocumentId,
                                    Option<std::path::PathBuf>,
                                )>(
                                ) {
                                    // Switch to the selected buffer
                                    if let Some(core) = core_for_on_select.upgrade() {
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
                                    if let Some(core) = core_for_on_select.upgrade() {
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
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            // Switch to the selected document
                                            core.editor.switch(
                                                *doc_id,
                                                helix_view::editor::Action::Replace,
                                            );
                                        });
                                    }
                                }
                                // (Picker was already dismissed above)
                            });

                            // Set up the cancel callback
                            view = view.on_cancel(move |cx| {
                                // The PickerView will handle its own dismissal
                                cx.emit(DismissEvent);
                            });

                            // Capability already attached above

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

        // Get ui.menu style with fallback to dropdown tokens
        let ui_menu = cx.theme_style("ui.menu");

        nucleotide_ui::prompt_view::PromptStyle {
            modal_style,
            completion_background: ui_menu
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(tokens.dropdown_tokens().container_background),
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

        // Use component/chrome tokens as fallbacks - guaranteed to exist
        let background = ui_popup
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.picker_tokens().container_background);
        let text = ui_text
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.chrome.text_on_chrome);
        let selected_background = ui_menu_selected
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.dropdown_tokens().item_background_selected);
        let selected_text = ui_menu_selected
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.dropdown_tokens().item_text_selected);
        let border = ui_popup
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.picker_tokens().border);
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

    /// Calculate cursor position for completion popup placement
    /// TODO: This should get actual cursor position from document view
    /// Calculate cursor-based completion position using exact cursor coordinates when available
    fn calculate_completion_position(&self, cx: &Context<Self>) -> (gpui::Pixels, gpui::Pixels) {
        let layout_info = self.get_workspace_layout_info(cx);

        // Use exact cursor coordinates if available from DocumentView rendering
        if let (Some(cursor_pos), Some(cursor_size)) =
            (layout_info.cursor_position, layout_info.cursor_size)
        {
            nucleotide_logging::debug!(
                pos = ?cursor_pos,
                size = ?cursor_size,
                "Using exact cursor coordinates"
            );
            // cursor_pos is already the top-left of cursor, we want bottom-left for completion
            return (cursor_pos.x, cursor_pos.y + cursor_size.height);
        }

        // Fallback to calculated position if no exact coordinates available
        nucleotide_logging::debug!("Using calculated cursor position (fallback)");
        if let Some(core) = self.core.upgrade() {
            let core_read = core.read(cx);
            let editor = &core_read.editor;

            // Get focused view and document
            let focused_view_id = editor.tree.focus;
            let (view, doc_id) = match editor.tree.try_get(focused_view_id) {
                Some(view) => (view, view.doc),
                None => {
                    // No focused view; fallback to conservative defaults
                    return (px(0.0), px(0.0));
                }
            };

            if let Some(document) = editor.documents.get(&doc_id) {
                let text = document.text();

                // Get primary cursor position
                let primary_selection = document.selection(focused_view_id).primary();
                let cursor_char_idx = primary_selection.cursor(text.slice(..));

                // Convert character position to screen coordinates
                if let Some(cursor_pos) =
                    view.screen_coords_at_pos(document, text.slice(..), cursor_char_idx)
                {
                    let view_offset = document.view_offset(focused_view_id);

                    // Check if cursor is visible in viewport
                    let viewport_height = view.inner_height();
                    if cursor_pos.row >= view_offset.vertical_offset
                        && cursor_pos.row < view_offset.vertical_offset + viewport_height
                    {
                        // Calculate relative position within viewport (0-based line in visible area)
                        let relative_row =
                            cursor_pos.row.saturating_sub(view_offset.vertical_offset);

                        // Account for UI layout: file tree + gutter + character position
                        let cursor_x = layout_info.file_tree_width
                            + layout_info.gutter_width
                            + px(cursor_pos.col as f32 * layout_info.char_width.0);

                        // Account for UI layout: title bar + tab bar + line position within document area
                        let document_area_y =
                            layout_info.title_bar_height + layout_info.tab_bar_height;
                        let cursor_y =
                            document_area_y + px(relative_row as f32 * layout_info.line_height.0);

                        return (cursor_x, cursor_y + layout_info.line_height); // Position below cursor
                    }
                }
            }
        }

        // Final fallback positioning
        let fallback_x = layout_info.file_tree_width + layout_info.gutter_width + px(10.0);
        let fallback_y = layout_info.title_bar_height + layout_info.tab_bar_height + px(20.0);
        println!(
            "DEBUG: Final fallback position: x={:?}, y={:?} (file_tree_width={:?}, gutter_width={:?}, title_bar_height={:?}, tab_bar_height={:?})",
            fallback_x,
            fallback_y,
            layout_info.file_tree_width,
            layout_info.gutter_width,
            layout_info.title_bar_height,
            layout_info.tab_bar_height
        );
        (fallback_x, fallback_y)
    }

    /// Get workspace layout information for completion positioning
    /// Attempts to access real workspace dimensions, falls back to reasonable defaults
    fn get_workspace_layout_info(&self, cx: &Context<Self>) -> WorkspaceLayoutInfo {
        // Try to access workspace layout through global state
        if let Some(layout) = cx.try_global::<WorkspaceLayoutInfo>() {
            return *layout;
        }

        // TODO: Alternative approaches to access workspace:
        // 1. Pass layout info when creating completion view
        // 2. Store layout in app-global state
        // 3. Make workspace accessible through entity hierarchy

        // For now, use reasonable defaults that match typical Nucleotide layout
        // These values should be close to the actual defaults but won't reflect user resizing
        WorkspaceLayoutInfo {
            file_tree_width: px(250.0), // Default file tree width (user can resize)
            gutter_width: px(60.0),     // Line number gutter width
            tab_bar_height: px(40.0),   // Tab bar height
            title_bar_height: px(30.0), // Much smaller - just window controls
            line_height: px(20.0),      // Default line height
            char_width: px(12.0),       // Default character width
            cursor_position: None,      // No exact cursor position available
            cursor_size: None,          // No exact cursor size available
        }
    }
}

// Translate a GPUI key event to terminal bytes suitable for PTY input.
// Handles printable text, control keys, arrows/navigation and a subset of function keys.
pub(crate) fn translate_key_to_bytes(event: &gpui::KeyDownEvent) -> Vec<u8> {
    use gpui::Modifiers;

    let ks = &event.keystroke;
    let mods: &Modifiers = &ks.modifiers;

    // Helper: map ctrl+key to control character
    fn ctrl_byte_for(key: &str) -> Option<u8> {
        if key.len() == 1 {
            let ch = key.as_bytes()[0].to_ascii_uppercase();
            if (b'A'..=b'Z').contains(&ch) {
                return Some(ch - b'@'); // Ctrl-A => 0x01 ... Ctrl-Z => 0x1A
            }
            if ch == b' ' {
                return Some(0x00); // Ctrl-Space => NUL
            }
        }
        match key {
            "@" => Some(0x00),
            "[" => Some(0x1B), // ESC
            "\\" => Some(0x1C),
            "]" => Some(0x1D),
            "^" => Some(0x1E),
            "_" => Some(0x1F),
            "space" => Some(0x00),
            _ => None,
        }
    }

    // If platform/cmd is held, treat as app shortcut; don't send to PTY
    if mods.platform {
        return Vec::new();
    }

    // If there is a typed character from IME and no control/platform/alt (except shift), send it
    if let Some(s) = &ks.key_char {
        // If Alt is held, prefix ESC to emulate Meta behavior
        if mods.alt && !mods.control {
            let mut out = vec![0x1B];
            out.extend_from_slice(s.as_bytes());
            return out;
        }
        return s.as_bytes().to_vec();
    }

    // Control-modified keys
    if mods.control {
        if let Some(b) = ctrl_byte_for(ks.key.as_str()) {
            // Support Alt as ESC prefix on control sequences that are printable/control bytes
            if mods.alt {
                return vec![0x1B, b];
            }
            return vec![b];
        }
    }

    // Named non-printable keys and navigation
    let seq: Option<&[u8]> = match ks.key.as_str() {
        // Basics
        "enter" => Some(b"\r"),
        "tab" => Some(b"\t"),
        "backspace" => Some(&[0x7F]),
        "escape" => Some(&[0x1B]),

        // Arrows
        "up" => Some(b"\x1b[A"),
        "down" => Some(b"\x1b[B"),
        "right" => Some(b"\x1b[C"),
        "left" => Some(b"\x1b[D"),

        // Navigation
        "home" => Some(b"\x1b[H"),
        "end" => Some(b"\x1b[F"),
        "insert" => Some(b"\x1b[2~"),
        "delete" => Some(b"\x1b[3~"),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),

        // Function keys (xterm common mappings)
        "f1" => Some(b"\x1bOP"),
        "f2" => Some(b"\x1bOQ"),
        "f3" => Some(b"\x1bOR"),
        "f4" => Some(b"\x1bOS"),
        "f5" => Some(b"\x1b[15~"),
        "f6" => Some(b"\x1b[17~"),
        "f7" => Some(b"\x1b[18~"),
        "f8" => Some(b"\x1b[19~"),
        "f9" => Some(b"\x1b[20~"),
        "f10" => Some(b"\x1b[21~"),
        "f11" => Some(b"\x1b[23~"),
        "f12" => Some(b"\x1b[24~"),

        _ => None,
    };
    if let Some(s) = seq {
        if mods.alt {
            // Prefix ESC to denote Meta for navigation keys
            let mut out = Vec::with_capacity(1 + s.len());
            out.push(0x1B);
            out.extend_from_slice(s);
            return out;
        }
        return s.to_vec();
    }

    // If key looks printable but key_char wasn't provided (e.g., synthetic), synthesize from key
    if ks.key.len() == 1 {
        let mut ch = ks.key.as_bytes()[0] as char;
        if mods.shift {
            ch = ch.to_ascii_uppercase();
        }
        let mut out = Vec::new();
        if mods.alt {
            out.push(0x1B);
        }
        out.extend_from_slice(ch.to_string().as_bytes());
        return out;
    }

    // Unhandled -> no bytes
    Vec::new()
}

/// Layout information for positioning UI elements relative to workspace
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceLayoutInfo {
    pub file_tree_width: gpui::Pixels,
    pub gutter_width: gpui::Pixels,
    pub tab_bar_height: gpui::Pixels,
    pub title_bar_height: gpui::Pixels,
    // Font metrics from the actual DocumentView
    pub line_height: gpui::Pixels,
    pub char_width: gpui::Pixels,
    // Exact cursor coordinates from DocumentView rendering
    pub cursor_position: Option<gpui::Point<Pixels>>,
    pub cursor_size: Option<gpui::Size<Pixels>>,
}

impl gpui::Global for WorkspaceLayoutInfo {}

impl OverlayView {
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

        // Use chrome/editor tokens as fallbacks - guaranteed to exist
        let preview_background = ui_background_separator
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.chrome.surface);
        let cursor = ui_cursor
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.editor.cursor_normal);

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
            nucleotide_logging::info!("DIAG: Render overlay branch: picker");
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
impl EventEmitter<nucleotide_ui::completion_v2::CompleteViaHelixEvent> for OverlayView {}

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
            nucleotide_logging::info!("DIAG: Render overlay branch: prompt");
            // Render prompt with design tokens and consistent overlay patterns
            let theme = cx.theme();
            let tokens = &theme.tokens;

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
                        .pt(tokens.sizes.space_8)
                        .child(prompt_view.clone()),
                )
                .into_any_element();
        }

        if let Some(diag_panel) = &self.diagnostics_panel {
            nucleotide_logging::info!("DIAG: Render overlay branch: diagnostics");
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .occlude()
                // Clicking outside the diagnostics panel dismisses it and restores editor focus group
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this: &mut OverlayView, _e, window, cx| {
                        // Release focus from overlay elements before dismissing
                        window.disable_focus();
                        this.diagnostics_panel = None;
                        cx.emit(DismissEvent);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        // Consume clicks inside the panel area so they don't bubble to the overlay
                        .on_mouse_down(MouseButton::Left, |_, _, _| {})
                        .child(diag_panel.clone()),
                )
                .into_any_element();
        }

        if let Some(completion_view) = &self.completion_view {
            nucleotide_logging::info!("DIAG: Render overlay branch: completion");
            use gpui::{Corner, anchored, point};

            // Calculate proper completion position based on cursor location
            let (cursor_x, cursor_y) = self.calculate_completion_position(cx);
            nucleotide_logging::debug!(
                x = ?cursor_x,
                y = ?cursor_y,
                "Rendering completion popup at calculated position"
            );

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .occlude() // Ensure overlay is on top of other elements
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    // Prevent click-through to elements below
                })
                .child(
                    anchored()
                        .position(point(cursor_x, cursor_y))
                        .anchor(Corner::TopLeft) // Anchor top-left of completion to cursor position
                        .offset(point(px(0.0), px(2.0))) // Small offset below cursor
                        .snap_to_window_with_margin(px(8.0))
                        .child(completion_view.clone()),
                )
                .into_any_element();
        }

        // Terminal panel - docked at bottom
        if let Some(terminal_panel) = &self.terminal_panel {
            {
                // Ensure we have a persistent focus handle for the terminal panel area
                if self.terminal_focus.is_none() {
                    self.terminal_focus = Some(cx.focus_handle());
                }
                let panel_focus = self.terminal_focus.as_ref().unwrap().clone();
                let panel_focus_for_keys = panel_focus.clone();

                // Sync from window-level resize state
                if let Ok(st) = self.resize_state.lock() {
                    if (st.height - self.terminal_height_px).abs() >= 0.5 {
                        self.terminal_height_px = st.height;
                    }
                }

                // Dynamically compute terminal cols/rows from current window bounds and font metrics
                // to keep the PTY/emulator sized to the visible area.
                if let Some(core) = self.core.upgrade() {
                    let layout = self.get_workspace_layout_info(cx);
                    let window_width = _window.bounds().size.width.0;
                    // Use editor monospace metrics for terminal sizing
                    let editor_font = cx.global::<nucleotide_types::EditorFontConfig>();
                    let font = gpui::Font {
                        family: editor_font.family.clone().into(),
                        features: gpui::FontFeatures::default(),
                        weight: editor_font.weight.into(),
                        style: gpui::FontStyle::Normal,
                        fallbacks: None,
                    };
                    let font_id = cx.text_system().resolve_font(&font);
                    let font_size = gpui::px(editor_font.size);
                    let char_w = cx
                        .text_system()
                        .advance(font_id, font_size, 'm')
                        .map(|a| a.width.0)
                        .unwrap_or(editor_font.size * 0.6)
                        .max(1.0);
                    // Approximate line height for terminal rows
                    let line_h = (editor_font.size * 1.35).max(1.0);
                    // Use resizable panel height
                    let panel_height = self.terminal_height_px;
                    // Constrain to editor content width by subtracting file tree width
                    let usable_width = (window_width - layout.file_tree_width.0).max(1.0);
                    let cols = (usable_width / char_w).floor().max(1.0) as u16;
                    let rows = (panel_height / line_h).floor().max(1.0) as u16;

                    let active_id = terminal_panel.read(cx).active;
                    let changed = match self.last_terminal_size {
                        Some((id, last_c, last_r))
                            if id == active_id && last_c == cols && last_r == rows =>
                        {
                            false
                        }
                        _ => true,
                    };
                    if changed {
                        self.last_terminal_size = Some((active_id, cols, rows));
                        core.update(cx, |app, _| {
                            if let Some(bus) = &app.event_aggregator {
                                bus.dispatch_terminal(
                                    nucleotide_events::v2::terminal::Event::Resized {
                                        id: active_id,
                                        cols,
                                        rows,
                                    },
                                );
                            }
                        });
                    }
                }

                // Constrain terminal height to avoid covering the entire editor
                let window_h = _window.bounds().size.height.0;
                let max_h = (window_h * 0.6).max(120.0);
                let clamped_h = self.terminal_height_px.clamp(80.0, max_h);
                if (clamped_h - self.terminal_height_px).abs() > 0.5 {
                    self.terminal_height_px = clamped_h;
                    if let Ok(mut st) = self.resize_state.lock() {
                        st.height = clamped_h;
                    }
                }

                // Update the panel entity with the current height before borrowing cx immutably
                if let Some(panel) = &self.terminal_panel {
                    let h = self.terminal_height_px;
                    panel.update(cx, |p, _| p.height_px = h);
                }

                // Theme after potential mutable cx borrow above
                let theme = cx.theme();
                let _tokens = &theme.tokens;

                return div()
                    .key_context("OverlayTerminalPanel")
                    .absolute()
                    .size_full()
                    .bottom_0()
                    .left_0()
                    // Do NOT occlude the entire window; allow clicks to reach the editor
                    .on_key_down(cx.listener(
                        move |this: &mut OverlayView, event: &gpui::KeyDownEvent, window, cx| {
                            if !panel_focus_for_keys.is_focused(window) {
                                return;
                            }
                            if let Some(core) = this.core.upgrade() {
                                let maybe_id =
                                    this.terminal_panel.as_ref().map(|p| p.read(cx).active);
                                if let Some(id) = maybe_id {
                                    let bytes = translate_key_to_bytes(event);
                                    if !bytes.is_empty() {
                                        core.update(cx, |app, _| {
                                            if let Some(bus) = &app.event_aggregator {
                                                bus.dispatch_terminal(
                                                    nucleotide_events::v2::terminal::Event::Input {
                                                        id,
                                                        bytes,
                                                    },
                                                );
                                            }
                                        });
                                        cx.stop_propagation();
                                    }
                                }
                            }
                        },
                    ))
                    .child({
                        // Use shared bottom panel split
                        let on_change_height = {
                            let entity = cx.entity().clone();
                            move |new_h: f32, app_cx: &mut gpui::App| {
                                entity.update(app_cx, |this: &mut OverlayView, cx| {
                                    this.terminal_height_px = new_h;
                                    if let Ok(mut st) = this.resize_state.lock() {
                                        st.height = new_h;
                                        st.resizing = true;
                                    }
                                    cx.notify();
                                });
                            }
                        };

                        nucleotide_ui::bottom_panel_split(
                            self.terminal_height_px,
                            80.0,
                            (window_h * 0.6).max(120.0),
                            4.0,
                            220.0,
                            on_change_height,
                            {
                                let debug = matches!(
                                    std::env::var("NUCL_DEBUG_COLORS")
                                        .map(|v| v.to_ascii_lowercase())
                                        .as_deref(),
                                    Ok("1") | Ok("true") | Ok("yes") | Ok("on")
                                );
                                let mut c = div().track_focus(&panel_focus);
                                if debug {
                                    c = c
                                        .relative()
                                        .bg(gpui::hsla(0.35, 0.8, 0.6, 0.18)) // light green tint
                                        .border_t_2()
                                        .border_color(gpui::hsla(0.35, 0.95, 0.55, 0.9));
                                }
                                // First, terminal content
                                c = c.child(terminal_panel.clone());
                                // Then the debug label so it stays on top
                                if debug {
                                    c = c.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .bg(gpui::hsla(0.35, 0.95, 0.55, 0.85))
                                            .text_color(gpui::black())
                                            .child("TERMINAL"),
                                    );
                                }
                                c
                            },
                        )
                    })
                    .into_any_element();
            }
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
        nucleotide_logging::info!("DIAG: Render overlay branch: none");
        div().size_0().into_any_element()
    }
}
