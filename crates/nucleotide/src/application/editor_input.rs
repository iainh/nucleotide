// ABOUTME: Native editor input boundary for GPUI-driven document views
// ABOUTME: Isolates the remaining Helix terminal input bridge from Application

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
    input::{Event, KeyEvent},
    keyboard::{KeyCode, KeyModifiers},
};
use nucleotide_logging::{debug, info};
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorInputOutcome {
    pub focused_view_id: ViewId,
    pub focused_doc_id: Option<DocumentId>,
    pub selection_changed: bool,
    pub handled_by_compositor: bool,
    pub handled_by_native_command: bool,
    pub handled_by_terminal_editor: bool,
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
        if !handled_by_compositor {
            let mode_before_fallback = context.editor.mode();
            match self
                .native_commands
                .handle_key(key, compositor, context.editor, context.jobs)
            {
                NativeInputResult::Handled => {
                    handled_by_native_command = true;
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
    current_insert_replay: Vec<KeyEvent>,
    last_insert_replay: Option<Vec<KeyEvent>>,
}

enum NativeInputResult {
    Handled,
    Fallback(Vec<KeyEvent>),
}

enum NativeCommandResult {
    Handled(Vec<compositor::Callback>),
    Fallback(Vec<KeyEvent>),
}

impl NativeCommandInput {
    fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            current_insert_replay: Vec::new(),
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
                NativeCommandResult::Handled(Vec::new())
            }
            .with_callbacks_from(&mut context, &mut self.on_next_key)
        };

        match command_result {
            NativeCommandResult::Handled(callbacks) => {
                finalize_native_command(editor, jobs, compositor, callbacks);
                self.finish_insert_replay_if_needed(mode_before, editor.mode());
                NativeInputResult::Handled
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
            self.current_insert_replay.clear();
            self.current_insert_replay.extend_from_slice(fallback_keys);
            return;
        }

        if mode_before == Mode::Insert {
            self.current_insert_replay.extend_from_slice(fallback_keys);
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
                if !native_insert_command_supported(command) {
                    return NativeCommandResult::Fallback(fallback_keys);
                }
                let mut last_mode = mode;
                execute_native_command(command, context, &mut last_mode);
                self.current_insert_replay.extend(fallback_keys);
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapResult::Pending(node) => {
                context.editor.autoinfo = Some(node.infobox());
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapResult::MatchedSequence(commands) => {
                if !commands.iter().all(native_insert_command_supported) {
                    return NativeCommandResult::Fallback(fallback_keys);
                }
                let mut last_mode = mode;
                for command in commands {
                    execute_native_command(command, context, &mut last_mode);
                }
                self.current_insert_replay.extend(fallback_keys);
                NativeCommandResult::Handled(Vec::new())
            }
            KeymapResult::NotFound => {
                if has_pending_keys {
                    return NativeCommandResult::Fallback(fallback_keys);
                }

                if self.run_on_next_key(OnKeyCallbackKind::Fallback, context, key) {
                    self.current_insert_replay.push(key);
                    return NativeCommandResult::Handled(Vec::new());
                }

                if let Some(ch) = key.char() {
                    commands::insert::insert_char(context, ch);
                    self.current_insert_replay.push(key);
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
            if let Some(keys) = self.last_insert_replay.clone() {
                return NativeCommandResult::Fallback(keys);
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
            self.current_insert_replay.clear();
            self.current_insert_replay.extend_from_slice(replay_keys);
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
                if !commands.iter().all(native_command_supported) {
                    return KeymapDispatch::Fallback;
                }
                for command in commands {
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
        if matches!(self, NativeCommandResult::Handled(_)) {
            *on_next_key = context.on_next_key_callback.take();
            return NativeCommandResult::Handled(std::mem::take(&mut context.callback));
        }
        self
    }
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
        || name.starts_with("move_")
        || name.starts_with("extend_")
        || matches!(
            name,
            "collapse_selection"
                | "flip_selections"
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
                | "select_all"
                | "select_line_above"
                | "select_line_below"
                | "select_mode"
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

fn native_insert_command_supported(command: &MappableCommand) -> bool {
    !native_insert_entry_command(command) && native_command_supported(command)
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
        assert!(!native_command_supported(&MappableCommand::goto_definition));
    }

    #[test]
    fn native_insert_support_reuses_movement_allowlist() {
        assert!(native_insert_command_supported(
            &MappableCommand::move_char_left
        ));
        assert!(native_insert_command_supported(
            &MappableCommand::normal_mode
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::delete_char_backward
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::completion
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::insert_mode
        ));
        assert!(!native_insert_command_supported(
            &MappableCommand::append_mode
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
        input.current_insert_replay.push(existing);

        input.seed_insert_replay_if_needed(Mode::Normal, Mode::Insert, &[enter_insert]);

        assert_eq!(input.current_insert_replay, vec![enter_insert]);
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
            input.last_insert_replay,
            Some(vec![enter_insert, inserted, escape])
        );
        assert!(input.current_insert_replay.is_empty());
    }
}
