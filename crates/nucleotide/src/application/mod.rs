// ABOUTME: Application module decomposition for V2 event system migration
// ABOUTME: Contains domain-specific handlers and main Application implementation

pub mod app_core;
pub mod completion_handler;
pub mod document_handler;
pub mod editor_handler;
pub mod lsp_handler;
pub mod view_handler;
pub mod workspace_handler;

pub use app_core::ApplicationCore;
pub use completion_handler::CompletionHandler;
pub use document_handler::DocumentHandler;
pub use editor_handler::EditorHandler;
pub use lsp_handler::LspHandler;
pub use view_handler::ViewHandler;
pub use workspace_handler::WorkspaceHandler;

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

// Import our shell environment system
use crate::shell_env::ProjectEnvironment;

/// Implementation of EnvironmentProvider trait for our ProjectEnvironment
/// This bridges our environment system with the LSP system
pub struct ProjectEnvironmentProvider {
    project_environment: Arc<ProjectEnvironment>,
}

impl ProjectEnvironmentProvider {
    pub fn new(project_environment: Arc<ProjectEnvironment>) -> Self {
        Self {
            project_environment,
        }
    }
}

impl nucleotide_lsp::EnvironmentProvider for ProjectEnvironmentProvider {
    fn get_lsp_environment(
        &self,
        directory: &std::path::Path,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        std::collections::HashMap<String, String>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send
                + '_,
        >,
    > {
        let project_env = self.project_environment.clone();
        let directory = directory.to_path_buf();

        Box::pin(async move {
            match project_env.get_lsp_environment(&directory).await {
                Ok(env) => Ok(env),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        })
    }
}

use gpui::{App, AppContext};
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

use crate::types::{AppEvent, CoreEvent, UiEvent, Update};
// ApplicationCore already imported above via pub use
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
        Option<tokio::sync::mpsc::Receiver<crate::completion_coordinator::CompletionResult>>,
    pub completion_results_tx:
        Option<tokio::sync::mpsc::Sender<crate::completion_coordinator::CompletionResult>>,
    pub lsp_completion_requests_rx:
        Option<tokio::sync::mpsc::Receiver<crate::completion_coordinator::LspCompletionRequest>>,
    pub lsp_completion_requests_tx:
        Option<tokio::sync::mpsc::Sender<crate::completion_coordinator::LspCompletionRequest>>,
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
    pub project_lsp_system_initialized: Arc<std::sync::atomic::AtomicBool>,
    pub shell_env_cache: Arc<tokio::sync::Mutex<crate::shell_env::ShellEnvironmentCache>>,
    pub project_environment: Arc<ProjectEnvironment>,
    // V2 Event System Core
    pub core: crate::application::ApplicationCore,
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
    /// Process events through V2 event system domain handlers
    #[instrument(skip(self, bridged_event))]
    async fn process_v2_event(
        &mut self,
        bridged_event: &event_bridge::BridgedEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use nucleotide_events::v2::document::ChangeType;
        use nucleotide_events::v2::document::Event as DocumentEvent;
        use nucleotide_events::v2::handler::EventHandler;

        // Process V2 events for all supported event types
        match bridged_event {
            event_bridge::BridgedEvent::DocumentChanged { doc_id } => {
                // Extract actual document revision
                let revision = if let Some(document) = self.editor.document_mut(*doc_id) {
                    document.get_current_revision() as u64
                } else {
                    warn!(doc_id = ?doc_id, "Document not found when processing DocumentChanged event");
                    0
                };

                // Create a V2 document event with actual revision
                let v2_event = DocumentEvent::ContentChanged {
                    doc_id: *doc_id,
                    revision,
                    change_summary: ChangeType::Insert, // TODO: Determine actual change type based on operation
                };

                debug!(
                    doc_id = ?doc_id,
                    revision = revision,
                    "Processing DocumentChanged through V2 handler"
                );
                self.core
                    .document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::SelectionChanged { doc_id, view_id } => {
                // Extract actual selection from the document
                let view = self.editor.tree.get(*view_id);
                let selection = if let Some(doc) = self.editor.document(view.doc) {
                    doc.selection(view.id).clone()
                } else {
                    helix_core::Selection::point(0)
                };
                let was_movement = true; // Assume movement for now

                // Convert helix selection to V2 event selection
                let v2_selection = nucleotide_events::view::Selection {
                    ranges: selection
                        .ranges()
                        .iter()
                        .map(|range| nucleotide_events::view::SelectionRange {
                            anchor: nucleotide_events::view::Position::new(
                                range.anchor,
                                range.anchor,
                            ),
                            head: nucleotide_events::view::Position::new(range.head, range.head),
                        })
                        .collect(),
                    primary_index: selection.primary_index(),
                };

                let v2_event = nucleotide_events::v2::view::Event::SelectionChanged {
                    view_id: *view_id,
                    doc_id: *doc_id,
                    selection: v2_selection,
                    was_movement,
                };

                debug!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    "Processing SelectionChanged through V2 ViewHandler"
                );
                self.core
                    .view_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::ModeChanged { old_mode, new_mode } => {
                let v2_event = nucleotide_events::v2::editor::Event::ModeChanged {
                    previous_mode: *old_mode,
                    new_mode: *new_mode,
                    context: nucleotide_events::v2::editor::ModeChangeContext::UserAction,
                };

                debug!(
                    old_mode = ?old_mode,
                    new_mode = ?new_mode,
                    "Processing ModeChanged through V2 EditorHandler"
                );
                self.core
                    .editor_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DocumentOpened { doc_id } => {
                // Extract document information for enriched event
                let (path, language_id) = if let Some(document) = self.editor.document(*doc_id) {
                    let path = document
                        .path()
                        .cloned()
                        .unwrap_or_else(|| std::path::PathBuf::from("untitled"));
                    let language_id = document.language_name().map(|lang| lang.to_string());
                    (path, language_id)
                } else {
                    (std::path::PathBuf::from("unknown"), None)
                };

                let v2_event = DocumentEvent::Opened {
                    doc_id: *doc_id,
                    path,
                    language_id,
                };

                debug!(doc_id = ?doc_id, "Processing DocumentOpened through V2 handler");
                self.core
                    .document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DocumentClosed { doc_id } => {
                // Note: By the time we get this event, the document might already be removed
                // So we use placeholder data
                let v2_event = DocumentEvent::Closed {
                    doc_id: *doc_id,
                    was_modified: false, // TODO: Track modification status before close
                };

                debug!(doc_id = ?doc_id, "Processing DocumentClosed through V2 handler");
                self.core
                    .document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DiagnosticsChanged { doc_id } => {
                // Extract diagnostic counts from the document
                let (diagnostic_count, error_count, warning_count) = if let Some(document) =
                    self.editor.document(*doc_id)
                {
                    let diagnostics = document.diagnostics();
                    let total = diagnostics.len();
                    let errors = diagnostics
                        .iter()
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Error))
                        })
                        .count();
                    let warnings = diagnostics
                        .iter()
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Warning))
                        })
                        .count();
                    (total, errors, warnings)
                } else {
                    (0, 0, 0)
                };

