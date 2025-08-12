use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::{access::Map, ArcSwap};
use futures_util::FutureExt;
use helix_core::{pos_at_coords, syntax, Position, Selection};
use helix_lsp::{lsp, LanguageServerId, LspProgressMap};
use helix_stdx::path::get_relative_path;
use helix_term::ui::FilePickerData;
use nucleotide_lsp::ServerStatus;

use helix_term::{
    args::Args,
    compositor::{self, Compositor},
    config::Config,
    job::Jobs,
    keymap::Keymaps,
    ui::EditorView,
};
use helix_view::document::DocumentSavedEventResult;
use helix_view::DocumentId;
use helix_view::{doc_mut, graphics::Rect, handlers::Handlers, Editor};

// Helper function to find workspace root from a specific directory
pub fn find_workspace_root_from(start_dir: &Path) -> PathBuf {
    // Walk up the directory tree looking for VCS directories
    for ancestor in start_dir.ancestors() {
        if ancestor.join(".git").exists()
            || ancestor.join(".svn").exists()
            || ancestor.join(".hg").exists()
            || ancestor.join(".jj").exists()
            || ancestor.join(".helix").exists()
        {
            return ancestor.to_path_buf();
        }
    }

    // If no VCS directory found, use the start directory
    start_dir.to_path_buf()
}

// Removed unused structs - now using event-driven architecture instead

// Removed unused Tag-related structs and enums

use anyhow::Error;
use log::{debug, warn};
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};

use crate::types::{AppEvent, CoreEvent, LspEvent, MessageSeverity, PickerType, UiEvent, Update};
use gpui::EventEmitter;
use tokio_stream::StreamExt;

pub struct Application {
    pub editor: Editor,
    pub compositor: Compositor,
    pub view: EditorView,
    pub jobs: Jobs,
    pub lsp_progress: LspProgressMap,
    pub lsp_state: Option<gpui::Entity<nucleotide_lsp::LspState>>,
    pub project_directory: Option<PathBuf>,
    pub event_bridge_rx: Option<event_bridge::BridgedEventReceiver>,
    pub gpui_to_helix_rx: Option<gpui_to_helix_bridge::GpuiToHelixEventReceiver>,
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(helix_view::input::KeyEvent),
    ScrollLines {
        line_count: usize,
        direction: helix_core::movement::Direction,
        view_id: helix_view::ViewId,
    },
    SetViewportAnchor {
        view_id: helix_view::ViewId,
        anchor: usize,
    },
}

pub struct Input;

impl EventEmitter<Update> for Application {}

impl gpui::EventEmitter<InputEvent> for Input {}

pub struct Crank;

impl gpui::EventEmitter<()> for Crank {}

