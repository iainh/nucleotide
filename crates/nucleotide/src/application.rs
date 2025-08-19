use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::{ArcSwap, access::Map};
use futures_util::FutureExt;
use helix_core::{Position, Selection, pos_at_coords, syntax};
use helix_lsp::{LanguageServerId, LspProgressMap};
use helix_stdx::path::get_relative_path;
use helix_term::ui::FilePickerData;
use nucleotide_lsp::ServerStatus;

use gpui::AppContext;
use helix_term::{
    args::Args,
    compositor::{self, Compositor},
    config::Config,
    job::Jobs,
    keymap::Keymaps,
    ui::EditorView,
};
use helix_view::DocumentId;
use helix_view::document::DocumentSavedEventResult;
use helix_view::{Editor, doc_mut, graphics::Rect, handlers::Handlers};

// Helper function to find workspace root from a specific directory
#[instrument]
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
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};
use nucleotide_logging::{debug, error, info, instrument, timed, warn};

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
    pub completion_rx:
        Option<tokio::sync::mpsc::Receiver<helix_view::handlers::completion::CompletionEvent>>,
    pub completion_results_rx:
        Option<tokio::sync::mpsc::Receiver<nucleotide_events::completion_events::CompletionResult>>,
    pub completion_results_tx:
        Option<tokio::sync::mpsc::Sender<nucleotide_events::completion_events::CompletionResult>>,
    pub lsp_completion_requests_rx: Option<
        tokio::sync::mpsc::Receiver<nucleotide_events::completion_events::LspCompletionRequest>,
    >,
    pub lsp_completion_requests_tx: Option<
        tokio::sync::mpsc::Sender<nucleotide_events::completion_events::LspCompletionRequest>,
    >,
    pub config: crate::config::Config,
    pub helix_config_arc: Arc<ArcSwap<helix_term::config::Config>>,
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
    #[instrument(skip(self, cx))]
    pub fn sync_lsp_state(&self, cx: &mut gpui::App) {
        if let Some(lsp_state) = &self.lsp_state {
            // Check for active language servers
            let active_servers: Vec<(LanguageServerId, String)> = self
                .editor
                .language_servers
                .iter_clients()
                .map(|client| (client.id(), client.name().to_string()))
                .collect();

            info!(active_servers = ?active_servers, "Syncing LSP state");

            // Check which servers are progressing
            let progressing_servers: Vec<LanguageServerId> = active_servers
                .iter()
                .filter(|(id, _)| self.lsp_progress.is_progressing(*id))
                .map(|(id, _)| *id)
                .collect();

            info!(
                progressing_servers = ?progressing_servers,
                "Servers currently progressing according to lsp_progress"
            );

            // Get editor status for detailed logging
            let editor_status = self.editor.get_status();
            info!(
                editor_status = ?editor_status,
                "Current editor status from Helix"
            );

            lsp_state.update(cx, |state, _cx| {
                // Log current state before clearing
                let old_progress_count = state.progress.len();
                info!(
                    old_progress_count = old_progress_count,
                    "UI state before sync - clearing old progress"
                );

                // Clear old progress state
                state.progress.clear();

                // Update server info if we have new servers
                for (id, name) in &active_servers {
                    if !state.servers.contains_key(id) {
                        info!(
                            server_id = ?id,
                            server_name = %name,
                            "Registering new LSP server"
                        );
                        state.register_server(*id, name.clone(), None);
                        state.update_server_status(*id, ServerStatus::Running);
                    }
                }

                // Ensure servers without progress still show status (idle state)
                for (server_id, server_name) in &active_servers {
                    if !progressing_servers.contains(server_id) {
                        // Server is active but not progressing - show idle status
                        let progress = nucleotide_lsp::LspProgress {
                            server_id: *server_id,
                            token: "idle".to_string(),
                            title: "Connected".to_string(),
                            message: Some("Ready".to_string()),
                            percentage: None,
                        };

                        let key = format!("{}-idle", server_id);
                        state.progress.insert(key, progress);
                        info!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            "Added idle indicator for ready LSP server"
                        );
                    }
                }

                // Use editor status for progressing servers to show real LSP messages
                // The LSP manager calls editor.set_status() with progress messages
                if !progressing_servers.is_empty() {
                    info!(
                        progressing_count = progressing_servers.len(),
                        "Processing progressing servers"
                    );

                    for server_id in &progressing_servers {
                        // Find the server name from active_servers
                        let server_name = active_servers.iter()
                            .find(|(id, _)| id == server_id)
                            .map(|(_, name)| name.as_str())
                            .unwrap_or("LSP Server");

                        // Get the most recent progress information for this specific server
                        // from the LspProgressMap instead of using global editor status
                        let current_progress = self.lsp_progress.progress_map(*server_id);
                        let active_token_count = current_progress.map(|p| p.len()).unwrap_or(0);

                        let message = if active_token_count > 0 {
                            // We have active progress tokens - try to get the most relevant one
                            if let Some(progress_map) = current_progress {
                                // Look for progress tokens with meaningful information
                                // Priority: 1) Progress messages, 2) Progress titles, 3) Editor status
                                let active_progress = progress_map
                                    .iter()
                                    .find_map(|(token, status)| {
                                        info!(
                                            server_id = ?server_id,
                                            token = ?token,
                                            status = ?status,
                                            "Examining progress token"
                                        );

                                        match status {
                                            helix_lsp::ProgressStatus::Started { title, progress } => {
                                                // Extract message from WorkDoneProgress variants
                                                let message_from_progress = match progress {
                                                    helix_lsp::lsp::WorkDoneProgress::Begin(begin) => {
                                                        begin.message.as_ref().or(Some(&begin.title))
                                                    }
                                                    helix_lsp::lsp::WorkDoneProgress::Report(report) => {
                                                        report.message.as_ref()
                                                    }
                                                    helix_lsp::lsp::WorkDoneProgress::End(end) => {
                                                        end.message.as_ref()
                                                    }
                                                };

                                                // Prioritize progress message, then title
                                                if let Some(msg) = message_from_progress.filter(|m| !m.is_empty()) {
                                                    info!(
                                                        message = %msg,
                                                        token = ?token,
                                                        "Using progress message"
                                                    );
                                                    Some(msg.clone())
                                                } else if !title.is_empty() {
                                                    info!(
                                                        title = %title,
                                                        token = ?token,
                                                        "Using progress title"
                                                    );
                                                    Some(title.clone())
                                                } else {
                                                    None
                                                }
                                            }
                                            helix_lsp::ProgressStatus::Created => {
                                                info!(
                                                    token = ?token,
                                                    "Skipping Created progress token"
                                                );
                                                None
                                            }
                                        }
                                    })
                                    .or_else(|| {
                                        // Only use editor status if we have no progress tokens at all
                                        // If we have Created tokens, it means new work is starting and old status should be ignored
                                        let has_created_tokens = progress_map.values().any(|status| {
                                            matches!(status, helix_lsp::ProgressStatus::Created)
                                        });

                                        if has_created_tokens {
                                            // We have Created tokens, but check if editor status indicates ongoing work
                                            // If editor status contains meaningful work info, use it; otherwise ignore stale status
                                            if let Some((status_msg, _)) = &editor_status {
                                                if !status_msg.is_empty() && !status_msg.contains("building proc-macros") {
                                                    // Editor status looks like active work, not stale build messages
                                                    info!("Have Created tokens but editor status shows active work");
                                                    return Some(status_msg.to_string());
                                                }
                                            }
                                            info!("Have Created progress tokens - ignoring stale/irrelevant editor status");
                                            None
                                        } else {
                                            // Fallback to editor status only if no progress tokens exist at all
                                            info!("No progress tokens found, checking editor status");
                                            editor_status.as_ref()
                                                .filter(|(msg, _)| !msg.is_empty())
                                                .map(|(msg, _)| {
                                                    info!(
                                                        editor_message = %msg,
                                                        "Using editor status as fallback"
                                                    );
                                                    msg.to_string()
                                                })
                                        }
                                    })
                                    .unwrap_or_else(|| {
                                        info!("No active progress or meaningful editor status - showing idle");
                                        "Ready".to_string()
                                    });
                                Some(active_progress)
                            } else {
                                Some("Indexing project".to_string())
                            }
                        } else if let Some((status_msg, _severity)) = &editor_status {
                            if !status_msg.is_empty() {
                                Some(status_msg.to_string())
                            } else {
                                Some("Indexing project".to_string())
                            }
                        } else {
                            Some("Indexing project".to_string())
                        };

                        // Choose appropriate token and title based on whether we have meaningful progress
                        let (token, title) = if message.as_ref().map_or(false, |m| m == "Ready") {
                            ("idle".to_string(), "Connected".to_string())
                        } else {
                            ("activity".to_string(), "Processing".to_string())
                        };

                        let progress = nucleotide_lsp::LspProgress {
                            server_id: *server_id,
                            token,
                            title,
                            message: message.clone(),
                            percentage: None,
                        };

                        let key = if message.as_ref().map_or(false, |m| m == "Ready") {
                            format!("{}-idle", server_id)
                        } else {
                            format!("{}-activity", server_id)
                        };

                        let is_idle = progress.token == "idle";
                        let token_clone = progress.token.clone();
                        let title_clone = progress.title.clone();

                        state.progress.insert(key, progress);
                        info!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            progress_message = ?message,
                            token = %token_clone,
                            title = %title_clone,
                            is_idle = is_idle,
                            active_token_count = active_token_count,
                            editor_status = ?editor_status,
                            "Added LSP indicator with appropriate visual state"
                        );
                    }
                } else {
                    // No progressing servers - ensure we're not stuck with old progress
                    info!(
                        active_servers_count = active_servers.len(),
                        "No progressing servers - should show idle indicators only"
                    );
                }

                // Log final state for debugging
                info!(
                    final_progress_count = state.progress.len(),
                    server_count = state.servers.len(),
                    "UI state after sync"
                );

                if !state.progress.is_empty() {
                    for (key, progress) in &state.progress {
                        info!(
                            progress_key = %key,
                            server_id = ?progress.server_id,
                            title = %progress.title,
                            message = ?progress.message,
                            token = %progress.token,
                            "Final progress item in UI state"
                        );
                    }
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
    #[instrument(skip(self, cx))]
    pub fn check_for_picker_and_emit_event(&mut self, cx: &mut gpui::Context<crate::Core>) -> bool {
        use helix_term::ui::{Picker, overlay::Overlay};

        // Check for file picker first
        if self
            .compositor
            .find_id::<Overlay<Picker<PathBuf, FilePickerData>>>(helix_term::ui::picker::ID)
            .is_some()
        {
            info!("Detected file picker in compositor, emitting ShowFilePicker event");
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
            info!("Found and removed picker from compositor");
            if self.editor.documents.len() > 1 {
                info!(
                    "Multiple documents open, assuming buffer picker, emitting ShowBufferPicker event"
                );
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

    pub fn create_sample_completion_items(
        &self,
    ) -> Vec<nucleotide_ui::completion_v2::CompletionItem> {
        use nucleotide_ui::completion_v2::{CompletionItem, CompletionItemKind};

        // Create sample completion items
        vec![
            CompletionItem::new("println!")
                .with_kind(CompletionItemKind::Snippet)
                .with_description("macro")
                .with_documentation("Prints to the standard output, with a newline."),
            CompletionItem::new("String")
                .with_kind(CompletionItemKind::Struct)
                .with_description("std::string::String")
                .with_documentation("A UTF-8 encoded, growable string."),
            CompletionItem::new("Vec")
                .with_kind(CompletionItemKind::Struct)
                .with_description("std::vec::Vec<T>")
                .with_documentation("A contiguous growable array type."),
            CompletionItem::new("HashMap")
                .with_kind(CompletionItemKind::Struct)
                .with_description("std::collections::HashMap<K, V>")
                .with_documentation("A hash map implementation."),
            CompletionItem::new("println")
                .with_kind(CompletionItemKind::Function)
                .with_description("fn println(&str)")
                .with_documentation("Print to stdout with newline"),
            CompletionItem::new("print")
                .with_kind(CompletionItemKind::Function)
                .with_description("fn print(&str)")
                .with_documentation("Print to stdout without newline"),
            CompletionItem::new("format")
                .with_kind(CompletionItemKind::Function)
                .with_description("fn format(&str, ...) -> String")
                .with_documentation("Create a formatted string"),
        ]
    }

    #[instrument(skip(self))]
    pub fn open_file(&mut self, path: &Path) -> Result<(), anyhow::Error> {
        timed!("open_file", warn_threshold: std::time::Duration::from_millis(500), {
            let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
            doc_manager.open_file(path)
        })
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
            .sort_by_file_name(std::cmp::Ord::cmp) // Sort alphabetically
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
        let current_doc_id = self
            .editor
            .tree
            .try_get(self.editor.tree.focus)
            .map(|view| view.doc)
            .unwrap_or_else(|| {
                // Fallback to the first document if no view exists
                self.editor
                    .documents
                    .keys()
                    .next()
                    .copied()
                    .unwrap_or_default()
            });

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
            let flags_str = format!("{flags:<2}");

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
            let label = format!("{id_str:<4} {flags_str} {path_str}");

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
                let doc_id = comp_ctx
                    .editor
                    .tree
                    .try_get(view_id)
                    .map(|view| view.doc)
                    .unwrap_or_default();

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
                debug!(
                    view_id = ?view_id,
                    anchor = anchor,
                    "SetViewportAnchor"
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

        // Process any pending completion results from the coordinator
        self.process_completion_results(cx);

        // Process any pending LSP completion requests from the coordinator
        self.process_lsp_completion_requests(cx);

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

                    debug!(event_count = events.len(), "Processing batched events");

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
                        EditorEvent::ConfigEvent(config_event) => {
                            info!("Application received ConfigEvent: {:?}", config_event);
                            // Handle config updates
                            let old_config = self.editor.config();
                            info!("Old bufferline config: {:?}", old_config.bufferline);

                            match &config_event {
                                helix_view::editor::ConfigEvent::Update(new_editor_config) => {
                                    info!("New bufferline config in Update event: {:?}", new_editor_config.bufferline);
                                    // The toggle command sent us a new config
                                    // We detect what changed and store it as overrides
                                    self.config.apply_helix_config_update(new_editor_config);

                                    // Update the ArcSwap with the new config so the editor sees it
                                    let updated_helix_config = self.config.to_helix_config();
                                    info!("Updated helix config bufferline: {:?}", updated_helix_config.editor.bufferline);
                                    self.helix_config_arc.store(Arc::new(updated_helix_config));

                                    info!("Config updated via generic patching system");
                                }
                                helix_view::editor::ConfigEvent::Refresh => {
                                    // Reload config from files
                                    info!("Config refresh requested - reloading from files");
                                    if let Ok(fresh_config) = crate::config::Config::load() {
                                        self.config = fresh_config;
                                        let updated_helix_config = self.config.to_helix_config();
                                        self.helix_config_arc.store(Arc::new(updated_helix_config));
                                    }
                                }
                            }

                            // Refresh the editor's config-dependent state
                            self.editor.refresh_config(&old_config);
                            info!("After refresh_config, editor bufferline: {:?}", self.editor.config().bufferline);

                            // Forward the ConfigEvent to the workspace so it knows config changed
                            info!("Forwarding ConfigEvent to workspace");
                            cx.emit(crate::Update::EditorEvent(EditorEvent::ConfigEvent(config_event)));

                            // Also trigger a redraw to reflect changes
                            cx.emit(crate::Update::Redraw);
                            cx.emit(crate::Update::Event(AppEvent::Core(CoreEvent::RedrawRequested)));
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

impl Application {
    /// Take the completion receiver, leaving None in its place
    pub fn take_completion_receiver(
        &mut self,
    ) -> Option<tokio::sync::mpsc::Receiver<helix_view::handlers::completion::CompletionEvent>>
    {
        let receiver = self.completion_rx.take();
        nucleotide_logging::info!(
            "take_completion_receiver called - receiver present: {}",
            receiver.is_some()
        );
        receiver
    }

    /// Take the completion results receiver, leaving None in its place
    pub fn take_completion_results_receiver(
        &mut self,
    ) -> Option<tokio::sync::mpsc::Receiver<nucleotide_events::completion_events::CompletionResult>>
    {
        let receiver = self.completion_results_rx.take();
        nucleotide_logging::info!(
            "take_completion_results_receiver called - receiver present: {}",
            receiver.is_some()
        );
        receiver
    }

    /// Take the completion results sender, leaving None in its place
    pub fn take_completion_results_sender(
        &mut self,
    ) -> Option<tokio::sync::mpsc::Sender<nucleotide_events::completion_events::CompletionResult>>
    {
        let sender = self.completion_results_tx.take();
        nucleotide_logging::info!(
            "take_completion_results_sender called - sender present: {}",
            sender.is_some()
        );
        sender
    }

    /// Take the LSP completion requests receiver, leaving None in its place
    pub fn take_lsp_completion_requests_receiver(
        &mut self,
    ) -> Option<
        tokio::sync::mpsc::Receiver<nucleotide_events::completion_events::LspCompletionRequest>,
    > {
        let receiver = self.lsp_completion_requests_rx.take();
        nucleotide_logging::info!(
            "take_lsp_completion_requests_receiver called - receiver present: {}",
            receiver.is_some()
        );
        receiver
    }

    /// Take the LSP completion requests sender, leaving None in its place
    pub fn take_lsp_completion_requests_sender(
        &mut self,
    ) -> Option<tokio::sync::mpsc::Sender<nucleotide_events::completion_events::LspCompletionRequest>>
    {
        let sender = self.lsp_completion_requests_tx.take();
        nucleotide_logging::info!(
            "take_lsp_completion_requests_sender called - sender present: {}",
            sender.is_some()
        );
        sender
    }

    /// Process completion results from the coordinator and emit Update::Completion events
    /// This implements the event-based approach suggested by the user
    #[instrument(skip(self, cx))]
    pub fn process_completion_results(&mut self, cx: &mut gpui::Context<Self>) {
        // Check if we have completion results to process
        if let Some(ref mut completion_results_rx) = self.completion_results_rx {
            // Process all available completion results from the coordinator
            while let Ok(completion_result) = completion_results_rx.try_recv() {
                nucleotide_logging::info!(
                    "Application processing completion result: {:?}",
                    completion_result
                );

                match completion_result {
                    nucleotide_events::completion_events::CompletionResult::ShowCompletions {
                        items,
                        cursor,
                        doc_id,
                        view_id,
                    } => {
                        nucleotide_logging::info!(
                            "Creating CompletionView entity for {} items",
                            items.len()
                        );

                        // Create completion view entity
                        let completion_view = cx.new(|cx| {
                            let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);

                            // Convert CompletionEventItem to the format expected by CompletionView
                            let completion_items: Vec<
                                nucleotide_ui::completion_v2::CompletionItem,
                            > = items
                                .into_iter()
                                .map(|item| {
                                    use nucleotide_ui::completion_v2::CompletionItemKind;

                                    // Convert string kind to CompletionItemKind enum
                                    let kind = match item.kind.as_str() {
                                        "Function" => Some(CompletionItemKind::Function),
                                        "Constructor" => Some(CompletionItemKind::Constructor),
                                        "Method" => Some(CompletionItemKind::Method),
                                        "Variable" => Some(CompletionItemKind::Variable),
                                        "Field" => Some(CompletionItemKind::Field),
                                        "Class" => Some(CompletionItemKind::Class),
                                        "Interface" => Some(CompletionItemKind::Interface),
                                        "Module" => Some(CompletionItemKind::Module),
                                        "Property" => Some(CompletionItemKind::Property),
                                        "Enum" => Some(CompletionItemKind::Enum),
                                        "Keyword" => Some(CompletionItemKind::Keyword),
                                        "File" => Some(CompletionItemKind::File),
                                        _ => Some(CompletionItemKind::Text), // Default fallback
                                    };

                                    nucleotide_ui::completion_v2::CompletionItem {
                                        text: item.text.into(),
                                        kind,
                                        description: item.description.map(|s| s.into()),
                                        display_text: None, // Use default display text
                                        documentation: item.documentation.map(|s| s.into()),
                                    }
                                })
                                .collect();

                            view.set_items(completion_items, cx);
                            // Note: cursor position handling could be added here if needed
                            view
                        });

                        nucleotide_logging::info!(
                            "Emitting Update::Completion event with CompletionView entity"
                        );

                        // Emit the completion view via the event system
                        cx.emit(crate::Update::Completion(completion_view));
                    }
                    nucleotide_events::completion_events::CompletionResult::HideCompletions => {
                        nucleotide_logging::info!(
                            "Completion coordinator requested to hide completions"
                        );

                        // Create an empty completion view to effectively hide completions
                        let empty_completion_view =
                            cx.new(|cx| nucleotide_ui::completion_v2::CompletionView::new(cx));

                        cx.emit(crate::Update::Completion(empty_completion_view));
                    }
                    nucleotide_events::completion_events::CompletionResult::Error {
                        message,
                        doc_id,
                        view_id,
                    } => {
                        nucleotide_logging::error!(
                            error_message = %message,
                            doc_id = ?doc_id,
                            view_id = ?view_id,
                            "Completion coordinator reported error"
                        );

                        // For now, just hide completions on error
                        let empty_completion_view =
                            cx.new(|cx| nucleotide_ui::completion_v2::CompletionView::new(cx));

                        cx.emit(crate::Update::Completion(empty_completion_view));
                    }
                }
            }
        }
    }

    /// Process LSP completion requests from the coordinator
    /// This method handles requests for real LSP completion data from language servers
    #[instrument(skip(self, cx))]
    pub fn process_lsp_completion_requests(&mut self, cx: &mut gpui::Context<Self>) {
        // Check if we have LSP completion requests to process
        if let Some(ref mut lsp_completion_requests_rx) = self.lsp_completion_requests_rx {
            // Collect all available requests first to avoid borrow checker issues
            let mut requests = Vec::new();
            while let Ok(request) = lsp_completion_requests_rx.try_recv() {
                requests.push(request);
            }

            // Process the collected requests
            for request in requests {
                nucleotide_logging::info!(
                    cursor = request.cursor,
                    doc_id = ?request.doc_id,
                    view_id = ?request.view_id,
                    "Application processing LSP completion request"
                );

                // Handle the request in the main thread context where we have access to editor and LSP clients
                self.handle_lsp_completion_request(request, cx);
            }
        }
    }

    /// Handle a single LSP completion request
    #[instrument(skip(self, request, cx))]
    fn handle_lsp_completion_request(
        &mut self,
        request: nucleotide_events::completion_events::LspCompletionRequest,
        cx: &mut gpui::Context<Self>,
    ) {
        use nucleotide_events::completion_events::{CompletionEventItem, LspCompletionResponse};

        nucleotide_logging::info!(
            cursor = request.cursor,
            doc_id = ?request.doc_id,
            view_id = ?request.view_id,
            "Handling LSP completion request with real language server access"
        );

        // Try to get the document
        let doc = match self.editor.documents.get(&request.doc_id) {
            Some(doc) => doc,
            None => {
                nucleotide_logging::error!(doc_id = ?request.doc_id, "Document not found for completion request");
                let response = LspCompletionResponse {
                    items: vec![],
                    is_incomplete: false,
                    error: Some("Document not found".to_string()),
                };
                let _ = request.response_tx.send(response);
                return;
            }
        };

        // Get language servers that support completion for this document
        let language_servers: Vec<_> = self
            .editor
            .language_servers
            .iter_clients()
            .filter(|client| {
                if client.is_initialized() {
                    client.capabilities().completion_provider.is_some()
                } else {
                    false
                }
            })
            .collect();

        if language_servers.is_empty() {
            nucleotide_logging::warn!(doc_id = ?request.doc_id, "No language servers with completion support available for document");
            let response = LspCompletionResponse {
                items: vec![],
                is_incomplete: false,
                error: Some("No completion-capable language servers available".to_string()),
            };
            let _ = request.response_tx.send(response);
            return;
        }

        nucleotide_logging::info!(
            language_server_count = language_servers.len(),
            "Found language servers with completion support - making real LSP completion request"
        );

        // Convert cursor position to LSP position
        let text = doc.text();
        let cursor_pos = std::cmp::min(request.cursor, text.len_chars());
        let lsp_pos =
            helix_lsp::util::pos_to_lsp_pos(text, cursor_pos, helix_lsp::OffsetEncoding::Utf16);
        let doc_identifier = doc.identifier();

        // Use the first available language server for completion
        let language_server = &language_servers[0];

        // Create completion context - for now use manual trigger
        let context = helix_lsp::lsp::CompletionContext {
            trigger_kind: helix_lsp::lsp::CompletionTriggerKind::INVOKED,
            trigger_character: None,
        };

        // Make the completion request
        match language_server.completion(doc_identifier, lsp_pos, None, context) {
            Some(completion_future) => {
                // Spawn the future directly with tokio
                let response_tx = request.response_tx;
                let lsp_id = language_server.id();

                cx.background_executor().spawn(async move {
                    nucleotide_logging::info!(
                        lsp_id = %lsp_id,
                        "Making async LSP completion request - using thread spawn with dedicated Tokio runtime"
                    );
                    // Use std::thread::spawn with a dedicated Tokio runtime
                    // This avoids the issue with tokio::task::spawn_blocking requiring an existing runtime
                    let handle = std::thread::spawn(move || {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        rt.block_on(completion_future)
                    });
                    match handle.join() {
                        Ok(lsp_result) => match lsp_result {
                        Ok(Some(lsp_response)) => {
                            nucleotide_logging::info!("Received LSP completion response, converting to our format");
                            // Convert LSP completion response to our format
                            let (items, is_incomplete) = match lsp_response {
                                helix_lsp::lsp::CompletionResponse::Array(items) => (items, false),
                                helix_lsp::lsp::CompletionResponse::List(list) => (list.items, list.is_incomplete),
                            };
                            let completion_items: Vec<CompletionEventItem> = items.into_iter().map(|lsp_item| {
                                CompletionEventItem {
                                    text: lsp_item.label.clone(),
                                    kind: lsp_item.kind
                                        .map(|k| format!("{:?}", k))
                                        .unwrap_or_else(|| "Unknown".to_string()),
                                    description: lsp_item.detail,
                                    documentation: lsp_item.documentation
                                        .map(|doc| match doc {
                                            helix_lsp::lsp::Documentation::String(s) => s,
                                            helix_lsp::lsp::Documentation::MarkupContent(markup) => markup.value,
                                        }),
                                }
                            }).collect();
                            nucleotide_logging::info!(
                                item_count = completion_items.len(),
                                is_incomplete = is_incomplete,
                                "Converted LSP completion response to nucleotide format"
                            );
                            let response = LspCompletionResponse {
                                items: completion_items,
                                is_incomplete,
                                error: None,
                            };
                            if let Err(_) = response_tx.send(response) {
                                nucleotide_logging::error!("Failed to send LSP completion response - receiver dropped");
                            } else {
                                nucleotide_logging::info!("Successfully sent real LSP completion response");
                            }
                        },
                        Ok(None) => {
                            nucleotide_logging::info!("LSP server returned no completions");
                            let response = LspCompletionResponse {
                                items: vec![],
                                is_incomplete: false,
                                error: None,
                            };
                            let _ = response_tx.send(response);
                        },
                        Err(e) => {
                            nucleotide_logging::error!(error = %e, "LSP completion request failed");
                            let response = LspCompletionResponse {
                                items: vec![],
                                is_incomplete: false,
                                error: Some(format!("LSP completion request failed: {}", e)),
                            };
                            let _ = response_tx.send(response);
                        }
                        },
                        Err(e) => {
                            nucleotide_logging::error!("Thread join failed for LSP completion: {:?}", e);
                            let response = LspCompletionResponse {
                                items: vec![],
                                is_incomplete: false,
                                error: Some("Thread execution failed".to_string()),
                            };
                            let _ = response_tx.send(response);
                        }
                    }
                }).detach();
            }
            None => {
                nucleotide_logging::warn!("Language server does not support completion");
                let response = LspCompletionResponse {
                    items: vec![],
                    is_incomplete: false,
                    error: Some("Language server does not support completion".to_string()),
                };
                let _ = request.response_tx.send(response);
            }
        }
    }

    // NOTE: handle_crank_event is defined earlier in the file and includes completion processing
}

pub fn init_editor(
    args: Args,
    helix_config: Config,
    gui_config: crate::config::Config,
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

    // Load initial theme - will be corrected based on system appearance after window creation
    // For non-system modes, load the appropriate theme directly
    let theme_name = match gui_config.gui.theme.mode {
        crate::config::ThemeMode::Light => Some(gui_config.gui.theme.get_light_theme()),
        crate::config::ThemeMode::Dark => Some(gui_config.gui.theme.get_dark_theme()),
        crate::config::ThemeMode::System => {
            // For system mode, start with light theme as default since most systems start light
            // The window appearance observer will correct it to match actual OS appearance
            helix_config
                .theme
                .clone()
                .or_else(|| Some(gui_config.gui.theme.get_light_theme()))
        }
    };

    // Check if theme loading should be disabled for testing fallback colors
    let disable_theme_loading = std::env::var("NUCLEOTIDE_DISABLE_THEME_LOADING")
        .map(|val| val == "1" || val.to_lowercase() == "true")
        .unwrap_or(false);

    let theme = if disable_theme_loading {
        warn!(
            "Theme loading disabled via NUCLEOTIDE_DISABLE_THEME_LOADING - using default theme but derive_ui_theme will ignore it"
        );
        // Use any theme here - the derive_ui_theme function will ignore it when testing mode is enabled
        helix_view::Theme::default()
    } else {
        theme_name
            .and_then(|theme_name| {
                theme_loader
                    .load(&theme_name)
                    .map_err(|e| {
                        warn!(theme_name = %theme_name, error = %e, "Failed to load theme");
                        e
                    })
                    .ok()
                    .filter(|theme| (true_color || theme.is_16_color()))
            })
            .unwrap_or_else(|| theme_loader.default_theme(true_color))
    };

    let syn_loader = Arc::new(ArcSwap::from_pointee(lang_loader));

    // CRITICAL: Enable true_color support for GUI mode before creating the editor
    // This is required for themes to work correctly
    let mut helix_config = helix_config;
    helix_config.editor.true_color = true;

    let config = Arc::new(ArcSwap::from_pointee(helix_config));

    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 25,
    };
    // CRITICAL: Register events FIRST, before creating handlers
    helix_term::events::register();

    let (completion_tx, completion_rx) = tokio::sync::mpsc::channel(1);
    nucleotide_logging::info!("Created completion channel (tx/rx) for helix completion events");

    // Create completion results channel for coordinator -> workspace communication
    let (completion_results_tx, completion_results_rx) = tokio::sync::mpsc::channel(32);
    nucleotide_logging::info!(
        "Created completion results channel for coordinator->workspace communication"
    );

    // Create LSP completion request channel for coordinator -> application communication
    let (lsp_completion_requests_tx, lsp_completion_requests_rx) = tokio::sync::mpsc::channel(32);
    nucleotide_logging::info!(
        "Created LSP completion requests channel for coordinator->application communication"
    );
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

            info!(
                file = ?file,
                action = ?action,
                "Opening file from command line"
            );
            match editor.open(&file, action) {
                Ok(doc_id) => {
                    info!(
                        file = ?file,
                        doc_id = ?doc_id,
                        "Successfully opened file from CLI"
                    );

                    // Log document info
                    if let Some(doc) = editor.document(doc_id) {
                        info!(
                            language = ?doc.language_name(),
                            path = ?doc.path(),
                            "Document information"
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
                    error!(file = ?file, error = %e, "Failed to open file");
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

    // Initialize completion coordinator - but we need to do this after Application is created
    // since it needs access to the Core. This will be done in the workspace.

    nucleotide_logging::info!(
        "Application created with completion_rx stored - ready for coordinator initialization"
    );

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
        completion_rx: Some(completion_rx),
        completion_results_rx: Some(completion_results_rx),
        completion_results_tx: Some(completion_results_tx),
        lsp_completion_requests_rx: Some(lsp_completion_requests_rx),
        lsp_completion_requests_tx: Some(lsp_completion_requests_tx),
        config: gui_config,
        helix_config_arc: config,
    })
}

// Tests moved to tests/integration_test.rs to avoid GPUI proc macro compilation issues
// The issue: When compiling with --test, GPUI proc macros cause stack overflow
// when processing certain patterns in our codebase
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use crate::test_utils::test_support::{
        TestUpdate, create_counting_channel, create_test_diagnostic_events,
        create_test_document_events, create_test_selection_events,
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
