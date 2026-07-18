use crate::types::RegexSelectionAction;
use gpui::{
    App, AppContext, ClipboardItem, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, Styled,
    Window, div, px,
};
use helix_stdx::rope::RopeSliceExt;
use nucleotide_terminal::TerminalBounds;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::completion_v2::CompletionView;
use nucleotide_ui::picker::Picker;
use nucleotide_ui::picker_view::{PickerItem, PickerView};
use nucleotide_ui::prompt::Prompt;
use nucleotide_ui::prompt_view::PromptView;
use nucleotide_ui::theme_manager::HelixThemedContext; // bring dispatch_* trait methods into scope
use nucleotide_ui::{CompletionMenuAction, OverlaySurface};
use std::sync::{Arc, Mutex};

pub fn init(cx: &mut App) {
    crate::remote_connection_manager::init(cx);
}

pub struct OverlayView {
    native_picker_view: Option<Entity<PickerView>>,
    native_prompt_view: Option<Entity<PromptView>>,
    remote_connection_manager_view:
        Option<Entity<crate::remote_connection_manager::RemoteConnectionManagerView>>,
    completion_view: Option<Entity<CompletionView>>,
    terminal_panel: Option<Entity<nucleotide_terminal_panel::TerminalPanel>>,
    // Resizable terminal panel height (pixels)
    terminal_height_px: f32,
    // Resize interaction state
    _terminal_resizing: bool,
    _resize_start_mouse_y: gpui::Pixels,
    _resize_start_height: f32,
    // Shared state for window-level resize listeners
    resize_state: Arc<Mutex<ResizeStateInner>>,
    // Track last dispatched terminal size to avoid redundant resize events
    last_terminal_size: Option<(nucleotide_events::v2::terminal::TerminalId, u16, u16)>,
    focus: FocusHandle,
    core: gpui::WeakEntity<crate::Core>,
    handle: tokio::runtime::Handle,
    // Cached terminal font metrics to avoid per-frame font measurement
    cached_font_key: Option<(String, f32, nucleotide_types::FontWeight)>, // (family, size, weight)
    cached_char_width: Option<f32>,
    cached_line_height: Option<f32>,
}

#[derive(Debug)]
struct ResizeStateInner {
    resizing: bool,
    _start_mouse_y: f32,
    _start_height: f32,
    height: f32,
}

const REMOTE_PATH_COMPLETION_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(1);
const REMOTE_PATH_COMPLETION_WAIT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(500);
const REMOTE_PATH_COMPLETION_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(25);

fn document_revisions(editor: &mut helix_view::Editor) -> Vec<(helix_view::DocumentId, usize)> {
    editor
        .documents
        .iter_mut()
        .map(|(doc_id, doc)| (*doc_id, doc.get_current_revision()))
        .collect()
}

fn changed_documents_since(
    editor: &mut helix_view::Editor,
    before: &[(helix_view::DocumentId, usize)],
) -> Vec<helix_view::DocumentId> {
    let mut changed = Vec::new();

    for (doc_id, before_revision) in before {
        if let Some(doc) = editor.document_mut(*doc_id)
            && doc.get_current_revision() != *before_revision
        {
            changed.push(*doc_id);
        }
    }

    changed
}

fn apply_code_action_or_command(
    core: &mut crate::Core,
    action: &helix_lsp::lsp::CodeActionOrCommand,
    language_server_id: helix_core::diagnostic::LanguageServerId,
) -> Vec<helix_view::DocumentId> {
    let mut changed_documents = Vec::new();

    let Some(offset_encoding) = core
        .editor
        .language_server_by_id(language_server_id)
        .map(|language_server| language_server.offset_encoding())
    else {
        core.editor.set_error("Language Server disappeared");
        return changed_documents;
    };

    match action {
        helix_lsp::lsp::CodeActionOrCommand::Command(command) => {
            core.editor
                .execute_lsp_command(command.clone(), language_server_id);
        }
        helix_lsp::lsp::CodeActionOrCommand::CodeAction(code_action) => {
            let resolved_code_action = if code_action.edit.is_none()
                || code_action.command.is_none()
            {
                core.editor
                    .language_server_by_id(language_server_id)
                    .and_then(|language_server| language_server.resolve_code_action(code_action))
                    .and_then(|future| helix_lsp::block_on(future).ok())
            } else {
                None
            };

            let resolved_or_original = resolved_code_action.as_ref().unwrap_or(code_action);

            if let Some(edit) = &resolved_or_original.edit {
                let before_revisions = document_revisions(&mut core.editor);
                match core.apply_lsp_workspace_edit(offset_encoding, edit) {
                    Ok(()) => {
                        changed_documents
                            .extend(changed_documents_since(&mut core.editor, &before_revisions));
                    }
                    Err(err) => core.editor.set_error(format!("{err:?}")),
                }
            }

            if let Some(command) = resolved_or_original
                .command
                .as_ref()
                .or(code_action.command.as_ref())
            {
                core.editor
                    .execute_lsp_command(command.clone(), language_server_id);
            }
        }
    }

    changed_documents
}

#[derive(Clone, Copy)]
enum PromptSubmitAction {
    Command,
    Search,
    GlobalSearch,
    FileTreeSearch,
    RemoteOpen,
    RegexSelection(RegexSelectionAction),
}

