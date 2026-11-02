// ABOUTME: Native editor input boundary for GPUI-driven document views
// ABOUTME: Executes Helix keymaps without routing editor input through helix-term views

use helix_core::{
    Range, char_idx_at_visual_offset,
    movement::{Direction, Movement, move_vertically_visual},
    visual_offset_from_block,
};
use helix_stdx::{
    path::{self, find_paths},
    rope::RopeSliceExt,
};
use helix_term::{
    commands::{self, MappableCommand, OnKeyCallback, OnKeyCallbackKind},
    compositor::{self, Compositor},
    events::{OnModeSwitch, PostCommand},
    job::Jobs,
    keymap::{KeymapResult, Keymaps},
};
use helix_view::{
    DocumentId, Editor, ViewId,
    document::Mode,
    editor::Action,
    input::KeyEvent,
    keyboard::{KeyCode, KeyModifiers},
};
use nucleotide_logging::{PerfTimer, debug, info};
use std::{
    borrow::Cow,
    collections::{BTreeSet, VecDeque},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

#[path = "workspace_commands.rs"]
pub mod workspace_commands;
use workspace_commands::WorkspaceCommandRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorInputOutcome {
    pub focused_view_id: ViewId,
    pub focused_doc_id: Option<DocumentId>,
    pub selection_changed: bool,
    pub handled_by_native_command: bool,
    pub reset_diff_change_executed: bool,
    pub unhandled_keys: Vec<KeyEvent>,
    pub completion_requested: Option<NativeCompletionRequest>,
    pub picker_requested: Option<NativePickerRequest>,
    pub prompt_requested: Option<NativePromptRequest>,
    pub lsp_navigation_requested: Option<NativeLspNavigationRequest>,
    pub workspace_requested: Option<NativeWorkspaceRequest>,
    pub semantic_shortcut_requested: Option<crate::types::SemanticShortcutIntent>,
    pub viewport_scroll_requested: Option<nucleotide_editor::EditorViewportScrollRequest>,
    pub viewport_cursor_requested: Option<nucleotide_editor::EditorViewportCursorRequest>,
    pub continuation: Option<EditorContinuation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorCommandToken(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorContinuationKind {
    Workspace,
    Viewport,
    LspNavigation,
    Ui,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorContinuation {
    pub token: EditorCommandToken,
    pub kind: EditorContinuationKind,
}

struct PendingContinuation {
    continuation: EditorContinuation,
    slot: Option<ViewId>,
    document: Option<DocumentId>,
    focus_generation: u64,
    claimed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeCompletionRequest {
    pub doc_id: DocumentId,
    pub view_id: ViewId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLspNavigationRequest {
    GotoDeclaration,
    GotoDefinition,
    GotoTypeDefinition,
    GotoImplementation,
    GotoReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeWorkspaceRequest {
    ToggleFileTree,
    Command(WorkspaceCommandRequest),
    OpenFiles {
        paths: Vec<PathBuf>,
        action: NativeFileOpenAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileOpenAction {
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

impl From<NativeFileOpenAction> for Action {
    fn from(action: NativeFileOpenAction) -> Self {
        match action {
            NativeFileOpenAction::Replace => Action::Replace,
            NativeFileOpenAction::HorizontalSplit => Action::HorizontalSplit,
            NativeFileOpenAction::VerticalSplit => Action::VerticalSplit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePromptRequest {
    Command,
    Search,
    ReverseSearch,
    GlobalSearch,
    RegexSelection(crate::types::RegexSelectionAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativePickerRequest {
    File,
    FileAt(PathBuf),
    FileCurrentDirectory,
    FileCurrentBufferDirectory,
    Buffer,
    JumpList,
    Symbols { workspace: bool },
    Diagnostics { workspace: bool },
    CodeActions,
    HoverDocs,
}

pub struct EditorInputBridge {
    native_commands: NativeCommandInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSemanticAction {
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Find,
    FindNext,
    FindPrevious,
    NextBuffer,
    PreviousBuffer,
    ExpandSelection,
    ShrinkSelection,
    SelectNextOccurrence,
    SelectPreviousOccurrence,
    SplitSelectionOnNewline,
    MergeSelections,
    NextDiagnostic,
    PreviousDiagnostic,
}

impl EditorSemanticAction {
    fn commands(self) -> &'static [&'static MappableCommand] {
        match self {
            Self::Undo => &[&MappableCommand::undo],
            Self::Redo => &[&MappableCommand::redo],
            Self::Copy => &[&MappableCommand::yank_to_clipboard],
            Self::Cut => &[
                &MappableCommand::yank_to_clipboard,
                &MappableCommand::delete_selection,
            ],
            Self::SelectAll => &[&MappableCommand::select_all],
            Self::FindNext => &[&MappableCommand::search_next],
            Self::FindPrevious => &[&MappableCommand::search_prev],
            Self::NextBuffer => &[&MappableCommand::goto_next_buffer],
            Self::PreviousBuffer => &[&MappableCommand::goto_previous_buffer],
            Self::ExpandSelection => &[&MappableCommand::expand_selection],
            Self::ShrinkSelection => &[&MappableCommand::shrink_selection],
            Self::SelectNextOccurrence => &[
                &MappableCommand::search_selection,
                &MappableCommand::extend_search_next,
            ],
            Self::SelectPreviousOccurrence => &[
                &MappableCommand::search_selection,
                &MappableCommand::extend_search_prev,
            ],
            Self::SplitSelectionOnNewline => &[&MappableCommand::split_selection_on_newline],
            Self::MergeSelections => &[&MappableCommand::merge_selections],
            Self::NextDiagnostic => &[&MappableCommand::goto_next_diag],
            Self::PreviousDiagnostic => &[&MappableCommand::goto_prev_diag],
            Self::Paste | Self::Find => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackShortcut {
    Workspace(crate::types::SemanticShortcutIntent),
    Editor(EditorSemanticAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPlatform {
    MacOS,
    Windows,
    Linux,
}

impl TargetPlatform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOS
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionSnapshot {
    cursor: usize,
    line: usize,
}

impl EditorInputBridge {
    pub fn new(native_keymaps: Keymaps) -> Self {
        Self {
            native_commands: NativeCommandInput::new_for_platform(
                native_keymaps,
                TargetPlatform::current(),
            ),
        }
    }

    /// Opt-in only. Standalone Helix keymaps retain their existing behaviour.
    pub fn enable_hosted_commands(&mut self) {
        self.native_commands.hosted = true;
    }

    /// Keep this in sync with every host focus/tab change, including clicks and
    /// menu actions, so IME and semantic editing cannot target a hidden slot.
    pub fn set_hosted_active_slot(&mut self, active_slot: Option<ViewId>) {
        let document = (self.native_commands.active_slot == active_slot)
            .then_some(self.native_commands.active_document)
            .flatten();
        self.set_hosted_active_context(active_slot, document);
    }

    /// Project both identities on every tab activation, even when ViewId is
    /// reused. Recording each transition rejects an unclaimed A -> B -> A result.
    /// A claimed operation can still project its own result before resuming.
    pub fn set_hosted_active_context(
        &mut self,
        active_slot: Option<ViewId>,
        active_document: Option<DocumentId>,
    ) {
        self.native_commands
            .set_active_context(active_slot, active_document);
    }

    /// Cancel the suspended mapping after a failed/rejected host operation.
    pub fn cancel_workspace_commands(&mut self, editor: &mut Editor) {
        self.native_commands.cancel_pending(editor);
    }

    /// A submitted native prompt may dismiss after yielding a host operation.
    /// Its dismissal must not acknowledge that operation or resume its tail.
    pub fn awaiting_workspace_command(&self) -> bool {
        self.native_commands.awaiting_host
    }

    /// Check an outstanding async operation without claiming it (for retries or
    /// polling). Completion still needs begin_command_completion before effects.
    pub fn command_continuation_is_current(
        &self,
        token: EditorCommandToken,
        editor: &Editor,
    ) -> bool {
        self.native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| {
                pending.continuation.token == token
                    && !pending.claimed
                    && pending.focus_generation == self.native_commands.focus_generation
                    && pending.slot == self.native_commands.active_slot
                    && pending.document
                        == self
                            .native_commands
                            .active_slot
                            .and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc))
                    && (pending.continuation.kind == EditorContinuationKind::Workspace
                        || pending.slot.is_none()
                        || self.native_commands.has_active_slot(editor))
            })
    }

    /// Claim the captured operation before applying its result or hosted facts.
    /// Focus/document changes invalidate unclaimed work; stale callbacks cannot
    /// claim a newer operation. Apply/project and finish in the same update.
    pub fn begin_command_completion(
        &mut self,
        token: EditorCommandToken,
        editor: &mut Editor,
    ) -> bool {
        let Some(pending) = &self.native_commands.continuation else {
            return false;
        };
        if pending.continuation.token != token || pending.claimed {
            return false;
        }
        if !self.command_continuation_is_current(token, editor) {
            self.cancel_workspace_commands(editor);
            return false;
        }
        self.native_commands.continuation.as_mut().unwrap().claimed = true;
        self.native_commands.awaiting_host = true;
        true
    }

    /// Complete only an operation claimed before its result was applied.
    pub fn resume_command_continuation(
        &mut self,
        token: EditorCommandToken,
        active_slot: Option<ViewId>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> Option<EditorInputOutcome> {
        if !self
            .native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.continuation.token == token && pending.claimed)
        {
            return None;
        }
        self.native_commands.continuation = None;
        self.set_hosted_active_slot(active_slot);
        self.native_commands.awaiting_host = false;
        Some(self.handle_input(None, compositor, editor, jobs))
    }

    pub fn cancel_command_continuation(
        &mut self,
        token: EditorCommandToken,
        editor: &mut Editor,
    ) -> bool {
        if !self
            .native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.continuation.token == token)
        {
            return false;
        }
        self.cancel_workspace_commands(editor);
        true
    }

    /// Transfer a claimed host completion to native UI without running its tail.
    /// The new token prevents late callbacks from completing the previous stage.
    pub fn acknowledge_command_ui(
        &mut self,
        token: EditorCommandToken,
        active_slot: Option<ViewId>,
        editor: &Editor,
    ) -> Option<EditorContinuation> {
        if !self
            .native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.continuation.token == token && pending.claimed)
        {
            return None;
        }
        self.set_hosted_active_slot(active_slot);
        Some(
            self.native_commands
                .suspend(EditorContinuationKind::Ui, editor),
        )
    }

    /// Submit only the command prompt that owns this continuation, placing the
    /// newly parsed typable command ahead of its saved editing/macro tail.
    pub fn submit_command_continuation(
        &mut self,
        token: EditorCommandToken,
        command: &str,
        active_slot: Option<ViewId>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> Option<EditorInputOutcome> {
        if !self
            .native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| pending.continuation.kind == EditorContinuationKind::Ui)
            || !self.begin_command_completion(token, editor)
        {
            return None;
        }
        self.native_commands.continuation = None;
        self.native_commands.awaiting_host = false;
        Some(self.execute_hosted_command_line(command, active_slot, compositor, editor, jobs))
    }

    /// A host operation opened native UI instead of completing synchronously.
    /// Project its slot first, then acknowledge without running the saved tail.
    /// The UI consumes replay keys until selection, submission or cancellation;
    /// only successful completion resumes document commands.
    pub fn acknowledge_workspace_ui(&mut self, active_slot: Option<ViewId>) {
        self.set_hosted_active_slot(active_slot);
        self.native_commands.awaiting_host = false;
        self.native_commands.next_token += 1;
        if let Some(pending) = &mut self.native_commands.continuation {
            pending.continuation = EditorContinuation {
                token: EditorCommandToken(self.native_commands.next_token),
                kind: EditorContinuationKind::Ui,
            };
            if pending.slot != active_slot {
                pending.document = None;
            }
            pending.slot = active_slot;
            pending.focus_generation = self.native_commands.focus_generation;
            pending.claimed = false;
        }
    }

    /// While native prompt/picker UI is open, feed one parsed macro key through
    /// that UI's existing input handler. Stop when it closes or yields a host
    /// operation; never drain the rest into the document. No keys can be taken
    /// while a structural host operation is awaiting completion.
    pub fn take_suspended_replay_key(&mut self) -> Option<KeyEvent> {
        if !self.native_commands.hosted || self.native_commands.awaiting_host {
            return None;
        }
        let PendingCommand::Replay {
            replay,
            index,
            remaining,
        } = self.native_commands.pending_commands.front_mut()?
        else {
            return None;
        };
        if replay.keys.is_empty() || *remaining == 0 {
            return None;
        }
        let key = replay.keys[*index];
        *index += 1;
        if *index == replay.keys.len() {
            *index = 0;
            *remaining -= 1;
        }
        Some(key)
    }

    /// Host projects focus first. None means terminal/image content, even when
    /// a hidden real editing slot still exists in the Helix tree.
    pub fn resume_workspace_commands(
        &mut self,
        active_slot: Option<ViewId>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        // Compatibility for synchronous workspace/UI callers only. Async and
        // viewport callbacks must carry the token captured with their request.
        if self
            .native_commands
            .continuation
            .as_ref()
            .is_some_and(|pending| {
                !matches!(
                    pending.continuation.kind,
                    EditorContinuationKind::Workspace | EditorContinuationKind::Ui
                )
            })
        {
            return self.handle_input(None, compositor, editor, jobs);
        }
        self.native_commands.continuation = None;
        self.set_hosted_active_slot(active_slot);
        self.native_commands.awaiting_host = false;
        self.handle_input(None, compositor, editor, jobs)
    }

    pub fn handle_hosted_key(
        &mut self,
        key: KeyEvent,
        active_slot: Option<ViewId>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        self.set_hosted_active_slot(active_slot);
        self.handle_input(Some(key), compositor, editor, jobs)
    }

    /// Command prompts and static-name fallback use the same ordered adapter.
    pub fn execute_hosted_command_line(
        &mut self,
        command: &str,
        active_slot: Option<ViewId>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        if !self.native_commands.hosted {
            crate::helix_command::execute_command_line(editor, jobs, command);
            return self.handle_input(None, compositor, editor, jobs);
        }
        self.set_hosted_active_context(
            active_slot,
            active_slot.and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc)),
        );
        // A fresh command cancels abandoned UI/async work. Matching prompt
        // submission has already claimed and removed its continuation instead.
        if !self.native_commands.prepare_new_input(editor) {
            editor.set_error("Workspace command is awaiting host completion");
            return self.handle_input(None, compositor, editor, jobs);
        }
        match crate::helix_command::command_lookup(command) {
            crate::helix_command::CommandLookup::Empty => {}
            crate::helix_command::CommandLookup::Unknown(name) => {
                self.cancel_workspace_commands(editor);
                editor.set_error(format!("no such command: '{name}'"));
            }
            crate::helix_command::CommandLookup::Found(command) => {
                self.native_commands
                    .pending_commands
                    .push_front(PendingCommand::Command {
                        command,
                        count: None,
                        register: None,
                        from_command_line: true,
                    });
            }
        }
        self.resume_workspace_commands(active_slot, compositor, editor, jobs)
    }

    /// Only unbound insert-mode text goes to the platform's committed-text path.
    /// Pending keymaps and callbacks still belong to Helix, even for printable keys.
    pub fn uses_native_text(&self, editor: &Editor, mut key: KeyEvent) -> bool {
        canonicalize_key(&mut key);
        (!self.native_commands.hosted || self.native_commands.has_active_slot(editor))
            && !self.native_commands.awaiting_host
            && editor.mode() == Mode::Insert
            && key.char().is_some()
            && self.native_commands.on_next_key.is_none()
            && self.native_commands.keymaps.pending().is_empty()
            && self.native_commands.keymaps.sticky().is_none()
            && !self.native_commands.keymaps.contains_key(Mode::Insert, key)
    }

    /// Committed text is text, not a sequence of keymap commands. Use Helix's
    /// insertion primitive so auto-pairs, multi-cursors and completion hooks remain intact.
    pub fn insert_text(
        &mut self,
        text: &str,
        replacement: Option<std::ops::Range<usize>>,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) {
        if self.native_commands.hosted
            && (!self.native_commands.prepare_new_input(editor)
                || !self.native_commands.has_active_slot(editor))
        {
            editor
                .set_error("Text input requires a focused document with no pending host operation");
            return;
        }
        self.native_commands
            .insert_text(text, replacement, editor, jobs);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        self.handle_input(Some(key), compositor, editor, jobs)
    }

    fn handle_input(
        &mut self,
        key: Option<KeyEvent>,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        let _timer = PerfTimer::new("EditorInputBridge::handle_key")
            .with_warn_threshold(Duration::from_millis(4));
        let slot = self.native_commands.active_slot;
        self.set_hosted_active_context(
            slot,
            slot.and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc)),
        );
        if let Some(key) = key
            && (!self.native_commands.hosted || self.native_commands.has_active_slot(editor))
        {
            log_completion_key_context(editor, key);
        }

        let before_focused_view_id = editor.tree.focus;
        let before_focused_doc_id = (!self.native_commands.hosted
            || self.native_commands.has_active_slot(editor))
        .then(|| {
            editor
                .tree
                .try_get(before_focused_view_id)
                .map(|view| view.doc)
        })
        .flatten();
        let before_selection = before_focused_doc_id
            .and_then(|doc_id| selection_snapshot(editor, doc_id, before_focused_view_id));

        if let Some(snapshot) = before_selection {
            debug!(
                cursor_pos = snapshot.cursor,
                line = snapshot.line,
                "Before key"
            );
        }

        let mut handled_by_native_command = false;
        let mut unhandled_keys = Vec::new();
        let mut completion_requested = None;
        let mut picker_requested = None;
        let mut prompt_requested = None;
        let mut lsp_navigation_requested = None;
        let mut workspace_requested = None;
        let mut semantic_shortcut_requested = None;
        let mut viewport_scroll_requested = None;
        let mut viewport_cursor_requested = None;
        let native_input_result = match key {
            Some(key) => self
                .native_commands
                .handle_key(key, compositor, editor, jobs),
            None => self
                .native_commands
                .resume_commands(compositor, editor, jobs),
        };
        let reset_diff_change_executed = self.native_commands.take_reset_diff_change_executed();

        match native_input_result {
            NativeInputResult::Handled {
                completion_requested: request,
                picker_requested: picker_request,
                prompt_requested: prompt_request,
            } => {
                handled_by_native_command = true;
                completion_requested = request;
                picker_requested = picker_request;
                prompt_requested = prompt_request;
            }
            NativeInputResult::RequestLspNavigation(request) => {
                handled_by_native_command = true;
                lsp_navigation_requested = Some(request);
            }
            NativeInputResult::RequestWorkspace(request) => {
                handled_by_native_command = true;
                workspace_requested = Some(request);
            }
            NativeInputResult::RequestSemanticShortcut(request) => {
                handled_by_native_command = true;
                semantic_shortcut_requested = Some(request);
            }
            NativeInputResult::RequestViewportScroll(request) => {
                handled_by_native_command = true;
                viewport_scroll_requested = Some(request);
            }
            NativeInputResult::RequestViewportCursor(request) => {
                handled_by_native_command = true;
                viewport_cursor_requested = Some(request);
            }
            NativeInputResult::Unhandled {
                keys,
                disposition: _,
            } => {
                debug!(?keys, "Native editor input was not handled");
                unhandled_keys = keys;
            }
        }

        let focused_view_id = editor.tree.focus;
        let focused_doc_id = (!self.native_commands.hosted
            || self.native_commands.has_active_slot(editor))
        .then(|| editor.tree.try_get(focused_view_id).map(|view| view.doc))
        .flatten();
        let after_selection =
            focused_doc_id.and_then(|doc_id| selection_snapshot(editor, doc_id, focused_view_id));

        if let Some(snapshot) = after_selection {
            debug!(
                cursor_pos = snapshot.cursor,
                line = snapshot.line,
                "After key"
            );
        }

        let focused_doc_changed = before_focused_doc_id != focused_doc_id;
        let selection_changed =
            focused_doc_changed || selection_changed(before_selection, after_selection);
        if selection_changed
            && let (Some(before), Some(after)) = (before_selection, after_selection)
        {
            debug!(
                before_pos = before.cursor,
                before_line = before.line,
                after_pos = after.cursor,
                after_line = after.line,
                "Cursor moved"
            );
        }

        EditorInputOutcome {
            focused_view_id,
            focused_doc_id,
            selection_changed,
            handled_by_native_command,
            reset_diff_change_executed,
            unhandled_keys,
            completion_requested,
            picker_requested,
            prompt_requested,
            lsp_navigation_requested,
            workspace_requested,
            semantic_shortcut_requested,
            viewport_scroll_requested,
            viewport_cursor_requested,
            continuation: self
                .native_commands
                .continuation
                .as_ref()
                .map(|pending| pending.continuation),
        }
    }

    pub fn handle_semantic_action(
        &mut self,
        action: EditorSemanticAction,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        if !self.native_commands.prepare_new_input(editor) {
            editor.set_error("Workspace command is awaiting host completion");
            return self.handle_input(None, compositor, editor, jobs);
        }
        if self.native_commands.hosted && !self.native_commands.has_active_slot(editor) {
            self.cancel_workspace_commands(editor);
            editor.set_error("Editor action requires a focused document");
            return self.handle_input(None, compositor, editor, jobs);
        }
        let before_view = editor.tree.focus;
        let before_doc = editor.tree.try_get(before_view).map(|view| view.doc);
        let before_selection =
            before_doc.and_then(|doc_id| selection_snapshot(editor, doc_id, before_view));
        let mut prompt_requested = None;

        if action == EditorSemanticAction::Find {
            prompt_requested = Some(NativePromptRequest::Search);
        } else {
            let callbacks = {
                let mut context = commands::Context {
                    editor,
                    count: None,
                    register: None,
                    callback: Vec::new(),
                    on_next_key_callback: None,
                    jobs,
                };
                let mut last_mode = context.editor.mode();
                if action == EditorSemanticAction::Paste {
                    self.native_commands.prepare_non_key_paste(&mut context);
                    execute_clipboard_paste(&mut context, &mut last_mode);
                    context.editor.count = None;
                } else {
                    for command in action.commands() {
                        execute_native_command(command, &mut context, &mut last_mode);
                    }
                }
                std::mem::take(&mut context.callback)
            };
            finalize_native_command(editor, jobs, compositor, callbacks);
        }

        let focused_view_id = editor.tree.focus;
        let focused_doc_id = editor.tree.try_get(focused_view_id).map(|view| view.doc);
        let after_selection =
            focused_doc_id.and_then(|doc_id| selection_snapshot(editor, doc_id, focused_view_id));
        EditorInputOutcome {
            focused_view_id,
            focused_doc_id,
            selection_changed: before_doc != focused_doc_id
                || selection_changed(before_selection, after_selection),
            handled_by_native_command: true,
            reset_diff_change_executed: false,
            unhandled_keys: Vec::new(),
            completion_requested: None,
            picker_requested: None,
            prompt_requested,
            lsp_navigation_requested: None,
            workspace_requested: None,
            semantic_shortcut_requested: None,
            viewport_scroll_requested: None,
            viewport_cursor_requested: None,
            continuation: self
                .native_commands
                .continuation
                .as_ref()
                .map(|pending| pending.continuation),
        }
    }
}

struct NativeCommandInput {
    keymaps: Keymaps,
    target_platform: TargetPlatform,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    current_insert_replay: InsertReplay,
    last_insert_replay: Option<InsertReplay>,
    reset_diff_change_executed: bool,
    hosted: bool,
    active_slot: Option<ViewId>,
    active_document: Option<DocumentId>,
    pending_commands: VecDeque<PendingCommand>,
    processing_queue: bool,
    awaiting_host: bool,
    continuation: Option<PendingContinuation>,
    next_token: u64,
    focus_generation: u64,
}

enum PendingCommand {
    Command {
        command: MappableCommand,
        count: Option<NonZeroUsize>,
        register: Option<char>,
        from_command_line: bool,
    },
    Replay {
        replay: InsertReplay,
        index: usize,
        remaining: usize,
    },
    EndMacro(char),
}

enum NativeInputResult {
    Handled {
        completion_requested: Option<NativeCompletionRequest>,
        picker_requested: Option<NativePickerRequest>,
        prompt_requested: Option<NativePromptRequest>,
    },
    RequestLspNavigation(NativeLspNavigationRequest),
    RequestWorkspace(NativeWorkspaceRequest),
    RequestSemanticShortcut(crate::types::SemanticShortcutIntent),
    RequestViewportScroll(nucleotide_editor::EditorViewportScrollRequest),
    RequestViewportCursor(nucleotide_editor::EditorViewportCursorRequest),
    Unhandled {
        keys: Vec<KeyEvent>,
        disposition: UnhandledDisposition,
    },
}

impl NativeInputResult {
    fn continuation_kind(&self) -> Option<EditorContinuationKind> {
        use EditorContinuationKind::*;
        match self {
            Self::Handled {
                completion_requested,
                picker_requested,
                prompt_requested,
            } if completion_requested.is_some()
                || picker_requested.is_some()
                || prompt_requested.is_some() =>
            {
                Some(Ui)
            }
            Self::RequestWorkspace(_) => Some(Workspace),
            Self::RequestLspNavigation(_) => Some(LspNavigation),
            Self::RequestViewportScroll(_) | Self::RequestViewportCursor(_) => Some(Viewport),
            Self::RequestSemanticShortcut(_) => Some(Semantic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnhandledDisposition {
    RootNotFound,
    ReservedUnsupported,
    Cancelled,
}

enum NativeCommandResult {
    RunQueued,
    Handled(Vec<compositor::Callback>),
    RequestCompletion {
        callbacks: Vec<compositor::Callback>,
        request: Option<NativeCompletionRequest>,
    },
    RequestPicker {
        callbacks: Vec<compositor::Callback>,
        request: NativePickerRequest,
    },
    RequestPrompt {
        callbacks: Vec<compositor::Callback>,
        request: NativePromptRequest,
    },
    RequestLspNavigation {
        callbacks: Vec<compositor::Callback>,
        request: NativeLspNavigationRequest,
    },
    RequestWorkspace {
        callbacks: Vec<compositor::Callback>,
        request: NativeWorkspaceRequest,
    },
    RequestSemanticShortcut(crate::types::SemanticShortcutIntent),
    RequestViewportScroll {
        callbacks: Vec<compositor::Callback>,
        request: nucleotide_editor::EditorViewportScrollRequest,
    },
    RequestViewportCursor {
        callbacks: Vec<compositor::Callback>,
        request: nucleotide_editor::EditorViewportCursorRequest,
    },
    ReplayInsert {
        replay: InsertReplay,
        count: usize,
    },
    Unhandled {
        keys: Vec<KeyEvent>,
        disposition: UnhandledDisposition,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InsertReplay {
    keys: Vec<KeyEvent>,
    /// Native commits replay as text even when a character has a key binding.
    text_indices: BTreeSet<usize>,
}

impl InsertReplay {
    fn from_keys(keys: &[KeyEvent]) -> Self {
        Self {
            keys: keys.to_vec(),
            text_indices: BTreeSet::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl NativeCommandInput {
    fn insert_text(
        &mut self,
        text: &str,
        replacement: Option<std::ops::Range<usize>>,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) {
        if editor.mode() != Mode::Insert {
            return;
        }
        editor.reset_idle_timer();
        editor.status_msg = None;
        let is_replacement = replacement.is_some();
        if let Some(range) = replacement {
            // Explicit native ranges are literal, atomic replacements. They must
            // not insert another copy at unrelated secondary cursors.
            let view_id = editor.tree.focus;
            let doc_id = editor.tree.get(view_id).doc;
            let doc = editor.documents.get_mut(&doc_id).unwrap();
            let transaction = helix_core::Transaction::change(
                doc.text(),
                [(range.start, range.end, Some(text.into()))].into_iter(),
            );
            let selection = doc.selection(view_id).clone().map(transaction.changes());
            let primary = selection.primary_index();
            let mut ranges = selection.ranges().to_vec();
            ranges[primary] = Range::point(range.start + text.chars().count());
            doc.apply(
                &transaction.with_selection(helix_core::Selection::new(ranges.into(), primary)),
                view_id,
            );
        }
        let mut context = commands::Context {
            editor,
            count: None,
            register: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs,
        };
        for ch in text.chars() {
            if !is_replacement {
                commands::insert::insert_char(&mut context, ch);
            }
            self.current_insert_replay
                .text_indices
                .insert(self.current_insert_replay.keys.len());
            self.current_insert_replay.keys.push(KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::empty(),
            });
        }
        if is_replacement && let Some(c) = text.chars().last() {
            helix_event::dispatch(helix_term::events::PostInsertChar {
                c,
                cx: &mut context,
            });
        }
    }

    #[cfg(test)]
    fn new(keymaps: Keymaps) -> Self {
        Self::new_for_platform(keymaps, TargetPlatform::current())
    }

    fn new_for_platform(keymaps: Keymaps, target_platform: TargetPlatform) -> Self {
        Self {
            keymaps,
            target_platform,
            on_next_key: None,
            current_insert_replay: InsertReplay::default(),
            last_insert_replay: None,
            reset_diff_change_executed: false,
            hosted: false,
            active_slot: None,
            active_document: None,
            pending_commands: VecDeque::new(),
            processing_queue: false,
            awaiting_host: false,
            continuation: None,
            next_token: 0,
            focus_generation: 0,
        }
    }

    fn take_reset_diff_change_executed(&mut self) -> bool {
        std::mem::take(&mut self.reset_diff_change_executed)
    }

    fn prepare_non_key_paste(&mut self, context: &mut commands::Context<'_>) {
        context.editor.status_msg = None;
        context.editor.reset_idle_timer();

        let null_key = KeyEvent {
            code: KeyCode::Null,
            modifiers: KeyModifiers::empty(),
        };
        if let Some((on_next_key, _)) = self.on_next_key.take() {
            on_next_key(context, null_key);
        }
        let _ = self.keymaps.get(context.editor.mode(), null_key);
        context.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());
        context.on_next_key_callback = None;
        context.count = context.editor.count;
    }

    fn handle_key(
        &mut self,
        mut key: KeyEvent,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> NativeInputResult {
        if !self.prepare_new_input(editor) {
            editor.set_error("Workspace command is awaiting host completion");
            return NativeInputResult::Handled {
                completion_requested: None,
                picker_requested: None,
                prompt_requested: None,
            };
        }
        if self.hosted && !self.has_active_slot(editor) {
            self.cancel_pending(editor);
            editor.set_error("Editor input requires a focused document");
            return NativeInputResult::Handled {
                completion_requested: None,
                picker_requested: None,
                prompt_requested: None,
            };
        }
        canonicalize_key(&mut key);
        let mode_before = editor.mode();
        let mut replay_keys = self.keymaps.pending().to_vec();
        replay_keys.push(key);

        if self.hosted
            && editor.macro_replaying.is_empty()
            && let Some((_, keys)) = &mut editor.macro_recording
        {
            keys.push(key);
        }

        editor.reset_idle_timer();
        editor.status_msg = None;

        let command_result = {
            let mut context = commands::Context {
                editor,
                count: None,
                register: None,
                callback: Vec::new(),
                on_next_key_callback: None,
                jobs,
            };

            if !self.run_on_next_key(OnKeyCallbackKind::PseudoPending, &mut context, key) {
                match mode_before {
                    Mode::Insert => self.handle_insert_mode(&mut context, key),
                    _ => self.handle_command_mode(&mut context, key),
                }
            } else {
                self.record_insert_replay_key_if_needed(mode_before, key);
                NativeCommandResult::Handled(Vec::new())
            }
            .with_callbacks_from(&mut context, &mut self.on_next_key)
        };

        let result =
            self.finish_command_result(command_result, mode_before, compositor, editor, jobs);
        if self.hosted && self.has_active_slot(editor) {
            self.seed_insert_replay_if_needed(mode_before, editor.mode(), &replay_keys);
        }
        result
    }

    fn finish_command_result(
        &mut self,
        command_result: NativeCommandResult,
        mode_before: Mode,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> NativeInputResult {
        match command_result {
            NativeCommandResult::RunQueued if self.processing_queue => NativeInputResult::Handled {
                completion_requested: None,
                picker_requested: None,
                prompt_requested: None,
            },
            NativeCommandResult::RunQueued => self.resume_commands(compositor, editor, jobs),
            NativeCommandResult::Handled(callbacks) => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: None,
                    prompt_requested: None,
                }
            }
            NativeCommandResult::RequestCompletion { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: request,
                    picker_requested: None,
                    prompt_requested: None,
                }
            }
            NativeCommandResult::RequestPicker { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: Some(request),
                    prompt_requested: None,
                }
            }
            NativeCommandResult::RequestPrompt { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: None,
                    prompt_requested: Some(request),
                }
            }
            NativeCommandResult::RequestLspNavigation { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestLspNavigation(request)
            }
            NativeCommandResult::RequestWorkspace { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestWorkspace(request)
            }
            NativeCommandResult::RequestSemanticShortcut(request) => {
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestSemanticShortcut(request)
            }
            NativeCommandResult::RequestViewportScroll { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestViewportScroll(request)
            }
            NativeCommandResult::RequestViewportCursor { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestViewportCursor(request)
            }
            NativeCommandResult::ReplayInsert { replay, count } => {
                if self.hosted {
                    self.pending_commands.push_front(PendingCommand::Replay {
                        replay,
                        index: 0,
                        remaining: count,
                    });
                    return self.finish_command_result(
                        NativeCommandResult::RunQueued,
                        mode_before,
                        compositor,
                        editor,
                        jobs,
                    );
                }
                for _ in 0..count {
                    for (index, replay_key) in replay.keys.iter().copied().enumerate() {
                        if replay.text_indices.contains(&index) {
                            let mut buffer = [0; 4];
                            self.insert_text(
                                replay_key.char().unwrap().encode_utf8(&mut buffer),
                                None,
                                editor,
                                jobs,
                            );
                            continue;
                        }
                        match self.handle_key(replay_key, compositor, editor, jobs) {
                            NativeInputResult::Handled { .. } => {}
                            NativeInputResult::RequestLspNavigation(request) => {
                                return NativeInputResult::RequestLspNavigation(request);
                            }
                            NativeInputResult::RequestWorkspace(request) => {
                                return NativeInputResult::RequestWorkspace(request);
                            }
                            NativeInputResult::RequestSemanticShortcut(request) => {
                                return NativeInputResult::RequestSemanticShortcut(request);
                            }
                            NativeInputResult::RequestViewportScroll(request) => {
                                return NativeInputResult::RequestViewportScroll(request);
                            }
                            NativeInputResult::RequestViewportCursor(request) => {
                                return NativeInputResult::RequestViewportCursor(request);
                            }
                            NativeInputResult::Unhandled { keys, disposition } => {
                                return NativeInputResult::Unhandled { keys, disposition };
                            }
                        }
                    }
                }
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: None,
                    prompt_requested: None,
                }
            }
            NativeCommandResult::Unhandled { keys, disposition } => {
                if disposition == UnhandledDisposition::RootNotFound
                    && let Some(key) = keys.last().copied()
                    && let Some(FallbackShortcut::Workspace(request)) =
                        resolve_fallback_shortcut(mode_before, key, self.target_platform)
                {
                    return NativeInputResult::RequestSemanticShortcut(request);
                }
                self.discard_insert_replay_if_needed(mode_before);
                NativeInputResult::Unhandled { keys, disposition }
            }
        }
    }

    fn set_active_context(&mut self, slot: Option<ViewId>, document: Option<DocumentId>) {
        if (self.active_slot, self.active_document) != (slot, document) {
            // Callbacks may capture a document in a reused slot. Keep a history
            // generation rather than accepting equality restored by an ABA visit.
            self.on_next_key = None;
            self.focus_generation += 1;
        }
        self.active_slot = slot;
        self.active_document = document;
    }

    fn has_active_slot(&self, editor: &Editor) -> bool {
        self.active_slot.is_some_and(|slot| {
            editor.tree.focus == slot
                && editor.tree.try_get(slot).is_some()
                && (!editor.is_hosted() || editor.hosted_active_slot() == Some(slot))
        })
    }

    fn suspend(&mut self, kind: EditorContinuationKind, editor: &Editor) -> EditorContinuation {
        self.set_active_context(
            self.active_slot,
            self.active_slot
                .and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc)),
        );
        self.next_token += 1;
        let continuation = EditorContinuation {
            token: EditorCommandToken(self.next_token),
            kind,
        };
        self.continuation = Some(PendingContinuation {
            continuation,
            slot: self.active_slot,
            document: self
                .active_slot
                .and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc)),
            focus_generation: self.focus_generation,
            claimed: false,
        });
        self.awaiting_host = kind != EditorContinuationKind::Ui;
        continuation
    }

    /// A fresh editor input replaces cancellable asynchronous/UI work; it must
    /// never prepend itself to that operation's old destructive mapping tail.
    fn prepare_new_input(&mut self, editor: &mut Editor) -> bool {
        if self.processing_queue {
            return true;
        }
        self.set_active_context(
            self.active_slot,
            self.active_slot
                .and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc)),
        );
        if let Some(pending) = &self.continuation {
            let document = self
                .active_slot
                .and_then(|slot| editor.tree.try_get(slot).map(|view| view.doc));
            if !pending.claimed
                && (matches!(
                    pending.continuation.kind,
                    EditorContinuationKind::LspNavigation | EditorContinuationKind::Ui
                ) || pending.focus_generation != self.focus_generation
                    || pending.document != document)
            {
                self.cancel_pending(editor);
            } else {
                return false;
            }
        }
        true
    }

    fn cancel_pending(&mut self, editor: &mut Editor) {
        self.awaiting_host = false;
        self.continuation = None;
        for pending in self.pending_commands.drain(..) {
            if let PendingCommand::EndMacro(register) = pending {
                if let Some(index) = editor
                    .macro_replaying
                    .iter()
                    .rposition(|reg| *reg == register)
                {
                    editor.macro_replaying.remove(index);
                }
            }
        }
        self.on_next_key = None;
        editor.count = None;
        editor.selected_register = None;
    }

    fn queue_commands(&mut self, commands: &[MappableCommand], context: &commands::Context<'_>) {
        for command in commands.iter().rev() {
            self.pending_commands.push_front(PendingCommand::Command {
                command: command.clone(),
                count: context.count,
                register: context.register,
                from_command_line: false,
            });
        }
    }

    fn resume_commands(
        &mut self,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> NativeInputResult {
        if self.awaiting_host {
            return NativeInputResult::Handled {
                completion_requested: None,
                picker_requested: None,
                prompt_requested: None,
            };
        }
        self.processing_queue = true;
        loop {
            // Ordinary Helix commands can replace a document in the same slot.
            // Only the host can reconcile their ordered facts with content tabs.
            if self.hosted && editor.has_hosted_effects() {
                self.suspend(EditorContinuationKind::Workspace, editor);
                self.processing_queue = false;
                return NativeInputResult::RequestWorkspace(NativeWorkspaceRequest::Command(
                    WorkspaceCommandRequest {
                        command: workspace_commands::WorkspaceCommand::Reconcile,
                        count: 1,
                    },
                ));
            }
            let Some(pending) = self.pending_commands.pop_front() else {
                break;
            };
            let result = match pending {
                PendingCommand::EndMacro(register) => {
                    debug_assert_eq!(editor.macro_replaying.last(), Some(&register));
                    editor.macro_replaying.pop();
                    continue;
                }
                PendingCommand::Replay {
                    replay,
                    mut index,
                    mut remaining,
                } => {
                    if replay.keys.is_empty() || remaining == 0 {
                        continue;
                    }
                    if !self.has_active_slot(editor) {
                        self.cancel_pending(editor);
                        editor.set_error("Macro/insert continuation requires a focused document");
                        break;
                    }
                    let key = replay.keys[index];
                    let text = replay.text_indices.contains(&index);
                    index += 1;
                    if index == replay.keys.len() {
                        index = 0;
                        remaining -= 1;
                    }
                    self.pending_commands.push_front(PendingCommand::Replay {
                        replay,
                        index,
                        remaining,
                    });
                    if text {
                        let mut buffer = [0; 4];
                        self.insert_text(
                            key.char().unwrap().encode_utf8(&mut buffer),
                            None,
                            editor,
                            jobs,
                        );
                        continue;
                    }
                    self.handle_key(key, compositor, editor, jobs)
                }
                PendingCommand::Command {
                    command,
                    count,
                    register,
                    from_command_line,
                } => {
                    let host_command = workspace_commands::is_workspace_command(&command);
                    // Host-only operations may follow each other across non-editor panes.
                    // Anything else must not read or mutate a hidden document.
                    if !self.has_active_slot(editor)
                        && (!host_command || workspace_commands::requires_document(&command))
                    {
                        self.cancel_pending(editor);
                        editor.set_error("Command continuation requires a focused document");
                        break;
                    }
                    let has_document = self.has_active_slot(editor);
                    let mode_before = if has_document {
                        editor.mode()
                    } else {
                        Mode::Normal
                    };
                    let command_result = {
                        let mut context = commands::Context {
                            editor,
                            jobs,
                            count,
                            register,
                            callback: Vec::new(),
                            on_next_key_callback: self.on_next_key.take(),
                        };
                        let result = if host_command {
                            match workspace_commands::prepare(&command, &mut context, has_document)
                            {
                                Ok(request) => NativeCommandResult::RequestWorkspace {
                                    callbacks: Vec::new(),
                                    request: NativeWorkspaceRequest::Command(request),
                                },
                                Err(error) => {
                                    self.cancel_pending(context.editor);
                                    context.editor.set_error(error);
                                    NativeCommandResult::Handled(Vec::new())
                                }
                            }
                        } else if matches!(command, MappableCommand::Macro { .. })
                            || command.name() == "replay_macro"
                        {
                            self.queue_macro(&command, &mut context);
                            NativeCommandResult::Handled(Vec::new())
                        } else if native_insert_completion_command(&command) {
                            NativeCommandResult::RequestCompletion {
                                callbacks: Vec::new(),
                                request: completion_request(&context),
                            }
                        } else {
                            self.dispatch_mapped_command(
                                &command,
                                mode_before,
                                &mut context,
                                from_command_line,
                            )
                            .into_command_result()
                        };
                        result.with_callbacks_from(&mut context, &mut self.on_next_key)
                    };
                    if !self.has_active_slot(editor) {
                        match command_result {
                            NativeCommandResult::RequestWorkspace { request, .. } => {
                                NativeInputResult::RequestWorkspace(request)
                            }
                            _ => NativeInputResult::Handled {
                                completion_requested: None,
                                picker_requested: None,
                                prompt_requested: None,
                            },
                        }
                    } else {
                        self.finish_command_result(
                            command_result,
                            mode_before,
                            compositor,
                            editor,
                            jobs,
                        )
                    }
                }
            };
            if !matches!(
                result,
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: None,
                    prompt_requested: None
                }
            ) {
                if let Some(kind) = result.continuation_kind() {
                    self.suspend(kind, editor);
                } else {
                    self.cancel_pending(editor);
                }
                self.processing_queue = false;
                return result;
            }
        }
        self.processing_queue = false;
        NativeInputResult::Handled {
            completion_requested: None,
            picker_requested: None,
            prompt_requested: None,
        }
    }

    fn queue_macro(&mut self, command: &MappableCommand, context: &mut commands::Context<'_>) {
        let register = if matches!(command, MappableCommand::Macro { .. }) {
            '@'
        } else {
            context.register.unwrap_or('@')
        };
        if context.editor.macro_replaying.contains(&register) {
            context.editor.set_error(format!("Cannot replay from register [{register}] because already replaying from same register"));
            self.cancel_pending(context.editor);
            return;
        }
        let keys = match command {
            MappableCommand::Macro { keys, .. } => Ok(keys.clone()),
            _ => context
                .editor
                .registers
                .read(register, context.editor)
                .filter(|values| values.len() == 1)
                .map(|mut values| {
                    helix_view::input::parse_macro(&values.next().unwrap())
                        .map_err(|error| format!("Invalid macro: {error}"))
                })
                .unwrap_or_else(|| Err(format!("Register [{register}] empty"))),
        };
        match keys {
            Ok(keys) => {
                context.editor.macro_replaying.push(register);
                self.pending_commands
                    .push_front(PendingCommand::EndMacro(register));
                let count = if matches!(command, MappableCommand::Macro { .. }) {
                    1
                } else {
                    context.count()
                };
                self.pending_commands.push_front(PendingCommand::Replay {
                    replay: InsertReplay::from_keys(&keys),
                    index: 0,
                    remaining: count,
                });
            }
            Err(error) => {
                self.cancel_pending(context.editor);
                context.editor.set_error(error);
            }
        }
    }

    fn finish_insert_replay_if_needed(&mut self, mode_before: Mode, mode_after: Mode) {
        if mode_before == Mode::Insert
            && mode_after != Mode::Insert
            && !self.current_insert_replay.is_empty()
        {
            self.last_insert_replay = Some(std::mem::take(&mut self.current_insert_replay));
        }
    }

    fn discard_insert_replay_if_needed(&mut self, mode: Mode) {
        if mode == Mode::Insert {
            self.current_insert_replay = InsertReplay::default();
        }
    }

    fn record_insert_replay_key_if_needed(&mut self, mode_before: Mode, key: KeyEvent) {
        if mode_before == Mode::Insert {
            self.current_insert_replay.keys.push(key);
        }
    }

    fn handle_insert_mode(
        &mut self,
        context: &mut commands::Context<'_>,
        key: KeyEvent,
    ) -> NativeCommandResult {
        let mode = Mode::Insert;
        let pending_keys = self.keymaps.pending().to_vec();
        let has_pending_keys = !pending_keys.is_empty();
        let mut unhandled_keys = pending_keys;
        unhandled_keys.push(key);

        let key_result = self.keymaps.get(mode, key);
        context.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        if self.hosted {
            match &key_result {
                KeymapResult::Matched(command) => {
                    self.current_insert_replay.keys.extend(unhandled_keys);
                    self.queue_commands(std::slice::from_ref(command), context);
                    return NativeCommandResult::RunQueued;
                }
                KeymapResult::MatchedSequence(commands) => {
                    self.current_insert_replay.keys.extend(unhandled_keys);
                    self.queue_commands(commands, context);
                    return NativeCommandResult::RunQueued;
                }
                _ => {}
            }
        }

        match &key_result {
            KeymapResult::Matched(command) => {
                if native_insert_completion_command(command) {
                    self.current_insert_replay.keys.extend(unhandled_keys);
                    return NativeCommandResult::RequestCompletion {
                        callbacks: Vec::new(),
                        request: completion_request(context),
                    };
                }
                if !native_insert_command_supported(command) {
                    return NativeCommandResult::Unhandled {
                        keys: unhandled_keys,
                        disposition: UnhandledDisposition::ReservedUnsupported,
                    };
                }
                let mut last_mode = mode;
                execute_native_command(command, context, &mut last_mode);
                self.current_insert_replay.keys.extend(unhandled_keys);
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapResult::Pending(node) => {
                context.editor.autoinfo = Some(node.infobox());
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapResult::MatchedSequence(commands) => {
                if !commands.iter().all(|command| {
                    native_insert_command_supported(command)
                        || native_insert_completion_command(command)
                }) {
                    return NativeCommandResult::Unhandled {
                        keys: unhandled_keys,
                        disposition: UnhandledDisposition::ReservedUnsupported,
                    };
                }
                let mut last_mode = mode;
                let mut completion_requested = None;
                for command in commands {
                    if native_insert_completion_command(command) {
                        completion_requested = completion_request(context);
                    } else {
                        execute_native_command(command, context, &mut last_mode);
                    }
                }
                self.current_insert_replay.keys.extend(unhandled_keys);
                if completion_requested.is_some() {
                    NativeCommandResult::RequestCompletion {
                        callbacks: Vec::new(),
                        request: completion_requested,
                    }
                } else {
                    NativeCommandResult::Handled(Vec::new())
                }
            }
            KeymapResult::NotFound => {
                if has_pending_keys {
                    return NativeCommandResult::Unhandled {
                        keys: unhandled_keys,
                        disposition: UnhandledDisposition::Cancelled,
                    };
                }

                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if is_native_insert_paste_shortcut(key) {
                    let mut last_mode = mode;
                    execute_clipboard_paste(context, &mut last_mode);
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if is_custom_completion_shortcut(mode, key) {
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::RequestCompletion {
                        callbacks: Vec::new(),
                        request: completion_request(context),
                    };
                }
                if is_custom_code_actions_shortcut(key) {
                    return NativeCommandResult::RequestPicker {
                        callbacks: Vec::new(),
                        request: NativePickerRequest::CodeActions,
                    };
                }
                if let Some(fallback) = resolve_fallback_shortcut(mode, key, self.target_platform) {
                    return match fallback {
                        FallbackShortcut::Workspace(request) => {
                            NativeCommandResult::RequestSemanticShortcut(request)
                        }
                        FallbackShortcut::Editor(action) => {
                            self.current_insert_replay.keys.push(key);
                            execute_editor_semantic_action(action, context)
                        }
                    };
                }

                if let Some(ch) = key.char() {
                    commands::insert::insert_char(context, ch);
                    self.current_insert_replay.keys.push(key);
                    NativeCommandResult::Handled(Vec::new())
                } else {
                    NativeCommandResult::Unhandled {
                        keys: unhandled_keys,
                        disposition: UnhandledDisposition::RootNotFound,
                    }
                }
            }
            KeymapResult::Cancelled(_) => NativeCommandResult::Unhandled {
                keys: unhandled_keys,
                disposition: UnhandledDisposition::Cancelled,
            },
        }
    }

    fn handle_command_mode(
        &mut self,
        context: &mut commands::Context<'_>,
        key: KeyEvent,
    ) -> NativeCommandResult {
        let mode = context.editor.mode();

        if let Some(digit) = command_count_digit(key)
            && let Some(count) = context.editor.count
        {
            let count = count.get() * 10 + digit;
            if count <= 100_000_000 {
                context.editor.count = NonZeroUsize::new(count);
            }
            return NativeCommandResult::Handled(Vec::new());
        }

        if let Some(digit) = non_zero_command_count_digit(key)
            && context.editor.count.is_none()
            && !self.keymaps.contains_key(mode, key)
        {
            context.editor.count = NonZeroUsize::new(digit);
            return NativeCommandResult::Handled(Vec::new());
        }

        if is_repeat_last_insert_key(key) && self.keymaps.pending().is_empty() {
            if let Some(replay) = self.last_insert_replay.clone() {
                let count = context.editor.count.map_or(1, NonZeroUsize::get);
                context.editor.count = None;
                return NativeCommandResult::ReplayInsert { replay, count };
            }
            return NativeCommandResult::Unhandled {
                keys: vec![key],
                disposition: UnhandledDisposition::ReservedUnsupported,
            };
        }

        context.count = context.editor.count;
        context.register = context.editor.selected_register.take();

        let pending_keys = self.keymaps.pending().to_vec();
        let mut unhandled_keys = pending_keys;
        unhandled_keys.push(key);
        let replay_keys = unhandled_keys.clone();

        match self.handle_keymap_event(mode, context, key) {
            KeymapDispatch::Queued => {
                context.editor.count = None;
                NativeCommandResult::RunQueued
            }
            KeymapDispatch::Handled => {
                self.seed_insert_replay_if_needed(mode, context.editor.mode(), &replay_keys);

                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapDispatch::RequestPicker(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestPicker {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::RequestPrompt(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestPrompt {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::RequestLspNavigation(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestLspNavigation {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::RequestWorkspace(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestWorkspace {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::RequestViewportScroll(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestViewportScroll {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::RequestViewportCursor(request) => {
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                } else {
                    context.editor.selected_register = context.register.take();
                }
                NativeCommandResult::RequestViewportCursor {
                    callbacks: Vec::new(),
                    request,
                }
            }
            KeymapDispatch::Pending => {
                context.editor.selected_register = context.register.take();
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapDispatch::Unhandled(disposition) => {
                context.editor.selected_register = context.register.take();
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                }
                if disposition == UnhandledDisposition::RootNotFound
                    && let Some(fallback) =
                        resolve_fallback_shortcut(mode, key, self.target_platform)
                {
                    return match fallback {
                        FallbackShortcut::Workspace(request) => {
                            NativeCommandResult::RequestSemanticShortcut(request)
                        }
                        FallbackShortcut::Editor(action) => {
                            execute_editor_semantic_action(action, context)
                        }
                    };
                }
                NativeCommandResult::Unhandled {
                    keys: unhandled_keys,
                    disposition,
                }
            }
        }
    }

    fn seed_insert_replay_if_needed(
        &mut self,
        mode_before: Mode,
        mode_after: Mode,
        replay_keys: &[KeyEvent],
    ) {
        if mode_before != Mode::Insert && mode_after == Mode::Insert {
            self.current_insert_replay = InsertReplay::from_keys(replay_keys);
        }
    }

    fn handle_keymap_event(
        &mut self,
        mode: Mode,
        context: &mut commands::Context<'_>,
        key: KeyEvent,
    ) -> KeymapDispatch {
        let mut last_mode = mode;
        let key_result = self.keymaps.get(mode, key);
        context.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        if self.hosted {
            match &key_result {
                KeymapResult::Matched(command) => {
                    self.queue_commands(std::slice::from_ref(command), context);
                    return KeymapDispatch::Queued;
                }
                KeymapResult::MatchedSequence(commands) => {
                    self.queue_commands(commands, context);
                    return KeymapDispatch::Queued;
                }
                _ => {}
            }
        }

        match &key_result {
            KeymapResult::Matched(command) => {
                self.dispatch_mapped_command(command, mode, context, false)
            }
            KeymapResult::Pending(node) => {
                context.editor.autoinfo = Some(node.infobox());
                KeymapDispatch::Pending
            }
            KeymapResult::MatchedSequence(commands) => {
                if !commands.iter().all(|command| {
                    native_command_supported(command)
                        || native_picker_command(command).is_some()
                        || native_prompt_command(command).is_some()
                        || native_lsp_navigation_command(command).is_some()
                        || native_workspace_command(command).is_some()
                        || native_viewport_cursor_command(command, None).is_some()
                        || native_page_cursor_command_supported(command)
                        || native_viewport_scroll_command(command, None).is_some()
                }) {
                    return KeymapDispatch::Unhandled(UnhandledDisposition::ReservedUnsupported);
                }

                for command in commands {
                    if let Some(request) = native_picker_command(command) {
                        return KeymapDispatch::RequestPicker(request);
                    }
                    if let Some(request) = native_prompt_command(command) {
                        return KeymapDispatch::RequestPrompt(request);
                    }
                    if let Some(action) = native_file_navigation_command(command) {
                        return handle_native_file_navigation(context, action);
                    }
                    if let Some(request) = native_lsp_navigation_command(command) {
                        return KeymapDispatch::RequestLspNavigation(request);
                    }
                    if let Some(request) = native_workspace_command(command) {
                        return KeymapDispatch::RequestWorkspace(request);
                    }
                    if let Some(request) = native_viewport_cursor_command(command, context.count) {
                        return KeymapDispatch::RequestViewportCursor(request);
                    }
                    if handle_native_align_view_middle(command, context) {
                        continue;
                    }
                    if let Some(request) = native_page_cursor_command(command, context) {
                        return KeymapDispatch::RequestViewportScroll(request);
                    }
                    if let Some(request) = native_viewport_scroll_command(command, context.count) {
                        return KeymapDispatch::RequestViewportScroll(request);
                    }
                    self.record_reset_diff_change_if_needed(command);
                    execute_native_command(command, context, &mut last_mode);
                }
                KeymapDispatch::Handled
            }
            KeymapResult::NotFound => {
                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    KeymapDispatch::Handled
                } else {
                    KeymapDispatch::Unhandled(UnhandledDisposition::RootNotFound)
                }
            }
            KeymapResult::Cancelled(_) => {
                KeymapDispatch::Unhandled(UnhandledDisposition::Cancelled)
            }
        }
    }

    fn dispatch_mapped_command(
        &mut self,
        command: &MappableCommand,
        mut last_mode: Mode,
        context: &mut commands::Context<'_>,
        from_command_line: bool,
    ) -> KeymapDispatch {
        if let Some(request) = native_picker_command(command) {
            return KeymapDispatch::RequestPicker(request);
        }
        if let Some(request) = native_prompt_command(command) {
            return KeymapDispatch::RequestPrompt(request);
        }
        if let Some(action) = native_file_navigation_command(command) {
            return handle_native_file_navigation(context, action);
        }
        if let Some(request) = native_lsp_navigation_command(command) {
            return KeymapDispatch::RequestLspNavigation(request);
        }
        if let Some(request) = native_workspace_command(command) {
            return KeymapDispatch::RequestWorkspace(request);
        }
        if let Some(request) = native_viewport_cursor_command(command, context.count) {
            return KeymapDispatch::RequestViewportCursor(request);
        }
        if handle_native_align_view_middle(command, context) {
            return KeymapDispatch::Handled;
        }
        if let Some(request) = native_page_cursor_command(command, context) {
            return KeymapDispatch::RequestViewportScroll(request);
        }
        if let Some(request) = native_viewport_scroll_command(command, context.count) {
            return KeymapDispatch::RequestViewportScroll(request);
        }
        if !from_command_line
            && !native_command_supported(command)
            && !(self.hosted
                && (matches!(command, MappableCommand::Typable { .. })
                    || native_insert_command_supported(command)
                    || command.name() == "record_macro"))
        {
            return KeymapDispatch::Unhandled(UnhandledDisposition::ReservedUnsupported);
        }
        self.record_reset_diff_change_if_needed(command);
        execute_native_command(command, context, &mut last_mode);
        KeymapDispatch::Handled
    }

    fn record_reset_diff_change_if_needed(&mut self, command: &MappableCommand) {
        if is_reset_diff_change_command(command) {
            self.reset_diff_change_executed = true;
        }
    }

    fn run_on_next_key(
        &mut self,
        kind: OnKeyCallbackKind,
        context: &mut commands::Context<'_>,
        key: KeyEvent,
    ) -> bool {
        let Some((on_next_key, callback_kind)) = self.on_next_key.take() else {
            return false;
        };

        if callback_kind == kind {
            on_next_key(context, key);
            true
        } else {
            self.on_next_key = Some((on_next_key, callback_kind));
            false
        }
    }
}

enum KeymapDispatch {
    Queued,
    Handled,
    RequestPicker(NativePickerRequest),
    RequestPrompt(NativePromptRequest),
    RequestLspNavigation(NativeLspNavigationRequest),
    RequestWorkspace(NativeWorkspaceRequest),
    RequestViewportScroll(nucleotide_editor::EditorViewportScrollRequest),
    RequestViewportCursor(nucleotide_editor::EditorViewportCursorRequest),
    Pending,
    Unhandled(UnhandledDisposition),
}

impl KeymapDispatch {
    fn into_command_result(self) -> NativeCommandResult {
        match self {
            Self::Queued => NativeCommandResult::RunQueued,
            Self::Handled | Self::Pending => NativeCommandResult::Handled(Vec::new()),
            Self::RequestPicker(request) => NativeCommandResult::RequestPicker {
                callbacks: Vec::new(),
                request,
            },
            Self::RequestPrompt(request) => NativeCommandResult::RequestPrompt {
                callbacks: Vec::new(),
                request,
            },
            Self::RequestLspNavigation(request) => NativeCommandResult::RequestLspNavigation {
                callbacks: Vec::new(),
                request,
            },
            Self::RequestWorkspace(request) => NativeCommandResult::RequestWorkspace {
                callbacks: Vec::new(),
                request,
            },
            Self::RequestViewportScroll(request) => NativeCommandResult::RequestViewportScroll {
                callbacks: Vec::new(),
                request,
            },
            Self::RequestViewportCursor(request) => NativeCommandResult::RequestViewportCursor {
                callbacks: Vec::new(),
                request,
            },
            Self::Unhandled(disposition) => NativeCommandResult::Unhandled {
                keys: Vec::new(),
                disposition,
            },
        }
    }
}

trait NativeCommandResultExt {
    fn with_callbacks_from(
        self,
        context: &mut commands::Context<'_>,
        on_next_key: &mut Option<(OnKeyCallback, OnKeyCallbackKind)>,
    ) -> Self;
}

impl NativeCommandResultExt for NativeCommandResult {
    fn with_callbacks_from(
        self,
        context: &mut commands::Context<'_>,
        on_next_key: &mut Option<(OnKeyCallback, OnKeyCallbackKind)>,
    ) -> Self {
        match self {
            NativeCommandResult::Handled(_) => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::Handled(std::mem::take(&mut context.callback))
            }
            NativeCommandResult::RequestCompletion { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestCompletion {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            NativeCommandResult::RequestPicker { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestPicker {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            NativeCommandResult::RequestLspNavigation { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestLspNavigation {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            NativeCommandResult::RequestWorkspace { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestWorkspace {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            NativeCommandResult::RequestViewportScroll { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestViewportScroll {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            NativeCommandResult::RequestViewportCursor { request, .. } => {
                *on_next_key = context.on_next_key_callback.take();
                NativeCommandResult::RequestViewportCursor {
                    callbacks: std::mem::take(&mut context.callback),
                    request,
                }
            }
            other => other,
        }
    }
}

fn completion_request(context: &commands::Context<'_>) -> Option<NativeCompletionRequest> {
    let view_id = context.editor.tree.focus;
    let doc_id = context.editor.tree.try_get(view_id)?.doc;
    Some(NativeCompletionRequest { doc_id, view_id })
}

fn execute_native_command(
    command: &MappableCommand,
    context: &mut commands::Context<'_>,
    last_mode: &mut Mode,
) {
    command.execute(context);
    helix_event::dispatch(PostCommand {
        command,
        cx: context,
    });

    let current_mode = context.editor.mode();
    if current_mode != *last_mode {
        helix_event::dispatch(OnModeSwitch {
            old_mode: *last_mode,
            new_mode: current_mode,
            cx: context,
        });
        *last_mode = current_mode;
    }
}

fn execute_editor_semantic_action(
    action: EditorSemanticAction,
    context: &mut commands::Context<'_>,
) -> NativeCommandResult {
    if action == EditorSemanticAction::Find {
        return NativeCommandResult::RequestPrompt {
            callbacks: Vec::new(),
            request: NativePromptRequest::Search,
        };
    }
    if action == EditorSemanticAction::Paste {
        let mut last_mode = context.editor.mode();
        execute_clipboard_paste(context, &mut last_mode);
        return NativeCommandResult::Handled(Vec::new());
    }
    let mut last_mode = context.editor.mode();
    for command in action.commands() {
        execute_native_command(command, context, &mut last_mode);
    }
    NativeCommandResult::Handled(Vec::new())
}

fn execute_clipboard_paste(context: &mut commands::Context<'_>, last_mode: &mut Mode) {
    let mode = context.editor.mode();
    if mode == Mode::Normal {
        execute_native_command(&MappableCommand::paste_clipboard_before, context, last_mode);
        return;
    }

    // Helix's bracketed-paste path uses Paste::Cursor in Insert and Select modes.
    // That implementation is crate-private, but insert_register reaches the same
    // paste path. Resolve its register callback immediately with the clipboard
    // register rather than leaving the editor waiting for another key.
    let previous_autoinfo = context.editor.autoinfo.take();
    MappableCommand::insert_register.execute(context);
    if let Some((paste_register, _)) = context.on_next_key_callback.take() {
        paste_register(
            context,
            KeyEvent {
                code: KeyCode::Char('+'),
                modifiers: KeyModifiers::empty(),
            },
        );
    }
    context.editor.autoinfo = previous_autoinfo;

    if mode == Mode::Select {
        context.editor.mode = Mode::Normal;
        helix_event::dispatch(OnModeSwitch {
            old_mode: Mode::Select,
            new_mode: Mode::Normal,
            cx: context,
        });
        *last_mode = Mode::Normal;
    }
}

fn finalize_native_command(
    editor: &mut Editor,
    jobs: &mut Jobs,
    compositor: &mut Compositor,
    callbacks: Vec<compositor::Callback>,
) {
    if editor.should_close() {
        return;
    }

    if editor.mode() != Mode::Insert {
        let view_id = editor.tree.focus;
        if let Some(doc_id) = editor.tree.try_get(view_id).map(|view| view.doc) {
            let tree = &mut editor.tree;
            let documents = &mut editor.documents;
            let view = tree.get_mut(view_id);
            if let Some(doc) = documents.get_mut(&doc_id) {
                doc.append_changes_to_history(view);
            } else {
                debug!(
                    doc_id = ?doc_id,
                    "Skipping native command history append because focused document is missing"
                );
            }
        } else {
            debug!(
                view_id = ?view_id,
                "Skipping native command history append because focused view is missing"
            );
        }
    }

    for callback in callbacks {
        let mut context = compositor::Context {
            editor,
            scroll: None,
            jobs,
        };
        callback(compositor, &mut context);
    }
}

fn native_command_supported(command: &MappableCommand) -> bool {
    let name = command.name();

    name == "normal_mode"
        || native_insert_entry_command(command)
        || native_prompt_command(command).is_some()
        || native_history_command(command)
        || native_register_edit_command(command)
        || native_register_selection_command(command)
        || native_selection_transform_command(command)
        || native_textobject_command(command)
        || native_surround_command(command)
        || native_search_command(command)
        || native_buffer_navigation_command(command)
        || native_diagnostic_navigation_command(command)
        || native_syntax_object_navigation_command(command)
        || native_jumplist_navigation_command(command)
        || native_split_navigation_command(command)
        || native_file_navigation_command(command).is_some()
        || name.starts_with("move_")
        || name.starts_with("extend_")
        || matches!(
            name,
            "add_newline_above"
                | "add_newline_below"
                | "align_view_bottom"
                | "align_view_center"
                | "align_view_middle"
                | "align_view_top"
                | "collapse_selection"
                | "exit_select_mode"
                | "flip_selections"
                | "goto_column"
                | "goto_file_end"
                | "goto_file_start"
                | "goto_first_nonwhitespace"
                | "goto_last_line"
                | "goto_line_end"
                | "goto_line_end_newline"
                | "goto_line_start"
                | "goto_next_paragraph"
                | "goto_prev_paragraph"
                | "goto_window_bottom"
                | "goto_window_center"
                | "goto_window_top"
                | "half_page_down"
                | "half_page_up"
                | "page_down"
                | "page_up"
                | "keep_primary_selection"
                | "page_cursor_up"
                | "page_cursor_down"
                | "page_cursor_half_up"
                | "page_cursor_half_down"
                | "remove_primary_selection"
                | "reset-diff-change"
                | "scroll_down"
                | "scroll_up"
                | "select_all"
                | "select_line_above"
                | "select_line_below"
                | "select_mode"
                | "match_brackets"
        )
}

fn is_reset_diff_change_command(command: &MappableCommand) -> bool {
    command.name() == "reset-diff-change"
}

fn native_insert_entry_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "insert_mode"
            | "append_mode"
            | "insert_at_line_start"
            | "insert_at_line_end"
            | "open_below"
            | "open_above"
    )
}

fn native_prompt_command(command: &MappableCommand) -> Option<NativePromptRequest> {
    match command.name() {
        "command_mode" => Some(NativePromptRequest::Command),
        "search" => Some(NativePromptRequest::Search),
        "rsearch" => Some(NativePromptRequest::ReverseSearch),
        "global_search" => Some(NativePromptRequest::GlobalSearch),
        "select_regex" => Some(NativePromptRequest::RegexSelection(
            crate::types::RegexSelectionAction::Select,
        )),
        "split_selection" => Some(NativePromptRequest::RegexSelection(
            crate::types::RegexSelectionAction::Split,
        )),
        "keep_selections" => Some(NativePromptRequest::RegexSelection(
            crate::types::RegexSelectionAction::Keep,
        )),
        "remove_selections" => Some(NativePromptRequest::RegexSelection(
            crate::types::RegexSelectionAction::Remove,
        )),
        _ => None,
    }
}

fn native_picker_command(command: &MappableCommand) -> Option<NativePickerRequest> {
    match command.name() {
        "file_picker" => Some(NativePickerRequest::File),
        "file_picker_in_current_directory" => Some(NativePickerRequest::FileCurrentDirectory),
        "file_picker_in_current_buffer_directory" => {
            Some(NativePickerRequest::FileCurrentBufferDirectory)
        }
        "buffer_picker" => Some(NativePickerRequest::Buffer),
        "jumplist_picker" => Some(NativePickerRequest::JumpList),
        "lsp_or_syntax_symbol_picker" | "symbol_picker" => {
            Some(NativePickerRequest::Symbols { workspace: false })
        }
        "lsp_or_syntax_workspace_symbol_picker" | "workspace_symbol_picker" => {
            Some(NativePickerRequest::Symbols { workspace: true })
        }
        "diagnostics_picker" => Some(NativePickerRequest::Diagnostics { workspace: false }),
        "workspace_diagnostics_picker" => {
            Some(NativePickerRequest::Diagnostics { workspace: true })
        }
        "code_action" => Some(NativePickerRequest::CodeActions),
        "hover" => Some(NativePickerRequest::HoverDocs),
        _ => None,
    }
}

fn native_lsp_navigation_command(command: &MappableCommand) -> Option<NativeLspNavigationRequest> {
    match command.name() {
        "goto_declaration" => Some(NativeLspNavigationRequest::GotoDeclaration),
        "goto_definition" => Some(NativeLspNavigationRequest::GotoDefinition),
        "goto_type_definition" => Some(NativeLspNavigationRequest::GotoTypeDefinition),
        "goto_implementation" => Some(NativeLspNavigationRequest::GotoImplementation),
        "goto_reference" => Some(NativeLspNavigationRequest::GotoReference),
        _ => None,
    }
}

fn native_page_cursor_command_supported(command: &MappableCommand) -> bool {
    native_page_cursor_command_spec(command).is_some()
}

fn native_page_cursor_command(
    command: &MappableCommand,
    context: &mut commands::Context<'_>,
) -> Option<nucleotide_editor::EditorViewportScrollRequest> {
    let (direction, pages, divisor) = native_page_cursor_command_spec(command)?;
    let view_id = context.editor.tree.focus;
    let rows = context.editor.tree.try_get(view_id)?.inner_height() / divisor;

    apply_native_page_cursor_movement(context.editor, view_id, direction, rows)?;

    Some(nucleotide_editor::EditorViewportScrollRequest::VisualPageWithCursor { pages, divisor })
}

fn native_page_cursor_command_spec(
    command: &MappableCommand,
) -> Option<(
    nucleotide_editor::EditorViewportScrollDirection,
    isize,
    usize,
)> {
    match command.name() {
        "page_cursor_down" => Some((
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            1,
            1,
        )),
        "page_cursor_up" => Some((
            nucleotide_editor::EditorViewportScrollDirection::Backward,
            -1,
            1,
        )),
        "page_cursor_half_down" => Some((
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            1,
            2,
        )),
        "page_cursor_half_up" => Some((
            nucleotide_editor::EditorViewportScrollDirection::Backward,
            -1,
            2,
        )),
        _ => None,
    }
}

fn native_viewport_cursor_command(
    command: &MappableCommand,
    count: Option<NonZeroUsize>,
) -> Option<nucleotide_editor::EditorViewportCursorRequest> {
    let target = match command.name() {
        "goto_window_top" => nucleotide_editor::EditorViewportCursorTarget::Top,
        "goto_window_center" => nucleotide_editor::EditorViewportCursorTarget::Center,
        "goto_window_bottom" => nucleotide_editor::EditorViewportCursorTarget::Bottom,
        _ => return None,
    };

    Some(nucleotide_editor::EditorViewportCursorRequest {
        target,
        count: count.map_or(1, NonZeroUsize::get),
    })
}

fn handle_native_align_view_middle(
    command: &MappableCommand,
    context: &mut commands::Context<'_>,
) -> bool {
    if command.name() != "align_view_middle" {
        return false;
    }

    let view_id = context.editor.tree.focus;
    let Some(view) = context.editor.tree.try_get(view_id).cloned() else {
        return true;
    };
    let Some(doc) = context.editor.document_mut(view.doc) else {
        return true;
    };

    let inner_width = view.inner_width(doc);
    let text_format = doc.text_format(inner_width, None);
    if text_format.soft_wrap {
        return true;
    }

    let text = doc.text().slice(..);
    let cursor = doc.selection(view_id).primary().cursor(text);
    let cursor_position = visual_offset_from_block(
        text,
        doc.view_offset(view_id).anchor,
        cursor,
        &text_format,
        &view.text_annotations(doc, None),
    )
    .0;

    let mut view_offset = doc.view_offset(view_id);
    view_offset.horizontal_offset = cursor_position
        .col
        .saturating_sub((view.inner_area(doc).width as usize) / 2);
    doc.set_view_offset(view_id, view_offset);
    true
}

fn native_viewport_scroll_command(
    command: &MappableCommand,
    count: Option<NonZeroUsize>,
) -> Option<nucleotide_editor::EditorViewportScrollRequest> {
    let rows = count.map_or(1, NonZeroUsize::get);
    let rows = isize::try_from(rows).unwrap_or(isize::MAX);

    match command.name() {
        "align_view_top" => Some(
            nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                nucleotide_editor::EditorCursorReveal::Top,
            ),
        ),
        "align_view_center" => Some(
            nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                nucleotide_editor::EditorCursorReveal::Center,
            ),
        ),
        "align_view_bottom" => Some(
            nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                nucleotide_editor::EditorCursorReveal::Bottom,
            ),
        ),
        "page_down" => Some(nucleotide_editor::EditorViewportScrollRequest::VisualPages(
            1,
        )),
        "page_up" => Some(nucleotide_editor::EditorViewportScrollRequest::VisualPages(
            -1,
        )),
        "half_page_down" => Some(
            nucleotide_editor::EditorViewportScrollRequest::VisualPageFraction {
                pages: 1,
                divisor: 2,
            },
        ),
        "half_page_up" => Some(
            nucleotide_editor::EditorViewportScrollRequest::VisualPageFraction {
                pages: -1,
                divisor: 2,
            },
        ),
        "scroll_down" => Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
            rows,
        )),
        "scroll_up" => Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
            rows.saturating_neg(),
        )),
        _ => None,
    }
}

pub(crate) fn apply_native_page_cursor_movement(
    editor: &mut Editor,
    view_id: ViewId,
    direction: nucleotide_editor::EditorViewportScrollDirection,
    rows: usize,
) -> Option<DocumentId> {
    let mode = editor.mode();
    let movement = match mode {
        Mode::Select => Movement::Extend,
        _ => Movement::Move,
    };
    let direction = match direction {
        nucleotide_editor::EditorViewportScrollDirection::Forward => Direction::Forward,
        nucleotide_editor::EditorViewportScrollDirection::Backward => Direction::Backward,
    };
    let doc_id = editor.tree.try_get(view_id)?.doc;

    let selection = {
        let view = editor.tree.try_get(view_id)?;
        let doc = editor.document(doc_id)?;
        let text = doc.text().slice(..);
        let text_format = doc.text_format(view.inner_area(doc).width, None);
        let mut annotations = view.text_annotations(doc, None);

        doc.selection(view_id).clone().transform(|range| {
            move_vertically_visual(
                text,
                range,
                direction,
                rows,
                movement,
                &text_format,
                &mut annotations,
            )
        })
    };

    let doc = editor.document_mut(doc_id)?;
    doc.set_selection(view_id, selection);
    Some(doc_id)
}

pub(crate) fn apply_native_viewport_cursor_request(
    editor: &mut Editor,
    view_id: ViewId,
    target_visual_row: usize,
) -> Option<DocumentId> {
    let extend = editor.mode() == Mode::Select;
    let doc_id = editor.tree.try_get(view_id)?.doc;

    let selection = {
        let view = editor.tree.try_get(view_id)?;
        let doc = editor.document(doc_id)?;
        let text = doc.text().slice(..);
        let text_format = doc.text_format(view.inner_area(doc).width, None);
        let annotations = view.text_annotations(doc, None);
        let horizontal_offset = doc.view_offset(view_id).horizontal_offset;
        let target_visual_row = isize::try_from(target_visual_row).unwrap_or(isize::MAX);
        let (pos, _) = char_idx_at_visual_offset(
            text,
            0,
            target_visual_row,
            horizontal_offset,
            &text_format,
            &annotations,
        );

        doc.selection(view_id)
            .clone()
            .transform(|range| range.put_cursor(text, pos, extend))
    };

    let doc = editor.document_mut(doc_id)?;
    doc.set_selection(view_id, selection);
    Some(doc_id)
}

pub(crate) fn sync_cursor_after_native_page_scroll(
    editor: &mut Editor,
    view_id: ViewId,
    direction: nucleotide_editor::EditorViewportScrollDirection,
    top_visual_row: usize,
    visible_rows: usize,
) -> Option<DocumentId> {
    let visible_rows = visible_rows.max(1);
    let mode = editor.mode();
    let scrolloff = editor
        .config()
        .scrolloff
        .min(visible_rows.saturating_sub(1) / 2);
    let doc_id = editor.tree.try_get(view_id)?.doc;

    let (primary_index, anchor, head) = {
        let view = editor.tree.try_get(view_id)?;
        let doc = editor.document(doc_id)?;
        let text = doc.text().slice(..);
        let range = doc.selection(view_id).primary();
        let cursor = range.cursor(text);
        let text_format = doc.text_format(view.inner_area(doc).width, None);
        let annotations = view.text_annotations(doc, None);

        let target_visual_row = match direction {
            nucleotide_editor::EditorViewportScrollDirection::Forward => {
                top_visual_row.saturating_add(scrolloff)
            }
            nucleotide_editor::EditorViewportScrollDirection::Backward => top_visual_row
                .saturating_add(visible_rows)
                .saturating_sub(scrolloff)
                .saturating_sub(1),
        };
        let target_visual_row = isize::try_from(target_visual_row).unwrap_or(isize::MAX);
        let (mut head, offset_within_visual_row) =
            char_idx_at_visual_offset(text, 0, target_visual_row, 0, &text_format, &annotations);

        match direction {
            nucleotide_editor::EditorViewportScrollDirection::Forward => {
                head = head.saturating_add((offset_within_visual_row != 0) as usize);
                if head <= cursor {
                    return None;
                }
            }
            nucleotide_editor::EditorViewportScrollDirection::Backward => {
                if head >= cursor {
                    return None;
                }
            }
        }

        let anchor = if mode == Mode::Select {
            range.anchor
        } else {
            head
        };
        (doc.selection(view_id).primary_index(), anchor, head)
    };

    let doc = editor.document_mut(doc_id)?;
    let selection = doc
        .selection(view_id)
        .clone()
        .replace(primary_index, Range::new(anchor, head));
    doc.set_selection(view_id, selection);
    Some(doc_id)
}

fn native_workspace_command(command: &MappableCommand) -> Option<NativeWorkspaceRequest> {
    (command.name() == "toggle-file-tree").then_some(NativeWorkspaceRequest::ToggleFileTree)
}

fn native_file_navigation_command(command: &MappableCommand) -> Option<NativeFileOpenAction> {
    match command.name() {
        "goto_file" => Some(NativeFileOpenAction::Replace),
        "goto_file_hsplit" => Some(NativeFileOpenAction::HorizontalSplit),
        "goto_file_vsplit" => Some(NativeFileOpenAction::VerticalSplit),
        _ => None,
    }
}

fn handle_native_file_navigation(
    context: &mut commands::Context<'_>,
    action: NativeFileOpenAction,
) -> KeymapDispatch {
    let Some((base_path, targets)) = native_file_navigation_targets(context.editor) else {
        return KeymapDispatch::Unhandled(UnhandledDisposition::ReservedUnsupported);
    };

    let mut paths = Vec::new();
    for target in targets {
        if target_is_external_url(&target) {
            return KeymapDispatch::Unhandled(UnhandledDisposition::ReservedUnsupported);
        }

        let target_path = path::expand(&target);
        let target_path = base_path.join(target_path.as_ref());

        paths.push(target_path);
    }

    if paths.is_empty() {
        KeymapDispatch::Handled
    } else {
        KeymapDispatch::RequestWorkspace(NativeWorkspaceRequest::OpenFiles { paths, action })
    }
}

fn target_is_external_url(target: &str) -> bool {
    if target.len() >= 3 {
        let bytes = target.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/') {
            return false;
        }
    }

    url::Url::parse(target).is_ok()
}

fn native_file_navigation_targets(editor: &Editor) -> Option<(PathBuf, Vec<String>)> {
    let view = editor.tree.try_get(editor.tree.focus)?;
    let doc = editor.documents.get(&view.doc)?;
    let text = doc.text().slice(..);
    let selections = doc.selection(view.id);
    let primary = selections.primary();
    let base_path = doc
        .relative_path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let targets = if selections.len() == 1 && primary.len() == 1 {
        let lookaround = 1000;
        let pos = text.char_to_byte(primary.cursor(text));
        let search_start = text
            .line_to_byte(text.byte_to_line(pos))
            .max(text.floor_char_boundary(pos.saturating_sub(lookaround)));
        let search_end = text
            .line_to_byte(text.byte_to_line(pos) + 1)
            .min(text.ceil_char_boundary(pos + lookaround));
        let search_range = text.byte_slice(search_start..search_end);
        let target = find_paths(search_range, true)
            .take_while(|range| search_start + range.start <= pos + 1)
            .find(|range| pos <= search_start + range.end)
            .map(|range| Cow::from(search_range.byte_slice(range)))
            .unwrap_or_else(|| primary.fragment(text))
            .into_owned();

        vec![target]
    } else {
        selections
            .fragments(text)
            .map(|selection| selection.trim().to_owned())
            .filter(|selection| !selection.is_empty())
            .collect()
    };

    Some((base_path, targets))
}

fn native_history_command(command: &MappableCommand) -> bool {
    matches!(command.name(), "undo" | "redo")
}

fn native_search_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "search_next"
            | "search_prev"
            | "extend_search_next"
            | "extend_search_prev"
            | "search_selection"
            | "search_selection_detect_word_boundaries"
            | "make_search_word_bounded"
    )
}

fn native_register_edit_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "delete_selection"
            | "delete_selection_noyank"
            | "change_selection"
            | "change_selection_noyank"
            | "yank"
            | "yank_to_clipboard"
            | "yank_to_primary_clipboard"
            | "yank_joined"
            | "yank_joined_to_clipboard"
            | "yank_joined_to_primary_clipboard"
            | "yank_main_selection_to_clipboard"
            | "yank_main_selection_to_primary_clipboard"
            | "paste_after"
            | "paste_before"
            | "paste_clipboard_after"
            | "paste_clipboard_before"
            | "paste_primary_clipboard_after"
            | "paste_primary_clipboard_before"
            | "replace_with_yanked"
            | "replace_selections_with_clipboard"
            | "replace_selections_with_primary_clipboard"
    )
}

fn native_register_selection_command(command: &MappableCommand) -> bool {
    matches!(command.name(), "select_register" | "copy_between_registers")
}

fn native_buffer_navigation_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "goto_next_buffer"
            | "goto_previous_buffer"
            | "goto_last_accessed_file"
            | "goto_last_modified_file"
            | "goto_last_modification"
    )
}

fn native_diagnostic_navigation_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "goto_first_diag"
            | "goto_last_diag"
            | "goto_next_diag"
            | "goto_prev_diag"
            | "goto_first_change"
            | "goto_last_change"
            | "goto_next_change"
            | "goto_prev_change"
    )
}

fn native_syntax_object_navigation_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "goto_next_function"
            | "goto_prev_function"
            | "goto_next_class"
            | "goto_prev_class"
            | "goto_next_parameter"
            | "goto_prev_parameter"
            | "goto_next_comment"
            | "goto_prev_comment"
            | "goto_next_test"
            | "goto_prev_test"
            | "goto_next_xml_element"
            | "goto_prev_xml_element"
            | "goto_next_entry"
            | "goto_prev_entry"
    )
}

fn native_jumplist_navigation_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "jump_forward" | "jump_backward" | "save_selection"
    )
}

fn native_split_navigation_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "jump_view_right"
            | "jump_view_left"
            | "jump_view_up"
            | "jump_view_down"
            | "swap_view_right"
            | "swap_view_left"
            | "swap_view_up"
            | "swap_view_down"
            | "transpose_view"
            | "rotate_view"
            | "rotate_view_reverse"
            | "hsplit"
            | "hsplit_new"
            | "vsplit"
            | "vsplit_new"
            | "wclose"
            | "wonly"
    )
}

fn native_selection_transform_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "copy_selection_on_prev_line"
            | "copy_selection_on_next_line"
            | "split_selection_on_newline"
            | "merge_selections"
            | "merge_consecutive_selections"
            | "shrink_to_line_bounds"
            | "ensure_selections_forward"
            | "trim_selections"
            | "align_selections"
            | "indent"
            | "unindent"
            | "join_selections"
            | "join_selections_space"
            | "switch_case"
            | "switch_to_lowercase"
            | "switch_to_uppercase"
            | "rotate_selections_forward"
            | "rotate_selections_backward"
            | "rotate_selections_first"
            | "rotate_selections_last"
            | "rotate_selection_contents_forward"
            | "rotate_selection_contents_backward"
            | "reverse_selection_contents"
            | "expand_selection"
            | "shrink_selection"
            | "select_next_sibling"
            | "select_prev_sibling"
            | "select_all_siblings"
            | "select_all_children"
            | "toggle_comments"
            | "toggle_line_comments"
            | "toggle_block_comments"
    )
}

fn native_textobject_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "select_textobject_around" | "select_textobject_inner"
    )
}

fn native_surround_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "surround_add" | "surround_replace" | "surround_delete"
    )
}

