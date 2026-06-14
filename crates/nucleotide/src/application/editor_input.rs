// ABOUTME: Native editor input boundary for GPUI-driven document views
// ABOUTME: Isolates the remaining Helix terminal input bridge from Application

use helix_stdx::{
    path::{self, find_paths},
    rope::RopeSliceExt,
};
use helix_term::{
    commands::{self, MappableCommand, OnKeyCallback, OnKeyCallbackKind},
    compositor::{self, Component, Compositor, EventResult},
    events::{OnModeSwitch, PostCommand},
    job::Jobs,
    keymap::{KeymapResult, Keymaps},
    ui::EditorView,
};
use helix_view::{
    DocumentId, Editor, ViewId,
    document::Mode,
    editor::Action,
    input::{Event, KeyEvent},
    keyboard::{KeyCode, KeyModifiers},
};
use nucleotide_logging::{debug, info};
use std::{
    borrow::Cow,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorInputOutcome {
    pub focused_view_id: ViewId,
    pub focused_doc_id: Option<DocumentId>,
    pub selection_changed: bool,
    pub handled_by_compositor: bool,
    pub handled_by_native_command: bool,
    pub handled_by_terminal_editor: bool,
    pub completion_requested: Option<NativeCompletionRequest>,
    pub picker_requested: Option<NativePickerRequest>,
    pub lsp_navigation_requested: Option<NativeLspNavigationRequest>,
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
    terminal_editor: EditorView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionSnapshot {
    cursor: usize,
    line: usize,
}

impl EditorInputBridge {
    pub fn new(native_keymaps: Keymaps, terminal_keymaps: Keymaps) -> Self {
        Self {
            native_commands: NativeCommandInput::new(native_keymaps),
            terminal_editor: EditorView::new(terminal_keymaps),
        }
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        compositor: &mut Compositor,
        editor: &mut Editor,
        jobs: &mut Jobs,
    ) -> EditorInputOutcome {
        log_completion_key_context(editor, key);

        let focused_view_id = editor.tree.focus;
        let focused_doc_id = editor.tree.try_get(focused_view_id).map(|view| view.doc);
        let before_selection =
            focused_doc_id.and_then(|doc_id| selection_snapshot(editor, doc_id, focused_view_id));

        if let Some(snapshot) = before_selection {
            debug!(
                cursor_pos = snapshot.cursor,
                line = snapshot.line,
                "Before key"
            );
        }

        let mut context = compositor::Context {
            editor,
            scroll: None,
            jobs,
        };
        let event = Event::Key(key);
        let handled_by_compositor = compositor.handle_event(&event, &mut context);
        let mut handled_by_native_command = false;
        let mut handled_by_terminal_editor = false;
        let mut completion_requested = None;
        let mut picker_requested = None;
        let mut lsp_navigation_requested = None;
        if !handled_by_compositor {
            let mode_before_fallback = context.editor.mode();
            match self
                .native_commands
                .handle_key(key, compositor, context.editor, context.jobs)
            {
                NativeInputResult::Handled {
                    completion_requested: request,
                    picker_requested: picker_request,
                } => {
                    handled_by_native_command = true;
                    completion_requested = request;
                    picker_requested = picker_request;
                }
                NativeInputResult::RequestLspNavigation(request) => {
                    handled_by_native_command = true;
                    lsp_navigation_requested = Some(request);
                }
                NativeInputResult::Fallback(keys) => {
                    for key in &keys {
                        let event = Event::Key(*key);
                        handled_by_terminal_editor |=
                            self.handle_terminal_editor_event(&event, compositor, &mut context);
                    }
                    self.native_commands.observe_terminal_fallback(
                        mode_before_fallback,
                        context.editor.mode(),
                        &keys,
                    );
                }
            }
        };

        let after_selection = focused_doc_id
            .and_then(|doc_id| selection_snapshot(context.editor, doc_id, focused_view_id));

        if let Some(snapshot) = after_selection {
            debug!(
                cursor_pos = snapshot.cursor,
                line = snapshot.line,
                "After key"
            );
        }

        let selection_changed = selection_changed(before_selection, after_selection);
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
            handled_by_compositor,
            handled_by_native_command,
            handled_by_terminal_editor,
            completion_requested,
            picker_requested,
            lsp_navigation_requested,
        }
    }

    fn handle_terminal_editor_event(
        &mut self,
        event: &Event,
        compositor: &mut Compositor,
        context: &mut compositor::Context<'_>,
    ) -> bool {
        match self.terminal_editor.handle_event(event, context) {
            EventResult::Consumed(Some(callback)) => {
                callback(compositor, context);
                true
            }
            EventResult::Consumed(None) => true,
            EventResult::Ignored(_) => false,
        }
    }
}

struct NativeCommandInput {
    keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    current_insert_replay: InsertReplay,
    last_insert_replay: Option<InsertReplay>,
}

enum NativeInputResult {
    Handled {
        completion_requested: Option<NativeCompletionRequest>,
        picker_requested: Option<NativePickerRequest>,
    },
    RequestLspNavigation(NativeLspNavigationRequest),
    Fallback(Vec<KeyEvent>),
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
    RequestLspNavigation {
        callbacks: Vec<compositor::Callback>,
        request: NativeLspNavigationRequest,
    },
    ReplayInsert {
        keys: Vec<KeyEvent>,
        count: usize,
    },
    Fallback(Vec<KeyEvent>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InsertReplay {
    keys: Vec<KeyEvent>,
    native: bool,
}

impl Default for InsertReplay {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            native: true,
        }
    }
}

impl InsertReplay {
    fn native(keys: &[KeyEvent]) -> Self {
        Self {
            keys: keys.to_vec(),
            native: true,
        }
    }

    fn terminal(keys: &[KeyEvent]) -> Self {
        Self {
            keys: keys.to_vec(),
            native: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn mark_terminal(&mut self) {
        self.native = false;
    }
}

impl NativeCommandInput {
    fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            current_insert_replay: InsertReplay::default(),
            last_insert_replay: None,
        }
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
                }
            }
            NativeCommandResult::RequestCompletion { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: request,
                    picker_requested: None,
                }
            }
            NativeCommandResult::RequestPicker { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: Some(request),
                }
            }
            NativeCommandResult::RequestLspNavigation { callbacks, request } => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::RequestLspNavigation(request)
            }
            NativeCommandResult::ReplayInsert { keys, count } => {
                for _ in 0..count {
                    for replay_key in keys.iter().copied() {
                        match self.handle_key(replay_key, compositor, editor, jobs) {
                            NativeInputResult::Handled { .. } => {}
                            NativeInputResult::RequestLspNavigation(request) => {
                                return NativeInputResult::RequestLspNavigation(request);
                            }
                            NativeInputResult::Fallback(fallback_keys) => {
                                return NativeInputResult::Fallback(fallback_keys);
                            }
                        }
                    }
                }
                NativeInputResult::Handled {
                    completion_requested: None,
                    picker_requested: None,
                }
            }
            NativeCommandResult::Fallback(keys) => NativeInputResult::Fallback(keys),
        }
    }

    fn observe_terminal_fallback(
        &mut self,
        mode_before: Mode,
        mode_after: Mode,
        fallback_keys: &[KeyEvent],
    ) {
        if mode_before != Mode::Insert && mode_after == Mode::Insert {
            self.current_insert_replay = InsertReplay::terminal(fallback_keys);
            return;
        }

        if mode_before == Mode::Insert {
            self.current_insert_replay.mark_terminal();
            self.current_insert_replay
                .keys
                .extend_from_slice(fallback_keys);
            self.finish_insert_replay_if_needed(mode_before, mode_after);
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
        let mut fallback_keys = pending_keys;
        fallback_keys.push(key);

        let key_result = self.keymaps.get(mode, key);
        context.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        match &key_result {
            KeymapResult::Matched(command) => {
                if native_insert_completion_command(command) {
                    self.current_insert_replay.mark_terminal();
                    self.current_insert_replay.keys.extend(fallback_keys);
                    return NativeCommandResult::RequestCompletion {
                        callbacks: Vec::new(),
                        request: completion_request(context),
                    };
                }
                if !native_insert_command_supported(command) {
                    return NativeCommandResult::Fallback(fallback_keys);
                }
                let mut last_mode = mode;
                execute_native_command(command, context, &mut last_mode);
                self.current_insert_replay.keys.extend(fallback_keys);
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
                    return NativeCommandResult::Fallback(fallback_keys);
                }
                let mut last_mode = mode;
                let mut completion_requested = None;
                for command in commands {
                    if native_insert_completion_command(command) {
                        self.current_insert_replay.mark_terminal();
                        completion_requested = completion_request(context);
                    } else {
                        execute_native_command(command, context, &mut last_mode);
                    }
                }
                self.current_insert_replay.keys.extend(fallback_keys);
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
                    return NativeCommandResult::Fallback(fallback_keys);
                }

                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    self.current_insert_replay.keys.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if let Some(ch) = key.char() {
                    commands::insert::insert_char(context, ch);
                    self.current_insert_replay.keys.push(key);
                    NativeCommandResult::Handled(Vec::new())
                } else {
                    NativeCommandResult::Fallback(fallback_keys)
                }
            }
            KeymapResult::Cancelled(_) => NativeCommandResult::Fallback(fallback_keys),
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
                if replay.native {
                    let count = context.editor.count.map_or(1, NonZeroUsize::get);
                    context.editor.count = None;
                    return NativeCommandResult::ReplayInsert {
                        keys: replay.keys,
                        count,
                    };
                }

                return NativeCommandResult::Fallback(replay.keys);
            }
            return NativeCommandResult::Fallback(vec![key]);
        }

        context.count = context.editor.count;
        context.register = context.editor.selected_register.take();

        let pending_keys = self.keymaps.pending().to_vec();
        let mut fallback_keys = pending_keys;
        fallback_keys.push(key);
        let replay_keys = fallback_keys.clone();

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
            KeymapDispatch::Pending => {
                context.editor.selected_register = context.register.take();
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapDispatch::Fallback => {
                context.editor.selected_register = context.register.take();
                NativeCommandResult::Fallback(fallback_keys)
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
            self.current_insert_replay = InsertReplay::native(replay_keys);
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

                if let Some(action) = native_file_navigation_command(command) {
                    return handle_native_file_navigation(context, action);
                }

                if let Some(request) = native_lsp_navigation_command(command) {
                    return KeymapDispatch::RequestLspNavigation(request);
                }

                if !native_command_supported(command) {
                    return KeymapDispatch::Fallback;
                }
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
                        || native_lsp_navigation_command(command).is_some()
                }) {
                    return KeymapDispatch::Fallback;
                }

                for command in commands {
                    if let Some(request) = native_picker_command(command) {
                        return KeymapDispatch::RequestPicker(request);
                    }
                    if let Some(action) = native_file_navigation_command(command) {
                        return handle_native_file_navigation(context, action);
                    }
                    if let Some(request) = native_lsp_navigation_command(command) {
                        return KeymapDispatch::RequestLspNavigation(request);
                    }
                    execute_native_command(command, context, &mut last_mode);
                }
                KeymapDispatch::Handled
            }
            KeymapResult::NotFound => {
                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    KeymapDispatch::Handled
                } else {
                    KeymapDispatch::Fallback
                }
            }
            KeymapResult::Cancelled(_) => KeymapDispatch::Fallback,
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
    RequestLspNavigation(NativeLspNavigationRequest),
    Pending,
    Fallback,
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
        let (view, doc) = helix_view::current!(editor);
        doc.append_changes_to_history(view);
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
        || native_prompt_command(command)
        || native_history_command(command)
        || native_register_edit_command(command)
        || native_register_selection_command(command)
        || native_selection_transform_command(command)
        || native_search_command(command)
        || native_buffer_navigation_command(command)
        || native_diagnostic_navigation_command(command)
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
                | "page_cursor_up_select"
                | "page_cursor_down_select"
                | "page_cursor_half_up_select"
                | "page_cursor_half_down_select"
                | "remove_primary_selection"
                | "scroll_down"
                | "scroll_up"
                | "select_all"
                | "select_line_above"
                | "select_line_below"
                | "select_mode"
                | "match_brackets"
        )
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