fn prompt_submit_action(prompt_text: &str) -> PromptSubmitAction {
    match prompt_text {
        "search:" | "rsearch:" => PromptSubmitAction::Search,
        "global-search:" => PromptSubmitAction::GlobalSearch,
        "file-tree-search:" => PromptSubmitAction::FileTreeSearch,
        crate::remote_open::REMOTE_OPEN_PROMPT => PromptSubmitAction::RemoteOpen,
        "select:" => PromptSubmitAction::RegexSelection(RegexSelectionAction::Select),
        "split:" => PromptSubmitAction::RegexSelection(RegexSelectionAction::Split),
        "keep:" => PromptSubmitAction::RegexSelection(RegexSelectionAction::Keep),
        "remove:" => PromptSubmitAction::RegexSelection(RegexSelectionAction::Remove),
        _ => PromptSubmitAction::Command,
    }
}

impl OverlayView {
    pub fn new(
        focus: &FocusHandle,
        core: &Entity<crate::Core>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            native_picker_view: None,
            native_prompt_view: None,
            remote_connection_manager_view: None,
            completion_view: None,
            terminal_panel: None,
            terminal_height_px: 220.0,
            _terminal_resizing: false,
            _resize_start_mouse_y: px(0.0),
            _resize_start_height: 220.0,
            resize_state: Arc::new(Mutex::new(ResizeStateInner {
                resizing: false,
                _start_mouse_y: 0.0,
                _start_height: 220.0,
                height: 220.0,
            })),
            last_terminal_size: None,
            focus: focus.clone(),
            core: core.downgrade(),
            handle,
            cached_font_key: None,
            cached_char_width: None,
            cached_line_height: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        let empty = self.native_picker_view.is_none()
            && self.native_prompt_view.is_none()
            && self.remote_connection_manager_view.is_none()
            && self.completion_view.is_none()
            && self.terminal_panel.is_none();

        if !empty && self.completion_view.is_some() {
            nucleotide_logging::debug!("COMP: Overlay not empty - has completion view");
        }

        empty
    }

    pub fn has_completion(&self) -> bool {
        self.completion_view.is_some()
    }

    pub fn has_picker(&self) -> bool {
        self.native_picker_view.is_some()
    }

    pub fn has_prompt(&self) -> bool {
        self.native_prompt_view.is_some()
    }