fn native_insert_command_supported(command: &MappableCommand) -> bool {
    (!native_insert_entry_command(command)
        && native_prompt_command(command).is_none()
        && !native_history_command(command)
        && !native_register_edit_command(command)
        && !native_register_selection_command(command)
        && !native_selection_transform_command(command)
        && !native_textobject_command(command)
        && !native_surround_command(command)
        && !native_syntax_object_navigation_command(command)
        && native_file_navigation_command(command).is_none()
        && native_command_supported(command))
        || native_insert_edit_command(command)
        || native_insert_interactive_command(command)
}

fn native_insert_edit_command(command: &MappableCommand) -> bool {
    matches!(
        command.name(),
        "commit_undo_checkpoint"
            | "delete_char_backward"
            | "delete_char_forward"
            | "delete_word_backward"
            | "delete_word_forward"
            | "insert_newline"
            | "insert_tab"
            | "kill_to_line_end"
            | "kill_to_line_start"
            | "smart_tab"
    )
}

fn native_insert_interactive_command(command: &MappableCommand) -> bool {
    matches!(command.name(), "insert_register")
}

fn native_insert_completion_command(command: &MappableCommand) -> bool {
    matches!(command.name(), "completion")
}

fn is_native_insert_paste_shortcut(key: KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
        } if ch.eq_ignore_ascii_case(&'v') && modifiers == KeyModifiers::CONTROL
    )
}

