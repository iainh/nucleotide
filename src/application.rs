use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::{access::Map, ArcSwap};
use futures_util::FutureExt;
use helix_core::{
    diagnostic::Severity,
    pos_at_coords, syntax, Position, Selection,
};
use helix_lsp::{
    lsp::Location,
    LanguageServerId, LspProgressMap,
};
use crate::core::lsp_state::ServerStatus;
use helix_stdx::path::get_relative_path;
use helix_term::ui::FilePickerData;

use helix_core::Uri;
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

// Helper function to find workspace root (similar to Helix)
fn find_workspace_root() -> PathBuf {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_workspace_root_from(&current_dir)
}

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
use tokio_stream::StreamExt;

pub struct Application {
    pub editor: Editor,
    pub compositor: Compositor,
    pub view: EditorView,
    pub jobs: Jobs,
    pub lsp_progress: LspProgressMap,
    pub lsp_state: Option<gpui::Entity<crate::core::lsp_state::LspState>>,
    pub project_directory: Option<PathBuf>,
    pub event_bridge_rx: Option<crate::event_bridge::BridgedEventReceiver>,
    pub gpui_to_helix_rx: Option<crate::gpui_to_helix_bridge::GpuiToHelixEventReceiver>,
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(helix_view::input::KeyEvent),
    ScrollLines {
        line_count: usize,
        direction: helix_core::movement::Direction,
        view_id: helix_view::ViewId,
    },
}

pub struct Input;

impl gpui::EventEmitter<InputEvent> for Input {}

impl gpui::EventEmitter<crate::Update> for Application {}

pub struct Crank;

impl gpui::EventEmitter<()> for Crank {}