impl Application {
    /// Sync LSP state from the editor and progress map
    pub fn sync_lsp_state(&self, cx: &mut gpui::App) {
        if let Some(lsp_state) = &self.lsp_state {
            // Check for active language servers
            let active_servers: Vec<(LanguageServerId, String)> = self
                .editor
                .language_servers
                .iter_clients()
                .map(|client| (client.id(), client.name().to_string()))
                .collect();

            log::info!("Syncing LSP state, active servers: {:?}", active_servers);

            // Check which servers are progressing
            let progressing_servers: Vec<LanguageServerId> = active_servers
                .iter()
                .filter(|(id, _)| self.lsp_progress.is_progressing(*id))
                .map(|(id, _)| *id)
                .collect();

            lsp_state.update(cx, |state, _cx| {
                // Clear old progress state
                state.progress.clear();

                // Update server info if we have new servers
                for (id, name) in active_servers {
                    if !state.servers.contains_key(&id) {
                        state.register_server(id, name, None);
                        state.update_server_status(id, ServerStatus::Running);
                    }
                }

                // Extract actual progress information from LspProgressMap
                for id in progressing_servers {
                    // Get the progress map for this server
                    if let Some(progress_map) = self.lsp_progress.progress_map(id) {
                        // Only process if we have actual progress items
                        if !progress_map.is_empty() {
                            log::debug!("Server {} has {} progress items", id, progress_map.len());

                            // Add each progress operation
                            for (token, status) in progress_map {
                                match status {
                                    helix_lsp::ProgressStatus::Created => {
                                        // Progress created but not started yet - skip these
                                        log::info!("Skipping LSP progress token {:?} - in Created state (not started yet)", token);
                                        continue;
                                    }
                                    helix_lsp::ProgressStatus::Started { title, progress } => {
                                        let key = format!("{}-{:?}", id, token);
                                        let (message, percentage) = match progress {
                                            lsp::WorkDoneProgress::Begin(begin) => {
                                                (begin.message.clone(), begin.percentage)
                                            }
                                            lsp::WorkDoneProgress::Report(report) => {
                                                (report.message.clone(), report.percentage)
                                            }
                                            lsp::WorkDoneProgress::End(_) => {
                                                // Progress ended, skip
                                                continue;
                                            }
                                        };

                                        log::info!("LSP progress active: {} - {:?} ({}%)",
                                            title, message, percentage.unwrap_or(0));

                                        state.progress.insert(key, nucleotide_lsp::LspProgress {
                                            server_id: id,
                                            token: format!("{:?}", token),
                                            title: title.clone(),
                                            message,
                                            percentage,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                // Log current state for debugging
                if !state.progress.is_empty() {
                    log::debug!("LSP servers with progress: {}", state.progress.len());
                    for progress in state.progress.values() {
                        log::debug!("  - {}: {:?} ({}%)",
                            progress.title,
                            progress.message,
                            progress.percentage.unwrap_or(0));
                    }
                }
                if !state.servers.is_empty() {
                    log::debug!("Active LSP servers: {}", state.servers.len());
                }
            });
        }
    }

    /// Safe document access API - read only
    pub fn with_document<F, R>(&self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&helix_view::Document) -> R,
    {
        let doc_manager = nucleotide_lsp::DocumentManager::new(&self.editor);
        doc_manager.with_document(doc_id, f)
    }

    /// Safe document access API - mutable
    pub fn with_document_mut<F, R>(&mut self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&mut helix_view::Document) -> R,
    {
        let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
        doc_manager.with_document_mut(doc_id, f)
    }

    /// Safe document access API - returns Result instead of Option
    pub fn try_with_document<F, R, E>(&self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let doc_manager = nucleotide_lsp::DocumentManager::new(&self.editor);
        doc_manager.try_with_document(doc_id, f)
    }

    /// Safe document access API - mutable with Result
    pub fn try_with_document_mut<F, R, E>(&mut self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&mut helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
        doc_manager.try_with_document_mut(doc_id, f)
    }
    fn try_create_picker_component(&mut self) -> Option<crate::picker::Picker> {
        // This method is no longer used for file/buffer pickers
        // They are handled via events now
        None
    }

    /// Check if helix created a picker and emit the appropriate event
    pub fn check_for_picker_and_emit_event(&mut self, cx: &mut gpui::Context<crate::Core>) -> bool {
        use helix_term::ui::{overlay::Overlay, Picker};

        // Check for file picker first
        if self
            .compositor
            .find_id::<Overlay<Picker<PathBuf, FilePickerData>>>(helix_term::ui::picker::ID)
            .is_some()
        {
            log::info!("Detected file picker in compositor, emitting ShowFilePicker event");
            self.compositor.remove(helix_term::ui::picker::ID);
            cx.emit(Update::Event(AppEvent::Ui(UiEvent::ShowPicker {
                picker_type: PickerType::File,
                picker_object: None,
            })));
            return true;
        }

        // Check for any picker - if we have multiple docs, it's likely buffer picker
        // We need to check if any picker exists by trying to remove it
        if self.compositor.remove(helix_term::ui::picker::ID).is_some() {
            log::info!("Found and removed picker from compositor");
            if self.editor.documents.len() > 1 {
                log::info!("Multiple documents open, assuming buffer picker, emitting ShowBufferPicker event");
                cx.emit(Update::Event(AppEvent::Ui(UiEvent::ShowPicker {
                    picker_type: PickerType::Buffer,
                    picker_object: None,
                })));
                return true;
            }
        }

        false
    }

    /// Legacy method - no longer used for event-based prompts
    pub fn check_for_prompt_and_emit_event(
        &mut self,
        _cx: &mut gpui::Context<crate::Core>,
    ) -> bool {
        // Disabled - prompts are now handled through the legacy Update::Prompt system
        false
    }

    // Native picker creation methods that demonstrate the new GPUI-native picker functionality

    pub fn create_sample_native_prompt(&self) -> crate::prompt::Prompt {
        crate::prompt::Prompt::native("Search:", "", |_input| {
            // For now, just show the input - we'll handle the actual search via a different mechanism
        })
        .with_cancel(|| {
            // Prompt cancelled
        })
    }

    pub fn create_sample_completion_items(&self) -> Vec<crate::completion::CompletionItem> {
        use crate::completion::{CompletionItem, CompletionItemKind};

        // Create sample completion items
        vec![
            CompletionItem::new("println!", CompletionItemKind::Snippet)
                .with_detail("macro")
                .with_documentation("Prints to the standard output, with a newline."),
            CompletionItem::new("String", CompletionItemKind::Struct)
                .with_detail("std::string::String")
                .with_documentation("A UTF-8 encoded, growable string."),
            CompletionItem::new("Vec", CompletionItemKind::Struct)
                .with_detail("std::vec::Vec<T>")
                .with_documentation("A contiguous growable array type."),
            CompletionItem::new("HashMap", CompletionItemKind::Struct)
                .with_detail("std::collections::HashMap<K, V>")
                .with_documentation("A hash map implementation."),
            CompletionItem::new("println", CompletionItemKind::Function)
                .with_detail("fn println(&str)")
                .with_documentation("Print to stdout with newline"),
            CompletionItem::new("print", CompletionItemKind::Function)
                .with_detail("fn print(&str)")
                .with_documentation("Print to stdout without newline"),
            CompletionItem::new("format", CompletionItemKind::Function)
                .with_detail("fn format(&str, ...) -> String")
                .with_documentation("Create a formatted string"),
        ]
    }

    pub fn open_file(&mut self, path: &Path) -> Result<(), anyhow::Error> {
        let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
        doc_manager.open_file(path)
    }

    #[allow(dead_code)]
    fn create_file_picker_items(&self) -> Vec<crate::picker_view::PickerItem> {
        use crate::picker_view::PickerItem;
        use ignore::WalkBuilder;
        use std::sync::Arc;

        let mut items = Vec::new();

        // Find workspace root (similar to Helix)
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace_root = find_workspace_root_from(&current_dir);

        // Use WalkBuilder from the ignore crate to walk all files
        let mut walk_builder = WalkBuilder::new(&workspace_root);
        walk_builder
            .hidden(false) // Show hidden files (can be made configurable)
            .follow_links(true)
            .git_ignore(true) // Respect .gitignore
            .git_global(true) // Respect global .gitignore
            .git_exclude(true) // Respect .git/info/exclude
            .sort_by_file_name(|a, b| a.cmp(b)) // Sort alphabetically
            .filter_entry(|entry| {
                // Filter out VCS directories and common build directories
                if let Some(name) = entry.file_name().to_str() {
                    !matches!(
                        name,
                        ".git" | ".svn" | ".hg" | ".jj" | "target" | "node_modules"
                    )
                } else {
                    true
                }
            });

        // Walk the directory tree and collect files only
        for entry in walk_builder.build().flatten() {
            // Skip directories - we only want files
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.path().to_path_buf();

                // Get relative path from workspace root
                let relative_path = path.strip_prefix(&workspace_root).unwrap_or(&path);

                // Format the label to show relative path like Helix
                let label = relative_path.display().to_string();

                items.push(PickerItem {
                    label: label.into(),
                    sublabel: None, // No sublabel needed since full path is in label
                    data: Arc::new(path.clone()) as Arc<dyn std::any::Any + Send + Sync>,
                });

                // Limit to reasonable number of files
                if items.len() >= 10000 {
                    break;
                }
            }
        }

        // If no files found, add a placeholder
        if items.is_empty() {
            items.push(PickerItem {
                label: "No files found".into(),
                sublabel: Some("Workspace is empty or unreadable".into()),
                data: Arc::new(std::path::PathBuf::new()) as Arc<dyn std::any::Any + Send + Sync>,
            });
        }

        items
    }

    #[allow(dead_code)]
    fn create_buffer_picker(&self) -> Option<crate::picker::Picker> {
        use crate::picker_view::PickerItem;
        use helix_view::DocumentId;
        use std::sync::Arc;

        // Structure to hold buffer metadata for sorting
        #[derive(Clone)]
        struct BufferMeta {
            doc_id: DocumentId,
            path: Option<std::path::PathBuf>,
            is_modified: bool,
            is_current: bool,
            focused_at: std::time::Instant,
        }

        // Get current document ID
        let current_doc_id = self.editor.tree.get(self.editor.tree.focus).doc;

        // Collect buffer metadata
        let mut buffer_metas = Vec::new();
        for (doc_id, doc) in self.editor.documents.iter() {
            let focused_at = doc.focused_at;

            buffer_metas.push(BufferMeta {
                doc_id: *doc_id,
                path: doc.path().map(|p| p.to_path_buf()),
                is_modified: doc.is_modified(),
                is_current: *doc_id == current_doc_id,
                focused_at,
            });
        }

        // Sort by MRU (Most Recently Used) - most recent first
        buffer_metas.sort_by(|a, b| b.focused_at.cmp(&a.focused_at));

        // Create picker items with terminal-like formatting
        let mut items = Vec::new();

        for meta in buffer_metas {
            // Format like terminal: "ID  FLAGS  PATH"
            let id_str = format!("{:?}", meta.doc_id);

            // Build flags column
            let mut flags = String::new();
            if meta.is_modified {
                flags.push('+');
            }
            if meta.is_current {
                flags.push('*');
            }
            // Pad flags to consistent width
            let flags_str = format!("{:<2}", flags);

            // Get path or [scratch] label
            let path_str = if let Some(path) = &meta.path {
                // Show relative path if possible
                if let Some(project_dir) = &self.project_directory {
                    path.strip_prefix(project_dir)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                } else {
                    path.display().to_string()
                }
            } else {
                "[scratch]".to_string()
            };

            // Combine into terminal-like format with proper spacing
            let label = format!("{:<4} {} {}", id_str, flags_str, path_str);

            // Store the document ID in the picker item data
            items.push(PickerItem {
                label: label.into(),
                sublabel: None, // No sublabel for terminal-style display
                data: Arc::new(meta.doc_id),
            });
        }

        if items.is_empty() {
            // No buffers open
            return None;
        }

        // Create the picker
        Some(crate::picker::Picker::native(
            "Switch Buffer",
            items,
            |_index| {
                // Buffer selection is handled by the overlay
            },
        ))
    }

    fn create_native_prompt_from_helix(
        &mut self,
        last_key: Option<helix_view::input::KeyEvent>,
        _cx: &mut gpui::Context<crate::Core>,
    ) -> Option<crate::prompt::Prompt> {
        use crate::prompt::Prompt;
        use std::sync::Arc;

        // Check if there's a helix prompt in the compositor
        if let Some(_helix_prompt) = self.compositor.find::<helix_term::ui::Prompt>() {
            // Determine prompt type based on the key that triggered it
            let prompt_text = if let Some(key) = last_key {
                match key.code {
                    helix_view::keyboard::KeyCode::Char('/') if key.modifiers.is_empty() => {
                        "search:"
                    }
                    helix_view::keyboard::KeyCode::Char('?') if key.modifiers.is_empty() => {
                        "rsearch:"
                    }
                    helix_view::keyboard::KeyCode::Char(':') if key.modifiers.is_empty() => ":",
                    _ => {
                        // For other keys, don't show a native prompt
                        // This prevents all keys from opening search
                        return None;
                    }
                }
            } else {
                // No key info, default to command prompt
                ":"
            };

            // Prompts should always start empty when first opened
            let initial_input = String::new();

            // Create native prompt with command execution through Update event
            let prompt = Prompt::Native {
                prompt: prompt_text.into(),
                initial_input: initial_input.into(),
                on_submit: Arc::new(move |_input: &str| {
                    // The actual command/search execution will be handled by workspace
                    // through a CommandSubmitted or SearchSubmitted event
                }),
                on_cancel: Some(Arc::new(|| {
                    // Prompt cancelled
                })),
            };

            Some(prompt)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    fn emit_overlays_except_prompt(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Check for picker events first
        if self.check_for_picker_and_emit_event(cx) {
            return;
        }

        // Don't check for prompts here - this method specifically excludes prompts

        // Legacy picker handling (for non-file/buffer pickers)
        let picker = self.try_create_picker_component();
        if let Some(picker) = picker {
            cx.emit(crate::Update::Picker(picker));
        }

        // Don't take() the autoinfo - just clone it so it persists
        if let Some(info) = &self.editor.autoinfo {
            cx.emit(crate::Update::Info(helix_view::info::Info {
                title: info.title.clone(),
                text: info.text.clone(),
                width: info.width,
                height: info.height,
            }));
        }
    }

    fn emit_overlays(
        &mut self,
        last_key: Option<helix_view::input::KeyEvent>,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        // Check for picker events first
        if self.check_for_picker_and_emit_event(cx) {
            return;
        }

        // Handle prompts through legacy system
        if let Some(prompt) = self.create_native_prompt_from_helix(last_key, cx) {
            cx.emit(crate::Update::Prompt(prompt));
            return;
        }

        // Legacy handling for other overlay types
        let picker = self.try_create_picker_component();
        if let Some(picker) = picker {
            cx.emit(crate::Update::Picker(picker));
        }

        // Don't take() the autoinfo - just clone it so it persists
        if let Some(info) = &self.editor.autoinfo {
            cx.emit(crate::Update::Info(helix_view::info::Info {
                title: info.title.clone(),
                text: info.text.clone(),
                width: info.width,
                height: info.height,
            }));
        }
    }
    pub fn handle_input_event(
        &mut self,
        event: InputEvent,
        cx: &mut gpui::Context<crate::Core>,
        handle: tokio::runtime::Handle,
    ) {
        let _guard = handle.enter();
        use helix_term::compositor::{Component, EventResult};

        let mut comp_ctx = compositor::Context {
            editor: &mut self.editor,
            scroll: None,
            jobs: &mut self.jobs,
        };
        match event {
            InputEvent::Key(key) => {
                debug!("Handling key event: {key:?}");
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

                let is_handled = self
                    .compositor
                    .handle_event(&helix_view::input::Event::Key(key), &mut comp_ctx);
                if !is_handled {
                    let event = &helix_view::input::Event::Key(key);

                    let res = self.view.handle_event(event, &mut comp_ctx);

                    if let EventResult::Consumed(Some(cb)) = res {
                        cb(&mut self.compositor, &mut comp_ctx);
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
                    if let Some((before_pos, before_line)) = before_cursor {
                        if before_line != line || before_pos != cursor_pos {
                            debug!(
                                "Cursor moved from pos {} (line {}) to pos {} (line {})",
                                before_pos, before_line, cursor_pos, line
                            );
                        }
                    }
                }

                // Ensure cursor is visible after keyboard navigation
                // Check if the view exists before trying to ensure cursor visibility
                if comp_ctx.editor.tree.contains(view_id) {
                    comp_ctx.editor.ensure_cursor_in_view(view_id);
                }

                // Emit overlays after key handling, passing the key that was just processed
                self.emit_overlays(Some(key), cx);

                cx.emit(crate::Update::Event(AppEvent::Core(
                    CoreEvent::RedrawRequested,
                )));
            }
            InputEvent::ScrollLines {
                line_count,
                direction,
                ..
            } => {
                let mut ctx = helix_term::commands::Context {
                    editor: &mut self.editor,
                    register: None,
                    count: None,
                    callback: Vec::new(),
                    on_next_key_callback: None,
                    jobs: &mut self.jobs,
                };
                helix_term::commands::scroll(&mut ctx, line_count, direction, false);
                cx.emit(crate::Update::Event(AppEvent::Core(
                    CoreEvent::RedrawRequested,
                )));
            }
            InputEvent::SetViewportAnchor { view_id, anchor } => {
                // Set the viewport anchor for scrollbar integration
                // For now, we'll use a simplified approach - just emit a redraw
                // TODO: Implement proper viewport anchor setting through document API
                log::debug!(
                    "SetViewportAnchor: view_id={:?}, anchor={}",
                    view_id,
                    anchor
                );
                cx.emit(crate::Update::Event(AppEvent::Core(
                    CoreEvent::RedrawRequested,
                )));
            }
        }
    }

    fn handle_document_write(&mut self, doc_save_event: &DocumentSavedEventResult) {
        let doc_save_event = match doc_save_event {
            Ok(event) => event,
            Err(err) => {
                self.editor.set_error(err.to_string());
                return;
            }
        };

        let doc = match self.editor.document_mut(doc_save_event.doc_id) {
            None => {
                warn!(
                    "received document saved event for non-existent doc id: {}",
                    doc_save_event.doc_id
                );

                return;
            }
            Some(doc) => doc,
        };

        debug!(
            "document {:?} saved with revision {}",
            doc.path(),
            doc_save_event.revision
        );

        doc.set_last_saved_revision(doc_save_event.revision, std::time::SystemTime::now());

        let lines = doc_save_event.text.len_lines();
        let bytes = doc_save_event.text.len_bytes();

        self.editor
            .set_doc_path(doc_save_event.doc_id, &doc_save_event.path);
        // TODO: fix being overwritten by lsp
        self.editor.set_status(format!(
            "'{}' written, {}L {}B",
            get_relative_path(&doc_save_event.path).to_string_lossy(),
            lines,
            bytes
        ));
    }

    pub fn handle_crank_event(
        &mut self,
        _event: (),
        cx: &mut gpui::Context<crate::Core>,
        handle: tokio::runtime::Handle,
    ) {
        let _guard = handle.enter();

        self.step(cx).now_or_never();

        // Sync LSP state periodically
        self.sync_lsp_state(cx);
    }

    pub async fn step(&mut self, cx: &mut gpui::Context<'_, crate::Core>) {
        loop {
            // Check if all views are closed and we should quit
            if self.editor.tree.views().count() == 0 {
                cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::ShouldQuit)));
                break;
            }

            tokio::select! {
                biased;

                Some(callback) = self.jobs.callbacks.recv() => {
                    self.jobs.handle_callback(&mut self.editor, &mut self.compositor, Ok(Some(callback)));
                }
                Some(msg) = self.jobs.status_messages.recv() => {
                    let severity = match msg.severity{
                        helix_event::status::Severity::Hint => crate::types::Severity::Hint,
                        helix_event::status::Severity::Info => crate::types::Severity::Info,
                        helix_event::status::Severity::Warning => crate::types::Severity::Warning,
                        helix_event::status::Severity::Error => crate::types::Severity::Error,
                    };
                    let status = crate::types::EditorStatus { status: msg.message.to_string(), severity };
                    cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::StatusChanged {
                        message: status.status,
                        severity: match status.severity {
                            crate::types::Severity::Hint => MessageSeverity::Info,
                            crate::types::Severity::Info => MessageSeverity::Info,
                            crate::types::Severity::Warning => MessageSeverity::Warning,
                            crate::types::Severity::Error => MessageSeverity::Error,
                        }
                    })));
                    // TODO: show multiple status messages at once to avoid clobbering
                    let helix_severity = match msg.severity {
                        helix_event::status::Severity::Hint => helix_view::editor::Severity::Hint,
                        helix_event::status::Severity::Info => helix_view::editor::Severity::Info,
                        helix_event::status::Severity::Warning => helix_view::editor::Severity::Warning,
                        helix_event::status::Severity::Error => helix_view::editor::Severity::Error,
                    };
                    self.editor.status_msg = Some((msg.message, helix_severity));
                    helix_event::request_redraw();
                }
                Some(bridged_event) = async {
                    if let Some(ref mut rx) = self.event_bridge_rx {
                        rx.recv().await
                    } else {
                        // Return None to make this branch never match
                        std::future::pending().await
                    }
                } => {
                    // EVENT BATCHING: Collect all pending events to reduce UI update overhead
                    let mut events = vec![bridged_event];

                    // Drain any other pending events from the channel
                    if let Some(ref mut rx) = self.event_bridge_rx {
                        while let Ok(event) = rx.try_recv() {
                            events.push(event);
                        }
                    }

                    log::debug!("Processing {} batched events", events.len());

                    // Track if we need to request a redraw
                    let mut needs_redraw = false;

                    // Convert all bridged events to Update events and emit them
                    for bridged_event in events {
                        let update = match bridged_event {
                            event_bridge::BridgedEvent::DocumentChanged { doc_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::DocumentChanged { doc_id }))
                            }
                            event_bridge::BridgedEvent::SelectionChanged { doc_id, view_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::SelectionChanged { doc_id, view_id }))
                            }
                            event_bridge::BridgedEvent::ModeChanged { old_mode, new_mode } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::ModeChanged { old_mode, new_mode }))
                            }
                            event_bridge::BridgedEvent::DiagnosticsChanged { doc_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::DiagnosticsChanged { doc_id }))
                            }
                            event_bridge::BridgedEvent::DocumentOpened { doc_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::DocumentOpened { doc_id }))
                            }
                            event_bridge::BridgedEvent::DocumentClosed { doc_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::DocumentClosed { doc_id }))
                            }
                            event_bridge::BridgedEvent::ViewFocused { view_id } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::ViewFocused { view_id }))
                            }
                            event_bridge::BridgedEvent::LanguageServerInitialized { server_id } => {
                                crate::Update::Event(AppEvent::Lsp(LspEvent::ServerInitialized { server_id }))
                            }
                            event_bridge::BridgedEvent::LanguageServerExited { server_id } => {
                                crate::Update::Event(AppEvent::Lsp(LspEvent::ServerExited { server_id }))
                            }
                            event_bridge::BridgedEvent::CompletionRequested { doc_id, view_id, trigger } => {
                                crate::Update::Event(AppEvent::Core(CoreEvent::CompletionRequested { doc_id, view_id, trigger }))
                            }
                        };
                        cx.emit(update);
                        needs_redraw = true;
                    }

                    // Request a single redraw for all batched events
                    if needs_redraw {
                        helix_event::request_redraw();
                    }
                }
                Some(gpui_event) = async {
                    if let Some(ref mut rx) = self.gpui_to_helix_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    // Handle GPUI events that affect Helix
                    gpui_to_helix_bridge::handle_gpui_event_in_helix(&gpui_event, &mut self.editor);
                    helix_event::request_redraw();
                }
                Some(callback) = self.jobs.wait_futures.next() => {
                    self.jobs.handle_callback(&mut self.editor, &mut self.compositor, callback);
                }
                event = self.editor.wait_event() => {
                    use helix_view::editor::EditorEvent;
                    match event {
                        EditorEvent::DocumentSaved(event) => {
                            self.handle_document_write(&event);
                            // Convert to CoreEvent if save was successful
                            if let Ok(event) = event {
                                let path = self.editor.document(event.doc_id)
                                    .and_then(|doc| doc.path())
                                    .map(|p| p.to_string_lossy().to_string());
                                cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::DocumentSaved {
                                    doc_id: event.doc_id,
                                    path,
                                })));
                            }
                        }
                        EditorEvent::IdleTimer => {
                            self.editor.clear_idle_timer();
                            /* dont send */
                        }
                        EditorEvent::Redraw => {
                            // Check if all views are closed after redraw
                            if self.editor.tree.views().count() == 0 {
                                cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::ShouldQuit)));
                                break;
                            }
                             cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::RedrawRequested)));
                        }
                        EditorEvent::ConfigEvent(_) => {
                            /* TODO */
                        }
                        EditorEvent::LanguageServerMessage((id, call)) => {
                            // We need cx here but it's not available in the async context
                            // For now, handle without UI updates
                            let mut lsp_manager = nucleotide_lsp::LspManager::new(&mut self.editor, &mut self.lsp_progress);
                            lsp_manager.handle_language_server_message(call, id).await;
                        }
                        EditorEvent::DebuggerEvent(_) => {
                            /* TODO */
                        }
                    }
                }
                else => {
                    break;
                }
            }
        }
    }

    // Removed unused handle_language_server_message - now handled via events
}