    pub fn has_remote_connection_manager(&self) -> bool {
        self.remote_connection_manager_view.is_some()
    }

    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if self.clear_completion(cx) {
            cx.emit(DismissEvent);
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

    fn handle_completion_arrow_key(&self, key: &str, cx: &mut Context<Self>) -> bool {
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

    pub fn handle_completion_menu_action(
        &mut self,
        action: CompletionMenuAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CompletionMenuAction::Confirm => self.handle_completion_tab_key(cx),
            CompletionMenuAction::Dismiss => {
                self.dismiss_completion(cx);
                true
            }
            CompletionMenuAction::SelectNext => self.handle_completion_arrow_key("down", cx),
            CompletionMenuAction::SelectPrevious => self.handle_completion_arrow_key("up", cx),
        }
    }

    pub fn handle_completion_tab_key(&self, cx: &mut Context<Self>) -> bool {
        if let Some(completion_view) = &self.completion_view {
            completion_view.update(cx, |view, cx| {
                // Accept the currently selected completion item
                if let Some(selected_index) = view.selected_index() {
                    nucleotide_logging::debug!(
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

    pub fn completion_commit_accept_index(
        &self,
        commit_character: char,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let completion_view = self.completion_view.as_ref()?;
        completion_view.update(cx, |view, _cx| {
            let selected_index = view.selected_index()?;
            let selected_item = view.selected_item()?;
            let commit_character = commit_character.to_string();

            selected_item
                .commit_characters
                .iter()
                .any(|character| character.as_ref() == commit_character)
                .then_some(selected_index)
        })
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

    pub fn dismiss_all(&mut self, cx: &mut Context<Self>) {
        if self.clear_overlays(cx) {
            cx.emit(DismissEvent);
        }
        cx.notify();
    }

    fn clear_overlays(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed_picker = self.clear_picker(cx);
        let dismissed_prompt = self.clear_prompt(cx);
        let dismissed_remote_manager = self.clear_remote_connection_manager(cx);
        let dismissed_completion = self.clear_completion(cx);

        dismissed_picker || dismissed_prompt || dismissed_remote_manager || dismissed_completion
    }

    fn clear_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(picker) = self.native_picker_view.take() else {
            return false;
        };

        picker.update(cx, |picker, cx| {
            picker.cleanup(cx);
        });
        Self::clear_picker_focus(cx);
        true
    }

    fn clear_prompt(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed = self.native_prompt_view.take().is_some();
        if dismissed {
            Self::clear_prompt_focus(cx);
        }
        dismissed
    }

    fn clear_remote_connection_manager(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed = self.remote_connection_manager_view.take().is_some();
        if dismissed {
            Self::clear_prompt_focus(cx);
        }
        dismissed
    }

    fn clear_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed = self.completion_view.take().is_some();
        if dismissed {
            Self::clear_completion_focus(cx);
        }
        dismissed
    }

    fn clear_picker_focus(cx: &mut Context<Self>) {
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.clear_picker_focus();
        }
    }

    fn clear_prompt_focus(cx: &mut Context<Self>) {
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.clear_prompt_focus();
        }
    }

    fn clear_completion_focus(cx: &mut Context<Self>) {
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.clear_completion_focus();
        }
    }

    fn dismiss_picker(&mut self, cx: &mut Context<Self>) {
        if self.clear_picker(cx) {
            cx.emit(DismissEvent);
            cx.notify();
        }
    }

    fn dismiss_prompt(&mut self, cx: &mut Context<Self>) {
        if self.clear_prompt(cx) {
            cx.emit(DismissEvent);
            cx.notify();
        }
    }

    fn dismiss_remote_connection_manager(&mut self, cx: &mut Context<Self>) {
        if self.clear_remote_connection_manager(cx) {
            cx.emit(DismissEvent);
            cx.notify();
        }
    }

    fn replace_picker(&mut self, cx: &mut Context<Self>) {
        if self.clear_picker(cx) {
            cx.notify();
        }
    }

    fn replace_prompt(&mut self, cx: &mut Context<Self>) {
        if self.clear_prompt(cx) {
            cx.notify();
        }
    }

    fn replace_remote_connection_manager(&mut self, cx: &mut Context<Self>) {
        if self.clear_remote_connection_manager(cx) {
            cx.notify();
        }
    }

    fn replace_completion(&mut self, cx: &mut Context<Self>) {
        if self.clear_completion(cx) {
            cx.notify();
        }
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
                nucleotide_logging::trace!("DIAG: Overlay Update::Prompt received");
                self.replace_prompt(cx);
                self.replace_remote_connection_manager(cx);
                let Prompt {
                    prompt: prompt_text,
                    initial_input,
                    on_submit,
                    on_cancel,
                } = prompt;
                let prompt_text = prompt_text.clone();
                let initial_input = initial_input.clone();
                let on_submit = on_submit.clone();
                let on_cancel = on_cancel.clone();

                let prompt_view = cx.new(|cx| {
                    let submit_action = prompt_submit_action(&prompt_text);

                    let mut view = PromptView::new(prompt_text.clone(), cx);

                    // Apply theme styling using testing-aware theme access
                    let style = Self::create_prompt_style_from_context(cx);
                    view = view.with_style(style);

                    if !initial_input.is_empty() {
                        view.set_text(&initial_input, cx);
                    }

                    // Only set up completion for command prompts, not search/regex prompts.
                    if matches!(submit_action, PromptSubmitAction::Command) {
                        let completion_context = self
                            .core
                            .upgrade()
                            .map(|core| {
                                let core = core.read(cx);
                                let complete_filesystem_paths = matches!(
                                    core.workspace_backend.identity(),
                                    nucleotide_workspace::WorkspaceIdentity::Local
                                );
                                let completion_cache =
                                    crate::completions::CommandCompletionCache::from_editor(
                                        &core.editor,
                                    )
                                    .with_filesystem_paths(complete_filesystem_paths);
                                let remote_path_context = (!complete_filesystem_paths)
                                    .then(|| {
                                        core.project_directory.clone().map(|base_dir| {
                                            (core.workspace_backend.clone(), base_dir)
                                        })
                                    })
                                    .flatten();

                                (completion_cache, remote_path_context)
                            })
                            .unwrap_or_default();
                        let (completion_cache, remote_path_context) = completion_context;

                        view = view.with_completion_fn(move |input| {
                            // Strip leading colon if present
                            let input = input.strip_prefix(':').unwrap_or(input);

                            // Use cached editor context so prompt completion does not
                            // depend on Helix's terminal UI prompt/completer layer.
                            crate::completions::get_command_completions_with_cache(
                                input,
                                Some(&completion_cache),
                            )
                        });

                        if let Some((workspace_backend, base_dir)) = remote_path_context {
                            let handle = self.handle.clone();
                            let remote_completion_cache =
                                crate::completions::PathCompletionCandidateCache::new(
                                    REMOTE_PATH_COMPLETION_CACHE_TTL,
                                );
                            view = view.with_completion_task_fn(move |input, cx| {
                                let input = input.strip_prefix(':').unwrap_or(input).to_string();
                                let query = crate::completions::workspace_path_completion_query(
                                    &input, &base_dir,
                                )?;
                                let query_for_items = query.clone();
                                let backend = workspace_backend.clone();
                                let handle = handle.clone();
                                let cache = remote_completion_cache.clone();

                                if let Some(candidates) = cache.cached_candidates(&query.directory)
                                {
                                    return Some(cx.spawn(async move |_view, _cx| {
                                        crate::completions::path_completion_items(
                                            &input,
                                            &query_for_items,
                                            candidates,
                                        )
                                    }));
                                }

                                let directory = query.directory.clone();
                                if cache.begin_request(directory.clone()) {
                                    let backend = backend.clone();
                                    let query = query.clone();
                                    let cache = cache.clone();
                                    handle.spawn(async move {
                                        let candidates =
                                            crate::completions::workspace_path_completion_candidates(
                                                backend, query,
                                            )
                                            .await;
                                        cache.finish_request(directory, candidates);
                                    });
                                }

                                Some(cx.spawn(async move |_view, cx| {
                                    let started = std::time::Instant::now();
                                    loop {
                                        if let Some(candidates) =
                                            cache.cached_candidates(&query_for_items.directory)
                                        {
                                            return crate::completions::path_completion_items(
                                                &input,
                                                &query_for_items,
                                                candidates,
                                            );
                                        }

                                        if !cache.is_in_flight(&query_for_items.directory)
                                            || started.elapsed()
                                                >= REMOTE_PATH_COMPLETION_WAIT_TIMEOUT
                                        {
                                            return Vec::new();
                                        }

                                        cx.background_executor()
                                            .timer(REMOTE_PATH_COMPLETION_POLL_INTERVAL)
                                            .await;
                                    }
                                }))
                            });
                        }
                    }

                    if matches!(submit_action, PromptSubmitAction::RemoteOpen) {
                        let connection_store =
                            match crate::remote_connections::RemoteConnectionStore::load_default()
                            {
                                Ok(store) => store,
                                Err(error) => {
                                    nucleotide_logging::warn!(
                                        error = %error,
                                        "Failed to load remote connection history for prompt completions"
                                    );
                                    crate::remote_connections::RemoteConnectionStore::default()
                                }
                            };
                        view = view.with_completion_fn(move |input| {
                            crate::remote_connections::completions_for_input(
                                input,
                                &connection_store,
                            )
                            .into_iter()
                            .map(|completion| nucleotide_ui::prompt_view::CompletionItem {
                                text: completion.insert_text.into(),
                                description: Some(completion.description.into()),
                                display_text: Some(completion.display_text.into()),
                            })
                            .collect()
                        });
                    }

                    // Set up the submit callback with command/search execution
                    let core_weak_submit = self.core.clone();
                    view = view.on_submit(move |input: &str, cx| {
                        use nucleotide_logging::debug;
                        debug!(input = %input, input_len = input.len(), "Overlay on_submit received input");
                        // Emit appropriate event based on prompt type
                        if let Some(core) = core_weak_submit.upgrade() {
                            core.update(cx, |_core, cx| {
                                match submit_action {
                                    PromptSubmitAction::Search => {
                                        cx.emit(crate::Update::SearchSubmitted(input.to_string()));
                                    }
                                    PromptSubmitAction::GlobalSearch => {
                                        cx.emit(crate::Update::GlobalSearchSubmitted(
                                            input.to_string(),
                                        ));
                                    }
                                    PromptSubmitAction::FileTreeSearch => {
                                        cx.emit(crate::Update::FileTreeSearchSubmitted(
                                            input.to_string(),
                                        ));
                                    }
                                    PromptSubmitAction::RemoteOpen => {
                                        cx.emit(crate::Update::OpenRemote(input.to_string()));
                                    }
                                    PromptSubmitAction::RegexSelection(action) => {
                                        cx.emit(crate::Update::RegexSelectionSubmitted {
                                            action,
                                            regex: input.to_string(),
                                        });
                                    }
                                    PromptSubmitAction::Command => {
                                        debug!(command = %input, "Emitting CommandSubmitted event");
                                        cx.emit(crate::Update::CommandSubmitted(input.to_string()));
                                    }
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
                    |this, prompt_view, _event: &DismissEvent, cx| {
                        if this.native_prompt_view.as_ref() == Some(&prompt_view) {
                            this.dismiss_prompt(cx);
                        }
                    },
                )
                .detach();

                // Focus will be handled by the prompt view's render method

                self.native_prompt_view = Some(prompt_view);

                cx.notify();
            }
            crate::Update::RemoteConnectionManager => {
                nucleotide_logging::trace!("Overlay Update::RemoteConnectionManager received");
                self.replace_prompt(cx);
                self.replace_picker(cx);
                self.replace_remote_connection_manager(cx);

                let Some(core) = self.core.upgrade() else {
                    nucleotide_logging::warn!(
                        "Cannot open remote connection manager after application shutdown"
                    );
                    return;
                };
                let backend_options = core.read(cx).config.remote_workspace_backend_options();
                let bootstrap = nucleotide_remote::RemoteWorkspaceBootstrap::new(backend_options);

                let manager_view = cx.new(|cx| {
                    crate::remote_connection_manager::RemoteConnectionManagerView::new(
                        self.core.clone(),
                        self.handle.clone(),
                        bootstrap,
                        cx,
                    )
                });

                cx.subscribe(
                    &manager_view,
                    |this, manager_view, _event: &DismissEvent, cx| {
                        if this.remote_connection_manager_view.as_ref() == Some(&manager_view) {
                            this.dismiss_remote_connection_manager(cx);
                        }
                    },
                )
                .detach();

                self.remote_connection_manager_view = Some(manager_view);
                cx.notify();
            }
            crate::Update::Completion(completion_view) => {
                nucleotide_logging::trace!("DIAG: Overlay Update::Completion received");
                nucleotide_logging::debug!(
                    "🎨 OVERLAY RECEIVED COMPLETION VIEW: Setting up completion overlay"
                );

                self.replace_completion(cx);
                // Set up completion view with event subscription
                self.completion_view = Some(completion_view.clone());

                // Subscribe to dismiss events from completion view
                cx.subscribe(
                    completion_view,
                    |this, completion_view, _event: &DismissEvent, cx| {
                        if this.completion_view.as_ref() == Some(&completion_view) {
                            this.dismiss_completion(cx);
                        }
                    },
                )
                .detach();

                // Subscribe to the new completion acceptance event
                cx.subscribe(
                    completion_view,
                    |_this, _completion_view, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                        nucleotide_logging::debug!(
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
                        nucleotide_logging::trace!("Completion acceptance forwarded - workspace will dismiss after processing");
                    },
                )
                .detach();

                cx.subscribe(
                    completion_view,
                    |_this, _completion_view, event: &nucleotide_ui::CompletionWarningEvent, cx| {
                        cx.emit(nucleotide_ui::CompletionWarningEvent {
                            message: event.message.clone(),
                        });
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
                nucleotide_logging::trace!("DIAG: Overlay Update::Picker received");
                // Clean up any existing picker before creating a new one
                self.replace_picker(cx);
                self.replace_remote_connection_manager(cx);

                match picker {
                    Picker::Native {
                        title,
                        items,
                        on_select,
                        show_preview,
                        preview_text_provider,
                        preview_text_task_provider,
                    } => {
                        let is_file_finder = title.as_ref() == "Open File";
                        let items = items.clone();
                        let on_select = on_select.clone();
                        let preview_text_provider = preview_text_provider.clone();
                        let preview_text_task_provider = preview_text_task_provider.clone();
                        let core_weak = self.core.clone();
                        let _items_count = items.len();

                        let picker_view = cx.new(|cx| {
                            // Use testing-aware theme constructor
                            let mut view = Self::create_picker_view_with_context(cx);
                            let items_for_callback = items.clone();

                            view = view.with_preview(*show_preview);

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
                                                dyn nucleotide_core::PickerCapability
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
                            let preview_settings_core = core_weak.clone();

                            view = view.with_preview_open_fn(move |path, picker_cx| {
                                let should_open_preview_tab =
                                    preview_settings_core.upgrade().is_some_and(|core| {
                                        let core = core.read(picker_cx);
                                        let settings = &core.config.gui.preview_tabs;
                                        settings.enabled
                                            && (!is_file_finder
                                                || settings.enable_preview_from_file_finder)
                                    });

                                if !should_open_preview_tab {
                                    return None;
                                }

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
                                                font_size: gpui::px((editor_font.size * 0.9).max(8.0)).into(),
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
                                            crate::document::DocumentView::new(core.clone(), None, view_id, default_style, &focus, false)
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
                                    .unwrap_or(cx.theme().tokens.chrome.text_on_chrome);

                                // Use the editor fixed font and editor font size for code preview
                                let editor_cfg = cx.global::<nucleotide_types::EditorFontConfig>().clone();
                                let editor_font: gpui::Font = nucleotide_types::Font {
                                    family: editor_cfg.family.clone(),
                                    weight: editor_cfg.weight,
                                    style: nucleotide_types::FontStyle::Normal,
                                }
                                .into();
                                let editor_size = (editor_cfg.size * 0.9).max(8.0);

                                let mut container = div()
                                    .px_3()
                                    .py_2()
                                    .font(editor_font)
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
                                                .unwrap_or(cx.theme().tokens.chrome.text_on_chrome);

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
                            let picker_preview_text_provider = preview_text_provider.clone();
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
                                } else if let Some(doc_id) =
                                    item.data.downcast_ref::<helix_view::DocumentId>()
                                    && let Some(core) = text_core.upgrade()
                                {
                                    let core_read = core.read(cx);
                                    if let Some(doc) = core_read.editor.documents.get(doc_id) {
                                        let content = String::from(doc.text().slice(..));
                                        let path_opt = doc.path().map(std::path::Path::to_path_buf);
                                        return Some((content, path_opt));
                                    }
                                } else if let Some(task) = item
                                    .data
                                    .downcast_ref::<nucleotide_events::v2::run::ResolvedTask>()
                                {
                                    return Some((
                                        crate::runnables::task_preview_text(task),
                                        task.source().map(|source| source.path.clone()),
                                    ));
                                }

                                if let Some(provider) = picker_preview_text_provider.as_ref() {
                                    return provider(item, cx);
                                }
                                None
                            });

                            if let Some(provider) = preview_text_task_provider {
                                view = view.with_preview_text_task_provider_fn(move |item, cx| {
                                    provider(item, cx)
                                });
                            }

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
                                if let Some((action, ls_id, _offset)) = selected_item
                                    .data
                                    .downcast_ref::<(
                                        helix_lsp::lsp::CodeActionOrCommand,
                                        helix_core::diagnostic::LanguageServerId,
                                        helix_lsp::OffsetEncoding,
                                    )>()
                                    && let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            let changed_documents = apply_code_action_or_command(
                                                core,
                                                action,
                                                *ls_id,
                                            );
                                            if !changed_documents.is_empty() {
                                                // Document::apply dispatches the precise content
                                                // change through the Helix event bridge.
                                                core_cx.notify();
                                            }
                                        });
                                    }
                                // Check if it's a buffer picker item (DocumentId, Option<PathBuf>)
                                if let Some(location) =
                                    selected_item.data.downcast_ref::<crate::types::LspLocation>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            match core.jump_to_lsp_location(location) {
                                                Ok((doc_id, view_id)) => {
                                                    core_cx.emit(
                                                        crate::Update::SelectionChanged {
                                                            doc_id,
                                                            view_id,
                                                        },
                                                    );
                                                    core_cx.emit(crate::Update::Redraw);
                                                }
                                                Err(err) => core.editor.set_error(err.to_string()),
                                            }
                                        });
                                    }
                                }
                                else if let Some(location) = selected_item
                                    .data
                                    .downcast_ref::<crate::types::JumpLocation>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            match core.jump_to_jumplist_location(location) {
                                                Ok((doc_id, view_id)) => {
                                                    core_cx.emit(
                                                        crate::Update::SelectionChanged {
                                                            doc_id,
                                                            view_id,
                                                        },
                                                    );
                                                    core_cx.emit(crate::Update::Redraw);
                                                }
                                                Err(err) => core.editor.set_error(err.to_string()),
                                            }
                                        });
                                    }
                                }
                                else if let Some(location) = selected_item
                                    .data
                                    .downcast_ref::<crate::types::DiagnosticLocation>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            match core.jump_to_diagnostic_location(location) {
                                                Ok((doc_id, view_id)) => {
                                                    core_cx.emit(
                                                        crate::Update::SelectionChanged {
                                                            doc_id,
                                                            view_id,
                                                        },
                                                    );
                                                    core_cx.emit(crate::Update::Redraw);
                                                }
                                                Err(err) => core.editor.set_error(err.to_string()),
                                            }
                                        });
                                    }
                                }
                                else if let Some(location) = selected_item
                                    .data
                                    .downcast_ref::<crate::types::SyntaxFileLocation>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            match core.jump_to_syntax_file_location(location) {
                                                Ok((doc_id, view_id)) => {
                                                    core_cx.emit(
                                                        crate::Update::SelectionChanged {
                                                            doc_id,
                                                            view_id,
                                                        },
                                                    );
                                                    core_cx.emit(crate::Update::Redraw);
                                                }
                                                Err(err) => core.editor.set_error(err.to_string()),
                                            }
                                        });
                                    }
                                }
                                else if let Some(location) = selected_item
                                    .data
                                    .downcast_ref::<crate::types::GlobalSearchLocation>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        core.update(picker_cx, |core, core_cx| {
                                            match core.jump_to_global_search_location(location) {
                                                Ok((doc_id, view_id)) => {
                                                    core_cx.emit(
                                                        crate::Update::SelectionChanged {
                                                            doc_id,
                                                            view_id,
                                                        },
                                                    );
                                                    core_cx.emit(crate::Update::Redraw);
                                                }
                                                Err(err) => core.editor.set_error(err.to_string()),
                                            }
                                        });
                                    }
                                }
                                // Check if it's a buffer picker item (DocumentId, Option<PathBuf>)
                                else if let Some((doc_id, _path)) = selected_item.data.downcast_ref::<(
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
                                else if let Some(task) = selected_item
                                    .data
                                    .downcast_ref::<nucleotide_events::v2::run::ResolvedTask>()
                                {
                                    if let Some(core) = core_for_on_select.upgrade() {
                                        let task = task.clone();
                                        core.update(picker_cx, |_core, core_cx| {
                                            core_cx.emit(crate::Update::RunTask(task));
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
                                if this.native_picker_view.as_ref() == Some(&picker_view) {
                                    this.dismiss_picker(cx);
                                }
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
                        prompt: Default::default(),
                    };

                    // Open the native directory picker
                    let receiver = cx.update(|cx| cx.prompt_for_paths(options));

                    // Wait for the user to select a directory
                    if let Ok(Ok(Some(paths))) = receiver.await {
                        if let Some(path) = paths.first() {
                            // Emit the selected directory through the core entity
                            if let Some(core) = core_weak.upgrade() {
                                cx.update(|cx| {
                                    core.update(cx, |_core, cx| {
                                        cx.emit(crate::Update::OpenDirectory(path.clone()));
                                    });
                                });
                            }
                            // Dismiss the overlay
                            cx.update(|cx| {
                                let _ = this.update(cx, |_this, cx| {
                                    cx.emit(DismissEvent);
                                });
                            });
                        }
                    } else {
                        // User cancelled - just dismiss
                        cx.update(|cx| {
                            let _ = this.update(cx, |_this, cx| {
                                cx.emit(DismissEvent);
                            });
                        });
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
        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        nucleotide_ui::prompt_view::PromptStyle {
            modal_style,
            completion_background: tokens.dropdown_tokens().container_background,
        }
    }

    /// Create ModalStyle using ThemedContext with design token fallbacks
    fn create_modal_style_from_context(cx: &App) -> nucleotide_ui::common::ModalStyle {
        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use component/chrome tokens directly
        let background = tokens.picker_tokens().container_background;
        let text = tokens.chrome.text_on_chrome;
        let selected_background = tokens.dropdown_tokens().item_background_selected;
        let selected_text = tokens.dropdown_tokens().item_text_selected;
        let border = tokens.picker_tokens().border;
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
                        let cursor_x = layout_info.content_inset_x
                            + layout_info.file_tree_width
                            + layout_info.gutter_width
                            + layout_info.char_width * (cursor_pos.col as f32);

                        // Account for UI layout: title bar + tab bar + line position within document area
                        let document_area_y = layout_info.title_bar_height
                            + layout_info.content_inset_y
                            + layout_info.tab_bar_height;
                        let cursor_y =
                            document_area_y + layout_info.line_height * (relative_row as f32);

                        return (cursor_x, cursor_y + layout_info.line_height); // Position below cursor
                    }
                }
            }
        }

        // Final fallback positioning
        let fallback_x = layout_info.content_inset_x
            + layout_info.file_tree_width
            + layout_info.gutter_width
            + px(10.0);
        let fallback_y = layout_info.title_bar_height
            + layout_info.content_inset_y
            + layout_info.tab_bar_height
            + px(20.0);
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

        // Fall back to defaults that match the normal Nucleotide layout.
        // These values stay approximate when the user has resized panes.
        WorkspaceLayoutInfo {
            file_tree_width: px(250.0), // Default file tree width (user can resize)
            content_inset_x: px(0.0),   // Full-bleed fallback for unknown workspace chrome
            content_inset_y: px(0.0),
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

/// Layout information for positioning UI elements relative to workspace
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceLayoutInfo {
    pub file_tree_width: gpui::Pixels,
    pub content_inset_x: gpui::Pixels,
    pub content_inset_y: gpui::Pixels,
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
    #[inline]
    fn terminal_metrics(&mut self, cx: &mut Context<Self>) -> (f32, f32) {
        // Build a cache key from the current editor font settings
        let editor_font = cx.global::<nucleotide_types::EditorFontConfig>();
        let font_key = (
            editor_font.family.clone(),
            editor_font.size,
            editor_font.weight,
        );

        let need_recalc = match &self.cached_font_key {
            Some(k) => k != &font_key,
            None => true,
        };

        if need_recalc {
            // Resolve font and measure advance for 'm'
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
                .map(|a| f32::from(a.width))
                .unwrap_or(editor_font.size * 0.6)
                .max(1.0);
            // Prefer configured line_height when available
            let line_h = if editor_font.line_height > 0.0 {
                editor_font.line_height
            } else {
                (editor_font.size * 1.35).max(1.0)
            };

            self.cached_font_key = Some(font_key);
            self.cached_char_width = Some(char_w);
            self.cached_line_height = Some(line_h);
        }

        (
            self.cached_char_width.unwrap_or(8.0),
            self.cached_line_height
                .unwrap_or((editor_font.size * 1.35).max(1.0)),
        )
    }
    /// Create PickerView using ThemedContext with design token fallbacks
    fn create_picker_view_with_context(cx: &mut gpui::Context<PickerView>) -> PickerView {
        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use chrome/editor tokens directly
        let preview_background = tokens.editor.background;
        let preview_text = tokens.editor.text_primary;
        let cursor = tokens.editor.cursor_normal;

        let picker_style = nucleotide_ui::picker_view::PickerStyle {
            modal_style,
            preview_background,
            preview_text,
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
            nucleotide_logging::debug!("DIAG: Render overlay branch: picker");
            picker_view.focus_handle(cx)
        } else if let Some(prompt_view) = &self.native_prompt_view {
            prompt_view.focus_handle(cx)
        } else if let Some(remote_manager) = &self.remote_connection_manager_view {
            remote_manager.focus_handle(cx)
        } else {
            // Don't delegate to completion_view - let editor keep focus
            self.focus.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for OverlayView {}
impl EventEmitter<nucleotide_ui::completion_v2::CompleteViaHelixEvent> for OverlayView {}
impl EventEmitter<nucleotide_ui::completion_v2::CompletionWarningEvent> for OverlayView {}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check what type of overlay we should render
        if let Some(picker_view) = &self.native_picker_view {
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return OverlaySurface::new()
                .top(tokens.sizes.space_8)
                .on_light_dismiss(cx.listener(|this: &mut OverlayView, _e, window, cx| {
                    window.disable_focus();
                    this.dismiss_picker(cx);
                }))
                .on_cancel(cx.listener(|this: &mut OverlayView, _action, window, cx| {
                    window.disable_focus();
                    this.dismiss_picker(cx);
                }))
                .child(picker_view.clone())
                .into_any_element();
        }

        if let Some(prompt_view) = &self.native_prompt_view {
            nucleotide_logging::trace!("DIAG: Render overlay branch: prompt");
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return OverlaySurface::new()
                .top(tokens.sizes.space_8)
                .on_light_dismiss(cx.listener(|this: &mut OverlayView, _e, window, cx| {
                    window.disable_focus();
                    this.dismiss_prompt(cx);
                }))
                .on_cancel(cx.listener(|this: &mut OverlayView, _action, window, cx| {
                    window.disable_focus();
                    this.dismiss_prompt(cx);
                }))
                .child(prompt_view.clone())
                .into_any_element();
        }

        if let Some(remote_manager) = &self.remote_connection_manager_view {
            nucleotide_logging::trace!("DIAG: Render overlay branch: remote connection manager");
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return OverlaySurface::new()
                .top(tokens.sizes.space_8)
                .on_light_dismiss(cx.listener(|this: &mut OverlayView, _e, window, cx| {
                    window.disable_focus();
                    this.dismiss_remote_connection_manager(cx);
                }))
                .on_cancel(cx.listener(|this: &mut OverlayView, _action, window, cx| {
                    window.disable_focus();
                    this.dismiss_remote_connection_manager(cx);
                }))
                .child(remote_manager.clone())
                .into_any_element();
        }

        if let Some(completion_view) = &self.completion_view {
            nucleotide_logging::trace!("DIAG: Render overlay branch: completion");
            use gpui::{Anchor, anchored, point};

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
                // Clicking outside the completion popup dismisses it and restores focus
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this: &mut OverlayView, _e, window, cx| {
                        window.disable_focus();
                        this.dismiss_completion(cx);
                    }),
                )
                .child(
                    anchored()
                        .position(point(cursor_x, cursor_y))
                        .anchor(Anchor::TopLeft) // Anchor top-left of completion to cursor position
                        .offset(point(px(0.0), px(2.0))) // Small offset below cursor
                        .snap_to_window_with_margin(px(8.0))
                        // Consume clicks inside the popup so they don't dismiss
                        .child(
                            div()
                                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                                .child(completion_view.clone()),
                        ),
                )
                .into_any_element();
        }

        // Terminal panel - docked at bottom
        if self.terminal_panel.is_some() {
            {
                // Ensure we have a persistent focus handle for the terminal panel area via coordinator
                let coordinator = cx
                    .try_global::<nucleotide_ui::FocusCoordinator>()
                    .cloned()
                    .unwrap_or_else(|| {
                        let c = nucleotide_ui::FocusCoordinator::default();
                        cx.set_global(c.clone());
                        c
                    });
                if coordinator.terminal_focus().is_none() {
                    coordinator.set_terminal_focus(cx.focus_handle());
                }
                let panel_focus = coordinator.terminal_focus().unwrap();
                let panel_focus_for_keys = panel_focus.clone();

                // Sync from window-level resize state
                if let Ok(st) = self.resize_state.lock()
                    && (st.height - self.terminal_height_px).abs() >= 0.5
                {
                    self.terminal_height_px = st.height;
                }

                // Constrain terminal height to avoid covering the entire editor
                let window_h = f32::from(_window.bounds().size.height);
                let max_h = (window_h * 0.6).max(120.0);
                let clamped_h = self.terminal_height_px.clamp(80.0, max_h);
                if (clamped_h - self.terminal_height_px).abs() > 0.5 {
                    self.terminal_height_px = clamped_h;
                    if let Ok(mut st) = self.resize_state.lock() {
                        st.height = clamped_h;
                    }
                }

                // Dynamically compute terminal cols/rows from current window
                // bounds and font metrics to keep the PTY/emulator sized to
                // the visible area. Snap the rendered panel height to whole
                // cells so the split and terminal surface cannot diverge.
                let layout = self.get_workspace_layout_info(cx);
                let window_width = f32::from(_window.bounds().size.width);
                let (char_w, line_h) = self.terminal_metrics(cx);
                let terminal_content_height = (self.terminal_height_px
                    - nucleotide_terminal_panel::TERMINAL_PANEL_HEADER_HEIGHT_PX)
                    .max(line_h);
                let file_tree_width = f32::from(layout.file_tree_width);
                let horizontal_inset = f32::from(layout.content_inset_x) * 2.0;
                let usable_width = (window_width - horizontal_inset - file_tree_width).max(1.0);
                let bounds = TerminalBounds::from_pixels(
                    char_w,
                    line_h,
                    usable_width,
                    terminal_content_height,
                );
                let snapped_panel_height =
                    nucleotide_terminal_panel::snapped_terminal_panel_height(
                        self.terminal_height_px,
                        line_h,
                    )
                    .clamp(80.0, max_h);

                if (snapped_panel_height - self.terminal_height_px).abs() > 0.5 {
                    self.terminal_height_px = snapped_panel_height;
                    if let Ok(mut st) = self.resize_state.lock() {
                        st.height = snapped_panel_height;
                    }
                }

                // Update the panel entity with the current height before
                // borrowing cx immutably.
                if let Some(panel) = &self.terminal_panel {
                    let h = self.terminal_height_px;
                    panel.update(cx, |p, cx| {
                        if (p.height_px - h).abs() > 0.5 {
                            p.height_px = h;
                            cx.notify();
                        }
                    });
                }

                // Read the active terminal id after metrics calculation to avoid borrow conflicts.
                let active_id = if let Some(panel) = &self.terminal_panel {
                    panel.read(cx).active
                } else {
                    // If panel disappeared mid-render, skip sizing.
                    return div().into_any_element();
                };
                let cols = bounds.cols();
                let rows = bounds.rows();
                let changed = !matches!(
                    self.last_terminal_size,
                    Some((id, last_c, last_r))
                        if id == active_id && last_c == cols && last_r == rows
                );
                if changed {
                    self.last_terminal_size = Some((active_id, cols, rows));
                    if let Some(core) = self.core.upgrade() {
                        core.update(cx, |app, _| {
                            app.terminal_runtime.dispatch(
                                &nucleotide_events::v2::terminal::Event::Resized {
                                    id: active_id,
                                    cols,
                                    rows,
                                    cell_width: char_w,
                                    cell_height: line_h,
                                },
                            );
                        });
                    }
                    // Notify the terminal view entity so it re-renders with
                    // the updated grid dimensions (new row/column count).
                    if let Some(panel) = &self.terminal_panel {
                        panel.update(cx, |p, cx| {
                            cx.notify();
                            if let Some(view) = &p.view_entity {
                                view.update(cx, |_, cx| cx.notify());
                            }
                        });
                    }
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
                            if event.keystroke.modifiers.secondary()
                                && event.keystroke.key.as_str() == "c"
                                && let Some(text) = this
                                    .terminal_panel
                                    .as_ref()
                                    .and_then(|panel| {
                                        nucleotide_terminal_view::get_view_model(
                                            panel.read(cx).active,
                                        )
                                    })
                                    .and_then(|model| {
                                        model
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .selected_text()
                                    })
                            {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                cx.stop_propagation();
                                return;
                            }
                            if let Some(core) = this.core.upgrade() {
                                let maybe_id =
                                    this.terminal_panel.as_ref().map(|p| p.read(cx).active);
                                if let Some(id) = maybe_id {
                                    let bytes =
                                        crate::terminal_input::encode_key_event_for_terminal(
                                            id, event,
                                        );
                                    if crate::terminal_input::send_terminal_input(
                                        &core, id, bytes, cx,
                                    ) {
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
                            let terminal_line_height_px = line_h;
                            move |new_h: f32, app_cx: &mut gpui::App| {
                                entity.update(app_cx, |this: &mut OverlayView, cx| {
                                    let snapped_h =
                                        nucleotide_terminal_panel::snapped_terminal_panel_height(
                                            new_h,
                                            terminal_line_height_px,
                                        )
                                        .clamp(80.0, max_h);
                                    this.terminal_height_px = snapped_h;
                                    if let Ok(mut st) = this.resize_state.lock() {
                                        st.height = snapped_h;
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
                                let mut c = div()
                                    .size_full()
                                    .overflow_hidden()
                                    .bg(cx.theme().tokens.editor.background)
                                    .track_focus(&panel_focus);
                                if debug {
                                    let theme = cx.global::<nucleotide_ui::Theme>();
                                    c = c
                                        .relative()
                                        .bg(theme.tokens.chrome.surface_elevated)
                                        .border_t_2()
                                        .border_color(theme.tokens.chrome.border_default);
                                }
                                // First, terminal content
                                if let Some(panel) = &self.terminal_panel {
                                    c = c.child(panel.clone());
                                }
                                // Then the debug label so it stays on top
                                if debug {
                                    let theme = cx.global::<nucleotide_ui::Theme>();
                                    c = c.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .bg(theme.tokens.chrome.primary)
                                            .text_color(theme.tokens.chrome.text_on_chrome)
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

        // Empty overlay using design tokens
        nucleotide_logging::debug!("DIAG: Render overlay branch: none");
        div().size_0().into_any_element()
    }
}
