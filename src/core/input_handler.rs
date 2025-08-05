// ABOUTME: Input handling functionality extracted from Application
// ABOUTME: Processes keyboard and mouse input events

use helix_term::{
    commands,
    compositor::{Compositor, Component, EventResult, Context as CompositorContext},
    job::Jobs,
    ui::EditorView,
};
use helix_view::{
    Editor,
    input::KeyEvent,
    ViewId,
    document::Mode,
};
use helix_core::movement::Direction;
use crate::application::InputEvent;
use log::debug;

/// Handles input events and dispatches them to the appropriate handlers
pub struct InputHandler<'a> {
    editor: &'a mut Editor,
    compositor: &'a mut Compositor,
    view: &'a mut EditorView,
    jobs: &'a mut Jobs,
}

impl<'a> InputHandler<'a> {
    pub fn new(
        editor: &'a mut Editor,
        compositor: &'a mut Compositor,
        view: &'a mut EditorView,
        jobs: &'a mut Jobs,
    ) -> Self {
        Self {
            editor,
            compositor,
            view,
            jobs,
        }
    }

    /// Handle an input event
    pub fn handle_input_event(
        &mut self,
        event: InputEvent,
        cx: &mut gpui::Context<crate::Core>,
        _handle: tokio::runtime::Handle,
    ) {
        let _guard = _handle.enter();
        match event {
            InputEvent::Key(key_event) => {
                self.handle_key_event(key_event, cx, _handle.clone());
            }
            InputEvent::ScrollLines {
                line_count,
                direction,
                view_id,
            } => {
                self.handle_scroll(line_count, direction, view_id);
                cx.emit(crate::Update::Redraw);
            }
        }
    }

    fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
        cx: &mut gpui::Context<crate::Core>,
        _handle: tokio::runtime::Handle,
    ) {
        debug!("Handling key event: {key_event:?}");
        
        // Create compositor context
        let mut comp_ctx = CompositorContext {
            editor: self.editor,
            scroll: None,
            jobs: self.jobs,
        };
        
        // Log cursor position before key handling
        let view_id = comp_ctx.editor.tree.focus;
        let doc_id = comp_ctx.editor.tree.get(view_id).doc;
        
        // Store before position
        let before_cursor = if let Some(doc) = comp_ctx.editor.document(doc_id) {
            let sel = doc.selection(view_id);
            let text = doc.text();
            let cursor_pos = sel.primary().cursor(text.slice(..));
            let line = text.char_to_line(cursor_pos);
            debug!("Before key - cursor pos: {cursor_pos}, line: {line}");
            Some((cursor_pos, line))
        } else {
            None
        };
        
        // Track if this is a command mode key
        let is_command_key = key_event.code == helix_view::keyboard::KeyCode::Char(':');
        
        let is_handled = self.compositor
            .handle_event(&helix_view::input::Event::Key(key_event), &mut comp_ctx);
        if !is_handled {
            let event = &helix_view::input::Event::Key(key_event);
            
            let res = self.view.handle_event(event, &mut comp_ctx);
            
            if let EventResult::Consumed(Some(cb)) = res {
                cb(self.compositor, &mut comp_ctx);
            }
        }
        
        // Log cursor position after key handling
        if let Some(doc) = comp_ctx.editor.document(doc_id) {
            let sel = doc.selection(view_id);
            let text = doc.text();
            let cursor_pos = sel.primary().cursor(text.slice(..));
            let line = text.char_to_line(cursor_pos);
            debug!("After key - cursor pos: {cursor_pos}, line: {line}");
            
            // Check if we moved lines
            if let Some((_before_pos, before_line)) = before_cursor {
                if before_line != line {
                    debug!("Moved from line {before_line} to line {line}");
                }
            }
        }
        
        // Ensure cursor is visible after keyboard navigation
        if comp_ctx.editor.tree.contains(view_id) {
            comp_ctx.editor.ensure_cursor_in_view(view_id);
        }
        
        // Only emit overlays if we pressed ':' for command mode
        if is_command_key {
            self.emit_overlays(cx);
        } else {
            // For other keys, only emit picker and other overlays, not prompts
            self.emit_overlays_except_prompt(cx);
        }
        
        cx.emit(crate::Update::Redraw);
    }

    fn handle_scroll(
        &mut self,
        line_count: usize,
        direction: Direction,
        _view_id: ViewId,
    ) {
        let mut ctx = commands::Context {
            editor: self.editor,
            register: None,
            count: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: self.jobs,
        };
        commands::scroll(&mut ctx, line_count, direction, false);
    }

    fn emit_overlays(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Implementation would go here - emit overlay updates
        self.emit_picker_overlay(cx);
        self.emit_prompt_overlay(cx);
    }

    fn emit_overlays_except_prompt(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Only emit picker and other overlays, not prompts
        self.emit_picker_overlay(cx);
    }

    fn emit_picker_overlay(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Check if there's a picker in the compositor and emit it
        use helix_term::ui::{overlay::Overlay, Picker};
        use std::path::PathBuf;
        use helix_term::ui::FilePickerData;

        if let Some(_picker) = self.compositor
            .find_id::<Overlay<Picker<PathBuf, FilePickerData>>>(helix_term::ui::picker::ID)
        {
            // Emit picker update event
            // This would be handled by the main application
            cx.emit(crate::Update::Redraw);
        }
    }

    fn emit_prompt_overlay(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Check if there's a prompt in the compositor and emit it
        use helix_term::ui::Prompt;

        if let Some(_prompt) = self.compositor.find::<Prompt>() {
            // For now, just emit a redraw when prompt is present
            // TODO: Extract prompt data when API is available
            cx.emit(crate::Update::Redraw);
        }
    }

    /// Get current keymaps for a given mode
    pub fn get_keymaps(&self, _mode: Mode) -> Vec<(String, String)> {
        // This would extract keymaps from the editor config
        // For now, return empty vec
        Vec::new()
    }
}