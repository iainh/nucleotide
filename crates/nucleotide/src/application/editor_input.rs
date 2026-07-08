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
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

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
    pub viewport_scroll_requested: Option<nucleotide_editor::EditorViewportScrollRequest>,
    pub viewport_cursor_requested: Option<nucleotide_editor::EditorViewportCursorRequest>,
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
struct SelectionSnapshot {
    cursor: usize,
    line: usize,
}

impl EditorInputBridge {
    pub fn new(native_keymaps: Keymaps) -> Self {
        Self {
            native_commands: NativeCommandInput::new(native_keymaps),
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        let _timer = PerfTimer::new("EditorInputBridge::handle_key")
            .with_warn_threshold(Duration::from_millis(4));
        log_completion_key_context(editor, key);

        let before_focused_view_id = editor.tree.focus;
        let before_focused_doc_id = editor
            .tree
            .try_get(before_focused_view_id)
            .map(|view| view.doc);
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
        let mut viewport_scroll_requested = None;
        let mut viewport_cursor_requested = None;
        let native_input_result = self
            .native_commands
            .handle_key(key, compositor, editor, jobs);
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
            NativeInputResult::RequestViewportScroll(request) => {
                handled_by_native_command = true;
                viewport_scroll_requested = Some(request);
            }
            NativeInputResult::RequestViewportCursor(request) => {
                handled_by_native_command = true;
                viewport_cursor_requested = Some(request);
            }
            NativeInputResult::Unhandled(keys) => {
                debug!(?keys, "Native editor input was not handled");
                unhandled_keys = keys;
            }
        }

        let focused_view_id = editor.tree.focus;
        let focused_doc_id = editor.tree.try_get(focused_view_id).map(|view| view.doc);
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
            viewport_scroll_requested,
            viewport_cursor_requested,
        }
    }
}

struct NativeCommandInput {
    keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    current_insert_replay: InsertReplay,
    last_insert_replay: Option<InsertReplay>,
    reset_diff_change_executed: bool,
}

enum NativeInputResult {
    Handled {
        completion_requested: Option<NativeCompletionRequest>,
        picker_requested: Option<NativePickerRequest>,
        prompt_requested: Option<NativePromptRequest>,
    },
    RequestLspNavigation(NativeLspNavigationRequest),
    RequestWorkspace(NativeWorkspaceRequest),
    RequestViewportScroll(nucleotide_editor::EditorViewportScrollRequest),
    RequestViewportCursor(nucleotide_editor::EditorViewportCursorRequest),
    Unhandled(Vec<KeyEvent>),
}