// Implement capability traits for Application
impl nucleotide_core::EditorReadAccess for Application {
    fn editor(&self) -> &Editor {
        &self.editor
    }
}

impl nucleotide_core::EditorWriteAccess for Application {
    fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }
}

impl nucleotide_core::JobSystemAccess for Application {
    fn jobs_mut(&mut self) -> &mut Jobs {
        &mut self.jobs
    }
}

pub fn init_editor(
    args: Args,
    config: Config,
    lang_loader: syntax::Loader,
) -> Result<Application, Error> {
    use helix_view::editor::Action;

    // Determine project directory from args before consuming args.files
    let project_directory = if let Some(path) = &args.working_directory {
        Some(path.clone())
    } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        // If the first file is a directory, use it as the project directory
        Some(path.clone())
    } else {
        None
    };

    let mut theme_parent_dirs = vec![helix_loader::config_dir()];
    theme_parent_dirs.extend(helix_loader::runtime_dirs().iter().cloned());

    // Add bundle runtime as a backup for macOS
    #[cfg(target_os = "macos")]
    if let Some(rt) = crate::utils::detect_bundle_runtime() {
        if !theme_parent_dirs.contains(&rt) {
            theme_parent_dirs.push(rt);
        }
    }

    let theme_loader = std::sync::Arc::new(helix_view::theme::Loader::new(&theme_parent_dirs));

    let true_color = true;

    let theme = config
        .theme
        .as_ref()
        .and_then(|theme_name| {
            theme_loader
                .load(theme_name)
                .map_err(|e| {
                    log::warn!("failed to load theme `{theme_name}` - {e}");
                    e
                })
                .ok()
                .filter(|theme| (true_color || theme.is_16_color()))
        })
        .or_else(|| {
            // Try to load nucleotide-teal as the default theme
            theme_loader
                .load("nucleotide-teal")
                .map_err(|e| {
                    log::info!("nucleotide-teal theme not found, falling back to default - {e}");
                    e
                })
                .ok()
        })
        .unwrap_or_else(|| theme_loader.default_theme(true_color));

    let syn_loader = Arc::new(ArcSwap::from_pointee(lang_loader));

    // CRITICAL: Enable true_color support for GUI mode before creating the editor
    // This is required for themes to work correctly
    let mut config = config;
    config.editor.true_color = true;

    let config = Arc::new(ArcSwap::from_pointee(config));

    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 25,
    };
    // CRITICAL: Register events FIRST, before creating handlers
    helix_term::events::register();

    let (completion_tx, _completion_rx) = tokio::sync::mpsc::channel(1);
    let (signature_tx, _signature_rx) = tokio::sync::mpsc::channel(1);
    let (auto_save_tx, _auto_save_rx) = tokio::sync::mpsc::channel(1);
    let (doc_colors_tx, _doc_colors_rx) = tokio::sync::mpsc::channel(1);
    let handlers = Handlers {
        completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
        signature_hints: signature_tx,
        auto_save: auto_save_tx,
        document_colors: doc_colors_tx,
        // TODO: Add word_index handler when available in new API
    };

    // CRITICAL FIX: Register handler hooks to enable LSP features
    helix_view::handlers::register_hooks(&handlers);

    // Initialize event bridge system for Helix -> GPUI event forwarding
    let (bridge_tx, bridge_rx) = event_bridge::create_bridge_channel();
    event_bridge::initialize_bridge(bridge_tx);
    event_bridge::register_event_hooks();

    // Initialize reverse event bridge system for GPUI -> Helix event forwarding
    let (gpui_to_helix_tx, gpui_to_helix_rx) = gpui_to_helix_bridge::create_gpui_to_helix_channel();
    gpui_to_helix_bridge::initialize_gpui_to_helix_bridge(gpui_to_helix_tx);
    gpui_to_helix_bridge::register_gpui_event_handlers();

    let mut editor = Editor::new(
        area,
        theme_loader.clone(),
        syn_loader.clone(),
        Arc::new(Map::new(Arc::clone(&config), |config: &Config| {
            &config.editor
        })),
        handlers,
    );

    if args.load_tutor {
        let path = helix_loader::runtime_file(Path::new("tutor"));
        let doc_id = editor.open(&path, Action::VerticalSplit)?;
        let view_id = editor.tree.focus;
        // Check if the view exists before setting selection
        if editor.tree.contains(view_id) {
            let doc = doc_mut!(editor, &doc_id);
            let pos = Selection::point(pos_at_coords(
                doc.text().slice(..),
                Position::new(0, 0),
                true,
            ));
            doc.set_selection(view_id, pos);
        }

        // Unset path to prevent accidentally saving to the original tutor file.
        doc_mut!(editor).set_path(None);
    } else if !args.files.is_empty() {
        // Open files from command line arguments
        let mut first = true;
        for (file, pos) in args.files {
            // Skip directories
            if file.is_dir() {
                continue;
            }

            let action = if first {
                Action::VerticalSplit
            } else {
                // For now, just load additional files in the same view
                // TODO: Support --vsplit and --hsplit arguments
                Action::Load
            };

            log::info!(
                "Opening file from command line: {:?} with action: {:?}",
                file,
                action
            );
            match editor.open(&file, action) {
                Ok(doc_id) => {
                    log::info!(
                        "Successfully opened file from CLI: {:?} with doc_id: {:?}",
                        file,
                        doc_id
                    );

                    // Log document info
                    if let Some(doc) = editor.document(doc_id) {
                        log::info!(
                            "Document language: {:?}, path: {:?}",
                            doc.language_name(),
                            doc.path()
                        );
                    }
                    let view_id = editor.tree.focus;
                    if !pos.is_empty() {
                        // Set cursor position if specified (use first position)
                        if editor.tree.contains(view_id) {
                            let doc = doc_mut!(editor, &doc_id);
                            let text = doc.text();
                            if let Some(first_pos) = pos.first() {
                                let line = first_pos.row.saturating_sub(1); // Convert to 0-indexed
                                let col = first_pos.col;
                                let char_pos = text.try_line_to_char(line).unwrap_or(0) + col;
                                let selection = Selection::point(char_pos);
                                doc.set_selection(view_id, selection);
                            }
                        }
                    }
                    first = false;
                }
                Err(e) => {
                    log::error!("Failed to open file {:?}: {}", file, e);
                }
            }
        }

        // If no files were successfully opened, create a new file
        if first {
            editor.new_file(Action::VerticalSplit);
        }
    } else {
        editor.new_file(Action::VerticalSplit);
    }

    editor.set_theme(theme);

    let keys = Box::new(Map::new(Arc::clone(&config), |config: &Config| {
        &config.keys
    }));
    let compositor = Compositor::new(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 25,
    });
    let keymaps = Keymaps::new(keys);
    let view = EditorView::new(keymaps);
    let jobs = Jobs::new();

    Ok(Application {
        editor,
        compositor,
        view,
        jobs,
        lsp_progress: LspProgressMap::new(),
        lsp_state: None,
        project_directory,
        event_bridge_rx: Some(bridge_rx),
        gpui_to_helix_rx: Some(gpui_to_helix_rx),
    })
}