fn canonicalize_key(key: &mut KeyEvent) {
    if matches!(key.code, KeyCode::Char(_)) {
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
}

fn key_is(key: KeyEvent, ch: char, modifiers: KeyModifiers) -> bool {
    fn normalize(mut ch: char, modifiers: &mut KeyModifiers) -> char {
        if modifiers.contains(KeyModifiers::SHIFT) {
            ch = match ch {
                'a'..='z' => ch.to_ascii_uppercase(),
                '`' => '~',
                '1' => '!',
                '2' => '@',
                '3' => '#',
                '4' => '$',
                '5' => '%',
                '6' => '^',
                '7' => '&',
                '8' => '*',
                '9' => '(',
                '0' => ')',
                '-' => '_',
                '=' => '+',
                '[' => '{',
                ']' => '}',
                '\\' => '|',
                ';' => ':',
                '\'' => '"',
                ',' => '<',
                '.' => '>',
                '/' => '?',
                _ => ch,
            };
            modifiers.remove(KeyModifiers::SHIFT);
        }
        ch
    }

    let KeyCode::Char(actual) = key.code else {
        return false;
    };
    let mut actual_modifiers = key.modifiers;
    let mut expected_modifiers = modifiers;
    normalize(actual, &mut actual_modifiers) == normalize(ch, &mut expected_modifiers)
        && actual_modifiers == expected_modifiers
}

fn is_custom_completion_shortcut(mode: Mode, key: KeyEvent) -> bool {
    mode == Mode::Insert && key_is(key, ' ', KeyModifiers::CONTROL)
}

fn is_custom_code_actions_shortcut(key: KeyEvent) -> bool {
    key_is(key, '.', KeyModifiers::CONTROL)
}

pub fn resolve_fallback_shortcut(
    _mode: Mode,
    key: KeyEvent,
    platform: TargetPlatform,
) -> Option<FallbackShortcut> {
    use crate::types::SemanticShortcutIntent as Intent;
    use EditorSemanticAction as Editor;
    use FallbackShortcut::{Editor as EditorFallback, Workspace};

    let custom = [
        ('r', KeyModifiers::CONTROL, Intent::ShowRunnables),
        (
            'r',
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            Intent::RunNearest,
        ),
        (
            'r',
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            Intent::RunLast,
        ),
        (
            't',
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            Intent::RunFileTests,
        ),
        ('b', KeyModifiers::CONTROL, Intent::ToggleFileTree),
        ('`', KeyModifiers::CONTROL, Intent::ToggleTerminal),
    ];
    if let Some((_, _, intent)) = custom
        .into_iter()
        .find(|(ch, modifiers, _)| key_is(key, *ch, *modifiers))
    {
        return Some(Workspace(intent));
    }
    if is_custom_code_actions_shortcut(key) {
        return Some(Workspace(Intent::ShowCodeActions));
    }

    let primary = if platform == TargetPlatform::MacOS {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    };
    let standard = [
        ('q', primary, Intent::Quit),
        ('o', primary, Intent::OpenFile),
        ('o', primary | KeyModifiers::SHIFT, Intent::OpenDirectory),
        ('s', primary, Intent::Save),
        ('w', primary, Intent::CloseFile),
        ('n', primary, Intent::NewFile),
        ('p', primary, Intent::ShowFileFinder),
        (
            'p',
            primary | KeyModifiers::SHIFT,
            Intent::ShowCommandPrompt,
        ),
        ('b', primary, Intent::ShowBufferPicker),
        ('+', primary | KeyModifiers::SHIFT, Intent::IncreaseFontSize),
        ('=', primary, Intent::IncreaseFontSize),
        ('-', primary, Intent::DecreaseFontSize),
        ('0', primary, Intent::ResetFontSize),
        (',', primary, Intent::OpenSettings),
    ];
    if let Some(intent) = standard
        .into_iter()
        .find_map(|(ch, modifiers, intent)| key_is(key, ch, modifiers).then_some(intent))
    {
        return Some(Workspace(intent));
    }

    let semantic = if platform == TargetPlatform::MacOS {
        [
            ('z', KeyModifiers::SUPER, Editor::Undo),
            ('z', KeyModifiers::SUPER | KeyModifiers::SHIFT, Editor::Redo),
            ('c', KeyModifiers::SUPER, Editor::Copy),
            ('x', KeyModifiers::SUPER, Editor::Cut),
            ('v', KeyModifiers::SUPER, Editor::Paste),
            ('a', KeyModifiers::SUPER, Editor::SelectAll),
            ('f', KeyModifiers::SUPER, Editor::Find),
            ('g', KeyModifiers::SUPER, Editor::FindNext),
            (
                'g',
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
                Editor::FindPrevious,
            ),
            (
                ']',
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
                Editor::NextBuffer,
            ),
            (
                '[',
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
                Editor::PreviousBuffer,
            ),
        ]
        .into_iter()
        .find_map(|(ch, modifiers, action)| key_is(key, ch, modifiers).then_some(action))
    } else {
        [
            ('z', KeyModifiers::CONTROL, Editor::Undo),
            ('y', KeyModifiers::CONTROL, Editor::Redo),
            (
                'z',
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                Editor::Redo,
            ),
            ('c', KeyModifiers::CONTROL, Editor::Copy),
            ('x', KeyModifiers::CONTROL, Editor::Cut),
            ('v', KeyModifiers::CONTROL, Editor::Paste),
            ('a', KeyModifiers::CONTROL, Editor::SelectAll),
            ('f', KeyModifiers::CONTROL, Editor::Find),
        ]
        .into_iter()
        .find_map(|(ch, modifiers, action)| key_is(key, ch, modifiers).then_some(action))
    };
    if let Some(action) = semantic {
        return Some(EditorFallback(action));
    }

    let desktop = matches!(platform, TargetPlatform::Linux | TargetPlatform::Windows);
    match (key.code, key.modifiers) {
        (KeyCode::F(3), KeyModifiers::NONE) if desktop => Some(EditorFallback(Editor::FindNext)),
        (KeyCode::F(3), KeyModifiers::SHIFT) if desktop => {
            Some(EditorFallback(Editor::FindPrevious))
        }
        (KeyCode::Tab, KeyModifiers::CONTROL) if desktop => {
            Some(EditorFallback(Editor::NextBuffer))
        }
        (KeyCode::Tab, modifiers)
            if desktop && modifiers == KeyModifiers::CONTROL | KeyModifiers::SHIFT =>
        {
            Some(EditorFallback(Editor::PreviousBuffer))
        }
        (KeyCode::PageDown, KeyModifiers::CONTROL) if desktop => {
            Some(EditorFallback(Editor::NextBuffer))
        }
        (KeyCode::PageUp, KeyModifiers::CONTROL) if desktop => {
            Some(EditorFallback(Editor::PreviousBuffer))
        }
        (KeyCode::F(4), KeyModifiers::CONTROL) if desktop => Some(Workspace(Intent::CloseFile)),
        (KeyCode::Insert, KeyModifiers::CONTROL) if desktop => Some(EditorFallback(Editor::Copy)),
        (KeyCode::Insert, KeyModifiers::SHIFT) if desktop => Some(EditorFallback(Editor::Paste)),
        (KeyCode::Delete, KeyModifiers::SHIFT) if desktop => Some(EditorFallback(Editor::Cut)),
        _ => None,
    }
}

fn command_count_digit(key: KeyEvent) -> Option<usize> {
    let KeyEvent {
        code: KeyCode::Char(ch),
        modifiers,
    } = key
    else {
        return None;
    };

    if !modifiers.is_empty() {
        return None;
    }

    ch.to_digit(10).map(|digit| digit as usize)
}

fn non_zero_command_count_digit(key: KeyEvent) -> Option<usize> {
    command_count_digit(key).filter(|digit| *digit > 0)
}

fn is_repeat_last_insert_key(key: KeyEvent) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char('.'),
            modifiers
        } if modifiers.is_empty()
    )
}