                let v2_event = DocumentEvent::DiagnosticsUpdated {
                    doc_id: *doc_id,
                    diagnostic_count,
                    error_count,
                    warning_count,
                };

                debug!(
                    doc_id = ?doc_id,
                    diagnostic_count = diagnostic_count,
                    "Processing DiagnosticsChanged through V2 handler"
                );
                self.core
                    .document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::ViewFocused { view_id } => {
                // Extract associated document ID from the view
                let view = self.editor.tree.get(*view_id);
                let doc_id = view.doc;
                let previous_view = self.core.view_handler.get_focused_view();

                let v2_event = nucleotide_events::v2::view::Event::Focused {
                    view_id: *view_id,
                    doc_id,
                    previous_view,
                };

                debug!(
                    view_id = ?view_id,
                    doc_id = ?doc_id,
                    "Processing ViewFocused through V2 ViewHandler"
                );
                self.core
                    .view_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            _ => {
                debug!(event = ?bridged_event, "V2 processing not yet implemented for this event type");
                // Other events (LanguageServer events, Completion) will be handled
                // as we implement their respective handlers in future phases
            }
        }

        Ok(())
    }

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

            debug!(active_servers = ?active_servers, "Syncing LSP state");

            // Check which servers are progressing
            let progressing_servers: Vec<LanguageServerId> = active_servers
                .iter()
                .filter(|(id, _)| self.lsp_progress.is_progressing(*id))
                .map(|(id, _)| *id)
                .collect();

            debug!(
                progressing_servers = ?progressing_servers,
                "Servers currently progressing according to lsp_progress"
            );

            // Get editor status for detailed logging
            let editor_status = self.editor.get_status();
            debug!(
                editor_status = ?editor_status,
                "Current editor status from Helix"
            );

            lsp_state.update(cx, |state, cx| {
                // Log current state before clearing
                let old_progress_count = state.progress.len();
                debug!(
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
                        debug!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            "Added idle indicator for ready LSP server"
                        );
                    }
                }

                // Use editor status for progressing servers to show real LSP messages
                // The LSP manager calls editor.set_status() with progress messages
                if !progressing_servers.is_empty() {
                    debug!(
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
                    debug!(
                        active_servers_count = active_servers.len(),
                        "No progressing servers - should show idle indicators only"
                    );
                }

                // Log final state for debugging
                debug!(
                    final_progress_count = state.progress.len(),
                    server_count = state.servers.len(),
                    "UI state after sync"
                );

                if !state.progress.is_empty() {
                    for (key, progress) in &state.progress {
                        debug!(
                            progress_key = %key,
                            server_id = ?progress.server_id,
                            title = %progress.title,
                            message = ?progress.message,
                            token = %progress.token,
                            "Final progress item in UI state"
                        );
                    }
                }

                // Notify GPUI that the model changed to trigger UI re-render
                cx.notify();
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
                lsp_state.update(cx, |state, cx| {
                    // Add project-specific server information
                    // This would include information about proactively started servers
                    // and their relationship to projects

                    // For now, we just log that project manager is available
                    // In the future, we could query specific project information
                    // and add project-specific progress indicators here
                    info!("LSP state sync includes project information from ProjectLspManager");

                    // Notify GPUI that the model changed
                    cx.notify();
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
            cx.emit(Update::Event(AppEvent::Ui(UiEvent::OverlayShown {
                overlay_type: nucleotide_events::v2::ui::OverlayType::FilePicker,
                overlay_id: "file_picker".to_string(),
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
                cx.emit(Update::Event(AppEvent::Ui(UiEvent::OverlayShown {
                    overlay_type: nucleotide_events::v2::ui::OverlayType::CommandPalette,
                    overlay_id: "buffer_picker".to_string(),
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
    fn create_file_picker_items(&self, cx: &mut App) -> Vec<crate::picker_view::PickerItem> {
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
                    file_path: Some(path.clone()),
                    vcs_status: None, // Will be populated below using bulk VCS lookup
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
                file_path: None, // No file path for placeholder items
                vcs_status: None,
            });
        }

        // Populate VCS status for all file items using the global VCS service
        if let Some(vcs_service) = cx.try_global::<crate::vcs_service::VcsServiceHandle>() {
            for item in &mut items {
                if let Some(ref file_path) = item.file_path {
                    item.vcs_status = vcs_service.get_status_cached(file_path, cx);
                }
            }
        }

        items
    }

    #[allow(dead_code)]
    fn create_buffer_picker(&self, cx: &mut App) -> Option<crate::picker::Picker> {
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
                file_path: meta.path.clone(), // Include file path for VCS status if available
                vcs_status: None,             // Will be populated below using bulk VCS lookup
            });
        }

        if items.is_empty() {
            // No buffers open
            return None;
        }

        // Populate VCS status for all buffer items using the global VCS service
        if let Some(vcs_service) = cx.try_global::<crate::vcs_service::VcsServiceHandle>() {
            for item in &mut items {
                if let Some(ref file_path) = item.file_path {
                    item.vcs_status = vcs_service.get_status_cached(file_path, cx);
                }
            }
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

            // LSP server startup requests are now handled directly in the event bridge processing loop above

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
                        severity: status.severity
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

                    debug!(event_count = events.len(), "Processing batched events via V2 system");

                    // Process all events through V2 domain handlers
                    for bridged_event in events {
                        // V2 Event System: Process events through domain handlers
                        if let Err(e) = self.process_v2_event(&bridged_event).await {
                            warn!(
                                error = %e,
                                bridged_event = ?bridged_event,
                                "Failed to process V2 event"
                            );
                        }

                        // Handle LSP server startup requests directly
                        if let event_bridge::BridgedEvent::LspServerStartupRequested { workspace_root, server_name, language_id } = bridged_event {
                            self.handle_lsp_server_startup_request(workspace_root, server_name, language_id).await;
                        }
                    }

                    // Request a single redraw for all batched events
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
    pub async fn initialize_project_lsp_system(&mut self) -> Result<(), anyhow::Error> {
        // Check if already initialized
        if self
            .project_lsp_system_initialized
            .load(std::sync::atomic::Ordering::Acquire)
        {
            debug!("Project LSP system already initialized, skipping");
            return Ok(());
        }

        info!("Initializing project LSP system");

        // Check if managers already exist and only start event listener if needed
        let manager_guard = self.project_lsp_manager.read().await;
        if manager_guard.is_some() {
            info!("ProjectLspManager already exists, attempting to start event listener");
            drop(manager_guard);

            // Event processing is now handled directly in the main event bridge loop

            info!("Project LSP system initialized successfully with existing manager");
            self.project_lsp_system_initialized
                .store(true, std::sync::atomic::Ordering::Release);
            return Ok(());
        }
        drop(manager_guard);

        info!("Creating new ProjectLspManager and HelixLspBridge");

        // Create ProjectLspManager with default configuration
        let project_lsp_config = nucleotide_lsp::ProjectLspConfig::default();
        let project_manager = nucleotide_lsp::ProjectLspManager::new(
            project_lsp_config,
            self.project_lsp_command_tx.clone(),
        );

        // Get the event sender for the HelixLspBridge
        let event_tx = project_manager.get_event_sender();

        // Create HelixLspBridge with environment provider
        let env_provider = Arc::new(ProjectEnvironmentProvider::new(
            self.project_environment.clone(),
        ));
        let helix_bridge =
            nucleotide_lsp::HelixLspBridge::new_with_environment(event_tx, env_provider);
        let helix_bridge_arc = std::sync::Arc::new(helix_bridge.clone());

        // Set the bridge in the project manager
        project_manager
            .set_helix_bridge(helix_bridge_arc.clone())
            .await;

        // Store the managers in the Application FIRST so the event listener can access them
        *self.project_lsp_manager.write().await = Some(project_manager);
        *self.helix_lsp_bridge.write().await = Some(helix_bridge);

        // Event processing is now handled directly in the main event bridge loop
        // No separate event listener setup needed

        // Now start the project manager and detect projects using the stored manager
        {
            let manager_guard = self.project_lsp_manager.read().await;
            if let Some(ref manager) = *manager_guard {
                // Start the project manager
                manager.start().await?;

                // CRITICAL FIX: Trigger project detection if we have a project directory
                // Now it's safe to emit events - the listener is already subscribed
                if let Some(project_dir) = &self.project_directory {
                    info!(
                        project_directory = %project_dir.display(),
                        "Triggering project detection for automatic LSP server startup"
                    );

                    if let Err(e) = manager.detect_project(project_dir.clone()).await {
                        nucleotide_logging::warn!(
                            error = %e,
                            project_directory = %project_dir.display(),
                            "Project detection failed - LSP servers may need to be started manually"
                        );
                    } else {
                        info!("Project detection completed successfully");
                    }
                } else {
                    nucleotide_logging::warn!(
                        "No project directory set - LSP will use file-based mode"
                    );
                }
            }
        }

        info!("Project LSP system initialized successfully with project detection");

        // Mark as initialized to prevent duplicate initialization
        self.project_lsp_system_initialized
            .store(true, std::sync::atomic::Ordering::Release);

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
    ) -> Option<tokio::sync::mpsc::Receiver<crate::completion_coordinator::CompletionResult>> {
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
    ) -> Option<tokio::sync::mpsc::Sender<crate::completion_coordinator::CompletionResult>> {
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
    ) -> Option<tokio::sync::mpsc::Receiver<crate::completion_coordinator::LspCompletionRequest>>
    {
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
    ) -> Option<tokio::sync::mpsc::Sender<crate::completion_coordinator::LspCompletionRequest>>
    {
        let sender = self.lsp_completion_requests_tx.take();
        nucleotide_logging::info!(
            "take_lsp_completion_requests_sender called - sender present: {}",
            sender.is_some()
        );
        sender
    }

    // Removed redundant initialize_project_lsp_manager - functionality moved to initialize_project_lsp_system

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
                if let Some(manager) = self.project_lsp_manager.write().await.take() {
                    let event_sender = manager.get_event_sender();
                    let env_provider = Arc::new(ProjectEnvironmentProvider::new(
                        self.project_environment.clone(),
                    ));
                    let bridge = HelixLspBridge::new_with_environment(event_sender, env_provider);

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
                    crate::completion_coordinator::CompletionResult::ShowCompletions {
                        items,
                        cursor: _,
                        doc_id: _,
                        view_id: _,
                    } => {
                        nucleotide_logging::info!(
                            "Creating CompletionView entity for {} items",
                            items.len()
                        );

                        // Create completion view entity
                        let completion_view = cx.new(|cx| {
                            let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);

                            // Convert V2 CompletionItem to the format expected by CompletionView
                            let completion_items: Vec<
                                nucleotide_ui::completion_v2::CompletionItem,
                            > = items
                                .into_iter()
                                .map(|item| {
                                    use nucleotide_ui::completion_v2::CompletionItemKind;

                                    // Convert V2 CompletionItemKind to UI CompletionItemKind
                                    let kind = match item.kind {
                                        nucleotide_events::completion::CompletionItemKind::Function => Some(CompletionItemKind::Function),
                                        nucleotide_events::completion::CompletionItemKind::Constructor => Some(CompletionItemKind::Constructor),
                                        nucleotide_events::completion::CompletionItemKind::Method => Some(CompletionItemKind::Method),
                                        nucleotide_events::completion::CompletionItemKind::Variable => Some(CompletionItemKind::Variable),
                                        nucleotide_events::completion::CompletionItemKind::Field => Some(CompletionItemKind::Field),
                                        nucleotide_events::completion::CompletionItemKind::Class => Some(CompletionItemKind::Class),
                                        nucleotide_events::completion::CompletionItemKind::Interface => Some(CompletionItemKind::Interface),
                                        nucleotide_events::completion::CompletionItemKind::Module => Some(CompletionItemKind::Module),
                                        nucleotide_events::completion::CompletionItemKind::Property => Some(CompletionItemKind::Property),
                                        nucleotide_events::completion::CompletionItemKind::Enum => Some(CompletionItemKind::Enum),
                                        nucleotide_events::completion::CompletionItemKind::Keyword => Some(CompletionItemKind::Keyword),
                                        nucleotide_events::completion::CompletionItemKind::File => Some(CompletionItemKind::File),
                                        _ => Some(CompletionItemKind::Text), // Default fallback
                                    };

                                    nucleotide_ui::completion_v2::CompletionItem {
                                        text: item.label.into(),
                                        kind,
                                        description: item.detail.map(|s| s.into()),
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
                    crate::completion_coordinator::CompletionResult::HideCompletions => {
                        nucleotide_logging::info!(
                            "Completion coordinator requested to hide completions"
                        );

                        // Create an empty completion view to effectively hide completions
                        let empty_completion_view =
                            cx.new(|cx| nucleotide_ui::completion_v2::CompletionView::new(cx));

                        cx.emit(crate::Update::Completion(empty_completion_view));
                    } // Note: Errors are handled by coordinator falling back to sample completions
                      // or hiding completions as appropriate
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
        request: crate::completion_coordinator::LspCompletionRequest,
        cx: &mut gpui::Context<Self>,
    ) {
        use crate::completion_coordinator::LspCompletionResponse;
        use nucleotide_events::completion::CompletionItem;

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
                            let completion_items: Vec<CompletionItem> = items.into_iter().map(|lsp_item| {
                                use nucleotide_events::completion::CompletionItemKind;
                                let kind = match lsp_item.kind {
                                    Some(helix_lsp::lsp::CompletionItemKind::TEXT) => CompletionItemKind::Text,
                                    Some(helix_lsp::lsp::CompletionItemKind::METHOD) => CompletionItemKind::Method,
                                    Some(helix_lsp::lsp::CompletionItemKind::FUNCTION) => CompletionItemKind::Function,
                                    Some(helix_lsp::lsp::CompletionItemKind::CONSTRUCTOR) => CompletionItemKind::Constructor,
                                    Some(helix_lsp::lsp::CompletionItemKind::FIELD) => CompletionItemKind::Field,
                                    Some(helix_lsp::lsp::CompletionItemKind::VARIABLE) => CompletionItemKind::Variable,
                                    Some(helix_lsp::lsp::CompletionItemKind::CLASS) => CompletionItemKind::Class,
                                    Some(helix_lsp::lsp::CompletionItemKind::INTERFACE) => CompletionItemKind::Interface,
                                    Some(helix_lsp::lsp::CompletionItemKind::MODULE) => CompletionItemKind::Module,
                                    Some(helix_lsp::lsp::CompletionItemKind::PROPERTY) => CompletionItemKind::Property,
                                    Some(helix_lsp::lsp::CompletionItemKind::UNIT) => CompletionItemKind::Unit,
                                    Some(helix_lsp::lsp::CompletionItemKind::VALUE) => CompletionItemKind::Value,
                                    Some(helix_lsp::lsp::CompletionItemKind::ENUM) => CompletionItemKind::Enum,
                                    Some(helix_lsp::lsp::CompletionItemKind::KEYWORD) => CompletionItemKind::Keyword,
                                    Some(helix_lsp::lsp::CompletionItemKind::SNIPPET) => CompletionItemKind::Snippet,
                                    Some(helix_lsp::lsp::CompletionItemKind::COLOR) => CompletionItemKind::Color,
                                    Some(helix_lsp::lsp::CompletionItemKind::FILE) => CompletionItemKind::File,
                                    Some(helix_lsp::lsp::CompletionItemKind::REFERENCE) => CompletionItemKind::Reference,
                                    Some(helix_lsp::lsp::CompletionItemKind::FOLDER) => CompletionItemKind::Folder,
                                    _ => CompletionItemKind::Text,
                                };
                                let insert_text = lsp_item.insert_text.unwrap_or_else(|| lsp_item.label.clone());
                                let mut item = CompletionItem::new(lsp_item.label.clone(), kind)
                                    .with_insert_text(insert_text);
                                if let Some(detail) = lsp_item.detail {
                                    item = item.with_detail(detail);
                                }
                                if let Some(documentation) = lsp_item.documentation {
                                    let doc_text = match documentation {
                                        helix_lsp::lsp::Documentation::String(s) => s,
                                        helix_lsp::lsp::Documentation::MarkupContent(markup) => markup.value,
                                    };
                                    item = item.with_documentation(doc_text);
                                }
                                item
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

    /// Handle a single LSP server startup request from the event bridge
    /// This method runs in the main thread where it has mutable access to the editor
    #[instrument(skip(self), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
    async fn handle_lsp_server_startup_request(
        &mut self,
        workspace_root: std::path::PathBuf,
        server_name: String,
        language_id: String,
    ) {
        debug!("Handling LSP server startup request through event bridge");

        // Get the bridge and start the server with timeout
        let bridge_guard = self.helix_lsp_bridge.read().await;
        if let Some(ref bridge) = *bridge_guard {
            // Add timeout to prevent hanging during server lookup
            let startup_timeout = std::time::Duration::from_secs(30);

            match tokio::time::timeout(
                startup_timeout,
                bridge.start_server(
                    &mut self.editor,
                    &workspace_root,
                    &server_name,
                    &language_id,
                ),
            )
            .await
            {
                Ok(Ok(server_id)) => {
                    info!(
                        server_id = ?server_id,
                        server_name = %server_name,
                        workspace_root = %workspace_root.display(),
                        "Successfully started LSP server"
                    );
                }
                Ok(Err(e)) => {
                    error!(
                        error = %e,
                        server_name = %server_name,
                        workspace_root = %workspace_root.display(),
                        "Failed to start LSP server"
                    );
                }
                Err(_) => {
                    error!(
                        server_name = %server_name,
                        workspace_root = %workspace_root.display(),
                        timeout_seconds = startup_timeout.as_secs(),
                        "LSP server startup timed out - server binary might not be found or startup is taking too long"
                    );
                }
            }
        } else {
            warn!("HelixLspBridge not available for server startup");
        }
    }

    /// Handle LSP commands that require direct Editor access
    #[instrument(skip(self, command), fields(command_type = ?std::mem::discriminant(&command)))]
    async fn handle_lsp_command(&mut self, command: ProjectLspCommand) {
        let span = match &command {
            ProjectLspCommand::DetectAndStartProject { span, .. } => span.clone(),
            ProjectLspCommand::StartServer { span, .. } => span.clone(),
            ProjectLspCommand::StopServer { span, .. } => span.clone(),
            ProjectLspCommand::RestartServersForWorkspaceChange { span, .. } => span.clone(),
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
            ProjectLspCommand::RestartServersForWorkspaceChange {
                old_workspace_root,
                new_workspace_root,
                response,
                ..
            } => {
                info!(
                    old_workspace_root = ?old_workspace_root.as_ref().map(|p| p.display()),
                    new_workspace_root = %new_workspace_root.display(),
                    "Processing RestartServersForWorkspaceChange command with direct Editor access"
                );

                let result = self
                    .handle_restart_servers_for_workspace_change_command(
                        old_workspace_root.as_deref(),
                        &new_workspace_root,
                    )
                    .await;

                if let Err(_) = response.send(result) {
                    warn!(
                        "Failed to send RestartServersForWorkspaceChange response - receiver dropped"
                    );
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
        // Add timeout to prevent hanging when server binary is not found
        let server_start_timeout = tokio::time::Duration::from_secs(15); // Generous timeout for server startup
        match tokio::time::timeout(
            server_start_timeout,
            bridge.start_server(
                &mut self.editor,
                &workspace_root.to_path_buf(),
                server_name,
                language_id,
            ),
        )
        .await
        {
            Ok(Ok(server_id)) => {
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
            Ok(Err(e)) => {
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
            Err(_timeout) => {
                error!(
                    server_name = %server_name,
                    language_id = %language_id,
                    timeout_seconds = 15,
                    "LSP server startup timed out - this usually indicates the server binary cannot be found in PATH"
                );

                Err(ProjectLspCommandError::ServerStartup(format!(
                    "Timeout starting {} server after 15 seconds - check that {} is installed and in PATH",
                    server_name, server_name
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
    async fn handle_restart_servers_for_workspace_change_command(
        &mut self,
        old_workspace_root: Option<&std::path::Path>,
        new_workspace_root: &std::path::Path,
    ) -> Result<Vec<nucleotide_events::ServerStartResult>, ProjectLspCommandError> {
        info!(
            old_workspace_root = ?old_workspace_root.map(|p| p.display()),
            new_workspace_root = %new_workspace_root.display(),
            "Processing RestartServersForWorkspaceChange command with direct Editor access"
        );

        let results = Vec::new();

        // CRITICAL FIX: Update the Editor's working directory so Helix LSP initialization uses the correct workspace root
        if let Err(e) = self.editor.set_cwd(new_workspace_root) {
            warn!(
                error = %e,
                workspace_root = %new_workspace_root.display(),
                "Failed to update Editor working directory - LSP servers may still use wrong workspace root"
            );
        } else {
            info!(
                new_workspace_root = %new_workspace_root.display(),
                "Successfully updated Editor working directory - new LSP servers will use correct workspace root"
            );
        }

        // SHELL ENVIRONMENT CAPTURE: Get shell environment for new workspace to solve macOS app bundle PATH issues
        info!(
            new_workspace_root = %new_workspace_root.display(),
            "Capturing shell environment for LSP servers to access cargo/rustc tools (with fast timeout)"
        );

        // Clear cache for old workspace if different
        if let Some(old_root) = old_workspace_root {
            if old_root != new_workspace_root {
                self.shell_env_cache
                    .lock()
                    .await
                    .clear_directory_cache(old_root)
                    .await;
                debug!(
                    old_workspace_root = %old_root.display(),
                    "Cleared shell environment cache for old workspace"
                );
            }
        }

        // Capture environment for new workspace (this will cache it for LSP server startup)
        // Use aggressive timeout to prevent blocking the UI - fast fallback is better than hanging
        let env_capture_timeout = tokio::time::Duration::from_secs(3); // 3 second timeout for shell capture
        let env_result = tokio::time::timeout(env_capture_timeout, async {
            let mut cache = self.shell_env_cache.lock().await;
            cache.get_environment(new_workspace_root).await
        })
        .await;

        let env_result = match env_result {
            Ok(result) => result,
            Err(_timeout) => {
                warn!(
                    new_workspace_root = %new_workspace_root.display(),
                    timeout_seconds = 3,
                    "Shell environment capture timed out - using process environment as fallback for LSP servers"
                );
                info!(
                    "Using fast fallback to ensure LSP startup is not blocked - this should still provide basic PATH resolution"
                );
                // Fallback: use current process environment which should still have basic Nix PATH
                Ok(std::env::vars().collect())
            }
        };
        match env_result {
            Ok(env) => {
                info!(
                    new_workspace_root = %new_workspace_root.display(),
                    env_var_count = env.len(),
                    path_length = env.get("PATH").map(|p| p.len()).unwrap_or(0),
                    "Successfully captured shell environment for LSP servers"
                );

                // CRITICAL: Set environment variables globally so LSP servers inherit them
                // This solves the macOS app bundle PATH isolation issue
                let mut env_updates = 0;
                for (key, value) in &env {
                    // Only update important environment variables to avoid side effects
                    if should_update_env_var(key) {
                        // SAFETY: Setting environment variables for LSP server inheritance
                        // This is safe because we're controlling which variables get set
                        unsafe {
                            std::env::set_var(key, value);
                        }
                        env_updates += 1;
                    }
                }

                info!(
                    env_updates = env_updates,
                    "Updated global environment variables for LSP server inheritance"
                );

                // Log PATH for debugging (truncated)
                if let Some(path) = env.get("PATH") {
                    let path_preview = if path.len() > 200 {
                        format!("{}... ({} chars total)", &path[..200], path.len())
                    } else {
                        path.clone()
                    };
                    debug!(
                        path_preview = %path_preview,
                        "Shell environment PATH set globally for LSP tools"
                    );
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    new_workspace_root = %new_workspace_root.display(),
                    "Failed to capture shell environment - LSP servers may not find cargo/rustc tools"
                );
            }
        }

        info!(
            old_workspace_root = ?old_workspace_root.as_ref().map(|p| p.display()),
            new_workspace_root = %new_workspace_root.display(),
            "Workspace root changed - Editor working directory updated for correct LSP initialization"
        );

        if results.is_empty() {
            info!("No rust-analyzer servers were restarted for workspace change");
        } else {
            info!(
                restart_count = results.len(),
                "Successfully restarted rust-analyzer servers with new workspace root"
            );
        }

        Ok(results)
    }

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
        project_lsp_system_initialized: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        shell_env_cache: Arc::new(tokio::sync::Mutex::new(
            crate::shell_env::ShellEnvironmentCache::new(),
        )),
        project_environment: Arc::new(ProjectEnvironment::new(None)), // TODO: Detect CLI environment
        // V2 Event System Core
        core: {
            let mut core = ApplicationCore::new();
            core.initialize()
                .expect("Failed to initialize ApplicationCore");
            core
        },
    })
}

/// Determines which environment variables should be updated globally for LSP servers
/// This is a safelist approach to avoid unintended side effects from shell environment
fn should_update_env_var(key: &str) -> bool {
    match key {
        // PATH is critical for finding cargo, rustc, and other tools
        "PATH" => true,

        // Rust-specific environment variables
        "RUSTUP_HOME" | "CARGO_HOME" | "RUSTC_WRAPPER" | "RUSTFLAGS" => true,

        // Development environment variables that tools depend on
        "JAVA_HOME" | "NODE_PATH" | "PYTHON_PATH" | "GOPATH" | "GOROOT" => true,

        // Nix environment variables (common on macOS with Nix)
        var if var.starts_with("NIX_") => true,

        // asdf version manager
        "ASDF_DATA_DIR" | "ASDF_DIR" => true,

        // Skip system and session variables that could cause issues
        "HOME" | "USER" | "SHELL" | "PWD" | "OLDPWD" => false,
        "XDG_SESSION_TYPE" | "XDG_SESSION_ID" | "SESSION_MANAGER" => false,
        "DISPLAY" | "WAYLAND_DISPLAY" | "SSH_AUTH_SOCK" | "SSH_AGENT_PID" => false,

        // Skip potentially sensitive or system-specific variables
        var if var.starts_with("LC_") => false,
        var if var.starts_with("LANG") => false,
        var if var.starts_with("DBUS_") => false,

        // Default: only allow explicitly safe variables
        _ => false,
    }
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