// Tests moved to tests/integration_test.rs to avoid GPUI proc macro compilation issues
// The issue: When compiling with --test, GPUI proc macros cause stack overflow
// when processing certain patterns in our codebase
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use crate::test_utils::test_support::{
        create_counting_channel, create_test_diagnostic_events, create_test_document_events,
        create_test_selection_events, TestUpdate,
    };
    use std::time::Duration;

    #[ignore] // Temporarily disabled due to SIGBUS compiler crash
    #[tokio::test]
    async fn test_event_batching_reduces_update_calls() {
        // This test SHOULD FAIL initially
        // We're testing that multiple rapid events get batched into fewer updates

        let (event_tx, mut event_rx, _counter) = create_counting_channel();

        // Send 10 rapid selection changed events
        let events = create_test_selection_events(10);
        for event in events {
            let _ = event_tx.send(event);
        }

        // Small delay to let events accumulate
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Process events (simulating what Application::step would do)
        let mut update_count = 0;
        while let Ok(_) = event_rx.try_recv() {
            update_count += 1;
        }

        // WITHOUT BATCHING: We expect 10 updates (one per event)
        // WITH BATCHING: We expect fewer updates (events batched together)
        assert!(
            update_count < 5,
            "Expected fewer than 5 updates with batching, but got {}. Events are not being batched.",
            update_count
        );
    }

    #[ignore] // Temporarily disabled due to SIGBUS compiler crash
    #[tokio::test]
    async fn test_document_change_events_are_batched() {
        // Test that rapid document changes (like fast typing) are batched

        let (event_tx, mut event_rx, _counter) = create_counting_channel();

        // Simulate rapid typing - 20 document change events
        let events = create_test_document_events(20);
        for event in events {
            let _ = event_tx.send(event);
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut doc_change_count = 0;
        while let Ok(update) = event_rx.try_recv() {
            if matches!(update, TestUpdate::DocumentChanged { .. }) {
                doc_change_count += 1;
            }
        }

        // With batching, 20 rapid changes should result in very few updates
        assert!(
            doc_change_count <= 3,
            "Expected 3 or fewer DocumentChanged updates with batching, but got {}",
            doc_change_count
        );
    }

    #[ignore] // Temporarily disabled due to SIGBUS compiler crash
    #[tokio::test]
    async fn test_diagnostic_events_are_deduplicated() {
        // Test that multiple diagnostic events for the same document are deduplicated

        let (event_tx, mut event_rx, _counter) = create_counting_channel();

        // Send 5 diagnostic events for the same document
        let events = create_test_diagnostic_events(5);
        for event in events {
            let _ = event_tx.send(event);
        }

        // Wait for potential deduplication delay
        tokio::time::sleep(Duration::from_millis(60)).await;

        let mut diag_count = 0;
        while let Ok(update) = event_rx.try_recv() {
            if matches!(update, TestUpdate::DiagnosticsChanged { .. }) {
                diag_count += 1;
            }
        }

        // With deduplication, we should see only 1 diagnostic update
        assert_eq!(
            diag_count, 1,
            "Expected exactly 1 DiagnosticsChanged update with deduplication, but got {}",
            diag_count
        );
    }
}