fn selection_snapshot(
    editor: &Editor,
    doc_id: DocumentId,
    view_id: ViewId,
) -> Option<SelectionSnapshot> {
    let doc = editor.document(doc_id)?;
    let text = doc.text();
    let cursor = doc.selection(view_id).primary().cursor(text.slice(..));
    let line = text.char_to_line(cursor);

    Some(SelectionSnapshot { cursor, line })
}

fn selection_changed(before: Option<SelectionSnapshot>, after: Option<SelectionSnapshot>) -> bool {
    before
        .zip(after)
        .is_some_and(|(before, after)| before != after)
}

fn log_completion_key_context(editor: &Editor, key: KeyEvent) {
    if !key.modifiers.contains(KeyModifiers::CONTROL) || !matches!(key.code, KeyCode::Char('x')) {
        return;
    }

    let focused_view_id = editor.tree.focus;
    let focused_doc_id = editor.tree.try_get(focused_view_id).map(|view| view.doc);
    let doc = focused_doc_id.and_then(|doc_id| editor.document(doc_id));
    let language_server_count = editor.language_servers.incoming.len();
    let file_path = doc
        .and_then(|doc| doc.path())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "no file".to_string());
    let language = doc
        .and_then(|doc| doc.language_config())
        .map(|language| language.language_id.clone())
        .unwrap_or_else(|| "no language".to_string());

    info!(
        editor_mode = ?editor.mode(),
        language_servers = language_server_count,
        file_path = %file_path,
        language = %language,
        "DEBUG: CTRL-X received - editor state for completion"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use helix_core::{Selection, Transaction, syntax};
    use helix_view::{
        clipboard::ClipboardProvider, editor::Action, editor::Config, graphics::Rect,
        handlers::Handlers, theme,
    };

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_links_tx, _) = tokio::sync::mpsc::channel(1);
        let (pull_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
        let (pull_all_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
        let (code_action_hint_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            document_links: doc_links_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
            pull_diagnostics: pull_diagnostics_tx,
            pull_all_documents_diagnostics: pull_all_diagnostics_tx,
            code_action_hint: code_action_hint_tx,
        }
    }

    fn canonicalize_for_assertion(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn test_editor_with_text(text: &str) -> Editor {
        test_editor_with_text_and_config(
            text,
            Config {
                default_line_ending: helix_view::editor::LineEndingConfig::LF,
                ..Config::default()
            },
        )
    }

    fn test_editor_with_text_and_config(text: &str, editor_config: Config) -> Editor {
        let config = Arc::new(ArcSwap::new(Arc::new(editor_config)));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
            helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        editor
    }

    fn numbered_lines(count: usize) -> String {
        (0..count).map(|line| format!("{line}\n")).collect()
    }

    fn set_test_cursor(editor: &mut Editor, cursor: usize) {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document_mut(doc_id).unwrap();
        doc.set_selection(view_id, Selection::point(cursor));
    }

    fn set_test_clipboard(editor: &mut Editor, value: &str) {
        editor.registers.write('+', vec![value.to_owned()]).unwrap();
    }

    fn set_test_cursor_line(editor: &mut Editor, line: usize) {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document_mut(doc_id).unwrap();
        let cursor = doc.text().line_to_char(line);
        doc.set_selection(view_id, Selection::point(cursor));
    }

    fn focused_cursor_line(editor: &Editor) -> usize {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document(doc_id).unwrap();
        let text = doc.text().slice(..);
        let cursor = doc.selection(view_id).primary().cursor(text);
        helix_core::coords_at_pos(text, cursor).row
    }

    fn focused_horizontal_offset(editor: &Editor) -> usize {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        editor
            .document(doc_id)
            .unwrap()
            .view_offset(view_id)
            .horizontal_offset
    }

    fn focused_align_middle_offset(editor: &Editor) -> usize {
        let view_id = editor.tree.focus;
        let view = editor.tree.try_get(view_id).unwrap();
        let doc = editor.document(view.doc).unwrap();
        let text = doc.text().slice(..);
        let cursor = doc.selection(view_id).primary().cursor(text);
        let text_format = doc.text_format(view.inner_width(doc), None);
        let cursor_position = visual_offset_from_block(
            text,
            doc.view_offset(view_id).anchor,
            cursor,
            &text_format,
            &view.text_annotations(doc, None),
        )
        .0;

        cursor_position
            .col
            .saturating_sub((view.inner_area(doc).width as usize) / 2)
    }

    fn set_test_selection(editor: &mut Editor, from: usize, to: usize) {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document_mut(doc_id).unwrap();
        doc.set_selection(view_id, Selection::single(from, to));
    }

    fn focused_document_text(editor: &Editor) -> String {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        editor.document(doc_id).unwrap().text().to_string()
    }

    fn focused_selection_fragments(editor: &Editor) -> Vec<String> {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document(doc_id).unwrap();
        doc.selection(view_id)
            .fragments(doc.text().slice(..))
            .map(|fragment| fragment.into_owned())
            .collect()
    }

    fn plain_char_key(ch: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::empty(),
        }
    }

    fn translated_gpui_key(key: &str) -> KeyEvent {
        crate::utils::translate_key(&gpui::Keystroke {
            key: key.into(),
            modifiers: gpui::Modifiers::default(),
            key_char: None,
        })
    }

    fn handle_key_str(
        bridge: &mut EditorInputBridge,
        editor: &mut Editor,
        compositor: &mut Compositor,
        jobs: &mut Jobs,
        key: &str,
    ) -> EditorInputOutcome {
        bridge.handle_key(KeyEvent::from_str(key).unwrap(), compositor, editor, jobs)
    }

    fn hosted_test_bridge(commands: &[&str]) -> EditorInputBridge {
        use helix_term::config::Config as HelixConfig;
        use helix_term::keymap::{KeyTrie, KeyTrieNode, merge_keys};
        let mut config = HelixConfig::default();
        let mut node = indexmap::IndexMap::new();
        node.insert(
            KeyEvent::from_str("F12").unwrap(),
            KeyTrie::Sequence(
                commands
                    .iter()
                    .map(|command| command.parse().unwrap())
                    .collect(),
            ),
        );
        merge_keys(
            &mut config.keys,
            std::collections::HashMap::from([(
                Mode::Normal,
                KeyTrie::Node(KeyTrieNode::new("Hosted test", node)),
            )]),
        );
        let mut bridge = EditorInputBridge::new(Keymaps::new(Box::new(
            arc_swap::access::Constant(config.keys),
        )));
        bridge.enable_hosted_commands();
        bridge
    }

    fn workspace_command(outcome: EditorInputOutcome) -> WorkspaceCommandRequest {
        match outcome.workspace_requested {
            Some(NativeWorkspaceRequest::Command(request)) => request,
            other => panic!("expected typed workspace command, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hosted_mapping_pauses_split_focus_edit_and_preserves_count_register() {
        use workspace_commands::{WorkspaceCommand as Host, WorkspaceDirection as Direction};
        let mut editor = test_editor_with_text("alpha");
        let first_slot = editor.tree.focus;
        let first_doc = editor.tree.get(first_slot).doc;
        let second_doc = editor.new_file(Action::VerticalSplit);
        let second_slot = editor.tree.focus;
        let doc = editor.document_mut(second_doc).unwrap();
        doc.apply(
            &Transaction::change(doc.text(), [(0, 0, Some("BRAVO".into()))].into_iter()),
            second_slot,
        );
        set_test_cursor(&mut editor, 0);
        editor.focus(first_slot);
        let mut bridge = hosted_test_bridge(&[
            "vsplit",
            "jump_view_left",
            "move_char_right",
            "delete_selection",
        ]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        // Real Helix count and register prefixes, not hand-assigned command state.
        for key in ["\"", "a", "2"] {
            bridge.handle_hosted_key(
                key.parse().unwrap(),
                Some(first_slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
        }
        let split = workspace_command(bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(first_slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(
            split,
            WorkspaceCommandRequest {
                command: Host::Split {
                    action: NativeFileOpenAction::VerticalSplit,
                    new_document: false
                },
                count: 2
            }
        );
        assert_eq!(
            editor.tree.views().count(),
            2,
            "host owns tiling; split has not run"
        );
        // Input arriving before host dispatch must not edit stale focus or
        // consume the suspended mapping. The original tail still resumes below.
        let blocked = bridge.handle_hosted_key(
            "d".parse().unwrap(),
            Some(first_slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(blocked.workspace_requested.is_none());
        bridge.insert_text("BAD", None, &mut editor, &mut jobs);
        assert_eq!(focused_document_text(&editor), "alpha\n");

        // A split can select non-editor content. Explicit host navigation still runs.
        let focus = workspace_command(bridge.resume_workspace_commands(
            None,
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(focus.command, Host::Focus(Direction::Left));
        assert_eq!(focus.count, 2);
        editor.focus(second_slot);
        let edited = bridge.resume_workspace_commands(
            Some(second_slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(edited.workspace_requested.is_none());
        assert_eq!(
            editor.document(first_doc).unwrap().text().to_string(),
            "alpha\n"
        );
        assert_eq!(
            editor.document(second_doc).unwrap().text().to_string(),
            "BRVO\n"
        );
        let register = editor
            .registers
            .read('a', &editor)
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(
            register,
            ["A"],
            "the saved register and count apply after host focus"
        );
    }

    #[tokio::test]
    async fn hosted_non_editor_continuation_and_failed_host_are_cancelled() {
        for failed_host in [false, true] {
            let mut editor = test_editor_with_text("hidden");
            let slot = editor.tree.focus;
            let mut bridge =
                hosted_test_bridge(&["jump_view_down", "delete_selection", "jump_view_up"]);
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            workspace_command(bridge.handle_hosted_key(
                "F12".parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            ));
            if failed_host {
                bridge.cancel_workspace_commands(&mut editor);
            }
            let result = bridge.resume_workspace_commands(
                if failed_host { Some(slot) } else { None },
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert!(result.workspace_requested.is_none());
            assert_eq!(focused_document_text(&editor), "hidden\n");
            if !failed_host {
                assert!(result.focused_doc_id.is_none());
                assert!(
                    editor
                        .status_msg
                        .as_ref()
                        .unwrap()
                        .0
                        .contains("focused document")
                );
                bridge.insert_text("BAD", None, &mut editor, &mut jobs);
                bridge.handle_semantic_action(
                    EditorSemanticAction::Cut,
                    &mut compositor,
                    &mut editor,
                    &mut jobs,
                );
                assert_eq!(focused_document_text(&editor), "hidden\n");
            }
            // A late resume cannot resurrect the rejected tail.
            assert!(
                bridge
                    .resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs)
                    .workspace_requested
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn hosted_macro_keys_survive_pause_and_cancel_clears_recursion_guard() {
        let mut editor = test_editor_with_text("abcdef");
        let slot = editor.tree.focus;
        set_test_cursor(&mut editor, 0);
        let mut bridge = hosted_test_bridge(&["@<C-w>vld"]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        workspace_command(bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(focused_document_text(&editor), "abcdef\n");
        assert_eq!(editor.macro_replaying, ['@']);
        bridge.resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs);
        assert_eq!(focused_document_text(&editor), "acdef\n");
        assert!(editor.macro_replaying.is_empty());

        workspace_command(bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        bridge.cancel_workspace_commands(&mut editor);
        assert!(editor.macro_replaying.is_empty());
        assert!(
            bridge
                .resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs)
                .workspace_requested
                .is_none()
        );
        assert_eq!(focused_document_text(&editor), "acdef\n");
    }

    #[tokio::test]
    async fn hosted_register_macro_count_retains_each_pause_and_rejects_non_editor_tail() {
        let mut editor = test_editor_with_text("abcdef");
        let slot = editor.tree.focus;
        set_test_cursor(&mut editor, 0);
        editor
            .registers
            .write('b', vec!["<C-w>vld".into()])
            .unwrap();
        let mut bridge = hosted_test_bridge(&["replay_macro"]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        for key in ["\"", "b", "2"] {
            bridge.handle_hosted_key(
                key.parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
        }
        workspace_command(bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        workspace_command(bridge.resume_workspace_commands(
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(focused_document_text(&editor), "acdef\n");
        assert_eq!(editor.macro_replaying, ['b']);
        bridge.resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs);
        assert_eq!(focused_document_text(&editor), "acef\n");
        assert!(editor.macro_replaying.is_empty());

        for key in ["\"", "b"] {
            bridge.handle_hosted_key(
                key.parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
        }
        workspace_command(bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        bridge.resume_workspace_commands(None, &mut compositor, &mut editor, &mut jobs);
        assert_eq!(focused_document_text(&editor), "acef\n");
        assert!(editor.macro_replaying.is_empty());
    }

    #[tokio::test]
    async fn hosted_macro_prompt_keys_stay_in_ui_before_host_and_document_continuation() {
        let mut editor = test_editor_with_text("alpha");
        let first_slot = editor.tree.focus;
        let first_doc = editor.tree.get(first_slot).doc;
        let second_doc = editor.new_file(Action::VerticalSplit);
        let second_slot = editor.tree.focus;
        let doc = editor.document_mut(second_doc).unwrap();
        doc.apply(
            &Transaction::change(doc.text(), [(0, 0, Some("BRAVO".into()))].into_iter()),
            second_slot,
        );
        set_test_cursor(&mut editor, 0);
        editor.focus(first_slot);
        let mut bridge = hosted_test_bridge(&["@:vs<ret>ld"]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let prompt = bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(first_slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert_eq!(prompt.prompt_requested, Some(NativePromptRequest::Command));
        assert!(!bridge.awaiting_workspace_command());
        // The native prompt consumes these keys, stopping at submission. It
        // passes its submitted command through the same typable adapter.
        for key in ["v", "s", "ret"] {
            assert_eq!(
                bridge.take_suspended_replay_key(),
                Some(key.parse().unwrap())
            );
        }
        workspace_command(
            bridge
                .submit_command_continuation(
                    prompt.continuation.unwrap().token,
                    "vs",
                    Some(first_slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs,
                )
                .unwrap(),
        );
        assert!(bridge.awaiting_workspace_command());
        assert!(bridge.take_suspended_replay_key().is_none());
        editor.focus(second_slot);
        bridge.resume_workspace_commands(
            Some(second_slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(!bridge.awaiting_workspace_command());
        assert_eq!(
            editor.document(first_doc).unwrap().text().to_string(),
            "alpha\n"
        );
        assert_eq!(
            editor.document(second_doc).unwrap().text().to_string(),
            "BAVO\n"
        );
        assert!(editor.macro_replaying.is_empty());
        assert!(bridge.take_suspended_replay_key().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hosted_buffer_close_yields_before_replacement_document_edit() {
        let temp = tempfile::tempdir().unwrap();
        let root = helix_stdx::path::canonicalize(temp.path());
        let mut editor = test_editor_with_text("BRAVO");
        let slot = editor.tree.focus;
        let document_b = editor.tree.get(slot).doc;
        editor
            .document_mut(document_b)
            .unwrap()
            .set_path(Some(&root.join("B")));
        set_test_cursor(&mut editor, 0);
        let document_a = editor.new_file(Action::Replace);
        let path_a = root.join("A");
        editor
            .document_mut(document_a)
            .unwrap()
            .set_path(Some(&path_a));
        editor.enable_hosted();
        let mut bridge = hosted_test_bridge(&[
            &format!(":bclose! '{}'", path_a.display()),
            "delete_selection",
        ]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let closed = bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(editor.document(document_a).is_none());
        assert_eq!(editor.tree.get(slot).doc, document_b);
        assert_eq!(
            editor.document(document_b).unwrap().text().to_string(),
            "BRAVO\n",
            "backend replacement must reconcile with host content before editing"
        );
        assert!(closed.workspace_requested.is_some());
        let token = closed.continuation.unwrap().token;
        assert!(bridge.begin_command_completion(token, &mut editor));
        assert!(editor.drain_hosted_effects().iter().any(|effect|
            matches!(effect, helix_view::editor::hosted::HostedEffect::DocumentClosed { document }
                if *document == document_a)));
        // Host MRU selected non-editor T, not Helix's fallback buffer B.
        editor.project_hosted_focus(None).unwrap();
        bridge
            .resume_command_continuation(token, None, &mut compositor, &mut editor, &mut jobs)
            .unwrap();
        assert_eq!(
            editor.document(document_b).unwrap().text().to_string(),
            "BRAVO\n"
        );
    }

    #[tokio::test]
    async fn hosted_external_request_tail_never_leaks_into_new_editor_input() {
        for command in ["goto_window_bottom", "goto_definition"] {
            let mut editor = test_editor_with_text("alpha");
            let first_slot = editor.tree.focus;
            let mut bridge = hosted_test_bridge(&[command, "delete_selection"]);
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            let request = bridge.handle_hosted_key(
                "F12".parse().unwrap(),
                Some(first_slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert!(
                request.viewport_cursor_requested.is_some()
                    || request.lsp_navigation_requested.is_some()
            );
            let document_c = editor.new_file(Action::VerticalSplit);
            let slot_c = editor.tree.focus;
            let doc = editor.document_mut(document_c).unwrap();
            doc.apply(
                &Transaction::change(doc.text(), [(0, 0, Some("charlie".into()))].into_iter()),
                slot_c,
            );
            set_test_cursor(&mut editor, 3);
            bridge.handle_hosted_key(
                "h".parse().unwrap(),
                Some(slot_c),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert_eq!(
                editor.document(document_c).unwrap().text().to_string(),
                "charlie\n",
                "{command}"
            );
        }
    }

    #[tokio::test]
    async fn hosted_viewport_completion_runs_tail_immediately_and_only_once() {
        let mut editor = test_editor_with_text("aa\nbb\nCC");
        let slot = editor.tree.focus;
        let mut bridge = hosted_test_bridge(&["goto_window_bottom", "delete_selection"]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let request = bridge.handle_hosted_key(
            "F12".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        let continuation = request.continuation.unwrap();
        assert_eq!(continuation.kind, EditorContinuationKind::Viewport);
        assert_eq!(focused_document_text(&editor), "aa\nbb\nCC\n");
        // Legacy/unmatched callbacks cannot acknowledge viewport work.
        bridge.resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs);
        assert_eq!(focused_document_text(&editor), "aa\nbb\nCC\n");
        assert!(
            bridge
                .resume_command_continuation(
                    continuation.token,
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs
                )
                .is_none()
        );
        assert!(bridge.begin_command_completion(continuation.token, &mut editor));
        assert!(!bridge.begin_command_completion(continuation.token, &mut editor));
        apply_native_viewport_cursor_request(&mut editor, slot, 2).unwrap();
        bridge
            .resume_command_continuation(
                continuation.token,
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            )
            .unwrap();
        assert_eq!(focused_document_text(&editor), "aa\nbb\nC\n");
        assert!(
            bridge
                .resume_command_continuation(
                    continuation.token,
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs
                )
                .is_none()
        );
        assert!(!bridge.cancel_command_continuation(continuation.token, &mut editor));
    }

    #[tokio::test]
    async fn hosted_fresh_command_discards_abandoned_ui_tail_but_matching_submit_keeps_it() {
        for matching_submit in [false, true] {
            let mut editor = test_editor_with_text("alpha");
            let slot = editor.tree.focus;
            set_test_cursor(&mut editor, 0);
            let mut bridge = hosted_test_bridge(&[
                if matching_submit {
                    "command_mode"
                } else {
                    "search"
                },
                "delete_selection",
            ]);
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            let prompt = bridge.handle_hosted_key(
                "F12".parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            let token = prompt.continuation.unwrap().token;
            if matching_submit {
                bridge
                    .submit_command_continuation(
                        token,
                        "move_char_right",
                        Some(slot),
                        &mut compositor,
                        &mut editor,
                        &mut jobs,
                    )
                    .unwrap();
            } else {
                // A global command prompt replaces the unsubmitted search.
                // Its tokenless submission must not adopt the search's tail.
                bridge.execute_hosted_command_line(
                    "move_char_right",
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs,
                );
            }
            assert_eq!(
                focused_document_text(&editor),
                if matching_submit { "apha\n" } else { "alpha\n" }
            );
            assert!(!bridge.begin_command_completion(token, &mut editor));
        }
    }

    #[tokio::test]
    async fn hosted_document_context_rejects_aba_but_allows_claimed_result_projection() {
        for case in [0, 1, 2] {
            let mut editor = test_editor_with_text("alpha");
            let slot = editor.tree.focus;
            let document_a = editor.tree.get(slot).doc;
            let document_b = editor.new_file(Action::Replace);
            let doc = editor.document_mut(document_b).unwrap();
            doc.apply(
                &Transaction::change(doc.text(), [(0, 0, Some("BRAVO".into()))].into_iter()),
                slot,
            );
            editor.switch(document_a, Action::Replace);
            set_test_cursor(&mut editor, 0);
            let mut bridge = hosted_test_bridge(&["goto_definition", "delete_selection"]);
            bridge.set_hosted_active_context(Some(slot), Some(document_a));
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            let token = bridge
                .handle_hosted_key(
                    "F12".parse().unwrap(),
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs,
                )
                .continuation
                .unwrap()
                .token;
            // Reprojecting the unchanged origin must not invalidate it.
            bridge.set_hosted_active_context(Some(slot), Some(document_a));
            if case == 2 {
                assert!(bridge.begin_command_completion(token, &mut editor));
            }
            if case != 0 {
                editor.switch(document_b, Action::Replace);
                bridge.set_hosted_active_context(Some(slot), Some(document_b));
                if case == 1 {
                    // No editor key occurs between the two tab activations.
                    editor.switch(document_a, Action::Replace);
                    bridge.set_hosted_active_context(Some(slot), Some(document_a));
                } else {
                    // A claimed navigation applies and projects its own result.
                    set_test_cursor(&mut editor, 0);
                }
            }
            if case != 2 {
                assert_eq!(
                    bridge.begin_command_completion(token, &mut editor),
                    case == 0
                );
            }
            let resumed = bridge.resume_command_continuation(
                token,
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert_eq!(resumed.is_some(), case != 1);
            assert_eq!(
                editor.document(document_a).unwrap().text().to_string(),
                if case == 0 { "lpha\n" } else { "alpha\n" }
            );
            assert_eq!(
                editor.document(document_b).unwrap().text().to_string(),
                if case == 2 { "RAVO\n" } else { "BRAVO\n" }
            );
        }
    }

    #[tokio::test]
    async fn hosted_tokens_reject_stale_results_and_rotate_ui_handoffs() {
        let mut editor = test_editor_with_text("alpha");
        let slot = editor.tree.focus;
        let mut bridge = hosted_test_bridge(&["goto_definition", "delete_selection"]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let first = bridge
            .handle_hosted_key(
                "F12".parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            )
            .continuation
            .unwrap();
        assert!(bridge.command_continuation_is_current(first.token, &editor));
        bridge.handle_hosted_key(
            "h".parse().unwrap(),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(!bridge.command_continuation_is_current(first.token, &editor));
        let second = bridge
            .handle_hosted_key(
                "F12".parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            )
            .continuation
            .unwrap();
        assert_ne!(first.token, second.token);
        assert!(!bridge.cancel_command_continuation(first.token, &mut editor));
        assert!(!bridge.begin_command_completion(first.token, &mut editor));
        assert!(bridge.begin_command_completion(second.token, &mut editor));
        let ui = bridge
            .acknowledge_command_ui(second.token, Some(slot), &editor)
            .unwrap();
        assert_ne!(ui.token, second.token);
        assert_eq!(ui.kind, EditorContinuationKind::Ui);
        assert!(!bridge.cancel_command_continuation(second.token, &mut editor));
        assert!(
            bridge
                .resume_command_continuation(
                    second.token,
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs
                )
                .is_none()
        );
        let submitted = bridge
            .submit_command_continuation(
                ui.token,
                "vs",
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            )
            .unwrap();
        let split = submitted.continuation.unwrap();
        assert_eq!(split.kind, EditorContinuationKind::Workspace);
        assert!(!bridge.cancel_command_continuation(ui.token, &mut editor));
        assert_eq!(focused_document_text(&editor), "alpha\n");
        assert!(bridge.cancel_command_continuation(split.token, &mut editor));

        // A buffer/tab switch can reuse the very same ViewId. Slot identity
        // alone must not let an old definition result mutate its replacement.
        let pending = bridge
            .handle_hosted_key(
                "F12".parse().unwrap(),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            )
            .continuation
            .unwrap();
        editor.new_file(Action::Replace);
        bridge.set_hosted_active_slot(Some(slot));
        assert!(!bridge.command_continuation_is_current(pending.token, &editor));
        assert!(!bridge.begin_command_completion(pending.token, &mut editor));
        assert!(
            bridge
                .resume_command_continuation(
                    pending.token,
                    Some(slot),
                    &mut compositor,
                    &mut editor,
                    &mut jobs
                )
                .is_none()
        );
    }

    #[tokio::test]
    async fn hosted_directory_picker_acknowledgement_keeps_replay_out_of_document() {
        for cancelled in [false, true] {
            let mut editor = test_editor_with_text("alpha");
            let first_slot = editor.tree.focus;
            let first_doc = editor.tree.get(first_slot).doc;
            let second_doc = editor.new_file(Action::VerticalSplit);
            let second_slot = editor.tree.focus;
            let doc = editor.document_mut(second_doc).unwrap();
            doc.apply(
                &Transaction::change(doc.text(), [(0, 0, Some("BRAVO".into()))].into_iter()),
                second_slot,
            );
            set_test_cursor(&mut editor, 0);
            editor.focus(first_slot);
            let mut bridge = hosted_test_bridge(&["@:open<space>.<ret>needle<ret>ld"]);
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            let prompt = bridge.handle_hosted_key(
                "F12".parse().unwrap(),
                Some(first_slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert_eq!(prompt.prompt_requested, Some(NativePromptRequest::Command));
            for key in ["o", "p", "e", "n", "space", ".", "ret"] {
                assert_eq!(
                    bridge.take_suspended_replay_key(),
                    Some(key.parse().unwrap())
                );
            }
            let open = workspace_command(
                bridge
                    .submit_command_continuation(
                        prompt.continuation.unwrap().token,
                        "open .",
                        Some(first_slot),
                        &mut compositor,
                        &mut editor,
                        &mut jobs,
                    )
                    .unwrap(),
            );
            assert_eq!(
                open.command,
                workspace_commands::WorkspaceCommand::Open {
                    paths: vec![".".into()],
                    action: NativeFileOpenAction::Replace,
                }
            );
            assert!(bridge.awaiting_workspace_command());
            assert!(bridge.take_suspended_replay_key().is_none());

            // Directory-open launches the native picker instead of completing
            // the document open. Acknowledgement must neither edit nor consume
            // the first picker key; it only enables replay through the UI.
            bridge.acknowledge_workspace_ui(Some(first_slot));
            assert!(!bridge.awaiting_workspace_command());
            assert_eq!(focused_document_text(&editor), "alpha\n");
            for key in ["n", "e", "e", "d", "l", "e", "ret"] {
                assert_eq!(
                    bridge.take_suspended_replay_key(),
                    Some(key.parse().unwrap())
                );
            }
            assert_eq!(focused_document_text(&editor), "alpha\n");
            if cancelled {
                bridge.cancel_workspace_commands(&mut editor);
                assert!(bridge.take_suspended_replay_key().is_none());
            }
            editor.focus(second_slot);
            bridge.resume_workspace_commands(
                Some(second_slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert_eq!(
                editor.document(first_doc).unwrap().text().to_string(),
                "alpha\n"
            );
            assert_eq!(
                editor.document(second_doc).unwrap().text().to_string(),
                if cancelled { "BRAVO\n" } else { "BAVO\n" },
            );
            assert!(editor.macro_replaying.is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hosted_typable_aliases_arguments_and_static_fallback_share_adapter() {
        use workspace_commands::{WorkspaceCommand as Host, WorkspaceDirection as Direction};
        let mut editor = test_editor_with_text("41");
        set_test_cursor(&mut editor, 0);
        let slot = editor.tree.focus;
        let mut bridge = hosted_test_bridge(&[]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        for (line, expected) in [
            (
                "vs 'a b.rs' other.rs:17",
                Host::Open {
                    paths: vec!["a b.rs".into(), "other.rs:17".into()],
                    action: NativeFileOpenAction::VerticalSplit,
                },
            ),
            (
                "hnew",
                Host::Split {
                    action: NativeFileOpenAction::HorizontalSplit,
                    new_document: true,
                },
            ),
            ("swap_view_up", Host::Swap(Direction::Up)),
        ] {
            let result = workspace_command(bridge.execute_hosted_command_line(
                line,
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            ));
            assert_eq!(result.command, expected);
            assert_eq!(editor.tree.views().count(), 1);
            bridge.resume_workspace_commands(Some(slot), &mut compositor, &mut editor, &mut jobs);
        }
        for line in [
            "q unexpected",
            "vnew unexpected",
            "swap_view_up unexpected",
            "vs --bogus",
            "vs 'unterminated",
        ] {
            editor.status_msg = None;
            let result = bridge.execute_hosted_command_line(
                line,
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert!(result.workspace_requested.is_none(), "{line}");
            assert!(
                editor.status_msg.is_some(),
                "{line} must report Helix's argument error"
            );
            assert_eq!(focused_document_text(&editor), "41\n");
        }
        let q = workspace_command(bridge.execute_hosted_command_line(
            "q!",
            None,
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(
            q.command,
            Host::ClosePane {
                force: true,
                quit: true
            }
        );
        bridge.cancel_workspace_commands(&mut editor);
        set_test_cursor(&mut editor, 1);
        let result = bridge.execute_hosted_command_line(
            "increment",
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(result.workspace_requested.is_none());
        assert_eq!(
            focused_document_text(&editor),
            "42\n",
            "static command-line fallback remains executable"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hosted_write_quit_writes_and_flushes_before_close_and_preserves_errors() {
        use workspace_commands::WorkspaceCommand as Host;
        let mut editor = test_editor_with_text("saved before close");
        let slot = editor.tree.focus;
        let mut bridge = hosted_test_bridge(&[]);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        assert!(workspace_commands::validate_workspace_quit(&editor, false).is_err());
        assert!(workspace_commands::validate_workspace_quit(&editor, true).is_ok());
        let scratch = bridge.execute_hosted_command_line(
            "wq",
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(
            scratch.workspace_requested.is_none(),
            "unnamed scratch cannot be written or closed"
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("saved file.txt");
        let outcome = bridge.execute_hosted_command_line(
            &format!("wq --no-format --no-code-actions '{}'", path.display()),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert_eq!(
            workspace_command(outcome).command,
            Host::ClosePane {
                force: false,
                quit: true
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "saved before close\n"
        );
        assert_eq!(editor.tree.views().count(), 1);
        assert!(workspace_commands::validate_workspace_quit(&editor, false).is_ok());
        // Keep the pane for the remaining write-error controls.
        bridge.cancel_workspace_commands(&mut editor);
        let invalid = temp.path().join("missing/child.txt");
        let outcome = bridge.execute_hosted_command_line(
            &format!("wq '{}'", invalid.display()),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(outcome.workspace_requested.is_none());
        assert!(editor.status_msg.as_ref().unwrap().0.contains("write-quit"));
        assert!(!invalid.exists());
        assert_eq!(editor.tree.views().count(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let blocked = temp.path().join("blocked");
            std::fs::create_dir(&blocked).unwrap();
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();
            let outcome = bridge.execute_hosted_command_line(
                &format!("wq '{}'", blocked.join("saved.txt").display()),
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                outcome.workspace_requested.is_none(),
                "failed disk write must not yield host close"
            );
        }
        let forced = bridge.execute_hosted_command_line(
            &format!("wq! --no-format --no-code-actions '{}'", invalid.display()),
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert_eq!(
            workspace_command(forced).command,
            Host::ClosePane {
                force: true,
                quit: true
            }
        );
        assert_eq!(
            std::fs::read_to_string(&invalid).unwrap(),
            "saved before close\n"
        );
        bridge.cancel_workspace_commands(&mut editor);

        // A remote/custom save handler can reject a write synchronously.
        // Helix's forced write-all-quit intentionally proceeds in this case,
        // unlike write-quit! and ordinary write-all-quit.
        struct RejectSave;
        impl helix_view::editor::DocumentSaveHandler for RejectSave {
            fn save(
                &self,
                _doc: &mut helix_view::Document,
                _path: Option<PathBuf>,
                _force: bool,
            ) -> Option<anyhow::Result<helix_view::document::DocumentSavedEventFuture>>
            {
                Some(Err(anyhow::anyhow!("storage rejected save")))
            }
        }
        let doc_id = editor.tree.get(slot).doc;
        let doc = editor.document_mut(doc_id).unwrap();
        doc.set_path(Some(&invalid));
        doc.apply(
            &Transaction::change(doc.text(), [(0, 0, Some("changed".into()))].into_iter()),
            slot,
        );
        assert!(doc.is_modified());
        editor.set_save_handler(Some(Arc::new(RejectSave)));
        for line in [
            "wq! --no-format --no-code-actions",
            "wqa --no-format --no-code-actions",
        ] {
            let rejected = bridge.execute_hosted_command_line(
                line,
                Some(slot),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert!(rejected.workspace_requested.is_none(), "{line}");
            assert!(
                editor
                    .status_msg
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("storage rejected save"),
                "{line}: {:?}",
                editor.status_msg
            );
        }
        let forced_all = bridge.execute_hosted_command_line(
            "wqa! --no-format --no-code-actions",
            Some(slot),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert_eq!(
            workspace_command(forced_all).command,
            Host::QuitWorkspace { force: true }
        );
    }

    #[tokio::test]
    async fn hosted_opt_in_preserves_standalone_splits_and_zero_slot_host_commands() {
        use workspace_commands::{WorkspaceCommand as Host, WorkspaceDirection as Direction};
        let mut editor = test_editor_with_text("real document\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let mut standalone = EditorInputBridge::new(Keymaps::default());
        let result = standalone.execute_hosted_command_line(
            "vs",
            None,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(result.workspace_requested.is_none());
        assert_eq!(editor.tree.views().count(), 2);

        let slots: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
        for slot in slots {
            editor.close(slot);
        }
        assert_eq!(editor.tree.views().count(), 0);
        let documents_before = editor.documents().count();
        standalone.enable_hosted_commands();
        let result = workspace_command(standalone.execute_hosted_command_line(
            "jump_view_down",
            None,
            &mut compositor,
            &mut editor,
            &mut jobs,
        ));
        assert_eq!(result.command, Host::Focus(Direction::Down));
        assert_eq!(
            editor.documents().count(),
            documents_before,
            "no fake backing document for host-only content"
        );
        assert_eq!(editor.tree.views().count(), 0);
    }

    fn reset_diff_change_keymaps() -> Keymaps {
        nucleotide_keymaps(MappableCommand::Typable {
            name: "toggle-file-tree".into(),
            args: String::new(),
            doc: "Toggle file tree".into(),
        })
    }

    fn nucleotide_keymaps(toggle_file_tree: MappableCommand) -> Keymaps {
        use helix_term::config::Config as HelixConfig;
        use helix_term::keymap::{KeyTrie, KeyTrieNode, merge_keys};
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let space = KeyEvent::from_str("space").unwrap();
        let v = KeyEvent::from_str("v").unwrap();
        let r = KeyEvent::from_str("r").unwrap();
        let t = KeyEvent::from_str("t").unwrap();

        let mut vcs_node = IndexMap::new();
        vcs_node.insert(
            r,
            KeyTrie::MappableCommand(MappableCommand::from_str(":reset-diff-change").unwrap()),
        );

        let mut space_node = IndexMap::new();
        space_node.insert(v, KeyTrie::Node(KeyTrieNode::new("VCS", vcs_node)));
        space_node.insert(t, KeyTrie::MappableCommand(toggle_file_tree));

        let mut normal_node = IndexMap::new();
        normal_node.insert(space, KeyTrie::Node(KeyTrieNode::new("Space", space_node)));

        let mut config = HelixConfig::default();
        merge_keys(
            &mut config.keys,
            HashMap::from([(
                Mode::Normal,
                KeyTrie::Node(KeyTrieNode::new("Normal mode", normal_node)),
            )]),
        );

        Keymaps::new(Box::new(arc_swap::access::Constant(config.keys)))
    }

    fn disposition_test_keymaps(mode: Mode) -> Keymaps {
        use helix_term::config::Config as HelixConfig;
        use helix_term::keymap::{KeyTrie, KeyTrieNode, merge_keys};
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let unsupported = MappableCommand::from_str(":quit").unwrap();
        let mut prefix_node = IndexMap::new();
        prefix_node.insert(
            KeyEvent::from_str("y").unwrap(),
            KeyTrie::MappableCommand(unsupported.clone()),
        );
        let mut mode_node = IndexMap::new();
        mode_node.insert(
            KeyEvent::from_str("x").unwrap(),
            KeyTrie::Node(KeyTrieNode::new("Prefix", prefix_node)),
        );
        mode_node.insert(
            KeyEvent::from_str("C-q").unwrap(),
            KeyTrie::MappableCommand(unsupported),
        );

        let mut config = HelixConfig::default();
        merge_keys(
            &mut config.keys,
            HashMap::from([(mode, KeyTrie::Node(KeyTrieNode::new("Test", mode_node)))]),
        );
        Keymaps::new(Box::new(arc_swap::access::Constant(config.keys)))
    }

    fn handle_native_test_key(
        input: &mut NativeCommandInput,
        editor: &mut Editor,
        key: &str,
    ) -> NativeInputResult {
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        input.handle_key(
            KeyEvent::from_str(key).unwrap(),
            &mut compositor,
            editor,
            &mut jobs,
        )
    }

    fn assert_unhandled_disposition(result: NativeInputResult, expected: UnhandledDisposition) {
        assert!(
            matches!(result, NativeInputResult::Unhandled { disposition, .. } if disposition == expected),
            "expected {expected:?} disposition"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn command_keymap_dispositions_preserve_helix_ownership() {
        let mut editor = test_editor_with_text("");
        let mut input = NativeCommandInput::new(disposition_test_keymaps(Mode::Normal));

        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "C-q"),
            UnhandledDisposition::ReservedUnsupported,
        );
        assert!(matches!(
            handle_native_test_key(&mut input, &mut editor, "x"),
            NativeInputResult::Handled { .. }
        ));
        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "z"),
            UnhandledDisposition::Cancelled,
        );
        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "F12"),
            UnhandledDisposition::RootNotFound,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn normalized_shifted_user_mapping_precedes_fallback() {
        use helix_term::config::Config as HelixConfig;
        use helix_term::keymap::{KeyTrie, KeyTrieNode, merge_keys};
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let mut mode_node = IndexMap::new();
        mode_node.insert(
            KeyEvent::from_str("C-R").unwrap(),
            KeyTrie::MappableCommand(MappableCommand::from_str(":quit").unwrap()),
        );
        let mut config = HelixConfig::default();
        merge_keys(
            &mut config.keys,
            HashMap::from([(
                Mode::Normal,
                KeyTrie::Node(KeyTrieNode::new("Test", mode_node)),
            )]),
        );
        let mut input = NativeCommandInput::new_for_platform(
            Keymaps::new(Box::new(arc_swap::access::Constant(config.keys))),
            TargetPlatform::Linux,
        );
        let mut editor = test_editor_with_text("");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let translated = crate::utils::translate_key(&gpui::Keystroke {
            key: "r".into(),
            key_char: Some("R".into()),
            modifiers: gpui::Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
        });

        assert_unhandled_disposition(
            input.handle_key(translated, &mut compositor, &mut editor, &mut jobs),
            UnhandledDisposition::ReservedUnsupported,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn insert_keymap_dispositions_preserve_helix_ownership() {
        let mut editor = test_editor_with_text("");
        editor.mode = Mode::Insert;
        let mut input = NativeCommandInput::new(disposition_test_keymaps(Mode::Insert));

        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "C-q"),
            UnhandledDisposition::ReservedUnsupported,
        );
        assert!(matches!(
            handle_native_test_key(&mut input, &mut editor, "x"),
            NativeInputResult::Handled { .. }
        ));
        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "z"),
            UnhandledDisposition::Cancelled,
        );
        assert_unhandled_disposition(
            handle_native_test_key(&mut input, &mut editor, "F12"),
            UnhandledDisposition::RootNotFound,
        );
    }

    #[test]
    fn platform_shortcut_resolver_uses_target_primary_modifier_and_shift() {
        use crate::types::SemanticShortcutIntent as Intent;

        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                KeyEvent::from_str("C-s").unwrap(),
                TargetPlatform::Linux,
            ),
            Some(FallbackShortcut::Workspace(Intent::Save))
        );
        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                KeyEvent::from_str("C-s").unwrap(),
                TargetPlatform::Windows,
            ),
            Some(FallbackShortcut::Workspace(Intent::Save))
        );
        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                KeyEvent::from_str("Meta-s").unwrap(),
                TargetPlatform::MacOS,
            ),
            Some(FallbackShortcut::Workspace(Intent::Save))
        );
        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                KeyEvent::from_str("Meta-S-p").unwrap(),
                TargetPlatform::MacOS,
            ),
            Some(FallbackShortcut::Workspace(Intent::ShowCommandPrompt))
        );
        for (key, platform, intent) in [
            ("Meta-,", TargetPlatform::MacOS, Intent::OpenSettings),
            ("Meta-0", TargetPlatform::MacOS, Intent::ResetFontSize),
            ("C-,", TargetPlatform::Windows, Intent::OpenSettings),
            ("C-0", TargetPlatform::Linux, Intent::ResetFontSize),
            ("C-F4", TargetPlatform::Windows, Intent::CloseFile),
            ("C-`", TargetPlatform::Windows, Intent::ToggleTerminal),
            ("C-`", TargetPlatform::Linux, Intent::ToggleTerminal),
        ] {
            assert_eq!(
                resolve_fallback_shortcut(Mode::Normal, KeyEvent::from_str(key).unwrap(), platform),
                Some(FallbackShortcut::Workspace(intent)),
                "failed shortcut {key} on {platform:?}"
            );
        }
    }

    #[test]
    fn translated_shifted_physical_chords_match_fallbacks() {
        let translated = |key: &str, key_char: &str, control: bool, platform: bool| {
            crate::utils::translate_key(&gpui::Keystroke {
                key: key.into(),
                key_char: Some(key_char.into()),
                modifiers: gpui::Modifiers {
                    control,
                    shift: true,
                    platform,
                    ..Default::default()
                },
            })
        };

        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                translated("[", "{", false, true),
                TargetPlatform::MacOS,
            ),
            Some(FallbackShortcut::Editor(
                EditorSemanticAction::PreviousBuffer
            ))
        );
        assert_eq!(
            resolve_fallback_shortcut(
                Mode::Normal,
                translated("r", "R", true, false),
                TargetPlatform::Linux,
            ),
            Some(FallbackShortcut::Workspace(
                crate::types::SemanticShortcutIntent::RunNearest
            ))
        );
    }

    #[test]
    fn canonicalization_removes_shift_from_translated_modified_chars() {
        let mut key = crate::utils::translate_key(&gpui::Keystroke {
            key: "r".into(),
            key_char: Some("R".into()),
            modifiers: gpui::Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
        });
        canonicalize_key(&mut key);
        assert_eq!(key.code, KeyCode::Char('R'));
        assert_eq!(key.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn platform_editor_shortcuts_cover_edit_search_and_buffer_navigation() {
        use EditorSemanticAction as Action;
        use FallbackShortcut::Editor;

        let cases = [
            ("Meta-z", TargetPlatform::MacOS, Action::Undo),
            ("Meta-S-g", TargetPlatform::MacOS, Action::FindPrevious),
            ("C-y", TargetPlatform::Windows, Action::Redo),
            ("C-S-z", TargetPlatform::Linux, Action::Redo),
            ("F3", TargetPlatform::Linux, Action::FindNext),
            ("S-F3", TargetPlatform::Windows, Action::FindPrevious),
            ("C-tab", TargetPlatform::Linux, Action::NextBuffer),
            ("C-S-tab", TargetPlatform::Windows, Action::PreviousBuffer),
            ("C-pagedown", TargetPlatform::Linux, Action::NextBuffer),
            ("C-pageup", TargetPlatform::Windows, Action::PreviousBuffer),
            ("C-ins", TargetPlatform::Linux, Action::Copy),
            ("S-ins", TargetPlatform::Windows, Action::Paste),
            ("S-del", TargetPlatform::Linux, Action::Cut),
        ];
        for (key, platform, action) in cases {
            assert_eq!(
                resolve_fallback_shortcut(Mode::Insert, KeyEvent::from_str(key).unwrap(), platform),
                Some(Editor(action)),
                "failed shortcut {key} on {platform:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_menu_action_in_insert_does_not_insert_printable_text() {
        let mut editor = test_editor_with_text("unchanged");
        editor.mode = Mode::Insert;
        let mut bridge = EditorInputBridge::new(disposition_test_keymaps(Mode::Insert));
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        let before = focused_selection_fragments(&editor);

        let outcome = bridge.handle_semantic_action(
            EditorSemanticAction::Find,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert_eq!(outcome.prompt_requested, Some(NativePromptRequest::Search));
        assert_eq!(focused_selection_fragments(&editor), before);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_occurrence_actions_add_matches_in_the_requested_direction() {
        for (action, expected) in [
            (
                EditorSemanticAction::SelectNextOccurrence,
                vec![(12, 15), (20, 23)],
            ),
            (
                EditorSemanticAction::SelectPreviousOccurrence,
                vec![(4, 7), (12, 15)],
            ),
        ] {
            let mut editor = test_editor_with_text("one cat mid cat end cat\n");
            editor.mode = Mode::Insert;
            set_test_selection(&mut editor, 12, 15);
            let before_text = focused_document_text(&editor);
            let mut bridge = EditorInputBridge::new(disposition_test_keymaps(Mode::Insert));
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            bridge.handle_semantic_action(action, &mut compositor, &mut editor, &mut jobs);

            let view = editor.tree.get(editor.tree.focus);
            let selection = editor.document(view.doc).unwrap().selection(view.id);
            let ranges = selection
                .ranges()
                .iter()
                .map(|range| (range.from(), range.to()))
                .collect::<Vec<_>>();
            assert_eq!(ranges, expected, "{action:?}");
            assert_eq!(focused_document_text(&editor), before_text);
            assert_eq!(editor.mode(), Mode::Insert);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_commits_replay_as_text_not_insert_keymaps() {
        let mut editor = test_editor_with_text_and_config("Z", Config::default());
        set_test_cursor(&mut editor, 0);
        let mut bridge = EditorInputBridge::new(disposition_test_keymaps(Mode::Insert));
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        bridge.handle_key(
            KeyEvent::from_str("a").unwrap(),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert!(bridge.uses_native_text(&editor, KeyEvent::from_str("n").unwrap()));
        assert!(!bridge.uses_native_text(&editor, KeyEvent::from_str("x").unwrap()));
        // An IME may commit a bound character. It must be inserted literally,
        // including during counted dot-repeat, not reinterpreted as that binding.
        bridge.insert_text("x你好😀", None, &mut editor, &mut jobs);
        bridge.handle_key(
            KeyEvent::from_str("esc").unwrap(),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        for key in ["2", "."] {
            bridge.handle_key(
                KeyEvent::from_str(key).unwrap(),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
        }
        // New scratch documents use the platform's native line ending.
        let line_ending = if cfg!(windows) { "\r\n" } else { "\n" };
        assert_eq!(
            focused_document_text(&editor),
            format!("Zx你好😀x你好😀x你好😀{line_ending}")
        );
        assert_eq!(editor.mode(), Mode::Normal);
        bridge.handle_semantic_action(
            EditorSemanticAction::Undo,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );
        assert_eq!(
            focused_document_text(&editor),
            format!("Zx你好😀x你好😀{line_ending}")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn insert_platform_fallback_precedes_printable_insertion() {
        let mut editor = test_editor_with_text("");
        editor.mode = Mode::Insert;
        let mut input = NativeCommandInput::new_for_platform(
            disposition_test_keymaps(Mode::Insert),
            TargetPlatform::MacOS,
        );
        let before = focused_selection_fragments(&editor);

        assert!(matches!(
            handle_native_test_key(&mut input, &mut editor, "Meta-s"),
            NativeInputResult::RequestSemanticShortcut(crate::types::SemanticShortcutIntent::Save)
        ));
        assert_eq!(focused_selection_fragments(&editor), before);
    }

    #[test]
    fn selection_delta_requires_before_and_after_snapshots() {
        let before = Some(SelectionSnapshot { cursor: 4, line: 1 });

        assert!(!selection_changed(None, before));
        assert!(!selection_changed(before, None));
    }

    #[test]
    fn selection_delta_detects_cursor_or_line_changes() {
        let before = Some(SelectionSnapshot { cursor: 4, line: 1 });

        assert!(!selection_changed(
            before,
            Some(SelectionSnapshot { cursor: 4, line: 1 })
        ));
        assert!(selection_changed(
            before,
            Some(SelectionSnapshot { cursor: 5, line: 1 })
        ));
        assert!(selection_changed(
            before,
            Some(SelectionSnapshot { cursor: 4, line: 2 })
        ));
    }

    #[test]
    fn native_command_supports_movement_and_insert_entry() {
        assert!(native_command_supported(&MappableCommand::move_line_down));
        assert!(native_command_supported(&MappableCommand::goto_line_start));
        assert!(native_command_supported(&MappableCommand::insert_mode));
        assert!(native_command_supported(&MappableCommand::append_mode));
        assert!(native_command_supported(
            &MappableCommand::insert_at_line_start
        ));
        assert!(native_command_supported(
            &MappableCommand::insert_at_line_end
        ));
        assert!(native_command_supported(&MappableCommand::open_below));
        assert!(native_command_supported(&MappableCommand::open_above));
        assert!(native_command_supported(&MappableCommand::command_mode));
        assert!(native_command_supported(&MappableCommand::search));
        assert!(native_command_supported(&MappableCommand::rsearch));
        assert!(native_command_supported(&MappableCommand::undo));
        assert!(native_command_supported(&MappableCommand::redo));
        assert!(native_command_supported(&MappableCommand::delete_selection));
        assert!(native_command_supported(&MappableCommand::change_selection));
        assert!(native_command_supported(&MappableCommand::yank));
        assert!(native_command_supported(&MappableCommand::paste_after));
        assert!(native_command_supported(
            &MappableCommand::replace_with_yanked
        ));
        assert!(native_command_supported(&MappableCommand::indent));
        assert!(native_command_supported(&MappableCommand::join_selections));
        assert!(native_command_supported(&MappableCommand::trim_selections));
        assert!(native_command_supported(
            &MappableCommand::select_textobject_inner
        ));
        assert!(native_command_supported(&MappableCommand::surround_add));
        assert!(!native_command_supported(&MappableCommand::goto_definition));
    }

    #[test]
    fn native_command_supports_view_scroll_commands() {
        assert!(native_command_supported(&MappableCommand::align_view_top));
        assert!(native_command_supported(
            &MappableCommand::align_view_center
        ));
        assert!(native_command_supported(
            &MappableCommand::align_view_bottom
        ));
        assert!(native_command_supported(
            &MappableCommand::align_view_middle
        ));
        assert!(native_command_supported(&MappableCommand::scroll_up));
        assert!(native_command_supported(&MappableCommand::scroll_down));
        assert!(native_command_supported(&MappableCommand::page_up));
        assert!(native_command_supported(&MappableCommand::page_down));
        assert!(native_command_supported(&MappableCommand::half_page_up));
        assert!(native_command_supported(&MappableCommand::half_page_down));
        assert!(native_command_supported(&MappableCommand::page_cursor_up));
        assert!(native_command_supported(&MappableCommand::page_cursor_down));
        assert!(native_command_supported(
            &MappableCommand::page_cursor_half_up
        ));
        assert!(native_command_supported(
            &MappableCommand::page_cursor_half_down
        ));
    }

    #[test]
    fn native_viewport_scroll_command_uses_counted_visual_rows() {
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::scroll_down, None),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
                1
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::scroll_down, NonZeroUsize::new(4)),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
                4
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::scroll_up, NonZeroUsize::new(3)),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
                -3
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::page_down, None),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualPages(
                1
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::page_up, None),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualPages(
                -1
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::page_down, NonZeroUsize::new(3)),
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualPages(
                1
            ))
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::half_page_down, None),
            Some(
                nucleotide_editor::EditorViewportScrollRequest::VisualPageFraction {
                    pages: 1,
                    divisor: 2,
                }
            )
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::half_page_up, None),
            Some(
                nucleotide_editor::EditorViewportScrollRequest::VisualPageFraction {
                    pages: -1,
                    divisor: 2,
                }
            )
        );
    }

    #[test]
    fn native_viewport_scroll_command_routes_vertical_alignment() {
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::align_view_top, None),
            Some(
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Top,
                )
            )
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::align_view_center, None),
            Some(
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Center,
                )
            )
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::align_view_bottom, None),
            Some(
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Bottom,
                )
            )
        );
        assert_eq!(
            native_viewport_scroll_command(&MappableCommand::align_view_middle, None),
            None
        );
    }

    #[test]
    fn native_page_cursor_command_spec_routes_page_variants() {
        assert_eq!(
            native_page_cursor_command_spec(&MappableCommand::page_cursor_half_down),
            Some((
                nucleotide_editor::EditorViewportScrollDirection::Forward,
                1,
                2,
            ))
        );
        assert_eq!(
            native_page_cursor_command_spec(&MappableCommand::page_cursor_up),
            Some((
                nucleotide_editor::EditorViewportScrollDirection::Backward,
                -1,
                1,
            ))
        );
    }

    #[test]
    fn native_viewport_cursor_command_routes_window_targets() {
        assert_eq!(
            native_viewport_cursor_command(&MappableCommand::goto_window_top, None),
            Some(nucleotide_editor::EditorViewportCursorRequest {
                target: nucleotide_editor::EditorViewportCursorTarget::Top,
                count: 1,
            })
        );
        assert_eq!(
            native_viewport_cursor_command(&MappableCommand::goto_window_top, NonZeroUsize::new(3)),
            Some(nucleotide_editor::EditorViewportCursorRequest {
                target: nucleotide_editor::EditorViewportCursorTarget::Top,
                count: 3,
            })
        );
        assert_eq!(
            native_viewport_cursor_command(&MappableCommand::goto_window_center, None),
            Some(nucleotide_editor::EditorViewportCursorRequest {
                target: nucleotide_editor::EditorViewportCursorTarget::Center,
                count: 1,
            })
        );
        assert_eq!(
            native_viewport_cursor_command(&MappableCommand::goto_window_bottom, None),
            Some(nucleotide_editor::EditorViewportCursorRequest {
                target: nucleotide_editor::EditorViewportCursorTarget::Bottom,
                count: 1,
            })
        );
        assert_eq!(
            native_viewport_cursor_command(&MappableCommand::page_down, None),
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_page_scroll_cursor_sync_moves_cursor_below_viewport() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        let top_visual_row = 10usize;
        let visible_rows = 10usize;
        let scrolloff = editor
            .config()
            .scrolloff
            .min(visible_rows.saturating_sub(1) / 2);
        set_test_cursor_line(&mut editor, 0);

        let changed_doc = sync_cursor_after_native_page_scroll(
            &mut editor,
            view_id,
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            top_visual_row,
            visible_rows,
        );

        assert!(changed_doc.is_some());
        assert_eq!(focused_cursor_line(&editor), top_visual_row + scrolloff);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_page_scroll_cursor_sync_moves_cursor_above_viewport() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        let top_visual_row = 10usize;
        let visible_rows = 10usize;
        let scrolloff = editor
            .config()
            .scrolloff
            .min(visible_rows.saturating_sub(1) / 2);
        set_test_cursor_line(&mut editor, 30);

        let changed_doc = sync_cursor_after_native_page_scroll(
            &mut editor,
            view_id,
            nucleotide_editor::EditorViewportScrollDirection::Backward,
            top_visual_row,
            visible_rows,
        );

        assert!(changed_doc.is_some());
        assert_eq!(
            focused_cursor_line(&editor),
            top_visual_row + visible_rows - scrolloff - 1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_page_scroll_cursor_sync_leaves_visible_cursor_unchanged() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        set_test_cursor_line(&mut editor, 15);

        let changed_doc = sync_cursor_after_native_page_scroll(
            &mut editor,
            view_id,
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            10,
            10,
        );

        assert_eq!(changed_doc, None);
        assert_eq!(focused_cursor_line(&editor), 15);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_viewport_cursor_request_moves_cursor_to_visual_row() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        set_test_cursor_line(&mut editor, 4);

        let changed_doc = apply_native_viewport_cursor_request(&mut editor, view_id, 12);

        assert!(changed_doc.is_some());
        assert_eq!(focused_cursor_line(&editor), 12);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_viewport_cursor_request_extends_selection_in_select_mode() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        set_test_cursor_line(&mut editor, 4);
        editor.mode = Mode::Select;

        let changed_doc = apply_native_viewport_cursor_request(&mut editor, view_id, 12);

        assert!(changed_doc.is_some());
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document(doc_id).unwrap();
        let text = doc.text().slice(..);
        let primary = doc.selection(view_id).primary();
        assert_eq!(helix_core::coords_at_pos(text, primary.anchor).row, 4);
        assert_eq!(helix_core::coords_at_pos(text, primary.head).row, 12);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_page_cursor_movement_moves_cursor_by_visual_rows() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        set_test_cursor_line(&mut editor, 4);

        let changed_doc = apply_native_page_cursor_movement(
            &mut editor,
            view_id,
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            7,
        );

        assert!(changed_doc.is_some());
        assert_eq!(focused_cursor_line(&editor), 11);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_page_cursor_movement_extends_selection_in_select_mode() {
        let mut editor = test_editor_with_text(&numbered_lines(40));
        let view_id = editor.tree.focus;
        set_test_cursor_line(&mut editor, 4);
        editor.mode = Mode::Select;

        let changed_doc = apply_native_page_cursor_movement(
            &mut editor,
            view_id,
            nucleotide_editor::EditorViewportScrollDirection::Forward,
            7,
        );

        assert!(changed_doc.is_some());
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document(doc_id).unwrap();
        let text = doc.text().slice(..);
        let primary = doc.selection(view_id).primary();
        assert_eq!(helix_core::coords_at_pos(text, primary.anchor).row, 4);
        assert_eq!(helix_core::coords_at_pos(text, primary.head).row, 11);
    }

    #[test]
    fn native_command_supports_search_repeat_commands() {
        assert!(native_command_supported(&MappableCommand::search_next));
        assert!(native_command_supported(&MappableCommand::search_prev));
        assert!(native_command_supported(
            &MappableCommand::extend_search_next
        ));
        assert!(native_command_supported(
            &MappableCommand::extend_search_prev
        ));
        assert!(native_command_supported(&MappableCommand::search_selection));
        assert!(native_command_supported(
            &MappableCommand::search_selection_detect_word_boundaries
        ));
        assert!(native_command_supported(
            &MappableCommand::make_search_word_bounded
        ));
        assert!(native_command_supported(&MappableCommand::global_search));
    }

    #[test]
    fn native_command_supports_buffer_and_jumplist_navigation() {
        assert!(native_command_supported(&MappableCommand::goto_next_buffer));
        assert!(native_command_supported(
            &MappableCommand::goto_previous_buffer
        ));
        assert!(native_command_supported(
            &MappableCommand::goto_last_accessed_file
        ));
        assert!(native_command_supported(
            &MappableCommand::goto_last_modified_file
        ));
        assert!(native_command_supported(
            &MappableCommand::goto_last_modification
        ));
        assert!(native_command_supported(&MappableCommand::jump_forward));
        assert!(native_command_supported(&MappableCommand::jump_backward));
        assert!(native_command_supported(&MappableCommand::save_selection));
    }

    #[test]
    fn native_command_supports_file_navigation() {
        assert!(native_command_supported(&MappableCommand::goto_file));
        assert!(native_command_supported(&MappableCommand::goto_file_hsplit));
        assert!(native_command_supported(&MappableCommand::goto_file_vsplit));
        assert!(!native_insert_command_supported(
            &MappableCommand::goto_file
        ));
    }

    #[test]
    fn native_command_supports_diagnostics_changes_and_matching() {
        assert!(native_command_supported(&MappableCommand::goto_first_diag));
        assert!(native_command_supported(&MappableCommand::goto_last_diag));
        assert!(native_command_supported(&MappableCommand::goto_next_diag));
        assert!(native_command_supported(&MappableCommand::goto_prev_diag));
        assert!(native_command_supported(
            &MappableCommand::goto_first_change
        ));
        assert!(native_command_supported(&MappableCommand::goto_last_change));
        assert!(native_command_supported(&MappableCommand::goto_next_change));
        assert!(native_command_supported(&MappableCommand::goto_prev_change));
        assert!(native_command_supported(
            &":reset-diff-change".parse::<MappableCommand>().unwrap()
        ));
        assert!(native_command_supported(&MappableCommand::match_brackets));
    }

    #[test]
    fn native_command_supports_syntax_object_navigation() {
        for command in [
            MappableCommand::goto_next_function,
            MappableCommand::goto_prev_function,
            MappableCommand::goto_next_class,
            MappableCommand::goto_prev_class,
            MappableCommand::goto_next_parameter,
            MappableCommand::goto_prev_parameter,
            MappableCommand::goto_next_comment,
            MappableCommand::goto_prev_comment,
            MappableCommand::goto_next_test,
            MappableCommand::goto_prev_test,
            MappableCommand::goto_next_xml_element,
            MappableCommand::goto_prev_xml_element,
            MappableCommand::goto_next_entry,
            MappableCommand::goto_prev_entry,
        ] {
            assert!(native_command_supported(&command));
            assert!(native_syntax_object_navigation_command(&command));
            assert!(!native_insert_command_supported(&command));
        }

        assert!(!native_syntax_object_navigation_command(
            &MappableCommand::goto_next_diag
        ));
        assert!(!native_syntax_object_navigation_command(
            &MappableCommand::normal_mode
        ));
    }

    #[test]
    fn native_command_supports_split_navigation() {
        assert!(native_command_supported(&MappableCommand::jump_view_right));
        assert!(native_command_supported(&MappableCommand::jump_view_left));
        assert!(native_command_supported(&MappableCommand::jump_view_up));
        assert!(native_command_supported(&MappableCommand::jump_view_down));
        assert!(native_command_supported(&MappableCommand::swap_view_right));
        assert!(native_command_supported(&MappableCommand::swap_view_left));
        assert!(native_command_supported(&MappableCommand::swap_view_up));
        assert!(native_command_supported(&MappableCommand::swap_view_down));
        assert!(native_command_supported(&MappableCommand::transpose_view));
        assert!(native_command_supported(&MappableCommand::rotate_view));
        assert!(native_command_supported(
            &MappableCommand::rotate_view_reverse
        ));
        assert!(native_command_supported(&MappableCommand::hsplit));
        assert!(native_command_supported(&MappableCommand::hsplit_new));
        assert!(native_command_supported(&MappableCommand::vsplit));
        assert!(native_command_supported(&MappableCommand::vsplit_new));
        assert!(native_command_supported(&MappableCommand::wclose));
        assert!(native_command_supported(&MappableCommand::wonly));
    }

    #[test]
    fn native_command_supports_register_selection_commands() {
        assert!(native_command_supported(&MappableCommand::select_register));
        assert!(native_command_supported(
            &MappableCommand::copy_between_registers
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::select_register
        ));
    }

    #[test]
    fn native_insert_support_reuses_movement_allowlist() {
        assert!(native_insert_command_supported(
            &MappableCommand::move_char_left
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::normal_mode
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::delete_char_backward
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::delete_char_forward
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::insert_newline
        ));
        assert!(native_insert_command_supported(&MappableCommand::smart_tab));
        assert!(native_insert_command_supported(
            &MappableCommand::insert_tab
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::insert_register
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::completion
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::command_mode
        ));
        assert!(!native_insert_command_supported(&MappableCommand::search));
        assert!(!native_insert_command_supported(&MappableCommand::rsearch));
        assert!(!native_insert_command_supported(
            &MappableCommand::insert_mode
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::append_mode
        ));
        assert!(!native_insert_command_supported(&MappableCommand::undo));
        assert!(!native_insert_command_supported(&MappableCommand::redo));
        assert!(!native_insert_command_supported(
            &MappableCommand::delete_selection
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::change_selection
        ));
        assert!(!native_insert_command_supported(&MappableCommand::yank));
        assert!(!native_insert_command_supported(
            &MappableCommand::paste_after
        ));
        assert!(!native_insert_command_supported(&MappableCommand::indent));
        assert!(!native_insert_command_supported(
            &MappableCommand::join_selections
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::trim_selections
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::goto_file
        ));
    }

    #[test]
    fn native_insert_shortcut_recognizes_ctrl_v_as_clipboard_paste() {
        assert!(is_native_insert_paste_shortcut(
            KeyEvent::from_str("C-v").unwrap()
        ));
        assert!(is_native_insert_paste_shortcut(
            KeyEvent::from_str("C-S-v").unwrap()
        ));
        assert!(!is_native_insert_paste_shortcut(plain_char_key('v')));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_does_not_insert_literal_v_for_ctrl_v_in_insert_mode() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("", config);
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let enter_insert =
            handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "i");
        assert!(enter_insert.completion_requested.is_none());
        assert_eq!(editor.mode(), Mode::Insert);
        let text_before_paste = focused_document_text(&editor);

        let ctrl_v = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "C-v");

        assert!(ctrl_v.completion_requested.is_none());
        let text_after_paste = focused_document_text(&editor);
        assert_eq!(text_after_paste, text_before_paste);
        assert!(!text_after_paste.contains('v'));
        assert_eq!(editor.mode(), Mode::Insert);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_paste_uses_helix_before_placement_in_normal_mode() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abc", config);
        set_test_selection(&mut editor, 1, 2);
        set_test_clipboard(&mut editor, "X");
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert_eq!(
            focused_document_text(&editor),
            format!("aXbc{}", helix_core::NATIVE_LINE_ENDING.as_str())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_paste_applies_and_clears_helix_count() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abc", config);
        set_test_selection(&mut editor, 1, 2);
        set_test_clipboard(&mut editor, "X");
        editor.count = NonZeroUsize::new(2);
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert_eq!(
            focused_document_text(&editor),
            format!("aXXbc{}", helix_core::NATIVE_LINE_ENDING.as_str())
        );
        assert_eq!(editor.count, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_paste_cancels_pending_helix_key_sequence() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abc", config);
        set_test_clipboard(&mut editor, "X");
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        handle_key_str(
            &mut bridge,
            &mut editor,
            &mut compositor,
            &mut jobs,
            "space",
        );
        assert!(!bridge.native_commands.keymaps.pending().is_empty());

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert!(bridge.native_commands.keymaps.pending().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_paste_uses_helix_cursor_placement_in_insert_mode() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abc", config);
        editor.mode = Mode::Insert;
        set_test_selection(&mut editor, 2, 1);
        set_test_clipboard(&mut editor, "X");
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert_eq!(
            focused_document_text(&editor),
            format!("aXbc{}", helix_core::NATIVE_LINE_ENDING.as_str())
        );
        assert_eq!(editor.mode(), Mode::Insert);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_linewise_paste_stays_at_helix_insert_cursor() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abc", config);
        editor.mode = Mode::Insert;
        set_test_selection(&mut editor, 2, 1);
        set_test_clipboard(&mut editor, "X\n");
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        let line_ending = helix_core::NATIVE_LINE_ENDING.as_str();
        assert_eq!(
            focused_document_text(&editor),
            format!("aX{line_ending}bc{line_ending}")
        );
        assert_eq!(editor.mode(), Mode::Insert);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_paste_uses_cursor_and_exits_helix_select_mode() {
        let config = Config {
            clipboard_provider: ClipboardProvider::None,
            ..Default::default()
        };
        let mut editor = test_editor_with_text_and_config("abcd", config);
        editor.mode = Mode::Select;
        set_test_selection(&mut editor, 1, 3);
        set_test_clipboard(&mut editor, "X");
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        bridge.handle_semantic_action(
            EditorSemanticAction::Paste,
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert_eq!(
            focused_document_text(&editor),
            format!("abXcd{}", helix_core::NATIVE_LINE_ENDING.as_str())
        );
        assert_eq!(editor.mode(), Mode::Normal);
    }

    #[test]
    fn insert_entry_commands_are_classified_separately() {
        assert!(native_insert_entry_command(&MappableCommand::insert_mode));
        assert!(native_insert_entry_command(&MappableCommand::append_mode));
        assert!(native_insert_entry_command(
            &MappableCommand::insert_at_line_start
        ));
        assert!(native_insert_entry_command(
            &MappableCommand::insert_at_line_end
        ));
        assert!(native_insert_entry_command(&MappableCommand::open_below));
        assert!(native_insert_entry_command(&MappableCommand::open_above));
        assert!(!native_insert_entry_command(
            &MappableCommand::move_char_left
        ));
        assert!(!native_insert_entry_command(&MappableCommand::normal_mode));
    }

    #[test]
    fn prompt_commands_are_classified_separately() {
        assert_eq!(
            native_prompt_command(&MappableCommand::command_mode),
            Some(NativePromptRequest::Command)
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::search),
            Some(NativePromptRequest::Search)
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::rsearch),
            Some(NativePromptRequest::ReverseSearch)
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::select_regex),
            Some(NativePromptRequest::RegexSelection(
                crate::types::RegexSelectionAction::Select
            ))
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::split_selection),
            Some(NativePromptRequest::RegexSelection(
                crate::types::RegexSelectionAction::Split
            ))
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::keep_selections),
            Some(NativePromptRequest::RegexSelection(
                crate::types::RegexSelectionAction::Keep
            ))
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::remove_selections),
            Some(NativePromptRequest::RegexSelection(
                crate::types::RegexSelectionAction::Remove
            ))
        );
        assert_eq!(
            native_prompt_command(&MappableCommand::global_search),
            Some(NativePromptRequest::GlobalSearch)
        );
        assert_eq!(native_prompt_command(&MappableCommand::file_picker), None);
        assert_eq!(native_prompt_command(&MappableCommand::buffer_picker), None);
        assert_eq!(native_prompt_command(&MappableCommand::insert_mode), None);
        assert_eq!(native_prompt_command(&MappableCommand::normal_mode), None);
    }

    #[test]
    fn default_prompt_keymaps_request_native_prompts() {
        for (key, request) in [
            (":", NativePromptRequest::Command),
            ("/", NativePromptRequest::Search),
            ("?", NativePromptRequest::ReverseSearch),
            (
                "s",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Select),
            ),
            (
                "S",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Split),
            ),
            (
                "K",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Keep),
            ),
            (
                "A-K",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Remove),
            ),
        ] {
            let mut keymaps = Keymaps::default();
            let key_event = KeyEvent::from_str(key).unwrap();

            match keymaps.get(Mode::Normal, key_event) {
                KeymapResult::Matched(command) => {
                    assert_eq!(native_prompt_command(&command), Some(request));
                }
                _ => panic!("expected {key} to resolve to native prompt request"),
            }
        }
    }

    #[test]
    fn default_space_slash_keymap_requests_global_search_prompt() {
        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let slash = KeyEvent::from_str("/").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, slash) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_prompt_command(&command),
                    Some(NativePromptRequest::GlobalSearch)
                );
            }
            _ => panic!("expected SPACE-/ to resolve to global_search"),
        }
    }

    #[test]
    fn picker_commands_are_classified_separately() {
        assert_eq!(
            native_picker_command(&MappableCommand::file_picker),
            Some(NativePickerRequest::File)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::file_picker_in_current_directory),
            Some(NativePickerRequest::FileCurrentDirectory)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::file_picker_in_current_buffer_directory),
            Some(NativePickerRequest::FileCurrentBufferDirectory)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::buffer_picker),
            Some(NativePickerRequest::Buffer)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::jumplist_picker),
            Some(NativePickerRequest::JumpList)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::lsp_or_syntax_symbol_picker),
            Some(NativePickerRequest::Symbols { workspace: false })
        );
        assert_eq!(
            native_picker_command(&MappableCommand::lsp_or_syntax_workspace_symbol_picker),
            Some(NativePickerRequest::Symbols { workspace: true })
        );
        assert_eq!(
            native_picker_command(&MappableCommand::diagnostics_picker),
            Some(NativePickerRequest::Diagnostics { workspace: false })
        );
        assert_eq!(
            native_picker_command(&MappableCommand::workspace_diagnostics_picker),
            Some(NativePickerRequest::Diagnostics { workspace: true })
        );
        assert_eq!(
            native_picker_command(&MappableCommand::code_action),
            Some(NativePickerRequest::CodeActions)
        );
        assert_eq!(
            native_picker_command(&MappableCommand::hover),
            Some(NativePickerRequest::HoverDocs)
        );
        assert_eq!(native_picker_command(&MappableCommand::command_mode), None);
        assert_eq!(native_picker_command(&MappableCommand::normal_mode), None);
    }

    #[test]
    fn lsp_navigation_commands_are_classified_separately() {
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::goto_declaration),
            Some(NativeLspNavigationRequest::GotoDeclaration)
        );
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::goto_definition),
            Some(NativeLspNavigationRequest::GotoDefinition)
        );
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::goto_type_definition),
            Some(NativeLspNavigationRequest::GotoTypeDefinition)
        );
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::goto_implementation),
            Some(NativeLspNavigationRequest::GotoImplementation)
        );
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::goto_reference),
            Some(NativeLspNavigationRequest::GotoReference)
        );
        assert_eq!(
            native_lsp_navigation_command(&MappableCommand::global_search),
            None
        );
        assert!(!native_command_supported(&MappableCommand::goto_definition));
    }

    #[test]
    fn workspace_commands_are_classified_separately() {
        assert_eq!(
            native_workspace_command(&MappableCommand::Typable {
                name: "toggle-file-tree".into(),
                args: String::new(),
                doc: "Toggle file tree".into(),
            }),
            Some(NativeWorkspaceRequest::ToggleFileTree)
        );
        assert_eq!(
            native_workspace_command(&MappableCommand::command_palette),
            None
        );
    }

    #[test]
    fn default_goto_keymaps_request_native_lsp_navigation() {
        let mut keymaps = Keymaps::default();
        let g = KeyEvent::from_str("g").unwrap();

        for (key, request) in [
            ("d", NativeLspNavigationRequest::GotoDefinition),
            ("D", NativeLspNavigationRequest::GotoDeclaration),
            ("y", NativeLspNavigationRequest::GotoTypeDefinition),
            ("i", NativeLspNavigationRequest::GotoImplementation),
            ("r", NativeLspNavigationRequest::GotoReference),
        ] {
            assert!(matches!(
                keymaps.get(Mode::Normal, g),
                KeymapResult::Pending(_)
            ));

            match keymaps.get(Mode::Normal, KeyEvent::from_str(key).unwrap()) {
                KeymapResult::Matched(command) => {
                    assert_eq!(native_lsp_navigation_command(&command), Some(request));
                }
                _ => panic!("expected g{key} to resolve to native LSP navigation"),
            }
        }
    }

    #[test]
    fn default_match_keymaps_route_textobjects_and_surrounds_natively() {
        for (key, command) in [
            ("a", MappableCommand::select_textobject_around),
            ("i", MappableCommand::select_textobject_inner),
            ("s", MappableCommand::surround_add),
            ("r", MappableCommand::surround_replace),
            ("d", MappableCommand::surround_delete),
        ] {
            let mut keymaps = Keymaps::default();
            let m = KeyEvent::from_str("m").unwrap();

            assert!(matches!(
                keymaps.get(Mode::Normal, m),
                KeymapResult::Pending(_)
            ));

            match keymaps.get(Mode::Normal, KeyEvent::from_str(key).unwrap()) {
                KeymapResult::Matched(resolved) => {
                    assert_eq!(resolved, command);
                    assert!(native_command_supported(&resolved));
                }
                _ => panic!("expected m{key} to resolve to native command"),
            }
        }
    }

    #[test]
    fn default_space_f_keymap_requests_file_picker() {
        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, f) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::File)
                );
            }
            _ => panic!("expected SPACE-f to resolve to file_picker"),
        }
    }

    #[test]
    fn default_space_j_keymap_requests_jumplist_picker() {
        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let j = KeyEvent::from_str("j").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, j) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::JumpList)
                );
            }
            _ => panic!("expected SPACE-j to resolve to jumplist_picker"),
        }
    }

    #[test]
    fn default_space_symbol_keymaps_request_native_symbol_pickers() {
        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let s = KeyEvent::from_str("s").unwrap();
        let shift_s = KeyEvent::from_str("S").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, s) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::Symbols { workspace: false })
                );
            }
            _ => panic!("expected SPACE-s to resolve to symbol picker"),
        }

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, shift_s) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::Symbols { workspace: true })
                );
            }
            _ => panic!("expected SPACE-S to resolve to workspace symbol picker"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_reports_reset_diff_change_command() {
        let mut bridge = EditorInputBridge::new(reset_diff_change_keymaps());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let pending = handle_key_str(
            &mut bridge,
            &mut editor,
            &mut compositor,
            &mut jobs,
            "space",
        );
        assert!(pending.handled_by_native_command);
        assert!(!pending.reset_diff_change_executed);

        let pending = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "v");
        assert!(pending.handled_by_native_command);
        assert!(!pending.reset_diff_change_executed);

        let reset = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "r");
        assert!(reset.handled_by_native_command);
        assert!(reset.reset_diff_change_executed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_file_picker_for_gpui_space_f() {
        for space_key in ["space", " "] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one\ntwo\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let pending = bridge.handle_key(
                translated_gpui_key(space_key),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );
            assert!(pending.handled_by_native_command);
            assert_eq!(pending.picker_requested, None);

            let picker = bridge.handle_key(
                translated_gpui_key("f"),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );

            assert!(picker.handled_by_native_command);
            assert_eq!(picker.picker_requested, Some(NativePickerRequest::File));
            assert_eq!(picker.workspace_requested, None);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_workspace_toggle_for_space_t() {
        let mut bridge = EditorInputBridge::new(reset_diff_change_keymaps());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let pending = handle_key_str(
            &mut bridge,
            &mut editor,
            &mut compositor,
            &mut jobs,
            "space",
        );
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.workspace_requested, None);

        let toggle = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "t");

        assert!(toggle.handled_by_native_command);
        assert_eq!(
            toggle.workspace_requested,
            Some(NativeWorkspaceRequest::ToggleFileTree)
        );
        assert_eq!(toggle.picker_requested, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_keeps_user_space_t_helix_owned() {
        let mut bridge = EditorInputBridge::new(nucleotide_keymaps(
            MappableCommand::from_str(":quit").unwrap(),
        ));
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let pending = handle_key_str(
            &mut bridge,
            &mut editor,
            &mut compositor,
            &mut jobs,
            "space",
        );
        assert!(pending.handled_by_native_command);

        let result = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "t");
        assert!(!result.handled_by_native_command);
        assert_eq!(result.workspace_requested, None);
        assert_eq!(
            result.unhandled_keys,
            [
                KeyEvent::from_str("space").unwrap(),
                KeyEvent::from_str("t").unwrap()
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_jumplist_picker_for_space_j() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let space = KeyEvent::from_str("space").unwrap();
        let j = KeyEvent::from_str("j").unwrap();

        let pending = bridge.handle_key(space, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.picker_requested, None);

        let picker = bridge.handle_key(j, &mut compositor, &mut editor, &mut jobs);
        assert!(picker.handled_by_native_command);
        assert_eq!(picker.picker_requested, Some(NativePickerRequest::JumpList));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_symbol_picker_for_space_s() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let space = KeyEvent::from_str("space").unwrap();
        let s = KeyEvent::from_str("s").unwrap();

        let pending = bridge.handle_key(space, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.picker_requested, None);

        let picker = bridge.handle_key(s, &mut compositor, &mut editor, &mut jobs);
        assert!(picker.handled_by_native_command);
        assert_eq!(
            picker.picker_requested,
            Some(NativePickerRequest::Symbols { workspace: false })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_global_search_for_space_slash() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let space = KeyEvent::from_str("space").unwrap();
        let slash = KeyEvent::from_str("/").unwrap();

        let pending = bridge.handle_key(space, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.prompt_requested, None);

        let prompt = bridge.handle_key(slash, &mut compositor, &mut editor, &mut jobs);
        assert!(prompt.handled_by_native_command);
        assert_eq!(
            prompt.prompt_requested,
            Some(NativePromptRequest::GlobalSearch)
        );
        assert!(compositor.pop().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_lsp_navigation_for_gd() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let g = KeyEvent::from_str("g").unwrap();
        let d = KeyEvent::from_str("d").unwrap();

        let pending = bridge.handle_key(g, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.lsp_navigation_requested, None);

        let navigation = bridge.handle_key(d, &mut compositor, &mut editor, &mut jobs);
        assert!(navigation.handled_by_native_command);
        assert_eq!(
            navigation.lsp_navigation_requested,
            Some(NativeLspNavigationRequest::GotoDefinition)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_jumplist_movement_natively() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let ctrl_o = KeyEvent::from_str("C-o").unwrap();

        let outcome = bridge.handle_key(ctrl_o, &mut compositor, &mut editor, &mut jobs);

        assert!(outcome.handled_by_native_command);
        assert_eq!(outcome.picker_requested, None);
        assert_eq!(outcome.lsp_navigation_requested, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_reports_document_switch_from_jumplist() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let first_doc_id = editor.tree.try_get(editor.tree.focus).unwrap().doc;
        let second_doc_id = editor.new_file(Action::Replace);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        assert_ne!(first_doc_id, second_doc_id);
        assert_eq!(
            editor.tree.try_get(editor.tree.focus).map(|view| view.doc),
            Some(second_doc_id)
        );

        let outcome = bridge.handle_key(
            KeyEvent::from_str("C-o").unwrap(),
            &mut compositor,
            &mut editor,
            &mut jobs,
        );

        assert!(outcome.handled_by_native_command);
        assert!(outcome.selection_changed);
        assert_eq!(outcome.focused_doc_id, Some(first_doc_id));
        assert_eq!(
            editor.tree.try_get(editor.tree.focus).map(|view| view.doc),
            Some(first_doc_id)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_viewport_scroll() {
        for (keys, request) in [
            (
                ["z", "j"],
                nucleotide_editor::EditorViewportScrollRequest::VisualRows(1),
            ),
            (
                ["z", "k"],
                nucleotide_editor::EditorViewportScrollRequest::VisualRows(-1),
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one\ntwo\nthree\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let pending = handle_key_str(
                &mut bridge,
                &mut editor,
                &mut compositor,
                &mut jobs,
                keys[0],
            );
            assert!(pending.handled_by_native_command);
            assert_eq!(pending.viewport_scroll_requested, None);

            let outcome = handle_key_str(
                &mut bridge,
                &mut editor,
                &mut compositor,
                &mut jobs,
                keys[1],
            );

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.viewport_scroll_requested, Some(request));
            assert!(!outcome.selection_changed);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_page_scroll() {
        for (key, request) in [
            (
                "C-f",
                nucleotide_editor::EditorViewportScrollRequest::VisualPages(1),
            ),
            (
                "C-b",
                nucleotide_editor::EditorViewportScrollRequest::VisualPages(-1),
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one\ntwo\nthree\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.viewport_scroll_requested, Some(request));
            assert!(!outcome.selection_changed);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_page_cursor_scroll() {
        for (key, start_line, request, expected_direction) in [
            (
                "C-d",
                4usize,
                nucleotide_editor::EditorViewportScrollRequest::VisualPageWithCursor {
                    pages: 1,
                    divisor: 2,
                },
                1isize,
            ),
            (
                "C-u",
                30usize,
                nucleotide_editor::EditorViewportScrollRequest::VisualPageWithCursor {
                    pages: -1,
                    divisor: 2,
                },
                -1isize,
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text(&numbered_lines(80));
            let half_page_rows = editor
                .tree
                .try_get(editor.tree.focus)
                .unwrap()
                .inner_height()
                / 2;
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();
            set_test_cursor_line(&mut editor, start_line);

            let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.viewport_scroll_requested, Some(request));
            assert!(outcome.selection_changed);
            let expected_line =
                start_line.saturating_add_signed(expected_direction * half_page_rows as isize);
            assert_eq!(focused_cursor_line(&editor), expected_line);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_extends_selection_for_page_cursor_in_select_mode() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text(&numbered_lines(80));
        let view_id = editor.tree.focus;
        let half_page_rows = editor.tree.try_get(view_id).unwrap().inner_height() / 2;
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        set_test_cursor_line(&mut editor, 4);
        editor.mode = Mode::Select;

        let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "C-d");

        assert!(outcome.handled_by_native_command);
        assert_eq!(
            outcome.viewport_scroll_requested,
            Some(
                nucleotide_editor::EditorViewportScrollRequest::VisualPageWithCursor {
                    pages: 1,
                    divisor: 2,
                }
            )
        );
        assert!(outcome.selection_changed);

        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document(doc_id).unwrap();
        let text = doc.text().slice(..);
        let primary = doc.selection(view_id).primary();
        assert_eq!(helix_core::coords_at_pos(text, primary.anchor).row, 4);
        assert_eq!(
            helix_core::coords_at_pos(text, primary.head).row,
            4 + half_page_rows
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_viewport_cursor() {
        for (keys, request) in [
            (
                vec!["g", "t"],
                nucleotide_editor::EditorViewportCursorRequest {
                    target: nucleotide_editor::EditorViewportCursorTarget::Top,
                    count: 1,
                },
            ),
            (
                vec!["g", "c"],
                nucleotide_editor::EditorViewportCursorRequest {
                    target: nucleotide_editor::EditorViewportCursorTarget::Center,
                    count: 1,
                },
            ),
            (
                vec!["g", "b"],
                nucleotide_editor::EditorViewportCursorRequest {
                    target: nucleotide_editor::EditorViewportCursorTarget::Bottom,
                    count: 1,
                },
            ),
            (
                vec!["3", "g", "t"],
                nucleotide_editor::EditorViewportCursorRequest {
                    target: nucleotide_editor::EditorViewportCursorTarget::Top,
                    count: 3,
                },
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text(&numbered_lines(20));
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            for key in keys.iter().take(keys.len() - 1) {
                let pending =
                    handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);
                assert!(pending.handled_by_native_command);
                assert_eq!(pending.viewport_cursor_requested, None);
            }

            let outcome = handle_key_str(
                &mut bridge,
                &mut editor,
                &mut compositor,
                &mut jobs,
                keys[keys.len() - 1],
            );

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.viewport_cursor_requested, Some(request));
            assert!(!outcome.selection_changed);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_viewport_alignment() {
        for (keys, request) in [
            (
                ["z", "t"],
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Top,
                ),
            ),
            (
                ["z", "z"],
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Center,
                ),
            ),
            (
                ["z", "c"],
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Center,
                ),
            ),
            (
                ["z", "b"],
                nucleotide_editor::EditorViewportScrollRequest::CursorReveal(
                    nucleotide_editor::EditorCursorReveal::Bottom,
                ),
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one\ntwo\nthree\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let pending = handle_key_str(
                &mut bridge,
                &mut editor,
                &mut compositor,
                &mut jobs,
                keys[0],
            );
            assert!(pending.handled_by_native_command);
            assert_eq!(pending.viewport_scroll_requested, None);

            let outcome = handle_key_str(
                &mut bridge,
                &mut editor,
                &mut compositor,
                &mut jobs,
                keys[1],
            );

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.viewport_scroll_requested, Some(request));
            assert!(!outcome.selection_changed);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_align_view_middle_natively() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text(&format!("{}\n", "a".repeat(140)));
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();
        set_test_cursor(&mut editor, 100);

        let expected_offset = focused_align_middle_offset(&editor);
        assert!(expected_offset > 0);
        assert_eq!(focused_horizontal_offset(&editor), 0);

        let pending = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "z");
        assert!(pending.handled_by_native_command);

        let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "m");

        assert!(outcome.handled_by_native_command);
        assert_eq!(focused_horizontal_offset(&editor), expected_offset);
        assert!(!outcome.selection_changed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_counted_native_viewport_scroll() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\nthree\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let count = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "3");
        assert!(count.handled_by_native_command);
        assert_eq!(count.viewport_scroll_requested, None);

        let pending = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "z");
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.viewport_scroll_requested, None);

        let scroll = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "j");

        assert!(scroll.handled_by_native_command);
        assert_eq!(
            scroll.viewport_scroll_requested,
            Some(nucleotide_editor::EditorViewportScrollRequest::VisualRows(
                3
            ))
        );
        assert!(!scroll.selection_changed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_syntax_object_navigation_natively() {
        for sequence in [["]", "f"], ["[", "t"]] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("fn main() {}\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            for key in sequence {
                let outcome =
                    handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);
                assert!(outcome.handled_by_native_command);
            }

            assert!(editor.status_msg.is_some());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_textobject_selection_natively() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one two\n");
        set_test_cursor(&mut editor, 1);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        for key in ["m", "i", "w"] {
            let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);
            assert!(outcome.handled_by_native_command);
        }

        assert_eq!(focused_selection_fragments(&editor), vec!["one"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_surround_add_natively() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one two\n");
        set_test_selection(&mut editor, 0, 3);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        for key in ["m", "s", "("] {
            let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, key);
            assert!(outcome.handled_by_native_command);
        }

        assert_eq!(
            focused_document_text(&editor).replace("\r\n", "\n"),
            "(one) two\n\n"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_command_prompt() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let outcome =
            bridge.handle_key(plain_char_key(':'), &mut compositor, &mut editor, &mut jobs);

        assert!(outcome.handled_by_native_command);
        assert_eq!(outcome.prompt_requested, Some(NativePromptRequest::Command));
        assert!(compositor.pop().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_search_prompts() {
        for (key, request) in [
            ('/', NativePromptRequest::Search),
            ('?', NativePromptRequest::ReverseSearch),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one\ntwo\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let outcome =
                bridge.handle_key(plain_char_key(key), &mut compositor, &mut editor, &mut jobs);

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.prompt_requested, Some(request));
            assert!(compositor.pop().is_none());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_regex_selection_prompts() {
        for (key, request) in [
            (
                "s",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Select),
            ),
            (
                "S",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Split),
            ),
            (
                "K",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Keep),
            ),
            (
                "A-K",
                NativePromptRequest::RegexSelection(crate::types::RegexSelectionAction::Remove),
            ),
        ] {
            let mut bridge = EditorInputBridge::new(Keymaps::default());
            let mut editor = test_editor_with_text("one two\n");
            let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
            let mut jobs = Jobs::new();

            let outcome = bridge.handle_key(
                KeyEvent::from_str(key).unwrap(),
                &mut compositor,
                &mut editor,
                &mut jobs,
            );

            assert!(outcome.handled_by_native_command);
            assert_eq!(outcome.prompt_requested, Some(request));
            assert!(compositor.pop().is_none());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_workspace_open_for_goto_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_path = temp_dir.path().join("target.txt");
        std::fs::write(&target_path, "opened\n").unwrap();
        let target_text = target_path.display().to_string();

        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text(&format!("{}\n", target_text));
        set_test_selection(&mut editor, 0, target_text.len());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let g = KeyEvent::from_str("g").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        let pending = bridge.handle_key(g, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);

        let outcome = bridge.handle_key(f, &mut compositor, &mut editor, &mut jobs);
        assert!(outcome.handled_by_native_command);
        assert_eq!(outcome.picker_requested, None);
        let expected_path = canonicalize_for_assertion(&target_path);
        match outcome.workspace_requested {
            Some(NativeWorkspaceRequest::OpenFiles { paths, action }) => {
                assert_eq!(paths.len(), 1);
                assert_eq!(canonicalize_for_assertion(&paths[0]), expected_path);
                assert_eq!(action, NativeFileOpenAction::Replace);
            }
            other => panic!("expected workspace open request, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_workspace_open_for_goto_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_text = temp_dir.path().display().to_string();

        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text(&format!("{}\n", target_text));
        set_test_selection(&mut editor, 0, target_text.len());
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let g = KeyEvent::from_str("g").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        let pending = bridge.handle_key(g, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);

        let outcome = bridge.handle_key(f, &mut compositor, &mut editor, &mut jobs);
        assert!(outcome.handled_by_native_command);
        assert_eq!(outcome.picker_requested, None);
        match outcome.workspace_requested {
            Some(NativeWorkspaceRequest::OpenFiles { paths, action }) => {
                assert_eq!(paths, vec![temp_dir.path().to_path_buf()]);
                assert_eq!(action, NativeFileOpenAction::Replace);
            }
            other => panic!("expected workspace open request, got {other:?}"),
        }
    }

    #[test]
    fn default_gf_keymap_routes_file_navigation_natively() {
        let mut keymaps = Keymaps::default();
        let g = KeyEvent::from_str("g").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, g),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, f) {
            KeymapResult::Matched(command) => {
                assert_eq!(command, MappableCommand::goto_file);
                assert!(matches!(
                    native_file_navigation_command(&command),
                    Some(NativeFileOpenAction::Replace)
                ));
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected gf to resolve to goto_file"),
        }
    }

    #[test]
    fn file_navigation_leaves_real_urls_unhandled() {
        assert!(target_is_external_url("https://example.com/file.rs"));
        assert!(target_is_external_url("file:///tmp/file.rs"));
    }

    #[test]
    fn file_navigation_accepts_windows_drive_paths() {
        assert!(!target_is_external_url("C:\\Users\\test\\file.rs"));
        assert!(!target_is_external_url("D:/a/nucleotide/target.txt"));
    }

    #[test]
    fn default_space_shift_f_keymap_requests_current_directory_picker() {
        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let shift_f = KeyEvent::from_str("F").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, shift_f) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::FileCurrentDirectory)
                );
            }
            _ => panic!("expected SPACE-F to resolve to file_picker_in_current_directory"),
        }
    }

    #[test]
    fn default_space_d_keymap_requests_buffer_diagnostics() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let d = KeyEvent::from_str("d").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, d) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::Diagnostics { workspace: false })
                );
            }
            _ => panic!("expected SPACE-d to resolve to diagnostics_picker"),
        }
    }

    #[test]
    fn default_space_shift_d_keymap_requests_workspace_diagnostics() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let shift_d = KeyEvent::from_str("D").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, shift_d) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::Diagnostics { workspace: true })
                );
            }
            _ => panic!("expected SPACE-D to resolve to workspace_diagnostics_picker"),
        }
    }

    #[test]
    fn default_space_a_keymap_requests_code_actions() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let a = KeyEvent::from_str("a").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, a) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::CodeActions)
                );
            }
            _ => panic!("expected SPACE-a to resolve to code_action"),
        }
    }

    #[test]
    fn default_space_k_keymap_requests_hover_docs() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let k = KeyEvent::from_str("k").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, k) {
            KeymapResult::Matched(command) => {
                assert_eq!(
                    native_picker_command(&command),
                    Some(NativePickerRequest::HoverDocs)
                );
            }
            _ => panic!("expected SPACE-k to resolve to hover"),
        }
    }

    #[test]
    fn history_commands_are_classified_separately() {
        assert!(native_history_command(&MappableCommand::undo));
        assert!(native_history_command(&MappableCommand::redo));
        assert!(!native_history_command(&MappableCommand::earlier));
        assert!(!native_history_command(&MappableCommand::later));
        assert!(!native_history_command(&MappableCommand::normal_mode));
    }

    #[test]
    fn search_commands_are_classified_separately() {
        assert!(native_search_command(&MappableCommand::search_next));
        assert!(native_search_command(&MappableCommand::search_prev));
        assert!(native_search_command(&MappableCommand::extend_search_next));
        assert!(native_search_command(&MappableCommand::extend_search_prev));
        assert!(native_search_command(&MappableCommand::search_selection));
        assert!(native_search_command(
            &MappableCommand::search_selection_detect_word_boundaries
        ));
        assert!(!native_search_command(&MappableCommand::search));
        assert!(!native_search_command(&MappableCommand::rsearch));
        assert!(!native_search_command(&MappableCommand::global_search));
        assert!(!native_search_command(&MappableCommand::select_regex));
    }

    #[test]
    fn register_edit_commands_are_classified_separately() {
        assert!(native_register_edit_command(
            &MappableCommand::delete_selection
        ));
        assert!(native_register_edit_command(
            &MappableCommand::delete_selection_noyank
        ));
        assert!(native_register_edit_command(
            &MappableCommand::change_selection
        ));
        assert!(native_register_edit_command(
            &MappableCommand::change_selection_noyank
        ));
        assert!(native_register_edit_command(&MappableCommand::yank));
        assert!(native_register_edit_command(
            &MappableCommand::yank_to_clipboard
        ));
        assert!(native_register_edit_command(
            &MappableCommand::yank_to_primary_clipboard
        ));
        assert!(native_register_edit_command(&MappableCommand::yank_joined));
        assert!(native_register_edit_command(
            &MappableCommand::yank_joined_to_clipboard
        ));
        assert!(native_register_edit_command(
            &MappableCommand::yank_joined_to_primary_clipboard
        ));
        assert!(native_register_edit_command(
            &MappableCommand::yank_main_selection_to_clipboard
        ));
        assert!(native_register_edit_command(
            &MappableCommand::yank_main_selection_to_primary_clipboard
        ));
        assert!(native_register_edit_command(&MappableCommand::paste_after));
        assert!(native_register_edit_command(&MappableCommand::paste_before));
        assert!(native_register_edit_command(
            &MappableCommand::paste_clipboard_after
        ));
        assert!(native_register_edit_command(
            &MappableCommand::paste_clipboard_before
        ));
        assert!(native_register_edit_command(
            &MappableCommand::paste_primary_clipboard_after
        ));
        assert!(native_register_edit_command(
            &MappableCommand::paste_primary_clipboard_before
        ));
        assert!(native_register_edit_command(
            &MappableCommand::replace_with_yanked
        ));
        assert!(native_register_edit_command(
            &MappableCommand::replace_selections_with_clipboard
        ));
        assert!(native_register_edit_command(
            &MappableCommand::replace_selections_with_primary_clipboard
        ));
        assert!(!native_register_edit_command(&MappableCommand::undo));
        assert!(!native_register_edit_command(&MappableCommand::normal_mode));
        assert!(!native_register_edit_command(
            &MappableCommand::goto_definition
        ));
    }

    #[test]
    fn selection_transform_commands_are_classified_separately() {
        assert!(native_selection_transform_command(
            &MappableCommand::copy_selection_on_prev_line
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::copy_selection_on_next_line
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::split_selection_on_newline
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::merge_selections
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::merge_consecutive_selections
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::shrink_to_line_bounds
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::ensure_selections_forward
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::trim_selections
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::align_selections
        ));
        assert!(native_selection_transform_command(&MappableCommand::indent));
        assert!(native_selection_transform_command(
            &MappableCommand::unindent
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::join_selections
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::join_selections_space
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::switch_case
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::switch_to_lowercase
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::switch_to_uppercase
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selections_forward
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selections_backward
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selections_first
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selections_last
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selection_contents_forward
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::rotate_selection_contents_backward
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::reverse_selection_contents
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::expand_selection
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::shrink_selection
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::select_next_sibling
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::select_prev_sibling
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::select_all_siblings
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::select_all_children
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::toggle_comments
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::toggle_line_comments
        ));
        assert!(native_selection_transform_command(
            &MappableCommand::toggle_block_comments
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::split_selection
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::select_regex
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::format_selections
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::keep_selections
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::remove_selections
        ));
        assert!(!native_selection_transform_command(
            &MappableCommand::normal_mode
        ));
    }

    #[test]
    fn textobject_commands_are_classified_separately() {
        assert!(native_textobject_command(
            &MappableCommand::select_textobject_around
        ));
        assert!(native_textobject_command(
            &MappableCommand::select_textobject_inner
        ));
        assert!(!native_textobject_command(&MappableCommand::surround_add));
        assert!(!native_textobject_command(&MappableCommand::normal_mode));
        assert!(!native_insert_command_supported(
            &MappableCommand::select_textobject_inner
        ));
    }

    #[test]
    fn surround_commands_are_classified_separately() {
        assert!(native_surround_command(&MappableCommand::surround_add));
        assert!(native_surround_command(&MappableCommand::surround_replace));
        assert!(native_surround_command(&MappableCommand::surround_delete));
        assert!(!native_surround_command(
            &MappableCommand::select_textobject_inner
        ));
        assert!(!native_surround_command(&MappableCommand::normal_mode));
        assert!(!native_insert_command_supported(
            &MappableCommand::surround_add
        ));
    }

    #[test]
    fn default_bracket_syntax_object_keymaps_are_native() {
        for (prefix, key, expected) in [
            ("[", "f", MappableCommand::goto_prev_function),
            ("[", "t", MappableCommand::goto_prev_class),
            ("[", "a", MappableCommand::goto_prev_parameter),
            ("[", "c", MappableCommand::goto_prev_comment),
            ("[", "e", MappableCommand::goto_prev_entry),
            ("[", "T", MappableCommand::goto_prev_test),
            ("[", "x", MappableCommand::goto_prev_xml_element),
            ("]", "f", MappableCommand::goto_next_function),
            ("]", "t", MappableCommand::goto_next_class),
            ("]", "a", MappableCommand::goto_next_parameter),
            ("]", "c", MappableCommand::goto_next_comment),
            ("]", "e", MappableCommand::goto_next_entry),
            ("]", "T", MappableCommand::goto_next_test),
            ("]", "x", MappableCommand::goto_next_xml_element),
        ] {
            let mut keymaps = Keymaps::default();
            let prefix = KeyEvent::from_str(prefix).unwrap();
            let key = KeyEvent::from_str(key).unwrap();

            assert!(matches!(
                keymaps.get(Mode::Normal, prefix),
                KeymapResult::Pending(_)
            ));

            match keymaps.get(Mode::Normal, key) {
                KeymapResult::Matched(command) => {
                    assert_eq!(command, expected);
                    assert!(native_command_supported(&command));
                    assert!(native_syntax_object_navigation_command(&command));
                }
                _ => panic!("expected bracket syntax-object keymap to resolve"),
            }
        }
    }

    #[test]
    fn default_space_comment_keymaps_are_native_transforms() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let space = KeyEvent::from_str("space").unwrap();
        let c = KeyEvent::from_str("c").unwrap();
        let shift_c = KeyEvent::from_str("C").unwrap();
        let alt_c = KeyEvent::from_str("A-c").unwrap();

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, c) {
            KeymapResult::Matched(command) => {
                assert_eq!(command, MappableCommand::toggle_comments);
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected SPACE-c to resolve to toggle_comments"),
        }

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, shift_c) {
            KeymapResult::Matched(command) => {
                assert_eq!(command, MappableCommand::toggle_block_comments);
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected SPACE-C to resolve to toggle_block_comments"),
        }

        assert!(matches!(
            keymaps.get(Mode::Normal, space),
            KeymapResult::Pending(_)
        ));

        match keymaps.get(Mode::Normal, alt_c) {
            KeymapResult::Matched(command) => {
                assert_eq!(command, MappableCommand::toggle_line_comments);
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected SPACE-A-c to resolve to toggle_line_comments"),
        }
    }

    #[test]
    fn default_ctrl_c_keymap_is_native_comment_toggle() {
        use std::str::FromStr;

        let mut keymaps = Keymaps::default();
        let ctrl_c = KeyEvent::from_str("C-c").unwrap();

        match keymaps.get(Mode::Normal, ctrl_c) {
            KeymapResult::Matched(command) => {
                assert_eq!(command, MappableCommand::toggle_comments);
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected C-c to resolve to toggle_comments"),
        }
    }

    #[test]
    fn insert_edit_commands_are_classified_separately() {
        assert!(native_insert_edit_command(
            &MappableCommand::commit_undo_checkpoint
        ));
        assert!(native_insert_edit_command(
            &MappableCommand::delete_char_backward
        ));
        assert!(native_insert_edit_command(
            &MappableCommand::delete_char_forward
        ));
        assert!(native_insert_edit_command(
            &MappableCommand::delete_word_backward
        ));
        assert!(native_insert_edit_command(
            &MappableCommand::delete_word_forward
        ));
        assert!(native_insert_edit_command(&MappableCommand::insert_newline));
        assert!(native_insert_edit_command(&MappableCommand::insert_tab));
        assert!(native_insert_edit_command(
            &MappableCommand::kill_to_line_end
        ));
        assert!(native_insert_edit_command(
            &MappableCommand::kill_to_line_start
        ));
        assert!(native_insert_edit_command(&MappableCommand::smart_tab));
        assert!(!native_insert_edit_command(&MappableCommand::completion));
        assert!(!native_insert_edit_command(
            &MappableCommand::insert_register
        ));
        assert!(!native_insert_edit_command(&MappableCommand::insert_mode));
        assert!(!native_insert_edit_command(&MappableCommand::normal_mode));
    }

    #[test]
    fn insert_interactive_commands_are_classified_separately() {
        assert!(native_insert_interactive_command(
            &MappableCommand::insert_register
        ));
        assert!(!native_insert_interactive_command(
            &MappableCommand::completion
        ));
        assert!(!native_insert_interactive_command(
            &MappableCommand::insert_newline
        ));
        assert!(!native_insert_interactive_command(
            &MappableCommand::normal_mode
        ));
    }

    #[test]
    fn insert_completion_commands_are_classified_separately() {
        assert!(native_insert_completion_command(
            &MappableCommand::completion
        ));
        assert!(!native_insert_completion_command(
            &MappableCommand::insert_register
        ));
        assert!(!native_insert_completion_command(
            &MappableCommand::insert_newline
        ));
        assert!(!native_insert_completion_command(
            &MappableCommand::normal_mode
        ));
    }

    #[test]
    fn native_insert_entry_starts_insert_replay() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let enter_insert = KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::empty(),
        };
        let existing = KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::empty(),
        };
        input.current_insert_replay.keys.push(existing);

        input.seed_insert_replay_if_needed(Mode::Normal, Mode::Insert, &[enter_insert]);

        assert_eq!(input.current_insert_replay.keys, vec![enter_insert]);
        assert_eq!(input.last_insert_replay, None);
    }

    #[test]
    fn non_insert_entry_commands_do_not_seed_insert_replay() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let movement = KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::empty(),
        };

        input.seed_insert_replay_if_needed(Mode::Normal, Mode::Normal, &[movement]);

        assert!(input.current_insert_replay.is_empty());
    }

    #[test]
    fn insert_pseudo_pending_key_is_recorded_for_replay() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let register_key = KeyEvent {
            code: KeyCode::Char('"'),
            modifiers: KeyModifiers::empty(),
        };

        input.record_insert_replay_key_if_needed(Mode::Insert, register_key);

        assert_eq!(input.current_insert_replay.keys, vec![register_key]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn insert_editor_fallback_is_recorded_for_replay() {
        let mut editor = test_editor_with_text("");
        editor.mode = Mode::Insert;
        let mut input = NativeCommandInput::new_for_platform(
            disposition_test_keymaps(Mode::Insert),
            TargetPlatform::Windows,
        );
        let key = KeyEvent::from_str("S-ins").unwrap();

        let _ = handle_native_test_key(&mut input, &mut editor, "S-ins");

        assert_eq!(input.current_insert_replay.keys, vec![key]);
    }

    #[test]
    fn command_pseudo_pending_key_is_not_recorded_for_insert_replay() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let register_key = KeyEvent {
            code: KeyCode::Char('"'),
            modifiers: KeyModifiers::empty(),
        };

        input.record_insert_replay_key_if_needed(Mode::Normal, register_key);

        assert!(input.current_insert_replay.is_empty());
    }

    #[test]
    fn native_insert_replay_finishes_as_native() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let enter_insert = KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::empty(),
        };
        let inserted = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
        };
        let escape = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
        };

        input.seed_insert_replay_if_needed(Mode::Normal, Mode::Insert, &[enter_insert]);
        input.current_insert_replay.keys.push(inserted);
        input.current_insert_replay.keys.push(escape);
        input.finish_insert_replay_if_needed(Mode::Insert, Mode::Normal);

        assert_eq!(
            input.last_insert_replay.as_ref().map(|replay| &replay.keys),
            Some(&vec![enter_insert, inserted, escape])
        );
        assert!(input.current_insert_replay.is_empty());
    }

    #[test]
    fn command_count_digit_requires_plain_digit_keys() {
        assert_eq!(
            command_count_digit(KeyEvent {
                code: KeyCode::Char('3'),
                modifiers: KeyModifiers::empty()
            }),
            Some(3)
        );
        assert_eq!(
            command_count_digit(KeyEvent {
                code: KeyCode::Char('3'),
                modifiers: KeyModifiers::CONTROL
            }),
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_reports_unhandled_native_file_url() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("https://example.com\n");
        set_test_cursor(&mut editor, 0);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let pending = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "g");
        assert!(pending.handled_by_native_command);
        assert!(pending.unhandled_keys.is_empty());

        let outcome = handle_key_str(&mut bridge, &mut editor, &mut compositor, &mut jobs, "f");

        assert!(!outcome.handled_by_native_command);
        assert_eq!(
            outcome.unhandled_keys,
            vec![
                KeyEvent::from_str("g").unwrap(),
                KeyEvent::from_str("f").unwrap()
            ]
        );
        assert_eq!(outcome.picker_requested, None);
        assert_eq!(outcome.prompt_requested, None);
    }
}