fn native_prompt_command(command: &MappableCommand) -> bool {
    matches!(command.name(), "command_mode" | "search" | "rsearch")
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

fn native_file_navigation_command(command: &MappableCommand) -> Option<Action> {
    match command.name() {
        "goto_file" => Some(Action::Replace),
        "goto_file_hsplit" => Some(Action::HorizontalSplit),
        "goto_file_vsplit" => Some(Action::VerticalSplit),
        _ => None,
    }
}

fn handle_native_file_navigation(
    context: &mut commands::Context<'_>,
    action: Action,
) -> KeymapDispatch {
    let (base_path, targets) = native_file_navigation_targets(context.editor);

    for target in targets {
        if url::Url::parse(&target).is_ok() {
            return KeymapDispatch::Fallback;
        }

        let target_path = path::expand(&target);
        let target_path = base_path.join(target_path.as_ref());

        if target_path.is_dir() {
            return KeymapDispatch::RequestPicker(NativePickerRequest::FileAt(target_path));
        }

        if let Err(err) = context.editor.open(&target_path, action) {
            context
                .editor
                .set_error(format!("Open file failed: {:?}", err));
        }
    }

    KeymapDispatch::Handled
}

fn native_file_navigation_targets(editor: &Editor) -> (PathBuf, Vec<String>) {
    let (view, doc) = helix_view::current_ref!(editor);
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

    (base_path, targets)
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

fn native_insert_command_supported(command: &MappableCommand) -> bool {
    (!native_insert_entry_command(command)
        && !native_prompt_command(command)
        && !native_history_command(command)
        && !native_register_edit_command(command)
        && !native_register_selection_command(command)
        && !native_selection_transform_command(command)
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
    use std::str::FromStr;
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use helix_core::{Selection, Transaction, syntax};
    use helix_view::{editor::Action, editor::Config, graphics::Rect, handlers::Handlers, theme};

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor_with_text(text: &str) -> Editor {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        editor
    }

    fn set_test_cursor(editor: &mut Editor, cursor: usize) {
        let view_id = editor.tree.focus;
        let doc_id = editor.tree.try_get(view_id).unwrap().doc;
        let doc = editor.document_mut(doc_id).unwrap();
        doc.set_selection(view_id, Selection::point(cursor));
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
        assert!(!native_command_supported(&MappableCommand::global_search));
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
        assert!(native_command_supported(&MappableCommand::match_brackets));
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
        assert!(native_prompt_command(&MappableCommand::command_mode));
        assert!(native_prompt_command(&MappableCommand::search));
        assert!(native_prompt_command(&MappableCommand::rsearch));
        assert!(!native_prompt_command(&MappableCommand::global_search));
        assert!(!native_prompt_command(&MappableCommand::file_picker));
        assert!(!native_prompt_command(&MappableCommand::buffer_picker));
        assert!(!native_prompt_command(&MappableCommand::insert_mode));
        assert!(!native_prompt_command(&MappableCommand::normal_mode));
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
        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_jumplist_picker_for_space_j() {
        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
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
        assert!(!picker.handled_by_terminal_editor);
        assert_eq!(picker.picker_requested, Some(NativePickerRequest::JumpList));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_symbol_picker_for_space_s() {
        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
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
        assert!(!picker.handled_by_terminal_editor);
        assert_eq!(
            picker.picker_requested,
            Some(NativePickerRequest::Symbols { workspace: false })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_lsp_navigation_for_gd() {
        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
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
        assert!(!navigation.handled_by_terminal_editor);
        assert_eq!(
            navigation.lsp_navigation_requested,
            Some(NativeLspNavigationRequest::GotoDefinition)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_jumplist_movement_natively() {
        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
        let mut editor = test_editor_with_text("one\ntwo\n");
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let ctrl_o = KeyEvent::from_str("C-o").unwrap();

        let outcome = bridge.handle_key(ctrl_o, &mut compositor, &mut editor, &mut jobs);

        assert!(outcome.handled_by_native_command);
        assert!(!outcome.handled_by_terminal_editor);
        assert_eq!(outcome.picker_requested, None);
        assert_eq!(outcome.lsp_navigation_requested, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_handles_goto_file_natively() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_path = temp_dir.path().join("target.txt");
        std::fs::write(&target_path, "opened\n").unwrap();

        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
        let mut editor = test_editor_with_text(&format!("{}\n", target_path.display()));
        set_test_cursor(&mut editor, 0);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let g = KeyEvent::from_str("g").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        let pending = bridge.handle_key(g, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert!(!pending.handled_by_terminal_editor);

        let outcome = bridge.handle_key(f, &mut compositor, &mut editor, &mut jobs);
        assert!(outcome.handled_by_native_command);
        assert!(!outcome.handled_by_terminal_editor);
        assert_eq!(outcome.picker_requested, None);

        let focused_path = editor
            .tree
            .try_get(editor.tree.focus)
            .and_then(|view| editor.document(view.doc))
            .and_then(|doc| doc.path())
            .cloned();
        assert_eq!(focused_path.as_deref(), Some(target_path.as_path()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn editor_input_bridge_requests_native_picker_for_goto_directory() {
        let temp_dir = tempfile::tempdir().unwrap();

        let mut bridge = EditorInputBridge::new(Keymaps::default(), Keymaps::default());
        let mut editor = test_editor_with_text(&format!("{}\n", temp_dir.path().display()));
        set_test_cursor(&mut editor, 0);
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut jobs = Jobs::new();

        let g = KeyEvent::from_str("g").unwrap();
        let f = KeyEvent::from_str("f").unwrap();

        let pending = bridge.handle_key(g, &mut compositor, &mut editor, &mut jobs);
        assert!(pending.handled_by_native_command);
        assert!(!pending.handled_by_terminal_editor);

        let outcome = bridge.handle_key(f, &mut compositor, &mut editor, &mut jobs);
        assert!(outcome.handled_by_native_command);
        assert!(!outcome.handled_by_terminal_editor);
        assert_eq!(
            outcome.picker_requested,
            Some(NativePickerRequest::FileAt(temp_dir.path().to_path_buf()))
        );
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
                    Some(Action::Replace)
                ));
                assert!(native_command_supported(&command));
            }
            _ => panic!("expected gf to resolve to goto_file"),
        }
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
        assert!(input.current_insert_replay.native);
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
    fn terminal_fallback_marks_insert_replay_non_native() {
        let mut input = NativeCommandInput::new(Keymaps::default());
        let enter_insert = KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::empty(),
        };
        let inserted = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
        };

        input.observe_terminal_fallback(Mode::Normal, Mode::Insert, &[enter_insert]);
        input.observe_terminal_fallback(Mode::Insert, Mode::Insert, &[inserted]);

        assert_eq!(
            input.current_insert_replay.keys,
            vec![enter_insert, inserted]
        );
        assert!(!input.current_insert_replay.native);
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
        assert_eq!(
            input
                .last_insert_replay
                .as_ref()
                .map(|replay| replay.native),
            Some(true)
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

    #[test]
    fn terminal_fallback_starts_and_finishes_insert_replay() {
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

        input.observe_terminal_fallback(Mode::Normal, Mode::Insert, &[enter_insert]);
        input.observe_terminal_fallback(Mode::Insert, Mode::Insert, &[inserted]);
        input.observe_terminal_fallback(Mode::Insert, Mode::Normal, &[escape]);

        assert_eq!(
            input.last_insert_replay.as_ref().map(|replay| &replay.keys),
            Some(&vec![enter_insert, inserted, escape])
        );
        assert_eq!(
            input
                .last_insert_replay
                .as_ref()
                .map(|replay| replay.native),
            Some(false)
        );
        assert!(input.current_insert_replay.is_empty());
    }
}