enum NativeCommandResult {
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
    RequestViewportScroll {
        callbacks: Vec<compositor::Callback>,
        request: nucleotide_editor::EditorViewportScrollRequest,
    },
    RequestViewportCursor {
        callbacks: Vec<compositor::Callback>,
        request: nucleotide_editor::EditorViewportCursorRequest,
    },
    ReplayInsert {
        keys: Vec<KeyEvent>,
        count: usize,
    },
    Unhandled(Vec<KeyEvent>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InsertReplay {
    keys: Vec<KeyEvent>,
}

impl InsertReplay {
    fn from_keys(keys: &[KeyEvent]) -> Self {
        Self {
            keys: keys.to_vec(),
        }
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl NativeCommandInput {
    fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            current_insert_replay: InsertReplay::default(),
            last_insert_replay: None,
            reset_diff_change_executed: false,
        }
    }

    fn take_reset_diff_change_executed(&mut self) -> bool {
        std::mem::take(&mut self.reset_diff_change_executed)
    }

    fn handle_key(
        &mut self,
        mut key: KeyEvent,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> NativeInputResult {
        canonicalize_key(&mut key);
        let mode_before = editor.mode();

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

        match command_result {
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
            NativeCommandResult::ReplayInsert { keys, count } => {
                for _ in 0..count {
                    for replay_key in keys.iter().copied() {
                        match self.handle_key(replay_key, compositor, editor, jobs) {
                            NativeInputResult::Handled { .. } => {}
                            NativeInputResult::RequestLspNavigation(request) => {
                                return NativeInputResult::RequestLspNavigation(request);
                            }
                            NativeInputResult::RequestWorkspace(request) => {
                                return NativeInputResult::RequestWorkspace(request);
                            }
                            NativeInputResult::RequestViewportScroll(request) => {
                                return NativeInputResult::RequestViewportScroll(request);
                            }
                            NativeInputResult::RequestViewportCursor(request) => {
                                return NativeInputResult::RequestViewportCursor(request);
                            }
                            NativeInputResult::Unhandled(unhandled_keys) => {
                                return NativeInputResult::Unhandled(unhandled_keys);
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
            NativeCommandResult::Unhandled(keys) => {
                self.discard_insert_replay_if_needed(mode_before);
                NativeInputResult::Unhandled(keys)
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
                    return NativeCommandResult::Unhandled(unhandled_keys);
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
                    return NativeCommandResult::Unhandled(unhandled_keys);
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
                    return NativeCommandResult::Unhandled(unhandled_keys);
                }

                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if let Some(command) = native_insert_shortcut_command(key) {
                    let mut last_mode = mode;
                    execute_native_command(command, context, &mut last_mode);
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if let Some(ch) = key.char() {
                    commands::insert::insert_char(context, ch);
                    self.current_insert_replay.keys.push(key);
                    NativeCommandResult::Handled(Vec::new())
                } else {
                    NativeCommandResult::Unhandled(unhandled_keys)
                }
            }
            KeymapResult::Cancelled(_) => NativeCommandResult::Unhandled(unhandled_keys),
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
                return NativeCommandResult::ReplayInsert {
                    keys: replay.keys,
                    count,
                };
            }
            return NativeCommandResult::Unhandled(vec![key]);
        }

        context.count = context.editor.count;
        context.register = context.editor.selected_register.take();

        let pending_keys = self.keymaps.pending().to_vec();
        let mut unhandled_keys = pending_keys;
        unhandled_keys.push(key);
        let replay_keys = unhandled_keys.clone();

        match self.handle_keymap_event(mode, context, key) {
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
            KeymapDispatch::Unhandled => {
                context.editor.selected_register = context.register.take();
                if let Some(request) = native_workspace_key_sequence(&unhandled_keys) {
                    context.editor.count = None;
                    return NativeCommandResult::RequestWorkspace {
                        callbacks: Vec::new(),
                        request,
                    };
                }
                if self.keymaps.pending().is_empty() {
                    context.editor.count = None;
                }
                NativeCommandResult::Unhandled(unhandled_keys)
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

        match &key_result {
            KeymapResult::Matched(command) => {
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

                if !native_command_supported(command) {
                    return KeymapDispatch::Unhandled;
                }
                self.record_reset_diff_change_if_needed(command);
                execute_native_command(command, context, &mut last_mode);
                KeymapDispatch::Handled
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
                        || native_viewport_cursor_command(command, None).is_some()
                        || native_page_cursor_command_supported(command)
                        || native_viewport_scroll_command(command, None).is_some()
                }) {
                    return KeymapDispatch::Unhandled;
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
                    KeymapDispatch::Unhandled
                }
            }
            KeymapResult::Cancelled(_) => KeymapDispatch::Unhandled,
        }
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
    Handled,
    RequestPicker(NativePickerRequest),
    RequestPrompt(NativePromptRequest),
    RequestLspNavigation(NativeLspNavigationRequest),
    RequestWorkspace(NativeWorkspaceRequest),
    RequestViewportScroll(nucleotide_editor::EditorViewportScrollRequest),
    RequestViewportCursor(nucleotide_editor::EditorViewportCursorRequest),
    Pending,
    Unhandled,
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

fn native_workspace_key_sequence(keys: &[KeyEvent]) -> Option<NativeWorkspaceRequest> {
    match keys {
        [space, key] if is_plain_space_key(*space) && is_plain_char_key(*key, 't') => {
            Some(NativeWorkspaceRequest::ToggleFileTree)
        }
        _ => None,
    }
}

fn is_plain_space_key(key: KeyEvent) -> bool {
    is_plain_char_key(key, ' ')
}

fn is_plain_char_key(key: KeyEvent, expected: char) -> bool {
    matches!(
        key,
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers
        } if ch == expected && modifiers.is_empty()
    )
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
        return KeymapDispatch::Unhandled;
    };

    let mut paths = Vec::new();
    for target in targets {
        if target_is_external_url(&target) {
            return KeymapDispatch::Unhandled;
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

fn native_insert_shortcut_command(key: KeyEvent) -> Option<&'static MappableCommand> {
    match key {
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
        } if ch.eq_ignore_ascii_case(&'v') && modifiers == KeyModifiers::CONTROL => {
            Some(&MappableCommand::paste_clipboard_after)
        }
        _ => None,
    }
}

fn canonicalize_key(key: &mut KeyEvent) {
    if matches!(key.code, KeyCode::Char(_)) {
        key.modifiers.remove(KeyModifiers::SHIFT);
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
        test_editor_with_text_and_config(text, Config::default())
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

    fn reset_diff_change_keymaps() -> Keymaps {
        use helix_term::config::Config as HelixConfig;
        use helix_term::keymap::{KeyTrie, KeyTrieNode, merge_keys};
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let space = KeyEvent::from_str("space").unwrap();
        let v = KeyEvent::from_str("v").unwrap();
        let r = KeyEvent::from_str("r").unwrap();

        let mut vcs_node = IndexMap::new();
        vcs_node.insert(
            r,
            KeyTrie::MappableCommand(MappableCommand::from_str(":reset-diff-change").unwrap()),
        );

        let mut space_node = IndexMap::new();
        space_node.insert(v, KeyTrie::Node(KeyTrieNode::new("VCS", vcs_node)));

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
    fn native_insert_shortcut_maps_ctrl_v_to_clipboard_paste() {
        assert_eq!(
            native_insert_shortcut_command(KeyEvent::from_str("C-v").unwrap()),
            Some(&MappableCommand::paste_clipboard_after)
        );
        assert_eq!(
            native_insert_shortcut_command(KeyEvent::from_str("C-S-v").unwrap()),
            Some(&MappableCommand::paste_clipboard_after)
        );
        assert_eq!(native_insert_shortcut_command(plain_char_key('v')), None);
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
    fn workspace_leader_sequences_are_classified_separately() {
        assert_eq!(
            native_workspace_key_sequence(&[
                KeyEvent::from_str("space").unwrap(),
                KeyEvent::from_str("t").unwrap(),
            ]),
            Some(NativeWorkspaceRequest::ToggleFileTree)
        );
        assert_eq!(
            native_workspace_key_sequence(&[
                KeyEvent::from_str("space").unwrap(),
                KeyEvent::from_str("f").unwrap(),
            ]),
            None
        );
        assert_eq!(
            native_workspace_key_sequence(&[KeyEvent::from_str("t").unwrap()]),
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
    async fn editor_input_bridge_requests_file_picker_for_space_f() {
        let mut bridge = EditorInputBridge::new(Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let space = KeyEvent::from_str("space").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        let pending = bridge.handle_key(space, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert_eq!(pending.picker_requested, None);

        let picker = bridge.handle_key(f, &mut compositor, &mut editor, &mut jobs);
        assert!(picker.handled_by_native_command);
        assert_eq!(picker.picker_requested, Some(NativePickerRequest::File));
        assert_eq!(picker.workspace_requested, None);
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
        let mut bridge = EditorInputBridge::new(Keymaps::default());
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
    fn native_completion_request_carries_focused_document_and_view() {
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();
        let request = NativeCompletionRequest { doc_id, view_id };

        assert_eq!(request.doc_id, doc_id);
        assert_eq!(request.view_id, view_id);
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