impl Application {
    /// Sync LSP state from the editor and progress map
    pub fn sync_lsp_state(&self, cx: &mut gpui::App) {
        if let Some(lsp_state) = &self.lsp_state {
            // Check for active language servers
            let active_servers: Vec<(LanguageServerId, String)> = self.editor.language_servers
                .iter_clients()
                .map(|client| (client.id(), client.name().to_string()))
                .collect();
            
            log::debug!("Syncing LSP state, active servers: {:?}", active_servers);
            
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
                
                // Mark servers that are progressing
                for id in progressing_servers {
                    // Add a generic progress indicator
                    state.start_progress(id, "lsp".to_string(), "Processing...".to_string());
                }
                
                // Log current state for debugging
                if !state.progress.is_empty() {
                    log::debug!("LSP servers with progress: {}", state.progress.len());
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
        let doc_manager = crate::core::DocumentManager::new(&self.editor);
        doc_manager.with_document(doc_id, f)
    }
    
    /// Safe document access API - mutable
    pub fn with_document_mut<F, R>(&mut self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&mut helix_view::Document) -> R,
    {
        let mut doc_manager = crate::core::DocumentManagerMut::new(&mut self.editor);
        doc_manager.with_document_mut(doc_id, f)
    }
    
    /// Safe document access API - returns Result instead of Option
    pub fn try_with_document<F, R, E>(&self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let doc_manager = crate::core::DocumentManager::new(&self.editor);
        doc_manager.try_with_document(doc_id, f)
    }
    
    /// Safe document access API - mutable with Result
    pub fn try_with_document_mut<F, R, E>(&mut self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&mut helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let mut doc_manager = crate::core::DocumentManagerMut::new(&mut self.editor);
        doc_manager.try_with_document_mut(doc_id, f)
    }
    fn try_create_picker_component(&mut self) -> Option<crate::picker::Picker> {
        use helix_term::ui::{overlay::Overlay, Picker};

        // Check for known picker types and create native implementations when possible
        // For now, we'll demonstrate the native picker capability by using it for file picker

        // Create a native file picker for file operations
        if let Some(_file_picker) = self
            .compositor
            .find_id::<Overlay<Picker<PathBuf, FilePickerData>>>(helix_term::ui::picker::ID)
        {
            // Remove the original picker from compositor to prevent infinite loop
            self.compositor.remove(helix_term::ui::picker::ID);
            
            // Create a native file picker with files from current directory
            let items = self.create_file_picker_items();
            
            return Some(crate::picker::Picker::native(
                "Open File",
                items,
                |_index| {
                    // File selection logic would go here
                },
            ));
        }

        // All picker types now use the native implementation
        None
    }


    // Native picker creation methods that demonstrate the new GPUI-native picker functionality

    pub fn create_sample_native_prompt(&self) -> crate::prompt::Prompt {
        crate::prompt::Prompt::native(
            "Search:",
            "",
            |_input| {
                // For now, just show the input - we'll handle the actual search via a different mechanism
            }
        ).with_cancel(|| {
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
        let mut doc_manager = crate::core::DocumentManagerMut::new(&mut self.editor);
        doc_manager.open_file(path)
    }

    fn create_file_picker_items(&self) -> Vec<crate::picker_view::PickerItem> {
        use crate::picker_view::PickerItem;
        use std::sync::Arc;
        use ignore::WalkBuilder;
        
        let mut items = Vec::new();
        
        // Find workspace root (similar to Helix)
        let workspace_root = find_workspace_root();
        
        // Use WalkBuilder from the ignore crate to walk all files
        let mut walk_builder = WalkBuilder::new(&workspace_root);
        walk_builder
            .hidden(false)  // Show hidden files (can be made configurable)
            .follow_links(true)
            .git_ignore(true)  // Respect .gitignore
            .git_global(true)  // Respect global .gitignore
            .git_exclude(true) // Respect .git/info/exclude
            .sort_by_file_name(|a, b| a.cmp(b))  // Sort alphabetically
            .filter_entry(|entry| {
                // Filter out VCS directories and common build directories
                if let Some(name) = entry.file_name().to_str() {
                    !matches!(name, ".git" | ".svn" | ".hg" | ".jj" | "target" | "node_modules")
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
                    let relative_path = path.strip_prefix(&workspace_root)
                        .unwrap_or(&path);
                    
                    // Format the label to show relative path like Helix
                    let label = relative_path.display().to_string();
                    
                    items.push(PickerItem {
                        label: label.into(),
                        sublabel: None,  // No sublabel needed since full path is in label
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

    fn create_native_prompt_from_helix(&mut self, _cx: &mut gpui::Context<crate::Core>) -> Option<crate::prompt::Prompt> {
        use crate::prompt::Prompt;
        use std::sync::Arc;
        
        // Check if there's a helix prompt in the compositor
        if let Some(_helix_prompt) = self.compositor.find::<helix_term::ui::Prompt>() {
            // To identify command prompts, we need to check the prompt text
            // Since the prompt field is private, we'll use a different approach:
            // 1. Get the current line (which we can access)
            // 2. Check if this is likely a command based on context
            
            // We'll use a heuristic: if there's a prompt in the compositor and
            // we just pressed ':', it's likely a command prompt
            // This is checked by the workspace before calling this function
            
            // For now, we'll only show native prompts for command mode
            // In the future, we might want to support other prompt types
            let prompt_text = ":";
            
            // Command prompts should always start empty when first opened
            // Any text in the helix prompt is likely from before the prompt was opened
            let initial_input = String::new();
            
            // Create native prompt with command execution through Update event
            let prompt = Prompt::Native {
                prompt: prompt_text.into(),
                initial_input: initial_input.into(),
                on_submit: Arc::new(move |_input: &str| {
                    // The actual command execution will be handled by workspace
                    // through a CommandSubmitted event
                }),
                on_cancel: Some(Arc::new(|| {
                    // Command cancelled
                })),
            };
            
            Some(prompt)
        } else {
            None
        }
    }
    
    fn emit_overlays_except_prompt(&mut self, cx: &mut gpui::Context<crate::Core>) {
        let picker = self.try_create_picker_component();
        
        if let Some(picker) = picker {
            cx.emit(crate::Update::Picker(picker));
        }
        
        // Don't emit prompts here
        
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

    fn emit_overlays(&mut self, cx: &mut gpui::Context<crate::Core>) {

        let picker = self.try_create_picker_component();

        // Check for helix prompt and convert to native GPUI prompt
        let prompt = self.create_native_prompt_from_helix(cx);

        if let Some(picker) = picker {
            cx.emit(crate::Update::Picker(picker));
        }

        if let Some(prompt) = prompt {
            cx.emit(crate::Update::Prompt(prompt));
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
                
                
                // Track if this is a command mode key
                let is_command_key = key.code == helix_view::keyboard::KeyCode::Char(':');
                
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
                    if let Some((_before_pos, before_line)) = before_cursor {
                        if before_line != line {
                            debug!("Moved from line {before_line} to line {line}");
                        }
                    }
                }
                
                // Ensure cursor is visible after keyboard navigation
                // Check if the view exists before trying to ensure cursor visibility
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
                cx.emit(crate::Update::Redraw);
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
        /*
        use std::future::Future;
        let fut = self.step(cx);
        let mut fut = Box::pin(fut);
        handle.block_on(std::future::poll_fn(move |cx| {
            let _ = fut.as_mut().poll(cx);
            Poll::Ready(())
        }));
        */
    }

    pub async fn step(&mut self, cx: &mut gpui::Context<'_, crate::Core>) {
        loop {
            // Check if all views are closed and we should quit
            if self.editor.tree.views().count() == 0 {
                cx.emit(crate::Update::ShouldQuit);
                break;
            }

            tokio::select! {
                biased;

                // Some(event) = input_stream.next() => {
                //     // self.handle_input_event(event, cx);
                //     //self.handle_terminal_events(event).await;
                // }
                Some(callback) = self.jobs.callbacks.recv() => {
                    self.jobs.handle_callback(&mut self.editor, &mut self.compositor, Ok(Some(callback)));
                    // self.render().await;
                }
                Some(msg) = self.jobs.status_messages.recv() => {
                    let severity = match msg.severity{
                        helix_event::status::Severity::Hint => Severity::Hint,
                        helix_event::status::Severity::Info => Severity::Info,
                        helix_event::status::Severity::Warning => Severity::Warning,
                        helix_event::status::Severity::Error => Severity::Error,
                    };
                    let status = crate::EditorStatus { status: msg.message.to_string(), severity };
                    cx.emit(crate::Update::EditorStatus(status));
                    // TODO: show multiple status messages at once to avoid clobbering
                    self.editor.status_msg = Some((msg.message, severity));
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
                    // Convert bridged Helix events to GPUI Update events
                    let update = match bridged_event {
                        crate::event_bridge::BridgedEvent::DocumentChanged { doc_id } => {
                            crate::Update::DocumentChanged { doc_id }
                        }
                        crate::event_bridge::BridgedEvent::SelectionChanged { doc_id, view_id } => {
                            crate::Update::SelectionChanged { doc_id, view_id }
                        }
                        crate::event_bridge::BridgedEvent::ModeChanged { old_mode, new_mode } => {
                            crate::Update::ModeChanged { old_mode, new_mode }
                        }
                        crate::event_bridge::BridgedEvent::DiagnosticsChanged { doc_id } => {
                            crate::Update::DiagnosticsChanged { doc_id }
                        }
                        crate::event_bridge::BridgedEvent::DocumentOpened { doc_id } => {
                            crate::Update::DocumentOpened { doc_id }
                        }
                        crate::event_bridge::BridgedEvent::DocumentClosed { doc_id } => {
                            crate::Update::DocumentClosed { doc_id }
                        }
                        crate::event_bridge::BridgedEvent::ViewFocused { view_id } => {
                            crate::Update::ViewFocused { view_id }
                        }
                        crate::event_bridge::BridgedEvent::LanguageServerInitialized { server_id } => {
                            crate::Update::LanguageServerInitialized { server_id }
                        }
                        crate::event_bridge::BridgedEvent::LanguageServerExited { server_id } => {
                            crate::Update::LanguageServerExited { server_id }
                        }
                        crate::event_bridge::BridgedEvent::CompletionRequested { doc_id, view_id, trigger } => {
                            crate::Update::CompletionRequested { doc_id, view_id, trigger }
                        }
                    };
                    cx.emit(update);
                    helix_event::request_redraw();
                }
                Some(gpui_event) = async {
                    if let Some(ref mut rx) = self.gpui_to_helix_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    // Handle GPUI events that affect Helix
                    crate::gpui_to_helix_bridge::handle_gpui_event_in_helix(&gpui_event, &mut self.editor);
                    helix_event::request_redraw();
                }
                Some(callback) = self.jobs.wait_futures.next() => {
                    self.jobs.handle_callback(&mut self.editor, &mut self.compositor, callback);
                    // self.render().await;
                }
                event = self.editor.wait_event() => {
                    use helix_view::editor::EditorEvent;
                    match event {
                        EditorEvent::DocumentSaved(event) => {
                            self.handle_document_write(&event);
                            cx.emit(crate::Update::EditorEvent(EditorEvent::DocumentSaved(event)));
                        }
                        EditorEvent::IdleTimer => {
                            self.editor.clear_idle_timer();
                            /* dont send */
                        }
                        EditorEvent::Redraw => {
                            // Check if all views are closed after redraw
                            if self.editor.tree.views().count() == 0 {
                                cx.emit(crate::Update::ShouldQuit);
                                break;
                            }
                             cx.emit(crate::Update::EditorEvent(EditorEvent::Redraw));
                        }
                        EditorEvent::ConfigEvent(_) => {
                            /* TODO */
                        }
                        EditorEvent::LanguageServerMessage((id, call)) => {
                            // We need cx here but it's not available in the async context
                            // For now, handle without UI updates
                            let mut lsp_manager = crate::core::LspManager::new(&mut self.editor, &mut self.lsp_progress);
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

pub fn init_editor(
    args: Args,
    config: Config,
    lang_loader: syntax::Loader,
) -> Result<Application, Error> {
    use helix_view::editor::Action;

    let mut theme_parent_dirs = vec![helix_loader::config_dir()];
    theme_parent_dirs.extend(helix_loader::runtime_dirs().iter().cloned());
    let theme_loader = std::sync::Arc::new(helix_view::theme::Loader::new(&theme_parent_dirs));

    let true_color = true;
    let theme = config
        .theme
        .as_ref()
        .and_then(|theme| {
            theme_loader
                .load(theme)
                .map_err(|e| {
                    log::warn!("failed to load theme `{theme}` - {e}");
                    e
                })
                .ok()
                .filter(|theme| (true_color || theme.is_16_color()))
        })
        .unwrap_or_else(|| theme_loader.default_theme(true_color));

    let syn_loader = Arc::new(ArcSwap::from_pointee(lang_loader));
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
    let (bridge_tx, bridge_rx) = crate::event_bridge::create_bridge_channel();
    crate::event_bridge::initialize_bridge(bridge_tx);
    crate::event_bridge::register_event_hooks();
    
    // Initialize reverse event bridge system for GPUI -> Helix event forwarding
    let (gpui_to_helix_tx, gpui_to_helix_rx) = crate::gpui_to_helix_bridge::create_gpui_to_helix_channel();
    crate::gpui_to_helix_bridge::initialize_gpui_to_helix_bridge(gpui_to_helix_tx);
    crate::gpui_to_helix_bridge::register_gpui_event_handlers();
    
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
        // let path = Path::new("./test.rs");
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
            
            log::info!("Opening file from command line: {:?} with action: {:?}", file, action);
            match editor.open(&file, action) {
                Ok(doc_id) => {
                    log::info!("Successfully opened file from CLI: {:?} with doc_id: {:?}", file, doc_id);
                    
                    // Log document info
                    if let Some(doc) = editor.document(doc_id) {
                        log::info!("Document language: {:?}, path: {:?}", doc.language_name(), doc.path());
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
        project_directory: None,
        event_bridge_rx: Some(bridge_rx),
        gpui_to_helix_rx: Some(gpui_to_helix_rx),
    })
}
