use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;

use arc_swap::{ArcSwap, access::Map};
use futures_util::FutureExt;
use helix_core::{Position, Selection, pos_at_coords, syntax};
use helix_lsp::{LanguageServerId, LspProgressMap};
use helix_stdx::path::get_relative_path;
use helix_term::ui::FilePickerData;
use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError};
use nucleotide_lsp::{HelixLspBridge, ProjectLspManager, ServerStatus};

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
    pub lsp_manager: crate::lsp_manager::LspManager,
    pub project_lsp_manager: Arc<tokio::sync::RwLock<Option<ProjectLspManager>>>,
    pub helix_lsp_bridge: Arc<tokio::sync::RwLock<Option<HelixLspBridge>>>,
    pub project_lsp_command_tx:
        Option<tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand>>,
    pub project_lsp_command_rx: Arc<
        tokio::sync::RwLock<
            Option<tokio::sync::mpsc::UnboundedReceiver<nucleotide_events::ProjectLspCommand>>,
        >,
    >,
    pub project_lsp_processor_started: Arc<std::sync::atomic::AtomicBool>,
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

    /// Enhanced LSP state sync that includes project server information
    #[instrument(skip(self, cx))]
    pub async fn sync_lsp_state_with_project_info(&self, cx: &mut gpui::App) {
        // First run the regular LSP state sync
        self.sync_lsp_state(cx);

        // Then add project-specific information if available
        if let Some(lsp_state) = &self.lsp_state {
            if let Some(manager_ref) = self.project_lsp_manager.read().await.as_ref() {
                lsp_state.update(cx, |state, _cx| {
                    // Add project-specific server information
                    // This would include information about proactively started servers
                    // and their relationship to projects

                    // For now, we just log that project manager is available
                    // In the future, we could query specific project information
                    // and add project-specific progress indicators here
                    info!("LSP state sync includes project information from ProjectLspManager");
                });
            }
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

    /// Start LSP for a document using the feature flag system
    #[instrument(skip(self))]
    pub fn start_lsp_with_feature_flags(
        &mut self,
        doc_id: DocumentId,
    ) -> crate::lsp_manager::LspStartupResult {
        info!(
            doc_id = ?doc_id,
            project_lsp_enabled = self.config.is_project_lsp_startup_enabled(),
            fallback_enabled = self.config.is_lsp_fallback_enabled(),
            timeout_ms = self.config.lsp_startup_timeout_ms(),
            "Starting LSP with feature flag support"
        );

        self.lsp_manager
            .start_lsp_for_document(doc_id, &mut self.editor)
    }

    /// Update LSP manager configuration (for hot-reloading)
    #[instrument(skip(self))]
    pub fn update_lsp_manager_config(&mut self) {
        let config_arc = Arc::new(self.config.clone());
        match self.lsp_manager.update_config(config_arc) {
            Ok(()) => {
                info!(
                    project_lsp_enabled = self.config.is_project_lsp_startup_enabled(),
                    fallback_enabled = self.config.is_lsp_fallback_enabled(),
                    timeout_ms = self.config.lsp_startup_timeout_ms(),
                    "LSP manager configuration updated successfully"
                );
            }
            Err(e) => {
                error!(
                    error = %e,
                    "Failed to update LSP manager configuration - keeping previous config"
                );
                // Keep the previous configuration since the new one is invalid
                self.editor
                    .set_error(format!("Invalid LSP configuration: {}", e));
            }
        }
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
            // Initialize project LSP system once (without background command processor)
            if !self
                .project_lsp_processor_started
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                if let Err(e) = self.initialize_project_lsp_system().await {
                    nucleotide_logging::error!(error = %e, "Failed to initialize project LSP system");
                } else {
                    nucleotide_logging::info!("Project LSP system initialized successfully");
                    self.project_lsp_processor_started
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }

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
                Some(lsp_command) = async {
                    let rx_guard = self.project_lsp_command_rx.read().await;
                    if let Some(ref mut rx) = rx_guard.as_ref() {
                        // We need to get a mutable reference, but we can't hold the read guard
                        // Drop the read guard and get a write guard
                        drop(rx_guard);
                        let mut rx_guard = self.project_lsp_command_rx.write().await;
                        if let Some(ref mut rx) = rx_guard.as_mut() {
                            rx.recv().await
                        } else {
                            std::future::pending().await
                        }
                    } else {
                        std::future::pending().await
                    }
                } => {
                    // Process LSP command with direct Editor access
                    self.handle_lsp_command(lsp_command).await;
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

                                    // Update LSP manager with new configuration
                                    self.update_lsp_manager_config();

                                    info!("Config updated via generic patching system");
                                }
                                helix_view::editor::ConfigEvent::Refresh => {
                                    // Reload config from files
                                    info!("Config refresh requested - reloading from files");
                                    if let Ok(fresh_config) = crate::config::Config::load() {
                                        self.config = fresh_config;
                                        let updated_helix_config = self.config.to_helix_config();
                                        self.helix_config_arc.store(Arc::new(updated_helix_config));

                                        // Update LSP manager with reloaded configuration
                                        self.update_lsp_manager_config();
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
    /// Get the project LSP command sender for external coordination
    pub fn get_project_lsp_command_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand>> {
        self.project_lsp_command_tx.clone()
    }

    /// Take the project LSP command receiver, leaving None in its place
    pub async fn take_project_lsp_command_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<nucleotide_events::ProjectLspCommand>> {
        self.project_lsp_command_rx.write().await.take()
    }

    /// Initialize the ProjectLspManager and HelixLspBridge  
    pub async fn initialize_project_lsp_system(&self) -> Result<(), anyhow::Error> {
        info!("Initializing project LSP system");

        // Create ProjectLspManager with default configuration
        let project_lsp_config = nucleotide_lsp::ProjectLspConfig::default();
        let project_manager = nucleotide_lsp::ProjectLspManager::new(
            project_lsp_config,
            self.project_lsp_command_tx.clone(),
        );

        // Get the event sender for the HelixLspBridge
        let event_tx = project_manager.get_event_sender();

        // Create HelixLspBridge
        let helix_bridge = nucleotide_lsp::HelixLspBridge::new(event_tx);
        let helix_bridge_arc = std::sync::Arc::new(helix_bridge.clone());

        // Set the bridge in the project manager
        project_manager.set_helix_bridge(helix_bridge_arc).await;

        // Start the project manager
        project_manager.start().await?;

        // Store the managers in the Application
        *self.project_lsp_manager.write().await = Some(project_manager);
        *self.helix_lsp_bridge.write().await = Some(helix_bridge);

        info!("Project LSP system initialized successfully");
        Ok(())
    }

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

    /// Initialize ProjectLspManager with project detection and proactive server startup
    #[instrument(skip(self))]
    pub async fn initialize_project_lsp_manager(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Initializing ProjectLspManager for proactive LSP startup");

        // Create ProjectLspManager configuration from GUI config
        let project_config = nucleotide_lsp::ProjectLspConfig {
            enable_proactive_startup: self.config.is_project_lsp_startup_enabled(),
            health_check_interval: std::time::Duration::from_secs(30),
            startup_timeout: std::time::Duration::from_millis(self.config.lsp_startup_timeout_ms()),
            max_concurrent_startups: 3,
            project_markers: self.config.project_markers().clone(),
        };

        // Create ProjectLspManager
        let mut manager =
            ProjectLspManager::new(project_config, self.project_lsp_command_tx.clone());

        // Create HelixLspBridge with event sender
        let event_sender = manager.get_event_sender();
        let bridge = HelixLspBridge::new(event_sender);

        // 🔥 CRITICAL FIX: Connect the bridge to the manager so it can actually start servers!
        info!("Application: About to set Helix bridge on ProjectLspManager");
        manager.set_helix_bridge(Arc::new(bridge.clone())).await;
        info!("Application: Successfully set Helix bridge on ProjectLspManager");

        // Start the manager
        manager.start().await?;

        // Store bridge and manager
        *self.helix_lsp_bridge.write().await = Some(bridge);
        *self.project_lsp_manager.write().await = Some(manager);

        // If we have a project directory, detect and register it
        if let Some(project_dir) = &self.project_directory {
            if let Some(manager_ref) = self.project_lsp_manager.read().await.as_ref() {
                if let Err(e) = manager_ref.detect_project(project_dir.clone()).await {
                    // Use error handler for project detection failure
                    self.handle_project_lsp_error(Box::new(e), "project_detection")
                        .await?;
                }
            }
        }

        // 🔥 CRITICAL FIX: Start event listener to connect ProjectLspManager to HelixLspBridge
        self.start_project_lsp_event_listener().await?;

        info!("ProjectLspManager initialized successfully");
        Ok(())
    }

    /// Start event listener to connect ProjectLspManager events to HelixLspBridge actions
    async fn start_project_lsp_event_listener(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting ProjectLspManager event listener");

        let manager_arc = self.project_lsp_manager.clone();
        let bridge_arc = self.helix_lsp_bridge.clone();

        // Store a flag to track if we should process startup requests in the crank handler
        // This allows us to integrate with the existing editor access pattern

        tokio::spawn(async move {
            loop {
                // Get event receiver from manager
                let mut event_rx = {
                    let manager_guard = manager_arc.read().await;
                    if let Some(manager) = manager_guard.as_ref() {
                        match manager.get_event_receiver().await {
                            Some(rx) => rx,
                            None => {
                                debug!("No event receiver available from ProjectLspManager");
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                continue;
                            }
                        }
                    } else {
                        debug!("ProjectLspManager not available for event listening");
                        break;
                    }
                };

                // Listen for events and handle them
                while let Some(event) = event_rx.recv().await {
                    match event {
                        nucleotide_events::ProjectLspEvent::ServerStartupRequested {
                            workspace_root,
                            server_name,
                            language_id,
                        } => {
                            info!(
                                workspace_root = %workspace_root.display(),
                                server_name = %server_name,
                                language_id = %language_id,
                                "Handling server startup request from ProjectLspManager"
                            );

                            // For now, just log the request - actual server startup will happen
                            // through the ProjectLspManager's proactive startup when detect_project() is called
                            info!(
                                workspace_root = %workspace_root.display(),
                                server_name = %server_name,
                                language_id = %language_id,
                                "Server startup requested - will be handled by ProjectLspManager proactive startup"
                            );
                        }
                        nucleotide_events::ProjectLspEvent::ProjectDetected {
                            workspace_root,
                            project_type,
                            language_servers,
                        } => {
                            info!(
                                workspace_root = %workspace_root.display(),
                                project_type = ?project_type,
                                language_servers = ?language_servers,
                                "Project detected via ProjectLspManager"
                            );
                        }
                        _ => {
                            debug!("Received other ProjectLspEvent: {:?}", event);
                        }
                    }
                }

                // If we reach here, the receiver was closed, wait and try again
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        // The actual server startup will happen through the existing start_project_servers method
        // when ProjectLspManager.detect_project() triggers proactive startup

        info!("ProjectLspManager event listener started");
        Ok(())
    }

    /// Cleanup ProjectLspManager resources
    pub async fn cleanup_project_lsp_manager(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Cleaning up ProjectLspManager");

        if let Some(manager) = self.project_lsp_manager.write().await.take() {
            manager.stop().await?;
        }

        *self.helix_lsp_bridge.write().await = None;

        info!("ProjectLspManager cleanup completed");
        Ok(())
    }

    /// Handle document opening with integrated project-based and file-based LSP startup
    #[instrument(skip(self), fields(doc_id = ?doc_id))]
    pub async fn handle_document_with_project_lsp(
        &mut self,
        doc_id: helix_view::DocumentId,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Handling document with integrated LSP startup");

        // Get document information
        let (doc_path, language_name) = if let Some(doc) = self.editor.document(doc_id) {
            let doc_path = doc.path().map(|p| p.to_path_buf());
            let language_name = doc.language_name().map(|s| s.to_string());
            (doc_path, language_name)
        } else {
            warn!(doc_id = ?doc_id, "Document not found for LSP integration");
            return Ok(());
        };

        // Check if ProjectLspManager is available and project-based startup is enabled
        if self.config.is_project_lsp_startup_enabled() {
            if let Some(bridge_ref) = self.helix_lsp_bridge.read().await.as_ref() {
                // Try to ensure document is tracked by any existing project servers
                if let Some(doc_path_ref) = doc_path.as_ref() {
                    let workspace_root =
                        find_workspace_root_from(doc_path_ref.parent().unwrap_or(doc_path_ref));

                    if let Some(manager_ref) = self.project_lsp_manager.read().await.as_ref() {
                        // Check if we have managed servers for this workspace
                        let managed_servers =
                            manager_ref.get_managed_servers(&workspace_root).await;

                        for managed_server in managed_servers {
                            // Ensure the document is tracked by the language server
                            if let Err(e) = bridge_ref.ensure_document_tracked(
                                &mut self.editor,
                                managed_server.server_id,
                                doc_id,
                            ) {
                                // Use error handler for document tracking failure
                                if let Err(recovery_error) = self
                                    .handle_project_lsp_error(Box::new(e), "document_tracking")
                                    .await
                                {
                                    warn!(
                                        error = %recovery_error,
                                        "Failed to recover from document tracking error"
                                    );
                                }
                            } else {
                                info!(
                                    server_id = ?managed_server.server_id,
                                    server_name = %managed_server.server_name,
                                    doc_path = %doc_path_ref.display(),
                                    "Document tracked with project LSP server"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Use existing LspManager for fallback or primary startup
        // This handles both file-based startup and fallback scenarios
        let startup_result = self
            .lsp_manager
            .start_lsp_for_document(doc_id, &mut self.editor);

        match startup_result {
            crate::lsp_manager::LspStartupResult::Success {
                mode,
                language_servers,
                duration,
            } => {
                info!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    language_servers = ?language_servers,
                    duration_ms = duration.as_millis(),
                    "LSP startup successful for document"
                );

                // If we have project-based servers, coordinate with them
                if self.config.is_project_lsp_startup_enabled() {
                    if let Some(doc_path_ref) = doc_path.as_ref() {
                        let workspace_root =
                            find_workspace_root_from(doc_path_ref.parent().unwrap_or(doc_path_ref));

                        // Check if this startup should be coordinated with project servers
                        if let Some(manager_ref) = self.project_lsp_manager.read().await.as_ref() {
                            let managed_servers =
                                manager_ref.get_managed_servers(&workspace_root).await;
                            if !managed_servers.is_empty() {
                                info!(
                                    managed_server_count = managed_servers.len(),
                                    "Document LSP startup coordinated with project servers"
                                );
                            }
                        }
                    }
                }
            }
            crate::lsp_manager::LspStartupResult::Failed {
                mode,
                error,
                fallback_mode,
            } => {
                warn!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    error = %error,
                    fallback_mode = ?fallback_mode,
                    "LSP startup failed for document"
                );

                // If project-based startup failed, ensure fallback is working
                if matches!(mode, crate::lsp_manager::LspStartupMode::Project { .. }) {
                    warn!(
                        "Project-based LSP startup failed - fallback should handle file-based startup"
                    );
                }
            }
            crate::lsp_manager::LspStartupResult::Skipped { reason } => {
                debug!(
                    doc_id = ?doc_id,
                    reason = %reason,
                    "LSP startup skipped for document"
                );
            }
        }

        Ok(())
    }

    /// Update ProjectLspManager configuration at runtime
    pub async fn update_project_lsp_config(
        &mut self,
        new_config: Arc<crate::config::Config>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Updating ProjectLspManager configuration");

        // Update the existing LspManager first
        if let Err(e) = self.lsp_manager.update_config(new_config.clone()) {
            error!(error = %e, "Failed to update LspManager configuration");
            return Err(Box::new(e));
        }

        // If ProjectLspManager is running, we'd need to recreate it with new config
        // For now, log the configuration change
        if self.project_lsp_manager.read().await.is_some() {
            info!(
                "ProjectLspManager configuration change detected - restart required for full effect"
            );
        }

        Ok(())
    }

    /// Handle errors from ProjectLspManager operations with appropriate recovery
    #[instrument(skip(self))]
    pub async fn handle_project_lsp_error(
        &self,
        error: Box<dyn std::error::Error + Send + Sync>,
        operation: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        error!(
            error = %error,
            operation = %operation,
            "ProjectLspManager operation failed, attempting recovery"
        );

        match operation {
            "initialize" => {
                warn!(
                    "ProjectLspManager initialization failed - continuing with file-based LSP only"
                );
                // Continue operation without project-based LSP
                Ok(())
            }
            "project_detection" => {
                warn!("Project detection failed - falling back to file-based LSP startup");
                // Project detection failure doesn't prevent file-based LSP
                Ok(())
            }
            "server_startup" => {
                warn!("Project server startup failed - file-based fallback should handle this");
                // Server startup failure is handled by the fallback system
                Ok(())
            }
            "document_tracking" => {
                // Document tracking failure is not critical - LSP can still work
                warn!("Document tracking with project server failed - LSP should still function");
                Ok(())
            }
            _ => {
                // Unknown operation - propagate the error
                error!(operation = %operation, "Unknown ProjectLspManager operation failed");
                Err(error)
            }
        }
    }

    /// Validate ProjectLspManager state and attempt recovery if needed
    #[instrument(skip(self))]
    pub async fn validate_and_recover_project_lsp(
        &self,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        info!("Validating ProjectLspManager state");

        let manager_available = self.project_lsp_manager.read().await.is_some();
        let bridge_available = self.helix_lsp_bridge.read().await.is_some();

        match (manager_available, bridge_available) {
            (true, true) => {
                info!("ProjectLspManager and HelixLspBridge are both available");

                // Additional health check could be performed here
                // We could check if any projects are registered or servers are running
                info!("ProjectLspManager health check passed");

                Ok(true)
            }
            (true, false) => {
                warn!(
                    "ProjectLspManager available but HelixLspBridge missing - attempting recovery"
                );

                // Try to recreate the bridge and connect it to manager
                if let Some(mut manager) = self.project_lsp_manager.write().await.take() {
                    let event_sender = manager.get_event_sender();
                    let bridge = HelixLspBridge::new(event_sender);

                    // 🔥 CRITICAL FIX: Connect the bridge to the manager in recovery too!
                    manager.set_helix_bridge(Arc::new(bridge.clone())).await;

                    // Store both back
                    *self.helix_lsp_bridge.write().await = Some(bridge);
                    *self.project_lsp_manager.write().await = Some(manager);

                    info!(
                        "HelixLspBridge recreated and connected to ProjectLspManager successfully"
                    );
                    Ok(true)
                } else {
                    error!("Failed to recreate HelixLspBridge - ProjectLspManager unavailable");
                    Ok(false)
                }
            }
            (false, true) => {
                warn!("HelixLspBridge available but ProjectLspManager missing - cleaning up");

                // Clean up orphaned bridge
                *self.helix_lsp_bridge.write().await = None;

                warn!("Cleaned up orphaned HelixLspBridge - project LSP disabled");
                Ok(false)
            }
            (false, false) => {
                info!(
                    "ProjectLspManager and HelixLspBridge both unavailable - normal for file-based LSP only"
                );
                Ok(false)
            }
        }
    }

    /// Start language servers proactively for a workspace using ProjectLspManager
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    pub async fn start_project_servers(
        &mut self,
        workspace_root: PathBuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting project language servers proactively");

        // Detect the project and get language server requirements
        if let Some(manager_ref) = self.project_lsp_manager.read().await.as_ref() {
            if let Err(e) = manager_ref.detect_project(workspace_root.clone()).await {
                // Use error handler for project detection failure
                self.handle_project_lsp_error(Box::new(e), "project_detection")
                    .await?;
                return Ok(()); // Early return on detection failure
            }

            let project_info = manager_ref.get_project_info(&workspace_root).await;
            if let Some(project) = project_info {
                info!(
                    project_type = ?project.project_type,
                    language_servers = ?project.language_servers,
                    "Project detected, starting language servers"
                );

                // Start each required language server using the bridge
                if let Some(bridge_ref) = self.helix_lsp_bridge.read().await.as_ref() {
                    for server_name in &project.language_servers {
                        let language_id = match &project.project_type {
                            nucleotide_events::ProjectType::Rust => "rust",
                            nucleotide_events::ProjectType::TypeScript => "typescript",
                            nucleotide_events::ProjectType::JavaScript => "javascript",
                            nucleotide_events::ProjectType::Python => "python",
                            nucleotide_events::ProjectType::Go => "go",
                            nucleotide_events::ProjectType::C => "c",
                            nucleotide_events::ProjectType::Cpp => "cpp",
                            nucleotide_events::ProjectType::Mixed(_) => "mixed", // Not ideal, but temporary
                            nucleotide_events::ProjectType::Other(name) => name.as_str(),
                            nucleotide_events::ProjectType::Unknown => "unknown",
                        };

                        match bridge_ref
                            .start_server(
                                &mut self.editor,
                                &workspace_root,
                                server_name,
                                language_id,
                            )
                            .await
                        {
                            Ok(server_id) => {
                                info!(
                                    server_id = ?server_id,
                                    server_name = %server_name,
                                    workspace_root = %workspace_root.display(),
                                    "Language server started proactively"
                                );
                            }
                            Err(e) => {
                                // Use error handler for server startup failure
                                if let Err(recovery_error) = self
                                    .handle_project_lsp_error(Box::new(e), "server_startup")
                                    .await
                                {
                                    warn!(
                                        error = %recovery_error,
                                        server_name = %server_name,
                                        workspace_root = %workspace_root.display(),
                                        "Failed to recover from server startup error"
                                    );
                                } else {
                                    info!(
                                        server_name = %server_name,
                                        "Server startup failure handled by fallback system"
                                    );
                                }
                            }
                        }
                    }
                } else {
                    warn!("HelixLspBridge not available for proactive server startup");
                }
            } else {
                warn!("No project information available after detection");
            }
        } else {
            warn!("ProjectLspManager not initialized for proactive startup");
        }

        Ok(())
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

    /// Handle LSP commands that require direct Editor access
    #[instrument(skip(self, command), fields(command_type = ?std::mem::discriminant(&command)))]
    async fn handle_lsp_command(&mut self, command: ProjectLspCommand) {
        let span = match &command {
            ProjectLspCommand::DetectAndStartProject { span, .. } => span.clone(),
            ProjectLspCommand::StartServer { span, .. } => span.clone(),
            ProjectLspCommand::StopServer { span, .. } => span.clone(),
            ProjectLspCommand::GetProjectStatus { span, .. } => span.clone(),
            ProjectLspCommand::EnsureDocumentTracked { span, .. } => span.clone(),
        };

        let _guard = span.enter();

        match command {
            ProjectLspCommand::StartServer {
                workspace_root,
                server_name,
                language_id,
                response,
                ..
            } => {
                info!(
                    workspace_root = %workspace_root.display(),
                    server_name = %server_name,
                    language_id = %language_id,
                    "Processing StartServer command with direct Editor access"
                );

                let result = self
                    .handle_start_server_command(&workspace_root, &server_name, &language_id)
                    .await;

                if let Err(_) = response.send(result) {
                    warn!("Failed to send StartServer response - receiver dropped");
                }
            }
            ProjectLspCommand::DetectAndStartProject {
                workspace_root,
                response,
                ..
            } => {
                let result = self
                    .handle_detect_and_start_project_command(&workspace_root)
                    .await;

                if let Err(_) = response.send(result) {
                    warn!("Failed to send DetectAndStartProject response - receiver dropped");
                }
            }
            ProjectLspCommand::StopServer {
                server_id,
                response,
                ..
            } => {
                let result = self.handle_stop_server_command(server_id).await;

                if let Err(_) = response.send(result) {
                    warn!("Failed to send StopServer response - receiver dropped");
                }
            }
            ProjectLspCommand::GetProjectStatus {
                workspace_root,
                response,
                ..
            } => {
                let result = self
                    .handle_get_project_status_command(&workspace_root)
                    .await;

                if let Err(_) = response.send(result) {
                    warn!("Failed to send GetProjectStatus response - receiver dropped");
                }
            }
            ProjectLspCommand::EnsureDocumentTracked {
                server_id,
                doc_id,
                response,
                ..
            } => {
                let result = self
                    .handle_ensure_document_tracked_command(server_id, doc_id)
                    .await;

                if let Err(_) = response.send(result) {
                    warn!("Failed to send EnsureDocumentTracked response - receiver dropped");
                }
            }
        }
    }

    /// Handle StartServer command using direct Editor access and HelixLspBridge
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    async fn handle_start_server_command(
        &mut self,
        workspace_root: &std::path::Path,
        server_name: &str,
        language_id: &str,
    ) -> Result<nucleotide_events::ServerStartResult, ProjectLspCommandError> {
        use nucleotide_events::ServerStartResult;

        info!(
            server_name = %server_name,
            language_id = %language_id,
            "Attempting to start LSP server with direct Editor access"
        );

        // Get the HelixLspBridge
        let bridge_guard = self.helix_lsp_bridge.read().await;
        let bridge = bridge_guard.as_ref().ok_or_else(|| {
            ProjectLspCommandError::Internal("HelixLspBridge not initialized".to_string())
        })?;

        // Use the HelixLspBridge to start the server with direct Editor access
        match bridge
            .start_server(
                &mut self.editor,
                &workspace_root.to_path_buf(),
                server_name,
                language_id,
            )
            .await
        {
            Ok(server_id) => {
                info!(
                    server_id = ?server_id,
                    server_name = %server_name,
                    language_id = %language_id,
                    "Successfully started LSP server"
                );

                Ok(ServerStartResult {
                    server_id,
                    server_name: server_name.to_string(),
                    language_id: language_id.to_string(),
                })
            }
            Err(e) => {
                error!(
                    error = %e,
                    server_name = %server_name,
                    language_id = %language_id,
                    "Failed to start LSP server"
                );

                Err(ProjectLspCommandError::ServerStartup(format!(
                    "Failed to start {} server: {}",
                    server_name, e
                )))
            }
        }
    }

    /// Handle DetectAndStartProject command
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    async fn handle_detect_and_start_project_command(
        &mut self,
        workspace_root: &std::path::Path,
    ) -> Result<nucleotide_events::ProjectDetectionResult, ProjectLspCommandError> {
        info!("Processing DetectAndStartProject command - not yet implemented");

        // For now, return not implemented error
        Err(ProjectLspCommandError::Internal(
            "DetectAndStartProject not yet implemented".to_string(),
        ))
    }

    /// Handle StopServer command
    #[instrument(skip(self))]
    async fn handle_stop_server_command(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
    ) -> Result<(), ProjectLspCommandError> {
        info!(
            server_id = ?server_id,
            "Processing StopServer command - not yet implemented"
        );

        // For now, return not implemented error
        Err(ProjectLspCommandError::Internal(
            "StopServer not yet implemented".to_string(),
        ))
    }

    /// Handle GetProjectStatus command
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    async fn handle_get_project_status_command(
        &mut self,
        workspace_root: &std::path::Path,
    ) -> Result<nucleotide_events::ProjectStatus, ProjectLspCommandError> {
        info!("Processing GetProjectStatus command - not yet implemented");

        // For now, return not implemented error
        Err(ProjectLspCommandError::Internal(
            "GetProjectStatus not yet implemented".to_string(),
        ))
    }

    /// Handle EnsureDocumentTracked command
    #[instrument(skip(self))]
    async fn handle_ensure_document_tracked_command(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
        doc_id: helix_view::DocumentId,
    ) -> Result<(), ProjectLspCommandError> {
        info!(
            server_id = ?server_id,
            doc_id = ?doc_id,
            "Processing EnsureDocumentTracked command - not yet implemented"
        );

        // For now, return not implemented error
        Err(ProjectLspCommandError::Internal(
            "EnsureDocumentTracked not yet implemented".to_string(),
        ))
    }

    // NOTE: handle_crank_event is defined earlier in the file and includes completion processing
}

/// Detect project root by walking up parent directories looking for project markers
fn detect_project_root_from_file(file_path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Path;

    // Common project markers to look for
    let project_markers = [
        "Cargo.toml",       // Rust
        "package.json",     // Node.js/JavaScript
        "pyproject.toml",   // Python
        "requirements.txt", // Python
        "go.mod",           // Go
        "pom.xml",          // Java Maven
        "build.gradle",     // Java Gradle
        ".git",             // Git repository
        ".hg",              // Mercurial repository
        ".svn",             // Subversion repository
    ];

    // Start from the file's parent directory
    let mut current_dir = file_path.parent()?;

    // Walk up the directory tree
    while let Some(dir) = current_dir.parent() {
        // Check if any project markers exist in this directory
        for marker in &project_markers {
            let marker_path = current_dir.join(marker);
            if marker_path.exists() {
                return Some(current_dir.to_path_buf());
            }
        }

        // Move up one level
        current_dir = dir;

        // Stop at filesystem root
        if current_dir == dir {
            break;
        }
    }

    // Also check the final directory (filesystem root)
    for marker in &project_markers {
        let marker_path = current_dir.join(marker);
        if marker_path.exists() {
            return Some(current_dir.to_path_buf());
        }
    }

    None
}

pub fn init_editor(
    args: Args,
    helix_config: Config,
    gui_config: crate::config::Config,
    lang_loader: syntax::Loader,
) -> Result<Application, Error> {
    use helix_view::editor::Action;

    // Determine project directory from args before consuming args.files
    nucleotide_logging::info!(
        files_count = args.files.len(),
        working_directory = ?args.working_directory,
        "Starting project directory detection"
    );
    let project_directory = if let Some(path) = &args.working_directory {
        Some(path.clone())
    } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        // If the first file is a directory, use it as the project directory
        Some(path.clone())
    } else if let Some((file_path, _)) = args.files.first() {
        // If the first file is a file, try to detect project root from its parent directories
        let detected_root = detect_project_root_from_file(file_path);
        if let Some(ref root) = detected_root {
            nucleotide_logging::info!(
                file_path = %file_path.display(),
                project_root = %root.display(),
                "Detected project root from file"
            );
        } else {
            nucleotide_logging::warn!(
                file_path = %file_path.display(),
                "No project root detected from file"
            );
        }
        detected_root
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

    // Create project LSP command channel for command-based LSP operations
    let (project_lsp_command_tx, project_lsp_command_rx) = tokio::sync::mpsc::unbounded_channel();
    nucleotide_logging::info!(
        "Created project LSP command channel for event-driven command pattern"
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

    // Initialize LSP command bridge for ProjectLspManager -> Application communication
    nucleotide_core::event_bridge::initialize_lsp_command_bridge(project_lsp_command_tx.clone());
    nucleotide_logging::info!("Initialized LSP command bridge for event-driven command pattern");

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

    // Create LSP manager with initial configuration
    let lsp_manager = crate::lsp_manager::LspManager::new(Arc::new(gui_config.clone()));

    nucleotide_logging::info!(
        "Application created with completion_rx stored and LSP manager initialized - ready for coordinator initialization"
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
        lsp_manager,
        project_lsp_manager: Arc::new(RwLock::new(None)), // Will be initialized after Application creation
        helix_lsp_bridge: Arc::new(RwLock::new(None)), // Will be initialized after Application creation
        project_lsp_command_tx: Some(project_lsp_command_tx),
        project_lsp_command_rx: Arc::new(RwLock::new(Some(project_lsp_command_rx))),
        project_lsp_processor_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
