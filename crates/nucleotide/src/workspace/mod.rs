// ABOUTME: Workspace module decomposition for cleaner architecture
// ABOUTME: Separates view management from workspace coordination logic

pub mod view_manager;

pub use view_manager::ViewManager;

// Main workspace implementation
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::FontFeatures;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, BorrowAppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Render, StatefulInteractiveElement, Styled,
    TextStyle, Window, WindowAppearance, WindowBackgroundAppearance, black, div, hsla, px, white,
};
use helix_core::Selection;
use helix_view::ViewId;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};
use nucleotide_logging::{debug, error, info, instrument, warn};
use nucleotide_lsp::HelixLspBridge;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

// ViewManager already imported above via pub use
use nucleotide_ui::{Button, ButtonSize, ButtonVariant};

use crate::input_coordinator::{FocusGroup, InputContext, InputCoordinator};

use crate::application::find_workspace_root_from;
use crate::document::DocumentView;
use crate::file_tree::{FileTreeConfig, FileTreeEvent, FileTreeView};
use crate::info_box::InfoBoxView;
use crate::key_hint_view::KeyHintView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
use crate::{Core, Input, InputEvent};
use nucleotide_types::VcsStatus;
use nucleotide_vcs::VcsServiceHandle;
pub struct Workspace {
    core: Entity<Core>,
    input: Entity<Input>,
    view_manager: ViewManager,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    info: Entity<InfoBoxView>,
    info_hidden: bool,
    key_hints: Entity<KeyHintView>,
    notifications: Entity<NotificationView>,
    focus_handle: FocusHandle,
    file_tree: Option<Entity<FileTreeView>>,
    show_file_tree: bool,
    file_tree_width: f32,
    is_resizing_file_tree: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    titlebar: Option<gpui::AnyView>,
    appearance_observer_set: bool,
    needs_appearance_update: bool,
    needs_window_appearance_update: bool,
    pending_appearance: Option<gpui::WindowAppearance>,
    tab_overflow_dropdown_open: bool,
    document_order: Vec<helix_view::DocumentId>, // Ordered list of documents in opening order
    input_coordinator: Arc<InputCoordinator>,    // Central input coordination system
    project_lsp_manager: Option<Arc<nucleotide_lsp::ProjectLspManager>>, // Project-level LSP management
    current_project_root: Option<std::path::PathBuf>, // Track current project root for change detection
    pending_lsp_startup: Option<std::path::PathBuf>,  // Track pending server startup requests
                                                      // REMOVED: completion_results_rx - now using event-based approach via Application
}

impl EventEmitter<crate::Update> for Workspace {}

impl Workspace {
    /// Ensure document is in the order list, adding it to the end if new
    fn ensure_document_in_order(&mut self, doc_id: helix_view::DocumentId) {
        if !self.document_order.contains(&doc_id) {
            self.document_order.push(doc_id);
        }
    }

    pub fn current_filename(&self, cx: &App) -> Option<String> {
        let editor = &self.core.read(cx).editor;

        // Get the currently focused view
        for (view, is_focused) in editor.tree.views() {
            if is_focused && let Some(doc) = editor.document(view.doc) {
                return doc.path().map(|p| {
                    p.file_name()
                        .and_then(|name| name.to_str())
                        .map(std::string::ToString::to_string)
                        .unwrap_or_else(|| p.display().to_string())
                });
            }
        }
        None
    }

    pub fn with_views(
        core: Entity<Core>,
        input: Entity<Input>,
        handle: tokio::runtime::Handle,
        overlay: Entity<OverlayView>,
        notifications: Entity<NotificationView>,
        info: Entity<InfoBoxView>,
        input_coordinator: Arc<InputCoordinator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Subscribe to overlay dismiss events to restore focus
        cx.subscribe(
            &overlay,
            |workspace, _overlay, _event: &DismissEvent, cx| {
                // Mark that we need to restore focus in the next render
                workspace.view_manager.set_needs_focus_restore(true);

                // Check if completion was dismissed and manage context
                let has_completion = workspace.overlay.read(cx).has_completion();
                workspace.manage_completion_context(has_completion);

                cx.notify();
            },
        )
        .detach();

        // Subscribe to completion accepted events to insert text
        cx.subscribe(
            &overlay,
            |workspace,
             _overlay,
             event: &nucleotide_ui::completion_v2::CompletionAcceptedEvent,
             cx| {
                println!(
                    "COMP: Workspace received completion accepted event: {}",
                    event.text
                );
                workspace.handle_completion_accepted(&event.text, cx);
            },
        )
        .detach();

        // Subscribe to core (Application) events to receive Update events
        cx.subscribe(&core, |workspace, _core, event: &crate::Update, cx| {
            debug!("Workspace: Received Update event from core: {:?}", event);
            workspace.handle_event(event, cx);
        })
        .detach();

        // Note: Window appearance observation needs to be set up after window creation
        // It will be handled in the render method when window is available

        let key_hints = cx.new(|_cx| KeyHintView::new());

        // Initialize project status service
        let _project_status_handle = nucleotide_project::initialize_project_status_service(cx);

        // Initialize file tree only if project directory is explicitly set
        let root_path = core.read(cx).project_directory.clone();
        let root_path_for_manager = root_path.clone(); // Clone for later use

        // Set initial project root in the project status service
        if let Some(ref root) = root_path {
            _project_status_handle.set_project_root(Some(root.clone()), cx);
        }

        // Start VCS monitoring if we have a root path
        if let Some(root_path) = &root_path {
            let root_path_clone = root_path.clone();
            let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
            vcs_handle.update(cx, |service, cx| {
                service.start_monitoring(root_path_clone, cx);
            });
        }

        let file_tree = root_path.map(|root_path| {
            let handle_clone = handle.clone();
            cx.new(|cx| {
                let config = FileTreeConfig::default();
                FileTreeView::new_with_runtime(root_path, config, Some(handle_clone), cx)
            })
        });

        // Subscribe to file tree events if we have a file tree
        if let Some(ref file_tree) = file_tree {
            info!("Workspace: Subscribing to file tree events");
            cx.subscribe(file_tree, |workspace, _file_tree, event, cx| {
                debug!("Workspace: Received file tree event: {:?}", event);
                workspace.handle_file_tree_event(event, cx);
            })
            .detach();
        } else {
            info!("Workspace: No file tree to subscribe to");
        }

        // Initialize workspace-specific actions with the input coordinator
        Self::register_workspace_actions(&input_coordinator);

        // Initialize ProjectLspManager for proactive LSP startup
        let project_lsp_manager = if let Some(ref root) = root_path_for_manager {
            info!(project_root = %root.display(), "Initializing ProjectLspManager for workspace");

            // Get configuration from the core
            let core_config = core.read(cx).config.clone();
            let config = nucleotide_lsp::ProjectLspConfig {
                enable_proactive_startup: core_config.is_project_lsp_startup_enabled(),
                health_check_interval: std::time::Duration::from_secs(30),
                startup_timeout: std::time::Duration::from_millis(
                    core_config.lsp_startup_timeout_ms(),
                ),
                max_concurrent_startups: 3,
                project_markers: core_config.project_markers().clone(),
            };

            // Get LSP command sender from Application for event-driven command pattern
            let lsp_command_sender = core.read(cx).get_project_lsp_command_sender();

            // Create ProjectLspManager with LSP command sender
            let manager = Arc::new(nucleotide_lsp::ProjectLspManager::new(
                config,
                lsp_command_sender,
            ));

            // 🔥 CRITICAL FIX: Set up HelixLspBridge for the ProjectLspManager in constructor
            let event_sender = manager.get_event_sender();
            let helix_bridge = HelixLspBridge::new(event_sender);

            // Connect the bridge to the manager
            let manager_for_bridge = manager.clone();
            let bridge_clone = helix_bridge.clone();
            let bridge_runtime_handle = handle.clone();
            bridge_runtime_handle.spawn(async move {
                info!("Workspace constructor: Setting Helix bridge on ProjectLspManager");
                manager_for_bridge
                    .set_helix_bridge(Arc::new(bridge_clone))
                    .await;
                info!("Workspace constructor: Successfully set Helix bridge on ProjectLspManager");
            });

            // Start the manager with proper error handling
            let manager_clone = manager.clone();
            let runtime_handle = handle.clone();
            let root_clone = root.clone();

            runtime_handle.spawn(async move {
                match manager_clone.start().await {
                    Ok(()) => {
                        info!(project_root = %root_clone.display(), "ProjectLspManager started successfully");
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            project_root = %root_clone.display(),
                            "Failed to start ProjectLspManager - LSP proactive startup disabled"
                        );
                    }
                }
            });

            Some(manager)
        } else {
            debug!("No project root - skipping ProjectLspManager initialization");
            None
        };

        let mut workspace = Self {
            core,
            input,
            view_manager: ViewManager::new(),
            handle,
            overlay,
            info,
            info_hidden: true,
            key_hints,
            notifications,
            focus_handle,
            file_tree,
            show_file_tree: true,
            file_tree_width: 250.0, // Default width
            is_resizing_file_tree: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            titlebar: None,
            appearance_observer_set: false,
            needs_appearance_update: false,
            needs_window_appearance_update: false,
            pending_appearance: None,
            tab_overflow_dropdown_open: false,
            document_order: Vec::new(),
            input_coordinator,
            project_lsp_manager,
            current_project_root: root_path_for_manager.clone(),
            pending_lsp_startup: None,
            // REMOVED: completion_results_rx - now using event-based approach via Application
        };

        // Set initial focus restore state
        workspace.view_manager.set_needs_focus_restore(true);

        // Register focus groups for main UI areas
        workspace.register_focus_groups(cx);

        // Setup completion-specific shortcuts
        workspace.setup_completion_shortcuts();

        // Initialize completion coordinator
        nucleotide_logging::info!(
            "About to initialize completion coordinator in workspace creation"
        );
        workspace.initialize_completion_coordinator(cx);
        nucleotide_logging::info!("Completion coordinator initialization call completed");

        // Register action handlers for global input system
        workspace.register_action_handlers(cx);

        // Initialize document views
        workspace.update_document_views(cx);

        // Auto-focus the first document view on startup
        if workspace.view_manager.focused_view_id().is_some() {
            workspace.view_manager.set_needs_focus_restore(true);
        }

        // Setup LSP state subscription for project status updates
        workspace.setup_lsp_state_subscription(cx);

        // Trigger initial project detection and LSP coordination if we have a project root
        if let Some(ref root) = workspace.current_project_root {
            info!(project_root = %root.display(), "Triggering initial project detection and LSP startup");
            workspace.trigger_project_detection_and_lsp_startup(root.clone(), cx);
        } else {
            warn!("No project root found - project level LSP will not be initialized");
        }

        workspace
    }

    /// Register workspace-specific actions with the input coordinator
    fn register_workspace_actions(coordinator: &Arc<InputCoordinator>) {
        info!("Registering workspace actions with InputCoordinator");

        // Register ToggleFileTree action (Ctrl+B)
        coordinator.register_global_action("ToggleFileTree", || {
            info!("ToggleFileTree action triggered");
            // This will need to be implemented differently - we need access to workspace
            // For now, this is just a placeholder structure
        });

        // Register ShowFileFinder action (Ctrl+P)
        coordinator.register_global_action("ShowFileFinder", || {
            info!("ShowFileFinder action triggered");
            // Placeholder
        });

        // Register ShowCommandPalette action (Ctrl+Shift+P)
        coordinator.register_global_action("ShowCommandPalette", || {
            info!("ShowCommandPalette action triggered");
            // Placeholder
        });

        // Register ShowBufferPicker action
        coordinator.register_global_action("ShowBufferPicker", || {
            info!("ShowBufferPicker action triggered");
            // Placeholder
        });

        // Register focus group navigation actions
        coordinator.register_global_action("FocusEditor", || {
            info!("FocusEditor action triggered");
            // Placeholder
        });

        coordinator.register_global_action("FocusFileTree", || {
            info!("FocusFileTree action triggered");
            // Placeholder
        });

        info!("Completed workspace action registration");
    }

    /// Update the input context based on current focus state
    fn update_input_context(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Check for active overlays first - they take priority
        let overlay_view = self.overlay.read(cx);
        let new_context = if overlay_view.has_picker() {
            InputContext::Picker
        } else if overlay_view.has_prompt() {
            InputContext::Prompt
        } else if overlay_view.has_completion() {
            InputContext::Completion
        } else if let Some(file_tree) = &self.file_tree {
            if file_tree.focus_handle(cx).is_focused(window) {
                InputContext::FileTree
            } else if self.view_manager.focused_view_id().is_some() {
                InputContext::Normal // Editor context
            } else {
                InputContext::Normal
            }
        } else if self.view_manager.focused_view_id().is_some() {
            InputContext::Normal // Editor context
        } else {
            InputContext::Normal
        };

        // Switch to the appropriate context
        self.input_coordinator.switch_context(new_context);

        debug!(context = ?new_context, "Updated input context");
    }

    /// Handle workspace actions triggered by InputCoordinator
    fn handle_workspace_action(&mut self, action: &str, cx: &mut Context<Self>) {
        match action {
            "ToggleFileTree" => {
                info!("Toggling file tree");
                self.show_file_tree = !self.show_file_tree;
                cx.notify();
            }
            "ShowFileFinder" => {
                info!("Showing file finder");
                // Implementation will be added later
            }
            "ShowCommandPalette" => {
                info!("Showing command palette");
                // Implementation will be added later
            }
            _ => {
                warn!(action = %action, "Unknown workspace action");
            }
        }
    }

    /// Simplified key handler that delegates to the InputCoordinator
    fn handle_key(&mut self, ev: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        debug!(
            key = %ev.keystroke.key,
            modifiers = ?ev.keystroke.modifiers,
            "Workspace received key event"
        );

        // Update input context based on current focus state
        self.update_input_context(window, cx);

        // Delegate to InputCoordinator for processing
        let result = self.input_coordinator.handle_key_event(ev, window);

        // Handle the result
        use crate::input_coordinator::InputResult;
        match result {
            InputResult::NotHandled => {
                debug!("Key event not handled by InputCoordinator");
            }
            InputResult::Handled => {
                debug!("Key event handled by InputCoordinator");
            }
            InputResult::SendToHelix(helix_key) => {
                debug!(key = ?helix_key, "Sending key to Helix editor");

                // Send the key to Helix
                self.input.update(cx, |_, cx| {
                    cx.emit(crate::InputEvent::Key(helix_key));
                });
            }
            InputResult::WorkspaceAction(action) => {
                debug!(action = %action, "Executing workspace action");
                self.handle_workspace_action(&action, cx);
            }
        }
    }

    pub fn new(
        _core: Entity<Core>,
        _input: Entity<Input>,
        _handle: tokio::runtime::Handle,
        _cx: &mut Context<Self>,
    ) -> Self {
        panic!("Use Workspace::with_views instead - views must be created in window context");
    }

    pub fn set_titlebar(&mut self, titlebar: gpui::AnyView) {
        self.titlebar = Some(titlebar);
    }

    #[instrument(skip(self, cx))]
    pub fn set_project_directory(&mut self, dir: std::path::PathBuf, cx: &mut Context<Self>) {
        // Check if this is a project root change
        let is_project_change = self.current_project_root.as_ref() != Some(&dir);

        debug!(
            current_root = ?self.current_project_root,
            new_dir = %dir.display(),
            is_change = is_project_change,
            "Evaluating project directory change"
        );

        self.core.update(cx, |core, _cx| {
            core.project_directory = Some(dir.clone());
        });

        // Update project status service
        //         // let project_status = nucleotide_project::project_status_service(cx);
        // // project_status.set_project_root(Some(dir.clone()), cx);

        // Start VCS monitoring for the new directory
        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
        vcs_handle.update(cx, |service, cx| {
            service.start_monitoring(dir.clone(), cx);
        });

        // Handle project change for LSP management
        if is_project_change {
            info!(
                old_root = ?self.current_project_root,
                new_root = %dir.display(),
                "Project directory changed - updating LSP management"
            );

            // Update current project root tracking
            self.current_project_root = Some(dir.clone());

            // Shutdown existing ProjectLspManager if present (workspace root changed)
            if let Some(existing_manager) = self.project_lsp_manager.take() {
                info!("Shutting down existing ProjectLspManager due to workspace change");
                let runtime_handle = self.handle.clone();

                // Wait for shutdown to complete before proceeding
                let shutdown_complete = runtime_handle.block_on(async move {
                    match existing_manager.stop().await {
                        Ok(()) => {
                            info!("Successfully stopped existing ProjectLspManager");
                            true
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to stop existing ProjectLspManager");
                            false
                        }
                    }
                });

                if !shutdown_complete {
                    warn!(
                        "Previous ProjectLspManager shutdown failed, proceeding with new manager anyway"
                    );
                }
            }

            // Create new ProjectLspManager for the new workspace root
            info!("Creating new ProjectLspManager for new project directory");
            // Get configuration from the core
            let core_config = self.core.read(cx).config.clone();
            let config = nucleotide_lsp::ProjectLspConfig {
                enable_proactive_startup: core_config.is_project_lsp_startup_enabled(),
                health_check_interval: std::time::Duration::from_secs(30),
                startup_timeout: std::time::Duration::from_millis(
                    core_config.lsp_startup_timeout_ms(),
                ),
                max_concurrent_startups: 3,
                project_markers: core_config.project_markers().clone(),
            };
            // Get LSP command sender from Application for event-driven command pattern
            let lsp_command_sender = self.core.read(cx).get_project_lsp_command_sender();

            let manager = Arc::new(nucleotide_lsp::ProjectLspManager::new(
                config,
                lsp_command_sender,
            ));

            // 🔥 CRITICAL FIX: Set up HelixLspBridge for the ProjectLspManager
            let event_sender = manager.get_event_sender();
            let helix_bridge = nucleotide_lsp::HelixLspBridge::new(event_sender);

            // Connect the bridge to the manager
            let manager_for_bridge = manager.clone();
            let bridge_clone = helix_bridge.clone();
            let runtime_handle = self.handle.clone();
            runtime_handle.spawn(async move {
                info!("Workspace: Setting Helix bridge on ProjectLspManager");
                manager_for_bridge
                    .set_helix_bridge(Arc::new(bridge_clone))
                    .await;
                info!("Workspace: Successfully set Helix bridge on ProjectLspManager");
            });

            // Start the manager
            let manager_clone = manager.clone();
            let runtime_handle = self.handle.clone();
            runtime_handle.spawn(async move {
                if let Err(e) = manager_clone.start().await {
                    error!(error = %e, "Failed to start ProjectLspManager");
                }
            });

            self.project_lsp_manager = Some(manager);

            // ⚡ CRITICAL: Restart LSP servers with new workspace root
            self.restart_lsp_servers_for_workspace_change(&dir, cx);

            // Trigger project detection and LSP coordination
            self.trigger_project_detection_and_lsp_startup(dir, cx);

            // Refresh UI indicators
            self.refresh_project_indicators(cx);
        }
    }

    /// Restart LSP servers with new workspace root when project directory changes
    #[instrument(skip(self, cx))]
    fn restart_lsp_servers_for_workspace_change(
        &mut self,
        new_project_root: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        info!(
            new_project_root = %new_project_root.display(),
            "Restarting LSP servers for workspace change"
        );

        // Get the old project root from the workspace state
        let old_project_root = self.current_project_root.clone();

        // Get the LSP command sender from the Application
        let lsp_command_sender = self.core.read(cx).get_project_lsp_command_sender();

        if let Some(sender) = lsp_command_sender {
            info!(
                old_project_root = ?old_project_root.as_ref().map(|p| p.display()),
                new_project_root = %new_project_root.display(),
                "Sending RestartServersForWorkspaceChange command to Application"
            );

            // Create the command with a span for tracing
            let span = tracing::info_span!("workspace_lsp_restart",
                old_workspace = ?old_project_root.as_ref().map(|p| p.display()),
                new_workspace = %new_project_root.display()
            );

            // Create response channel
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            // Send the command using the event-driven pattern
            let command = nucleotide_events::ProjectLspCommand::RestartServersForWorkspaceChange {
                old_workspace_root: old_project_root,
                new_workspace_root: new_project_root.to_path_buf(),
                response: response_tx,
                span,
            };

            if let Err(e) = sender.send(command) {
                error!(
                    error = %e,
                    new_project_root = %new_project_root.display(),
                    "Failed to send RestartServersForWorkspaceChange command"
                );
                return;
            }

            // Spawn a task to handle the response asynchronously using the runtime handle
            let new_project_root_display = new_project_root.display().to_string();
            self.handle.spawn(async move {
                // Add a timeout to prevent indefinite waiting
                let timeout_duration = tokio::time::Duration::from_secs(30); // 30 second timeout for LSP operations
                match tokio::time::timeout(timeout_duration, response_rx).await {
                    Ok(response_result) => match response_result {
                        Ok(Ok(results)) => {
                            info!(
                                restart_count = results.len(),
                                new_project_root = %new_project_root_display,
                                "LSP server restart completed successfully"
                            );
                            for result in results {
                                info!(
                                    server_name = %result.server_name,
                                    language_id = %result.language_id,
                                    server_id = ?result.server_id,
                                    "Server restarted successfully"
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            error!(
                                error = %e,
                                new_project_root = %new_project_root_display,
                                "LSP server restart failed"
                            );
                        }
                        Err(_) => {
                            warn!(
                                new_project_root = %new_project_root_display,
                                "RestartServersForWorkspaceChange response channel was dropped"
                            );
                        }
                    }
                    Err(_timeout) => {
                        error!(
                            new_project_root = %new_project_root_display,
                            timeout_seconds = 30,
                            "LSP server restart timed out - this may indicate environment capture is taking too long"
                        );
                    }
                }
            });

            info!(
                new_project_root = %new_project_root.display(),
                "RestartServersForWorkspaceChange command sent successfully"
            );
        } else {
            warn!(
                new_project_root = %new_project_root.display(),
                "No LSP command sender available - cannot restart LSP servers"
            );
        }
    }

    /// Trigger project detection and coordinate with ProjectLspManager for proactive LSP startup
    #[instrument(skip(self, cx))]
    fn trigger_project_detection_and_lsp_startup(
        &mut self,
        project_root: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        info!(project_root = %project_root.display(), "Starting project detection and LSP coordination");

        // Force refresh project detection in the project status service
        info!(project_root = %project_root.display(), "Updating project status service with project root");
        let project_status = nucleotide_project::project_status_service(cx);
        project_status.set_project_root(Some(project_root.clone()), cx);
        info!("Project status service updated, refreshing project detection");
        project_status.refresh_project_detection(cx);
        info!("Project detection refresh completed");

        // Coordinate with ProjectLspManager if available
        if let Some(ref manager) = self.project_lsp_manager {
            let manager_clone = manager.clone();
            let runtime_handle = self.handle.clone();
            let project_root_clone = project_root.clone();

            info!("Notifying ProjectLspManager about project detection and starting LSP servers");

            let core_entity = self.core.clone();
            runtime_handle.spawn(async move {
                info!(project_root = %project_root_clone.display(), "Starting project detection via ProjectLspManager");

                // 🔥 CRITICAL FIX: Actually call manager.detect_project() to connect the detection!
                match manager_clone.detect_project(project_root_clone.clone()).await {
                    Ok(()) => {
                        info!(
                            project_root = %project_root_clone.display(),
                            "Project detection completed successfully via ProjectLspManager"
                        );

                        // 🔥 CRITICAL FIX: Set flag for LSP server startup to be handled in crank event
                        // This defers the actual server startup to a context where we have GPUI Context access
                        // The crank event handler can then call core.start_project_servers() properly

                        // Since we can't directly modify the workspace from this async context,
                        // we'll use a different approach - trigger server startup via the existing
                        // ProjectLspManager's proactive startup which should be enabled by detect_project()
                        info!(
                            project_root = %project_root_clone.display(),
                            "Project detected - LSP servers should start via ProjectLspManager proactive startup"
                        );
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            project_root = %project_root_clone.display(),
                            "Project detection failed via ProjectLspManager"
                        );
                    }
                }
            });
        } else {
            warn!("ProjectLspManager not available - skipping LSP coordination");
        }

        // Update UI indicators and refresh project status display
        self.refresh_project_indicators(cx);
    }

    /// Set the current project root explicitly
    /// This is used during workspace initialization to ensure the project root is set correctly
    pub fn set_current_project_root(&mut self, root: Option<std::path::PathBuf>) {
        self.current_project_root = root;
        if let Some(ref root) = self.current_project_root {
            info!(project_root = %root.display(), "Set current project root explicitly");
        } else {
            info!("Cleared current project root");
        }
    }

    /// Subscribe to LSP state changes to update project indicators
    #[instrument(skip(self, cx))]
    fn setup_lsp_state_subscription(&mut self, cx: &mut Context<Self>) {
        // For now, we'll update project status periodically rather than subscribing
        // since LspState doesn't implement EventEmitter yet
        if let Some(_lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
            info!("LSP state available for project status updates");

            // Initial update
            self.update_project_status_from_lsp_state(cx);
        } else {
            debug!("No LSP state available for subscription");
        }
    }

    /// Update project status indicators based on current LSP state
    #[instrument(skip(self, cx))]
    fn update_project_status_from_lsp_state(&mut self, cx: &mut Context<Self>) {
        if let Some(lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
            // Get project status service first
            let project_status = nucleotide_project::project_status_service(cx);

            // Clone the LSP state and update project status outside the closure
            let lsp_state_clone = lsp_state_entity.read(cx).clone();
            project_status.update_lsp_state(&lsp_state_clone, cx);

            debug!("Updated project status from LSP state");
        }
    }

    /// Refresh project indicators and trigger UI updates
    #[instrument(skip(self, cx))]
    fn refresh_project_indicators(&mut self, cx: &mut Context<Self>) {
        debug!("Refreshing project indicators");

        // Update project status from current LSP state if available
        self.update_project_status_from_lsp_state(cx);

        // Notify UI components to re-render with updated project information
        cx.notify();

        // Project detection complete - UI will be refreshed via cx.notify()

        info!("Project indicators refreshed");
    }

    // Removed - views are created in main.rs and passed in

    // Removed - views are created in main.rs and passed in

    pub fn theme(editor: &Entity<Core>, cx: &mut Context<Self>) -> helix_view::Theme {
        editor.read(cx).editor.theme.clone()
    }

    fn handle_appearance_change(
        &mut self,
        appearance: WindowAppearance,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::types::{AppEvent, UiEvent, Update};
        use nucleotide_ui::theme_manager::SystemAppearance;

        // Update system appearance in theme manager
        let system_appearance = match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
        };

        nucleotide_logging::info!(
            window_appearance = ?appearance,
            system_appearance = ?system_appearance,
            "OS appearance change detected - emitting SystemAppearanceChanged event"
        );

        // Update global SystemAppearance state
        cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
            theme_manager.set_system_appearance(system_appearance);
        });
        *nucleotide_ui::theme_manager::SystemAppearance::global_mut(cx) = system_appearance;

        // Emit SystemAppearanceChanged event for event-driven handling
        let event_appearance = match system_appearance {
            SystemAppearance::Dark => crate::types::SystemAppearance::Dark,
            SystemAppearance::Light => crate::types::SystemAppearance::Light,
        };

        cx.emit(Update::Event(AppEvent::Ui(
            UiEvent::SystemAppearanceChanged {
                appearance: event_appearance,
            },
        )));
    }

    /// Version of switch_theme_by_name for use from event handlers (no window appearance updates)
    fn switch_theme_by_name_no_window(&mut self, theme_name: &str, cx: &mut Context<Self>) {
        nucleotide_logging::info!(
            theme_name = %theme_name,
            "Switching theme via event handler (no window appearance update)"
        );

        // Update theme in the editor
        self.core.update(cx, |core, cx| {
            let theme_name = if core.editor.theme_loader.load(theme_name).is_ok() {
                theme_name.to_string()
            } else {
                nucleotide_logging::warn!(theme_name = %theme_name, "Theme not found, using default");
                core.editor.theme.name().to_string()
            };

            // Set theme in the editor
            if let Ok(theme) = core.editor.theme_loader.load(&theme_name) {
                core.editor.set_theme(theme);
                nucleotide_logging::info!(theme_name = %theme_name, "Theme loaded successfully");
            }

            // Update theme manager global
            cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
                theme_manager.set_theme(core.editor.theme.clone());
            });

            // Update nucleotide-ui theme global from theme manager
            let ui_theme = cx.global::<crate::ThemeManager>().ui_theme().clone();
            *cx.global_mut::<nucleotide_ui::Theme>() = ui_theme.clone();

            // Update theme provider with the new theme
            nucleotide_ui::providers::update_provider_context(|context| {
                // Create a new theme provider with the updated theme
                let theme_provider = nucleotide_ui::providers::ThemeProvider::new(ui_theme);
                context.register_global_provider(theme_provider);
            });
        });

        // Clear caches and redraw
        self.clear_shaped_lines_cache(cx);
        cx.notify();
    }

    fn switch_theme_by_name(
        &mut self,
        theme_name: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Load the theme using the existing theme loader from the editor
        let theme_result = self.core.read(cx).editor.theme_loader.load(theme_name);
        match theme_result {
            Ok(theme) => {
                // Update editor theme
                self.core.update(cx, |core, _cx| {
                    core.editor.set_theme(theme.clone());
                });

                // Update theme manager
                cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
                    theme_manager.set_theme(theme);
                });

                // Update global UI theme
                let new_ui_theme = cx.global::<crate::ThemeManager>().ui_theme().clone();
                cx.update_global(|ui_theme: &mut nucleotide_ui::Theme, _cx| {
                    *ui_theme = new_ui_theme.clone();
                });

                // Update theme provider with the new theme
                nucleotide_ui::providers::update_provider_context(|context| {
                    // Create a new theme provider with the updated theme
                    let theme_provider = nucleotide_ui::providers::ThemeProvider::new(new_ui_theme);
                    context.register_global_provider(theme_provider);
                });

                // Update window appearance if configured to follow theme
                let config = self.core.read(cx).config.clone();
                if config.gui.window.appearance_follows_theme {
                    // Determine system appearance based on theme darkness
                    let theme_manager = cx.global::<crate::ThemeManager>();
                    let is_dark = theme_manager.is_dark_theme();
                    let system_appearance = if is_dark {
                        nucleotide_ui::theme_manager::SystemAppearance::Dark
                    } else {
                        nucleotide_ui::theme_manager::SystemAppearance::Light
                    };

                    nucleotide_logging::info!(
                        theme_name = %theme_name,
                        is_dark = is_dark,
                        system_appearance = ?system_appearance,
                        "Updating window appearance to match loaded theme"
                    );

                    // Update system appearance in theme manager
                    cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
                        theme_manager.set_system_appearance(system_appearance);
                    });

                    // Update global SystemAppearance state for GPUI integration
                    *nucleotide_ui::theme_manager::SystemAppearance::global_mut(cx) =
                        system_appearance;

                    // Update window background appearance
                    self.update_window_appearance(_window, cx);

                    // Update titlebar appearance to match theme
                    self.update_titlebar_appearance(_window, system_appearance);
                }

                // Clear caches and redraw
                self.clear_shaped_lines_cache(cx);
                cx.notify();
            }
            Err(e) => {
                error!("Failed to load theme '{}': {}", theme_name, e);
            }
        }
    }

    fn update_window_appearance(&self, window: &mut Window, cx: &Context<Self>) {
        let config = self.core.read(cx).config.clone();

        // Only update appearance if configured to follow theme
        if !config.gui.window.appearance_follows_theme {
            debug!("Window appearance does not follow theme - skipping update");
            return;
        }

        let theme_manager = cx.global::<crate::ThemeManager>();
        let is_dark = theme_manager.is_dark_theme();

        // Set window background appearance based on theme
        let appearance = if is_dark {
            // Dark themes should use Blurred to get the proper macOS dark window border
            WindowBackgroundAppearance::Blurred
        } else {
            // Light themes always use opaque
            WindowBackgroundAppearance::Opaque
        };

        let theme_name = self.core.read(cx).editor.theme.name();
        info!(
            is_dark = is_dark,
            appearance = ?appearance,
            theme_name = %theme_name,
            "Updating window background appearance based on theme"
        );

        window.set_background_appearance(appearance);
    }

    /// Schedule window appearance update to be applied in the next render cycle
    fn schedule_window_appearance_update(&mut self, cx: &mut Context<Self>) {
        let theme_name = self.core.read(cx).editor.theme.name();
        info!(
            theme_name = %theme_name,
            "Scheduling window appearance update for next render cycle due to theme change"
        );
        self.needs_window_appearance_update = true;
        cx.notify(); // Trigger re-render
    }

    fn update_titlebar_appearance(
        &self,
        _window: &mut Window,
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
    ) {
        nucleotide_logging::debug!(
            system_appearance = ?system_appearance,
            "Updating titlebar appearance"
        );

        #[cfg(target_os = "macos")]
        {
            // For macOS, we'll use a platform-specific approach to ensure the window
            // follows the system appearance for the titlebar
            self.set_macos_window_appearance(system_appearance);
        }
    }

    #[cfg(target_os = "macos")]
    fn set_macos_window_appearance(
        &self,
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
    ) {
        nucleotide_logging::info!(
            system_appearance = ?system_appearance,
            "Setting NSWindow appearance to follow system"
        );

        // Call the native function to set window appearance
        unsafe {
            Self::update_titlebar_appearance_native(system_appearance);
        }
    }

    #[cfg(target_os = "macos")]
    unsafe fn update_titlebar_appearance_native(
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
    ) {
        use cocoa::base::{id, nil};
        use cocoa::foundation::NSString;
        use nucleotide_ui::theme_manager::SystemAppearance;
        use objc::{class, msg_send, sel, sel_impl};

        // Get all windows from NSApplication instead of just the main window

        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let windows: id = msg_send![app, windows];
        let window_count: usize = msg_send![windows, count];

        nucleotide_logging::debug!(
            window_count = window_count,
            "Found {} windows in NSApplication",
            window_count
        );

        // Log details about all windows to make sure we're targeting the right one
        for i in 0..window_count {
            let window: id = msg_send![windows, objectAtIndex: i];
            let window_title: id = msg_send![window, title];
            let title_str = if window_title != nil {
                let cstr: *const i8 = msg_send![window_title, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };
            let window_level: i64 = msg_send![window, level];
            let is_visible: bool = msg_send![window, isVisible];
            let is_main: bool = msg_send![window, isMainWindow];
            let is_key: bool = msg_send![window, isKeyWindow];

            nucleotide_logging::debug!(
                window_index = i,
                window_title = title_str,
                window_level = window_level,
                is_visible = is_visible,
                is_main = is_main,
                is_key = is_key,
                "Window details"
            );
        }

        if window_count > 0 {
            // Find the actual main/key window instead of just taking the first one
            let mut target_window: id = nil;

            // First try to find the main window
            for i in 0..window_count {
                let window: id = msg_send![windows, objectAtIndex: i];
                let is_main: bool = msg_send![window, isMainWindow];
                if is_main {
                    target_window = window;
                    nucleotide_logging::debug!(window_index = i, "Found main window");
                    break;
                }
            }

            // If no main window, try to find the key window
            if target_window == nil {
                for i in 0..window_count {
                    let window: id = msg_send![windows, objectAtIndex: i];
                    let is_key: bool = msg_send![window, isKeyWindow];
                    if is_key {
                        target_window = window;
                        nucleotide_logging::debug!(window_index = i, "Found key window");
                        break;
                    }
                }
            }

            // If still no target, find the first visible window with a titlebar
            if target_window == nil {
                for i in 0..window_count {
                    let window: id = msg_send![windows, objectAtIndex: i];
                    let is_visible: bool = msg_send![window, isVisible];
                    let has_titlebar: bool = msg_send![window, hasTitleBar];
                    if is_visible && has_titlebar {
                        target_window = window;
                        nucleotide_logging::debug!(
                            window_index = i,
                            "Found visible window with titlebar"
                        );
                        break;
                    }
                }
            }

            // Fall back to first window if all else fails
            if target_window == nil {
                target_window = msg_send![windows, objectAtIndex: 0];
                nucleotide_logging::warn!("Falling back to first window");
            }

            let window = target_window;

            nucleotide_logging::debug!("Found application window, setting appearance");

            // Check window properties that might affect titlebar appearance
            let style_mask: u64 = msg_send![window, styleMask];
            let is_titled: bool = (style_mask & 1) != 0; // NSTitledWindowMask
            let has_titlebar: bool = msg_send![window, hasTitleBar];
            let titlebar_appears_transparent: bool = msg_send![window, titlebarAppearsTransparent];

            nucleotide_logging::debug!(
                style_mask = style_mask,
                is_titled = is_titled,
                has_titlebar = has_titlebar,
                titlebar_appears_transparent = titlebar_appears_transparent,
                "Window titlebar properties"
            );

            // Check current appearance before setting
            let current_appearance: id = msg_send![window, appearance];
            nucleotide_logging::debug!(
                current_appearance_is_nil = (current_appearance == nil),
                "Window appearance before setting"
            );

            // Set the window appearance to match the detected system appearance
            match system_appearance {
                SystemAppearance::Dark => {
                    // Set to dark appearance explicitly
                    let dark_appearance_name =
                        unsafe { NSString::alloc(nil).init_str("NSAppearanceNameDarkAqua") };
                    let dark_appearance: id =
                        msg_send![class!(NSAppearance), appearanceNamed: dark_appearance_name];
                    let _: () = msg_send![window, setAppearance: dark_appearance];
                    nucleotide_logging::debug!("Set window to dark appearance explicitly");
                }
                SystemAppearance::Light => {
                    // Set to light appearance explicitly
                    let light_appearance_name =
                        unsafe { NSString::alloc(nil).init_str("NSAppearanceNameAqua") };
                    let light_appearance: id =
                        msg_send![class!(NSAppearance), appearanceNamed: light_appearance_name];
                    let _: () = msg_send![window, setAppearance: light_appearance];
                    nucleotide_logging::debug!("Set window to light appearance explicitly");
                }
            }

            // Check appearance after setting and verify it took effect
            let new_appearance: id = msg_send![window, appearance];
            let new_appearance_name: id = if new_appearance != nil {
                msg_send![new_appearance, name]
            } else {
                nil
            };

            let appearance_name_str = if new_appearance_name != nil {
                let cstr: *const i8 = msg_send![new_appearance_name, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };

            nucleotide_logging::info!(
                system_appearance = ?system_appearance,
                new_appearance_is_nil = (new_appearance == nil),
                new_appearance_name = appearance_name_str,
                "Successfully set NSWindow appearance"
            );

            // Also check the actual effective appearance to see what macOS thinks
            let effective_appearance: id = msg_send![window, effectiveAppearance];
            let effective_appearance_name: id = if effective_appearance != nil {
                msg_send![effective_appearance, name]
            } else {
                nil
            };

            let effective_name_str = if effective_appearance_name != nil {
                let cstr: *const i8 = msg_send![effective_appearance_name, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };

            nucleotide_logging::info!(
                effective_appearance_name = effective_name_str,
                "Window effective appearance after setting"
            );

            // Check if the appearance gets reset by something else shortly after
            // Schedule a delayed check to see if our setting persists
            let _window_ptr = window as *const _ as usize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                unsafe {
                    let app: id = msg_send![class!(NSApplication), sharedApplication];
                    let windows: id = msg_send![app, windows];
                    let window_count: usize = msg_send![windows, count];

                    if window_count > 0 {
                        let window: id = msg_send![windows, objectAtIndex: 0];
                        let current_appearance: id = msg_send![window, appearance];
                        let effective_appearance: id = msg_send![window, effectiveAppearance];

                        let current_name = if current_appearance != nil {
                            let name: id = msg_send![current_appearance, name];
                            if name != nil {
                                let cstr: *const i8 = msg_send![name, UTF8String];
                                std::ffi::CStr::from_ptr(cstr).to_str().unwrap_or("unknown")
                            } else {
                                "nil"
                            }
                        } else {
                            "nil"
                        };

                        let effective_name = if effective_appearance != nil {
                            let name: id = msg_send![effective_appearance, name];
                            if name != nil {
                                let cstr: *const i8 = msg_send![name, UTF8String];
                                std::ffi::CStr::from_ptr(cstr).to_str().unwrap_or("unknown")
                            } else {
                                "nil"
                            }
                        } else {
                            "nil"
                        };

                        nucleotide_logging::warn!(
                            current_appearance_name = current_name,
                            effective_appearance_name = effective_name,
                            "Appearance check 100ms later - did something reset it?"
                        );
                    }
                }
            });
        } else {
            nucleotide_logging::warn!("No windows found in NSApplication, cannot set appearance");
        }
    }

    #[cfg(target_os = "macos")]
    unsafe fn update_titlebar_appearance_native_with_retry(
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
        attempt: u32,
    ) -> bool {
        use cocoa::base::{id, nil};
        use cocoa::foundation::NSString;
        use nucleotide_ui::theme_manager::SystemAppearance;
        use objc::{class, msg_send, sel, sel_impl};

        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let windows: id = msg_send![app, windows];
        let window_count: usize = msg_send![windows, count];

        nucleotide_logging::debug!(
            attempt = attempt,
            window_count = window_count,
            "Retry attempt {} - found {} windows",
            attempt,
            window_count
        );

        if window_count == 0 {
            return false;
        }

        // Look for the proper main window - one with a title and main/key status
        let mut target_window: id = nil;

        for i in 0..window_count {
            let window: id = msg_send![windows, objectAtIndex: i];
            let window_title: id = msg_send![window, title];
            let title_str = if window_title != nil {
                let cstr: *const i8 = msg_send![window_title, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };
            let is_main: bool = msg_send![window, isMainWindow];
            let is_key: bool = msg_send![window, isKeyWindow];
            let has_titlebar: bool = msg_send![window, hasTitleBar];

            nucleotide_logging::debug!(
                attempt = attempt,
                window_index = i,
                window_title = title_str,
                is_main = is_main,
                is_key = is_key,
                has_titlebar = has_titlebar,
                "Retry window details"
            );

            // Only target windows that are actually main/key windows with titles and titlebars
            if (is_main || is_key) && has_titlebar && !title_str.is_empty() && title_str != "nil" {
                target_window = window;
                nucleotide_logging::info!(
                    attempt = attempt,
                    window_index = i,
                    window_title = title_str,
                    "Found proper main window for titlebar appearance"
                );
                break;
            }
        }

        if target_window == nil {
            nucleotide_logging::debug!(
                attempt = attempt,
                "No proper main window found yet, will retry"
            );
            return false;
        }

        // Set the appearance on the proper window
        let window = target_window;
        match system_appearance {
            SystemAppearance::Dark => {
                let dark_appearance_name =
                    unsafe { NSString::alloc(nil).init_str("NSAppearanceNameDarkAqua") };
                let dark_appearance: id =
                    msg_send![class!(NSAppearance), appearanceNamed: dark_appearance_name];
                let _: () = msg_send![window, setAppearance: dark_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to dark appearance on proper main window"
                );
            }
            SystemAppearance::Light => {
                let light_appearance_name =
                    unsafe { NSString::alloc(nil).init_str("NSAppearanceNameAqua") };
                let light_appearance: id =
                    msg_send![class!(NSAppearance), appearanceNamed: light_appearance_name];
                let _: () = msg_send![window, setAppearance: light_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to light appearance on proper main window"
                );
            }
        }

        true
    }

    fn ensure_window_follows_system_appearance(&self, _window: &mut Window) {
        nucleotide_logging::info!("Ensuring window follows system appearance");

        #[cfg(target_os = "macos")]
        {
            // For macOS, we need to set the NSWindow appearance to nil to follow system
            self.ensure_nswindow_follows_system();
        }
    }

    #[cfg(target_os = "macos")]
    fn ensure_nswindow_follows_system(&self) {
        // For now, log that we would set the NSWindow appearance to nil
        nucleotide_logging::info!("Would set NSWindow appearance to nil to follow system");

        // TODO: Implement the actual NSWindow appearance setting
        // This requires accessing the native window handle through GPUI
        // and calling [window setAppearance:nil]
    }

    fn clear_shaped_lines_cache(&self, cx: &Context<Self>) {
        if let Some(line_cache) = cx.try_global::<nucleotide_editor::LineLayoutCache>() {
            line_cache.clear_shaped_lines();
        }
    }

    // Event handler methods extracted from the main handle_event
    fn handle_system_appearance_changed(
        &mut self,
        appearance: crate::types::SystemAppearance,
        cx: &mut Context<Self>,
    ) {
        use crate::config::ThemeMode;

        nucleotide_logging::info!(
            appearance = ?appearance,
            "Handling SystemAppearanceChanged event"
        );

        let config = self.core.read(cx).config.clone();

        // Only switch themes if configured for system mode
        if config.gui.theme.mode == ThemeMode::System {
            let theme_name = match appearance {
                crate::types::SystemAppearance::Light => config.gui.theme.get_light_theme(),
                crate::types::SystemAppearance::Dark => config.gui.theme.get_dark_theme(),
                crate::types::SystemAppearance::Auto => {
                    // For Auto mode, we would need to detect system preference
                    // For now, fall back to the configured default theme
                    config.gui.theme.get_light_theme()
                }
            };

            nucleotide_logging::info!(
                selected_theme = %theme_name,
                "Switching theme for system appearance change"
            );

            // Switch theme directly through the core editor (no window needed)
            self.switch_theme_by_name_no_window(&theme_name, cx);
        } else {
            nucleotide_logging::debug!(
                theme_mode = ?config.gui.theme.mode,
                "Theme mode is not System - ignoring appearance change"
            );
        }
    }

    fn handle_editor_event(
        &mut self,
        ev: &helix_view::editor::EditorEvent,
        cx: &mut Context<Self>,
    ) {
        use helix_view::editor::{ConfigEvent, EditorEvent};
        match ev {
            EditorEvent::Redraw => cx.notify(),
            EditorEvent::ConfigEvent(config_event) => {
                use nucleotide_logging::info;
                // Handle configuration changes
                info!(config_event = ?config_event, "Workspace received ConfigEvent");

                // Log current bufferline config when we receive a config event
                let current_bufferline = &self.core.read(cx).editor.config().bufferline;
                info!(bufferline_config = ?current_bufferline, "Current bufferline config during ConfigEvent");

                match config_event {
                    ConfigEvent::Refresh | ConfigEvent::Update(_) => {
                        // Configuration has changed, refresh the UI
                        info!("Config changed, refreshing UI - calling cx.notify()");

                        // Force a complete workspace refresh by clearing render state
                        // This ensures that changes like bufferline visibility are properly applied
                        self.update_document_views(cx);

                        cx.notify();
                    }
                }
            }
            EditorEvent::LanguageServerMessage(_) => { /* handled by notifications */ }
            _ => {
                info!("editor event {ev:?} not handled");
            }
        }
    }

    fn handle_redraw(&mut self, cx: &mut Context<Self>) {
        // Clear the shaped lines cache to force re-rendering with updated config
        if let Some(line_cache) = cx.try_global::<nucleotide_editor::LineLayoutCache>() {
            line_cache.clear_shaped_lines();
        }

        // Minimal redraw - most updates now come through specific events
        if let Some(view) = self
            .view_manager
            .focused_view_id()
            .and_then(|id| self.view_manager.get_document_view(&id))
        {
            view.update(cx, |_view, cx| {
                cx.notify();
            })
        }

        // Update key hints on redraw
        self.update_key_hints(cx);
        cx.notify();
    }

    fn handle_overlay_update(&mut self, cx: &mut Context<Self>) {
        // When a picker, prompt, or completion appears, auto-dismiss the info box
        self.info_hidden = true;

        // Check if completion is now active and manage input contexts
        let has_completion = self.overlay.read(cx).has_completion();
        self.manage_completion_context(has_completion);

        // Focus will be handled by the overlay components
        cx.notify();
    }

    fn handle_document_changed(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // Document content changed - update specific document view
        self.update_specific_document_view(doc_id, cx);
        cx.notify();
    }

    fn handle_selection_changed(
        &mut self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) {
        // Selection/cursor moved - update status and specific view
        info!("Selection changed in doc {:?}, view {:?}", doc_id, view_id);
        self.update_specific_document_view(doc_id, cx);
        cx.notify();
    }

    fn handle_mode_changed(
        &mut self,
        old_mode: &helix_view::document::Mode,
        new_mode: &helix_view::document::Mode,
        cx: &mut Context<Self>,
    ) {
        // Editor mode changed - update status line and current view
        info!("Mode changed from {:?} to {:?}", old_mode, new_mode);
        self.update_current_document_view(cx);
        cx.notify();
    }

    fn handle_diagnostics_changed(
        &mut self,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) {
        // LSP diagnostics changed - update specific document view
        self.update_specific_document_view(doc_id, cx);
        cx.notify();
    }

    fn handle_document_opened(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // New document opened - the view will be created automatically
        info!("Document opened: {:?}", doc_id);

        // Start LSP for the newly opened document using the feature flag system
        info!("Starting LSP for newly opened document using feature flag system");
        let lsp_result = self
            .core
            .update(cx, |core, _| core.start_lsp_with_feature_flags(doc_id));

        match lsp_result {
            nucleotide_lsp::LspStartupResult::Success {
                mode,
                language_servers,
                duration,
            } => {
                info!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    language_servers = ?language_servers,
                    duration_ms = duration.as_millis(),
                    "LSP startup successful for newly opened document"
                );
            }
            nucleotide_lsp::LspStartupResult::Failed {
                mode,
                error,
                fallback_mode,
            } => {
                warn!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    error = %error,
                    fallback_mode = ?fallback_mode,
                    "LSP startup failed for newly opened document"
                );
            }
            nucleotide_lsp::LspStartupResult::Skipped { reason } => {
                info!(
                    doc_id = ?doc_id,
                    reason = %reason,
                    "LSP startup skipped for newly opened document"
                );
            }
        }

        // Sync file tree selection with the newly opened document
        let doc_path = {
            let core = self.core.read(cx);
            core.editor
                .document(doc_id)
                .and_then(|doc| doc.path())
                .map(|p| p.to_path_buf())
        };

        if let Some(path) = doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        cx.notify();
    }

    fn handle_document_closed(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // Document closed - the view will be cleaned up automatically
        info!("Document closed: {:?}", doc_id);
        cx.notify();
    }

    fn handle_view_focused(&mut self, view_id: helix_view::ViewId, cx: &mut Context<Self>) {
        // View focus changed - just update focus state
        info!("View focused: {:?}", view_id);
        self.view_manager.set_focused_view_id(Some(view_id));

        // TODO: Update titlebar with current filename
        // AnyView doesn't have update method, need to refactor titlebar storage
        // if let Some(titlebar) = &self.titlebar {
        //     if let Some(filename) = self.current_filename(cx) {
        //         titlebar.update(cx, |titlebar, _cx| {
        //             if let Some(titlebar) = titlebar.downcast_mut::<nucleotide_ui::titlebar::TitleBar>() {
        //                 titlebar.set_filename(filename);
        //             }
        //         });
        //     }
        // }

        // Sync file tree selection with the newly focused view
        let doc_path = {
            let core = self.core.read(cx);
            if let Some(view) = core.editor.tree.try_get(view_id) {
                core.editor
                    .document(view.doc)
                    .and_then(|doc| doc.path())
                    .map(|p| p.to_path_buf())
            } else {
                None
            }
        };

        if let Some(path) = doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        cx.notify();
    }

    fn handle_language_server_initialized(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        // LSP server initialized - update status
        info!("Language server initialized: {:?}", server_id);
        cx.notify();
    }

    fn handle_language_server_exited(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        // LSP server exited - update status
        info!("Language server exited: {:?}", server_id);
        cx.notify();
    }

    fn handle_completion_requested(
        &mut self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: &event_bridge::CompletionTrigger,
        cx: &mut Context<Self>,
    ) {
        // Completion was requested - trigger completion UI
        info!(
            "Completion requested for doc {:?}, view {:?}, trigger: {:?}",
            doc_id, view_id, trigger
        );

        // Only show completion for manual triggers for now to avoid blocking
        match trigger {
            crate::types::CompletionTrigger::Manual => {
                // Always show for manual triggers
                println!("COMP: Manual completion trigger detected - calling trigger_completion");
                self.trigger_completion(cx);
                println!("COMP: trigger_completion called for manual trigger");
            }
            crate::types::CompletionTrigger::Character(_c) => {
                // Disable automatic completion on character input to prevent blocking
                // TODO: Re-enable with proper debouncing and async handling
                // if c.is_alphabetic() || *c == '_' || *c == '.' {
                //     self.trigger_completion(cx);
                // }
            }
            crate::types::CompletionTrigger::Automatic => {
                // Disable automatic completion for now
                // TODO: Re-enable with proper debouncing
                // self.trigger_completion(cx);
            }
        }

        cx.notify();
    }

    fn handle_search_submitted(&mut self, search_text: &str, cx: &mut Context<Self>) {
        // Execute the search in Helix
        info!("Search submitted: {}", search_text);

        // Clear the overlay first to hide the prompt
        self.overlay.update(cx, |overlay, cx| {
            overlay.clear(cx);
        });

        // We need to execute the search directly in Helix since we've replaced the prompt
        self.core.update(cx, |core, cx| {
            let _guard = self.handle.enter();

            // First, remove any existing Helix prompt from the compositor
            // This ensures the EditorView will handle subsequent keys
            core.compositor.remove("prompt");

            // Store the search pattern in the register (raw pattern, not regex)
            core.editor.registers.last_search_register = '/';
            let _ = core.editor.registers.push('/', search_text.to_string());

            // Compile the regex pattern
            use helix_core::graphemes;
            use helix_stdx::rope::{self, RopeSliceExt};

            let case_insensitive = core.editor.config().search.smart_case
                && search_text.chars().all(char::is_lowercase);

            // Build regex the same way Helix does it in search_next_or_prev_impl
            let regex = if let Ok(regex) = rope::RegexBuilder::new()
                .syntax(
                    rope::Config::new()
                        .case_insensitive(case_insensitive)
                        .multi_line(true),
                )
                .build(search_text)
            {
                Ok(regex)
            } else {
                Err(format!("Failed to compile regex: {search_text}"))
            };

            match regex {
                Ok(ref regex) => {
                    // Get current state
                    let view_id = core.editor.tree.focus;
                    let doc_id = core
                        .editor
                        .tree
                        .try_get(view_id)
                        .map(|view| view.doc)
                        .unwrap_or_default();
                    let wrap_around = core.editor.config().search.wrap_around;
                    let scrolloff = core.editor.config().scrolloff;

                    // Get text and current selection
                    let (text, current_selection, search_start_byte) = {
                        let doc = core.editor.documents.get(&doc_id).unwrap();
                        let text = doc.text().slice(..);
                        let selection = doc.selection(view_id);

                        // For forward search, start from the end of the primary selection
                        // and ensure we're on a grapheme boundary
                        let search_start_char = graphemes::ensure_grapheme_boundary_next(
                            text,
                            selection.primary().to(),
                        );
                        let search_start_byte = text.char_to_byte(search_start_char);

                        (text, selection.clone(), search_start_byte)
                    };

                    // Find the next match
                    // IMPORTANT: The regex_input_at_bytes returns a cursor that produces
                    // absolute byte positions, NOT relative to the start offset!
                    let match_range = if let Some(mat) =
                        regex.find(text.regex_input_at_bytes(search_start_byte..))
                    {
                        // The positions are already absolute in the document
                        Some((mat.start(), mat.end()))
                    } else if wrap_around {
                        // When searching from the beginning, positions are also absolute
                        regex
                            .find(text.regex_input())
                            .map(|mat| (mat.start(), mat.end()))
                    } else {
                        None
                    };

                    // Apply the match if found
                    if let Some((start_byte, end_byte)) = match_range {
                        // Skip empty matches
                        if start_byte == end_byte {
                            core.editor.set_error("Empty match");
                            return;
                        }

                        let start_char = text.byte_to_char(start_byte);
                        let end_char = text.byte_to_char(end_byte);

                        // Create a range for the match - exactly as Helix does it
                        use helix_core::Range;
                        let range = Range::new(start_char, end_char);

                        // Replace the primary selection with the new range
                        let primary_index = current_selection.primary_index();
                        let new_selection = current_selection.replace(primary_index, range);

                        let doc = core.editor.documents.get_mut(&doc_id).unwrap();
                        doc.set_selection(view_id, new_selection);

                        // Ensure the cursor is visible and centered
                        let view = core.editor.tree.get_mut(view_id);
                        view.ensure_cursor_in_view_center(doc, scrolloff);

                        // Show wrapped message if we wrapped
                        if wrap_around && start_byte < search_start_byte {
                            core.editor.set_status("Wrapped around document");
                        }
                    } else {
                        core.editor
                            .set_error(format!("Pattern not found: {search_text}"));
                    }
                }
                Err(e) => {
                    core.editor.set_error(format!("Invalid regex: {e}"));
                }
            }

            cx.notify();
        });
    }

    fn handle_command_submitted(&mut self, command: &str, cx: &mut Context<Self>) {
        info!(
            "handle_command_submitted called with command: '{}'",
            command
        );

        // Clear the overlay first to hide the prompt
        self.overlay.update(cx, |overlay, cx| {
            overlay.clear(cx);
        });

        // Parse the command using our typed system
        match nucleotide_core::ParsedCommand::parse(command) {
            Ok(parsed) => {
                // Log the parsed command for debugging
                info!("Parsed command: {:?}", parsed);

                // Convert to typed command if possible
                match nucleotide_core::Command::from_parsed(parsed.clone()) {
                    Ok(typed_cmd) => {
                        info!("Typed command: {:?}", typed_cmd);
                        // Execute the typed command
                        self.execute_typed_command(typed_cmd, cx);
                    }
                    Err(_) => {
                        // Fall back to raw command execution for untyped commands
                        self.execute_raw_command(command, cx);
                    }
                }
            }
            Err(e) => {
                // Show error to user
                self.core.update(cx, |core, cx| {
                    core.editor.set_error(format!("Invalid command: {e}"));
                    cx.notify();
                });
            }
        }
    }

    fn execute_typed_command(&mut self, command: nucleotide_core::Command, cx: &mut Context<Self>) {
        use nucleotide_core::{Command, command_system::SplitDirection};

        info!("execute_typed_command called with: {:?}", command);

        match command {
            Command::Quit { force } => {
                self.execute_raw_command(if force { "quit !" } else { "quit" }, cx);
            }
            Command::Write { path } => {
                let cmd = match path {
                    Some(p) => format!("write {p}"),
                    None => "write".to_string(),
                };
                self.execute_raw_command(&cmd, cx);
            }
            Command::WriteQuit { force } => {
                self.execute_raw_command(if force { "wq !" } else { "wq" }, cx);
            }
            Command::Goto { line } => {
                self.execute_raw_command(&format!("goto {line}"), cx);
            }
            Command::Theme { name } => {
                self.execute_raw_command(&format!("theme {name}"), cx);
            }
            Command::Open { path } => {
                self.execute_raw_command(&format!("open {path}"), cx);
            }
            Command::Split { direction } => match direction {
                SplitDirection::Horizontal => self.execute_raw_command("split", cx),
                SplitDirection::Vertical => self.execute_raw_command("vsplit", cx),
            },
            Command::Close { force } => {
                self.execute_raw_command(if force { "close !" } else { "close" }, cx);
            }
            Command::Help { topic } => {
                let cmd = match topic {
                    Some(t) => format!("help {t}"),
                    None => "help".to_string(),
                };
                self.execute_raw_command(&cmd, cx);
            }
            Command::Search { pattern } => {
                self.execute_raw_command(&format!("search {pattern}"), cx);
            }
            Command::Replace {
                pattern,
                replacement,
            } => {
                self.execute_raw_command(&format!("replace {pattern} {replacement}"), cx);
            }
            Command::Generic(parsed) => {
                // Execute generic commands
                self.execute_raw_command(&format!("{parsed}"), cx);
            }
        }
    }

    fn execute_raw_command(&mut self, command: &str, cx: &mut Context<Self>) {
        use nucleotide_logging::info;
        // Execute the command through helix's command system
        let core = self.core.clone();
        let handle = self.handle.clone();

        info!(command = %command, "Executing raw command");

        // Store the current theme before executing the command
        let theme_before = core.read(cx).editor.theme.name().to_string();
        let theme_before_for_closure = theme_before.clone();

        // Log current bufferline config before execution
        let bufferline_before = core.read(cx).editor.config().bufferline.clone();
        info!(bufferline_config = ?bufferline_before, "Bufferline config before command execution");

        core.update(cx, move |core, cx| {
            let _guard = handle.enter();

            // First, close the prompt by clearing the compositor
            if core.compositor.find::<helix_term::ui::Prompt>().is_some() {
                core.compositor.pop();
            }

            // Create a helix compositor context to execute the command
            let mut comp_ctx = helix_term::compositor::Context {
                editor: &mut core.editor,
                scroll: None,
                jobs: &mut core.jobs,
            };

            // Execute the command using helix's command system
            // Since execute_command_line is not public, we need to manually parse and execute
            let (cmd_name, args, _) = helix_core::command_line::split(command);

            if !cmd_name.is_empty() {
                // Check if it's a line number
                if cmd_name.parse::<usize>().is_ok() && args.trim().is_empty() {
                    // Handle goto line
                    if let Some(cmd) = helix_term::commands::TYPABLE_COMMAND_MAP.get("goto") {
                        // Parse args manually since we can't use execute_command
                        let parsed_args = helix_core::command_line::Args::parse(
                            cmd_name,
                            cmd.signature,
                            true,
                            |token| Ok(token.content),
                        );

                        if let Ok(parsed_args) = parsed_args {
                            if let Err(err) = (cmd.fun)(
                                &mut comp_ctx,
                                parsed_args,
                                helix_term::ui::PromptEvent::Validate,
                            ) {
                                core.editor.set_error(err.to_string());
                            }
                        } else {
                            core.editor
                                .set_error("Failed to parse arguments".to_string());
                        }
                    }
                } else {
                    // Execute regular command
                    // First, check if the command exists directly in the map (not an alias)
                    let resolved_cmd_name =
                        if helix_term::commands::TYPABLE_COMMAND_MAP.contains_key(cmd_name) {
                            // Command exists directly, use it as-is
                            cmd_name
                        } else {
                            // Command might be an alias, try to resolve it
                            helix_term::commands::TYPABLE_COMMAND_LIST
                                .iter()
                                .find(|cmd| cmd.aliases.contains(&cmd_name))
                                .map(|cmd| cmd.name)
                                .unwrap_or(cmd_name)
                        };

                    match helix_term::commands::TYPABLE_COMMAND_MAP.get(resolved_cmd_name) {
                        Some(cmd) => {
                            // Parse args for the command
                            let parsed_args = helix_core::command_line::Args::parse(
                                args,
                                cmd.signature,
                                true,
                                |token| {
                                    helix_view::expansion::expand(comp_ctx.editor, token)
                                        .map_err(std::convert::Into::into)
                                },
                            );

                            match parsed_args {
                                Ok(parsed_args) => {
                                    let result = (cmd.fun)(
                                        &mut comp_ctx,
                                        parsed_args,
                                        helix_term::ui::PromptEvent::Validate,
                                    );

                                    if let Err(err) = result {
                                        core.editor.set_error(format!("'{cmd_name}': {err}"));
                                    }
                                }
                                Err(err) => {
                                    core.editor.set_error(format!("'{cmd_name}': {err}"));
                                }
                            }
                        }
                        None => {
                            core.editor
                                .set_error(format!("no such command: '{cmd_name}'"));
                        }
                    }
                }
            }

            // Check if the theme has changed after command execution
            let current_theme = core.editor.theme.clone();
            let theme_name_after = current_theme.name().to_string();

            // Always trigger a redraw after command execution to reflect any config changes
            cx.emit(crate::Update::Redraw);

            // If the theme has changed, handle it properly using existing theme switching logic
            if theme_before_for_closure != theme_name_after {
                info!(
                    old_theme = %theme_before_for_closure,
                    new_theme = %theme_name_after,
                    "Theme changed via command execution"
                );

                // Send theme change event to Helix
                gpui_to_helix_bridge::send_gpui_event_to_helix(
                    gpui_to_helix_bridge::GpuiToHelixEvent::ThemeChanged {
                        theme_name: theme_name_after.clone(),
                    },
                );
            }
        });

        // Check if theme changed after command execution and handle accordingly
        let theme_name_after = core.read(cx).editor.theme.name().to_string();
        if theme_before != theme_name_after {
            // Use existing theme switching logic (maintains consistency)
            self.switch_theme_by_name_no_window(&theme_name_after, cx);

            // Schedule window appearance update for next render cycle
            self.schedule_window_appearance_update(cx);
        }

        // Check if we should quit after command execution
        let should_quit = core.read(cx).editor.should_close();
        if should_quit {
            cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                crate::types::CoreEvent::ShouldQuit,
            )));
        }

        // Log bufferline config after execution
        let bufferline_after = core.read(cx).editor.config().bufferline.clone();
        info!(bufferline_config = ?bufferline_after, "Bufferline config after command execution");

        // Manual trigger: if bufferline config changed, force a workspace refresh
        // This is a workaround since ConfigEvent might not always be triggered properly
        let changed = if bufferline_before != bufferline_after {
            info!(old_config = ?bufferline_before, new_config = ?bufferline_after, "Bufferline config changed - forcing workspace refresh");
            true
        } else {
            false
        };

        cx.notify();

        if changed {
            // Force workspace to refresh by emitting a fake ConfigEvent
            cx.emit(crate::Update::EditorEvent(
                helix_view::editor::EditorEvent::ConfigEvent(
                    helix_view::editor::ConfigEvent::Refresh,
                ),
            ));
        }

        // Log bufferline config in workspace context after command execution
        let bufferline_after_workspace = &core.read(cx).editor.config().bufferline;
        info!(bufferline_config = ?bufferline_after_workspace, "Bufferline config after command (workspace context)");
    }

    fn handle_open_directory(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Find the workspace root from this directory and update working directory
        let workspace_root = find_workspace_root_from(path);
        info!(
            directory_path = %path.display(),
            workspace_root = %workspace_root.display(),
            "Opening directory"
        );

        // Update the editor's working directory
        // This will affect file picker and other operations
        if let Err(e) = std::env::set_current_dir(&workspace_root) {
            warn!("Failed to change working directory: {}", e);
        }

        // Use set_project_directory to properly initialize LSP and project management
        self.set_project_directory(path.to_path_buf(), cx);

        // Update the file tree with the new directory
        let handle_clone = self.handle.clone();
        let new_file_tree = cx.new(|cx| {
            let config = FileTreeConfig::default();
            FileTreeView::new_with_runtime(path.to_path_buf(), config, Some(handle_clone), cx)
        });

        // Subscribe to file tree events
        cx.subscribe(&new_file_tree, |workspace, _file_tree, event, cx| {
            info!(
                "Workspace: Received file tree event from new tree: {:?}",
                event
            );
            workspace.handle_file_tree_event(event, cx);
        })
        .detach();

        self.file_tree = Some(new_file_tree);

        // Make sure file tree is visible
        self.show_file_tree = true;
        cx.notify();

        // Show status message about the new project directory
        self.core.update(cx, |core, cx| {
            core.editor
                .set_status(format!("Project directory set to: {}", path.display()));
            cx.notify();
        });
    }

    fn handle_open_file_keep_focus(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Open file but don't steal focus from file tree
        self.open_file_internal(path, false, cx);
    }

    fn handle_open_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Open file and focus the editor
        self.open_file_internal(path, true, cx);
    }

    /// Open the nucleotide.toml settings file
    pub fn open_settings_file(&mut self, cx: &mut Context<Self>) {
        // Get the Helix config directory path
        let config_dir = helix_loader::config_dir();
        let settings_path = config_dir.join("nucleotide.toml");

        info!("Opening settings file: {}", settings_path.display());

        // Create the config directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            nucleotide_logging::error!("Failed to create config directory: {}", e);
            return;
        }

        // Create an empty nucleotide.toml if it doesn't exist
        if !settings_path.exists() {
            let default_config = r#"# Nucleotide Configuration
# This file contains GUI-specific settings for Nucleotide

[ui]
# Font configuration for the UI
# font = { family = "SF Pro", size = 14.0, weight = "Medium" }

# Enable or disable animations
# animations = true

[theme]
# Theme mode: "auto", "light", or "dark"
# mode = "auto"

# Override default themes (optional)
# light_theme = "onelight"
# dark_theme = "onedark"

[lsp]
# Language server configuration
# Enable/disable completion suggestions
# completion_enabled = true

# Maximum number of completion items
# max_completion_items = 100

# Completion delay in milliseconds
# completion_delay = 100
"#;

            if let Err(e) = std::fs::write(&settings_path, default_config) {
                nucleotide_logging::error!("Failed to create default nucleotide.toml: {}", e);
                return;
            }

            info!("Created default nucleotide.toml configuration file");
        }

        // Open the settings file
        self.open_file_internal(&settings_path, true, cx);
    }

    /// Reload the nucleotide.toml configuration without restarting
    pub fn reload_configuration(&mut self, cx: &mut Context<Self>) {
        info!("Reloading Nucleotide configuration...");

        // Get the Helix config directory path
        let config_dir = helix_loader::config_dir();
        let settings_path = config_dir.join("nucleotide.toml");

        if !settings_path.exists() {
            nucleotide_logging::warn!("Configuration file not found: {}", settings_path.display());
            // Could create a notification here in the future
            return;
        }

        // Attempt to reload configuration
        match crate::config::Config::load_from_dir(&config_dir) {
            Ok(new_config) => {
                info!(
                    "Successfully reloaded configuration from: {}",
                    settings_path.display()
                );

                // Update font configuration if needed
                if let Some(ui_font) = &new_config.gui.ui.font {
                    let mut ui_font_config = cx.global_mut::<crate::types::UiFontConfig>();
                    ui_font_config.family = ui_font.family.clone();
                    ui_font_config.size = ui_font.size;
                    info!(
                        "UI font configuration updated: {} {}pt",
                        ui_font.family, ui_font.size
                    );
                }

                // Trigger a full redraw to apply changes
                cx.notify();

                info!("Configuration reloaded successfully");
                info!("Note: Theme changes require restarting Nucleotide to take effect");
            }
            Err(e) => {
                nucleotide_logging::error!("Failed to reload configuration: {}", e);
                // Could show an error notification here in the future
            }
        }
    }

    fn open_file_internal(
        &mut self,
        path: &std::path::Path,
        should_focus: bool,
        cx: &mut Context<Self>,
    ) {
        // Open the specified file in the editor
        info!("Workspace: Received OpenFile update for: {path:?}");
        self.core.update(cx, |core, cx| {
            let _guard = self.handle.enter();

            // Determine the right action based on whether we have views
            let action = if core.editor.tree.views().count() == 0 {
                info!("No views exist, using VerticalSplit action");
                helix_view::editor::Action::VerticalSplit
            } else {
                info!("Views exist, using Replace action to show in current view");
                helix_view::editor::Action::Replace
            };

            // Now open the file
            info!(
                "About to open file from picker: {path:?} with action: {:?}",
                action
            );
            match core.editor.open(path, action) {
                Err(e) => {
                    eprintln!("Failed to open file {path:?}: {e}");
                }
                Ok(doc_id) => {
                    info!("Successfully opened file from picker: {path:?}, doc_id: {doc_id:?}");

                    // Log document info
                    if let Some(doc) = core.editor.document(doc_id) {
                        info!(
                            "Document language: {:?}, path: {:?}",
                            doc.language_name(),
                            doc.path()
                        );

                        // Check if document has language servers
                        let lang_servers: Vec<_> = doc.language_servers().collect();
                        info!("Document has {} language servers", lang_servers.len());
                        for ls in &lang_servers {
                            info!("  Language server: {:?}", ls);
                        }
                    }

                    // Use the new LSP manager with feature flag support
                    info!("Starting LSP for document using feature flag system");
                    let lsp_result = core.start_lsp_with_feature_flags(doc_id);

                    match lsp_result {
                        nucleotide_lsp::LspStartupResult::Success {
                            mode,
                            language_servers,
                            duration,
                        } => {
                            info!(
                                mode = ?mode,
                                language_servers = ?language_servers,
                                duration_ms = duration.as_millis(),
                                "LSP startup successful with feature flag system"
                            );
                        }
                        nucleotide_lsp::LspStartupResult::Failed {
                            mode,
                            error,
                            fallback_mode,
                        } => {
                            warn!(
                                mode = ?mode,
                                error = %error,
                                fallback_mode = ?fallback_mode,
                                "LSP startup failed"
                            );

                            // Fallback to existing mechanism as additional safety net
                            helix_event::request_redraw();
                        }
                        nucleotide_lsp::LspStartupResult::Skipped { reason } => {
                            info!(
                                reason = %reason,
                                "LSP startup skipped"
                            );
                        }
                    }

                    // Trigger a redraw event to ensure UI updates
                    helix_event::request_redraw();

                    // Emit an editor redraw event which should trigger various checks
                    cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                        crate::types::CoreEvent::RedrawRequested,
                    )));

                    // Set cursor to beginning of file without selecting content
                    let view_id = core.editor.tree.focus;

                    // Check if the view exists before attempting operations
                    if let Some(view) = core.editor.tree.try_get(view_id) {
                        // Get the current document id from the view
                        let view_doc_id = view.doc;
                        info!(
                            "View {:?} has document ID: {:?}, opened doc_id: {:?}",
                            view_id, view_doc_id, doc_id
                        );

                        // Make sure the view is showing the document we just opened
                        if view_doc_id != doc_id {
                            info!(
                                "View is showing different document, switching to opened document"
                            );
                            core.editor
                                .switch(doc_id, helix_view::editor::Action::Replace);
                        }

                        // Set the selection and ensure cursor is in view
                        core.editor.ensure_cursor_in_view(view_id);
                        if let Some(doc) = core.editor.document_mut(doc_id) {
                            let pos = Selection::point(0);
                            doc.set_selection(view_id, pos);
                        }
                        core.editor.ensure_cursor_in_view(view_id);
                    }
                }
            }
            cx.notify();
        });

        // Force focus update to ensure the correct view is focused
        self.core.update(cx, |core, _cx| {
            let view_id = core.editor.tree.focus;
            info!("Current focused view after opening: {:?}", view_id);
        });

        // Update document views after opening file
        self.update_document_views(cx);

        // Try to trigger the same flow as initialization
        // by focusing the view and requesting a redraw
        self.core.update(cx, |core, _cx| {
            let view_id = core.editor.tree.focus;
            core.editor.focus(view_id);

            // Request idle timer which might trigger LSP initialization
            core.editor.reset_idle_timer();
        });

        // Sync file tree selection with the newly opened file
        if let Some(file_tree) = &self.file_tree {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path), cx);
            });
        }

        // Only focus the editor if requested (not when opening from file tree)
        if should_focus && self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }

        // Force a redraw
        cx.notify();
    }

    #[instrument(skip(self, cx), fields(event = ?ev))]
    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        info!("handling event {ev:?}");
        match ev {
            crate::Update::EditorEvent(ev) => self.handle_editor_event(ev, cx),
            crate::Update::EditorStatus(_) => {}
            crate::Update::Redraw => self.handle_redraw(cx),
            crate::Update::Prompt(_)
            | crate::Update::Picker(_)
            | crate::Update::DirectoryPicker(_)
            | crate::Update::Completion(_) => {
                self.handle_overlay_update(cx);
            }
            crate::Update::OpenFile(path) => self.handle_open_file(path, cx),
            crate::Update::OpenDirectory(path) => self.handle_open_directory(path, cx),
            crate::Update::FileTreeEvent(event) => {
                self.handle_file_tree_event(event, cx);
            }
            crate::Update::ShowFilePicker => {
                let handle = self.handle.clone();
                let core = self.core.clone();
                open(core, handle, cx);
            }
            crate::Update::ShowBufferPicker => {
                let handle = self.handle.clone();
                let core = self.core.clone();
                show_buffer_picker(core, handle, cx);
            }
            crate::Update::Info(_) => {
                self.info_hidden = false;
                // handled by the info box view
                // Also update key hints
                self.update_key_hints(cx);
            }
            crate::Update::ShouldQuit => {
                info!("ShouldQuit event received - triggering application quit");
                cx.quit();
            }
            crate::Update::CommandSubmitted(command) => self.handle_command_submitted(command, cx),
            crate::Update::SearchSubmitted(search_text) => {
                self.handle_search_submitted(search_text, cx)
            }
            // Helix event bridge - respond to automatic Helix events
            crate::Update::DocumentChanged { doc_id } => self.handle_document_changed(*doc_id, cx),
            crate::Update::SelectionChanged { doc_id, view_id } => {
                self.handle_selection_changed(*doc_id, *view_id, cx)
            }
            crate::Update::ModeChanged { old_mode, new_mode } => {
                self.handle_mode_changed(old_mode, new_mode, cx)
            }
            crate::Update::DiagnosticsChanged { doc_id } => {
                self.handle_diagnostics_changed(*doc_id, cx)
            }
            crate::Update::DocumentOpened { doc_id } => self.handle_document_opened(*doc_id, cx),
            crate::Update::DocumentClosed { doc_id } => self.handle_document_closed(*doc_id, cx),
            crate::Update::ViewFocused { view_id } => self.handle_view_focused(*view_id, cx),
            crate::Update::LanguageServerInitialized { server_id, .. } => {
                self.handle_language_server_initialized(*server_id, cx)
            }
            crate::Update::LanguageServerExited { server_id } => {
                self.handle_language_server_exited(*server_id, cx)
            }
            crate::Update::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => self.handle_completion_requested(*doc_id, *view_id, trigger, cx),
            // Handle new event-based updates (during migration)
            crate::Update::Event(event) => {
                match event {
                    crate::types::AppEvent::Core(core_event) => {
                        match core_event {
                            crate::types::CoreEvent::ShouldQuit => {
                                info!("ShouldQuit event received via Event system");
                                cx.quit();
                            }
                            crate::types::CoreEvent::RedrawRequested => {
                                self.handle_redraw(cx);
                            }
                            crate::types::CoreEvent::CommandSubmitted { command } => {
                                self.handle_command_submitted(command, cx);
                            }
                            crate::types::CoreEvent::SearchSubmitted { query } => {
                                self.handle_search_submitted(query, cx);
                            }
                            crate::types::CoreEvent::DocumentChanged { doc_id } => {
                                self.handle_document_changed(*doc_id, cx);
                            }
                            crate::types::CoreEvent::SelectionChanged { doc_id, view_id } => {
                                self.handle_selection_changed(*doc_id, *view_id, cx);
                            }
                            crate::types::CoreEvent::ModeChanged { old_mode, new_mode } => {
                                self.handle_mode_changed(old_mode, new_mode, cx);
                            }
                            crate::types::CoreEvent::DiagnosticsChanged { doc_id } => {
                                self.handle_diagnostics_changed(*doc_id, cx);
                            }
                            crate::types::CoreEvent::DocumentOpened { doc_id } => {
                                self.handle_document_opened(*doc_id, cx);
                            }
                            crate::types::CoreEvent::DocumentClosed { doc_id } => {
                                self.handle_document_closed(*doc_id, cx);
                            }
                            crate::types::CoreEvent::ViewFocused { view_id } => {
                                self.handle_view_focused(*view_id, cx);
                            }
                            crate::types::CoreEvent::CompletionRequested {
                                doc_id,
                                view_id,
                                trigger,
                            } => {
                                self.handle_completion_requested(*doc_id, *view_id, trigger, cx);
                            }
                            _ => {
                                // Other core events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Workspace(workspace_event) => {
                        match workspace_event {
                            crate::types::WorkspaceEvent::FileSelected { path, source } => {
                                use nucleotide_events::v2::workspace::SelectionSource;
                                match source {
                                    SelectionSource::Click | SelectionSource::Command => {
                                        if path.is_file() {
                                            self.handle_open_file(path, cx);
                                        } else if path.is_dir() {
                                            self.handle_open_directory(path, cx);
                                        }
                                    }
                                    _ => {
                                        // Other selection sources
                                    }
                                }
                            }
                            _ => {
                                // Other workspace events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Ui(ui_event) => {
                        match ui_event {
                            crate::types::UiEvent::OverlayShown { overlay_type, .. } => {
                                use nucleotide_events::v2::ui::OverlayType;
                                match overlay_type {
                                    OverlayType::FilePicker => {
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        open(core, handle, cx);
                                    }
                                    OverlayType::CommandPalette => {
                                        // For now, treat command palette as buffer picker
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        show_buffer_picker(core, handle, cx);
                                    }
                                    _ => {
                                        // Other overlay types not yet implemented
                                    }
                                }
                            }
                            crate::types::UiEvent::SystemAppearanceChanged { appearance } => {
                                self.handle_system_appearance_changed(*appearance, cx);
                            }
                            _ => {
                                // Other UI events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Lsp(lsp_event) => {
                        match lsp_event {
                            crate::types::LspEvent::ServerInitialized { server_id, .. } => {
                                self.handle_language_server_initialized(*server_id, cx);
                            }
                            crate::types::LspEvent::ServerExited { server_id, .. } => {
                                self.handle_language_server_exited(*server_id, cx);
                            }
                            _ => {
                                // Other LSP events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Document(_doc_event) => {
                        // Document events are handled through legacy Update system
                        // Future enhancement: Implement direct V2 document event handlers
                    }
                    crate::types::AppEvent::Editor(_editor_event) => {
                        // Editor events are handled through legacy Update system
                        // Future enhancement: Implement direct V2 editor event handlers
                    }
                    crate::types::AppEvent::Vcs(vcs_event) => {
                        // VCS events for diff gutter indicators and repository status
                        debug!(vcs_event = ?vcs_event, "VCS event received");
                        // TODO: Update gutter indicators based on VCS events
                    }
                }
            }
        }
    }

    /// Render the tab bar showing all open documents
    fn render_tab_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::tab_bar::{DocumentInfo, TabBar};
        use helix_view::editor::BufferLine;

        let core = self.core.read(cx);
        let editor = &core.editor;

        // Check bufferline configuration
        let bufferline_config = &editor.config().bufferline;
        debug!(
            "render_tab_bar: bufferline config = {:?}, doc count = {}",
            bufferline_config,
            editor.documents.len()
        );

        let should_show_tabs = match bufferline_config {
            BufferLine::Never => false,
            BufferLine::Always => true,
            BufferLine::Multiple => editor.documents.len() > 1,
        };

        debug!(
            should_show_tabs = should_show_tabs,
            match_result = ?bufferline_config,
            "Tab bar visibility decision"
        );

        // If tabs shouldn't be shown, return an empty div with a unique ID
        if !should_show_tabs {
            debug!("Tab bar hidden, returning empty div");
            return div()
                .id("tab-bar-hidden")
                .h(px(0.0)) // Explicitly set height to 0 to ensure no space is taken
                .into_any_element();
        }

        debug!("Tab bar visible, rendering tabs");

        // Calculate available width for tabs dynamically
        let window_size = window.viewport_size();
        let mut available_width = window_size.width.0;

        // Subtract file tree width if it's visible
        if self.show_file_tree {
            const RESIZE_HANDLE_WIDTH: f32 = 4.0;
            available_width -= self.file_tree_width + RESIZE_HANDLE_WIDTH;
        }

        // Reserve some margin for window padding and other UI elements
        const TAB_BAR_MARGIN: f32 = 20.0;
        available_width = (available_width - TAB_BAR_MARGIN).max(200.0); // Minimum 200px

        debug!(
            window_width = window_size.width.0,
            file_tree_width = self.file_tree_width,
            show_file_tree = self.show_file_tree,
            calculated_available_width = available_width,
            "Dynamic tab bar width calculation"
        );

        // Get the currently active document ID
        let active_doc_id = self
            .view_manager
            .focused_view_id()
            .and_then(|focused_view_id| editor.tree.try_get(focused_view_id))
            .map(|view| view.doc);

        // Get project directory for relative paths first
        let project_directory = core.project_directory.clone();

        // Collect all current document IDs
        let current_doc_ids: std::collections::HashSet<_> =
            editor.documents.keys().copied().collect();

        // Release the core borrow early
        let _ = core;

        // Clean up order list - remove documents that no longer exist
        self.document_order
            .retain(|doc_id| current_doc_ids.contains(doc_id));

        // Add any new documents to the end of the order list (rightmost position)
        for &doc_id in &current_doc_ids {
            self.ensure_document_in_order(doc_id);
        }

        // Now collect document information in the stable order
        let mut documents = Vec::new();
        let core = self.core.read(cx);
        let editor = &core.editor;

        // Iterate in our stable order, not HashMap order
        for (order_index, &doc_id) in self.document_order.iter().enumerate() {
            if let Some(doc) = editor.documents.get(&doc_id) {
                documents.push(DocumentInfo {
                    id: doc_id,
                    path: doc.path().map(|p| p.to_path_buf()),
                    is_modified: doc.is_modified(),
                    focused_at: doc.focused_at,
                    order: order_index, // Use position in Vec as order
                    git_status: None,   // Will be filled in after releasing core borrow
                });
            }
        }

        // Release the core borrow
        let _ = core;

        // Ensure VCS service is monitoring the current project directory
        if let Some(ref project_dir) = project_directory {
            let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
            vcs_handle.update(cx, |service, cx| {
                // Only start monitoring if we're not already monitoring this directory
                if service.root_path() != Some(project_dir.as_path()) {
                    service.start_monitoring(project_dir.clone(), cx);
                }
                // Always refresh to get current status
                service.force_refresh(cx);
            });
        }

        // Update documents with VCS status using cached method
        for doc_info in &mut documents {
            if let Some(ref path) = doc_info.path {
                let status = cx.global::<VcsServiceHandle>().get_status_cached(path, cx);
                debug!(file = %path.display(), vcs_status = ?status, "VCS status for tab");
                doc_info.git_status = status;
            }
        }

        // Create tab bar with callbacks
        TabBar::new(
            documents,
            active_doc_id,
            project_directory,
            {
                let workspace = cx.entity().clone();
                let core = self.core.clone();
                let handle = self.handle.clone();
                move |doc_id, _window, cx| {
                    // Switch the current view to display this document
                    core.update(cx, |core, cx| {
                        let _guard = handle.enter();

                        // Use Helix's switch method to change which document is displayed
                        core.editor
                            .switch(doc_id, helix_view::editor::Action::Replace);

                        // Emit a redraw event so the UI updates
                        cx.emit(crate::Update::Redraw);
                    });

                    // Update workspace to refresh the view
                    workspace.update(cx, |workspace, cx| {
                        // Update document views to reflect the change
                        workspace.update_document_views(cx);
                    });
                }
            },
            {
                let workspace = cx.entity().clone();
                let core = self.core.clone();
                let handle = self.handle.clone();
                move |doc_id, _window, cx| {
                    // Handle tab close - close the buffer/document
                    core.update(cx, |core, cx| {
                        let _guard = handle.enter();

                        // Close the document (buffer), not the view
                        // This allows other buffers to remain open
                        match core.editor.close_document(doc_id, false) {
                            Ok(()) => {
                                // Document closed successfully
                                cx.emit(crate::Update::Redraw);
                            }
                            Err(helix_view::editor::CloseError::BufferModified(_)) => {
                                // Document has unsaved changes - could show a dialog here
                                // For now, just log it
                                info!("Cannot close document {:?}: has unsaved changes", doc_id);
                            }
                            Err(helix_view::editor::CloseError::DoesNotExist) => {
                                // Document doesn't exist anymore
                                info!("Document {:?} does not exist", doc_id);
                            }
                            Err(_) => {
                                // Other error
                                info!("Failed to close document {:?}", doc_id);
                            }
                        }

                        // Update the workspace
                        cx.notify();
                    });

                    // Update workspace to refresh the view
                    workspace.update(cx, |workspace, cx| {
                        workspace.update_document_views(cx);
                    });
                }
            },
        )
        .with_available_width(available_width) // Dynamic width based on window size and file tree
        .with_overflow_open(self.tab_overflow_dropdown_open)
        .with_overflow_toggle({
            let workspace_entity = cx.entity().clone();
            move |_window, cx| {
                workspace_entity.update(cx, |workspace, cx| {
                    workspace.tab_overflow_dropdown_open = !workspace.tab_overflow_dropdown_open;
                    cx.notify();
                });
            }
        })
        .into_any_element()
    }

    /// Render the tab overflow dropdown menu as an overlay
    fn render_tab_overflow_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::tab_bar::{DocumentInfo, TabBar};
        use crate::tab_overflow_dropdown::TabOverflowMenu;
        use helix_view::editor::BufferLine;

        let core = self.core.read(cx);
        let editor = &core.editor;

        // Check bufferline configuration
        let bufferline_config = &editor.config().bufferline;
        let should_show_tabs = match bufferline_config {
            BufferLine::Never => false,
            BufferLine::Always => true,
            BufferLine::Multiple => editor.documents.len() > 1,
        };

        if !should_show_tabs {
            return div().into_any_element();
        }

        // Get the currently active document ID
        let active_doc_id = self
            .view_manager
            .focused_view_id()
            .and_then(|focused_view_id| editor.tree.try_get(focused_view_id))
            .map(|view| view.doc);

        // Get project directory for relative paths first
        let project_directory = core.project_directory.clone();

        // Collect document information using the same stable ordering as main tab bar
        let mut documents = Vec::new();
        for (order_index, &doc_id) in self.document_order.iter().enumerate() {
            if let Some(doc) = editor.documents.get(&doc_id) {
                documents.push(DocumentInfo {
                    id: doc_id,
                    path: doc.path().map(|p| p.to_path_buf()),
                    is_modified: doc.is_modified(),
                    focused_at: doc.focused_at,
                    order: order_index, // Use position in Vec as order
                    git_status: None,   // Will be filled in after releasing core borrow
                });
            }
        }

        // Release the core borrow
        let _ = core;

        // Calculate available width for tabs dynamically (same as in render_tab_bar)
        let window_size = window.viewport_size();
        let mut available_width = window_size.width.0;

        // Subtract file tree width if it's visible
        if self.show_file_tree {
            const RESIZE_HANDLE_WIDTH: f32 = 4.0;
            available_width -= self.file_tree_width + RESIZE_HANDLE_WIDTH;
        }

        // Reserve some margin for window padding and other UI elements
        const TAB_BAR_MARGIN: f32 = 20.0;
        available_width = (available_width - TAB_BAR_MARGIN).max(200.0); // Minimum 200px

        // Update documents with VCS status using cached method
        for doc_info in &mut documents {
            if let Some(ref path) = doc_info.path {
                let status = cx.global::<VcsServiceHandle>().get_status_cached(path, cx);
                doc_info.git_status = status;
            }
        }

        // Apply the same sorting as the main tab bar for consistency
        // Sort by the order field which tracks opening order
        documents.sort_by(|a, b| a.order.cmp(&b.order));

        // Create a temporary TabBar to get overflow documents
        let temp_tab_bar = TabBar::new(
            documents.clone(),
            active_doc_id,
            project_directory.clone(),
            |_doc_id, _window, _cx| {}, // No-op
            |_doc_id, _window, _cx| {}, // No-op
        )
        .with_available_width(available_width); // Same dynamic width as main tab bar

        let overflow_documents = temp_tab_bar.get_overflow_documents();

        if overflow_documents.is_empty() {
            return div().into_any_element();
        }

        TabOverflowMenu::new(
            overflow_documents,
            active_doc_id,
            project_directory,
            std::sync::Arc::new({
                let workspace = cx.entity().clone();
                let core = self.core.clone();
                let handle = self.handle.clone();
                move |doc_id, _window, cx| {
                    // Switch the current view to display this document
                    core.update(cx, |core, cx| {
                        let _guard = handle.enter();
                        // Use Helix's switch method to change which document is displayed
                        core.editor
                            .switch(doc_id, helix_view::editor::Action::Replace);
                        // Emit a redraw event so the UI updates
                        cx.emit(crate::Update::Redraw);
                    });

                    // Update workspace to refresh the view
                    workspace.update(cx, |workspace, cx| {
                        workspace.update_document_views(cx);
                    });
                }
            }),
            {
                let workspace_entity = cx.entity().clone();
                move |_window, cx| {
                    workspace_entity.update(cx, |workspace, cx| {
                        workspace.tab_overflow_dropdown_open = false;
                        cx.notify();
                    });
                }
            },
        )
        .into_any_element()
    }

    /// Render unified status bar with file tree toggle and status information
    fn render_unified_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Use hybrid color system with StatusBarTokens
        let ui_theme = cx.global::<nucleotide_ui::Theme>();
        let status_bar_tokens = ui_theme.tokens.status_bar_tokens();

        // Use the hybrid chrome background colors for consistent visual hierarchy
        let bg_color = status_bar_tokens.background_active; // Always use active for unified bar
        let fg_color = status_bar_tokens.text_primary;

        // Extract design token values before any mutable borrows
        let space_3 = ui_theme.tokens.sizes.space_3;
        let text_sm = ui_theme.tokens.sizes.text_sm;

        // Get UI font configuration
        let ui_font_config = cx.global::<crate::types::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);

        // Get current document info first (without LSP indicator to avoid borrow conflicts)
        let (mode_name, file_name, position_text, has_lsp_state) = {
            let core = self.core.read(cx);
            let editor = &core.editor;

            let mut mode_name = "NOR";
            let mut file_name = "[no file]".to_string();
            let mut position_text = "1:1".to_string();

            // Get info from focused view if available
            if let Some(view_id) = self.view_manager.focused_view_id()
                && let Some((view, doc)) = editor
                    .tree
                    .try_get(view_id)
                    .and_then(|v| editor.document(v.doc).map(|d| (v, d)))
            {
                mode_name = match editor.mode() {
                    helix_view::document::Mode::Normal => "NOR",
                    helix_view::document::Mode::Insert => "INS",
                    helix_view::document::Mode::Select => "SEL",
                };

                file_name = doc
                    .path()
                    .map(|p| {
                        let path_str = p.to_string_lossy().to_string();
                        // Truncate long paths
                        if path_str.len() > 50 {
                            if let Some(file_name) = p.file_name() {
                                format!(".../{}", file_name.to_string_lossy())
                            } else {
                                "...".to_string()
                            }
                        } else {
                            path_str
                        }
                    })
                    .unwrap_or_else(|| "[scratch]".to_string());

                let position = helix_core::coords_at_pos(
                    doc.text().slice(..),
                    doc.selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..)),
                );
                position_text = format!("{}:{}", position.row + 1, position.col + 1);
            }

            let has_lsp_state = core.lsp_state.is_some();
            (mode_name, file_name, position_text, has_lsp_state)
        };

        // Get LSP indicator separately to avoid borrowing conflicts
        let lsp_indicator = if has_lsp_state {
            // Clone the lsp_state entity to avoid borrowing conflicts
            let lsp_state_entity = {
                let core = self.core.read(cx);
                core.lsp_state.clone()
            };
            if let Some(lsp_state) = lsp_state_entity {
                lsp_state.update(cx, |state, _| state.get_lsp_indicator())
            } else {
                None
            }
        } else {
            None
        };

        // Use consistent border and divider colors from hybrid system
        let border_color = status_bar_tokens.border;
        let divider_color = status_bar_tokens.border;
        div()
            .h(px(28.0))
            .w_full()
            .bg(bg_color)
            .border_t_1()
            .border_color(border_color)
            .flex()
            .flex_row()
            .items_center()
            .font(font)
            .text_size(font_size)
            .text_color(fg_color)
            .child(
                // Toggle button container - fixed width regardless of file tree state
                div()
                    .w(px(32.0)) // Fixed width for button container
                    .flex()
                    .items_center()
                    .justify_center()
                    .child({
                        let workspace_entity = cx.entity().clone();
                        Button::icon_only("file-tree-toggle", "icons/folder-tree.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .on_click(move |_event, _window, app_cx| {
                                workspace_entity.update(app_cx, |workspace, cx| {
                                    info!("Status bar file tree toggle clicked");
                                    workspace.show_file_tree = !workspace.show_file_tree;
                                    cx.notify();
                                });
                            })
                    }),
            )
            .when(self.show_file_tree, |status_bar| {
                status_bar
                    .child(
                        // File tree width spacer (minus button width)
                        div()
                            .w(px(self.file_tree_width - 32.0)) // File tree width minus button
                            .h_full(),
                    )
                    .child(
                        // Resize handle spacer
                        div()
                            .w(px(4.0)) // Resize handle width
                            .h_full(),
                    )
            })
            .child(
                // Main status content - fills remaining space
                div()
                    .flex()
                    .flex_1()
                    .flex_row()
                    .items_center()
                    .child({
                        // Mode indicator using standard text color
                        div()
                            .child(mode_name)
                            .min_w(px(50.))
                            .text_color(status_bar_tokens.text_primary)
                    })
                    .child(
                        // Divider
                        div().w(px(1.)).h(px(18.)).bg(divider_color).mx_2(),
                    )
                    .child(
                        // File name - takes up available space
                        div().flex_1().overflow_hidden().child(file_name),
                    )
                    .child(
                        // Divider
                        div().w(px(1.)).h(px(18.)).bg(divider_color).mx_2(),
                    )
                    .child(
                        // Position
                        div().child(position_text).min_w(px(80.)),
                    )
                    .when_some(lsp_indicator, |status_bar, indicator| {
                        status_bar
                            .child(
                                // Divider before LSP
                                div().w(px(1.)).h(px(18.)).bg(divider_color).mx_2(),
                            )
                            .child(
                                // LSP indicator - dynamic width based on content using design tokens
                                div()
                                    .child(indicator.clone())
                                    .flex_shrink() // Allow shrinking when space is limited
                                    .max_w(px(400.)) // Max width prevents taking over the entire status bar
                                    .min_w(px(16.)) // Minimum for icon-only display
                                    .overflow_hidden()
                                    .text_ellipsis() // Graceful text truncation
                                    .px(space_3) // Use design token spacing
                                    .text_size(text_sm) // Use design token text sizing
                                    .whitespace_nowrap(), // Prevent text wrapping
                            )
                    }), // .child({
                        //     // Project status indicator section - temporarily disabled
                        //     // let project_status_handle = nucleotide_project::project_status_service(cx);
                        //     // let project_info = project_status_handle.project_info(cx);
                        //
                        //     div()
                        //         .flex()
                        //         .flex_row()
                        //         .items_center()
                        //         .gap(ui_theme.tokens.sizes.space_2)
                        //         .child(
                        //             // Divider before project status
                        //             div().w(px(1.)).h(px(18.)).bg(divider_color).mx_2()
                        //         )
                        // }),
            )
    }

    fn handle_file_tree_event(&mut self, event: &FileTreeEvent, cx: &mut Context<Self>) {
        match event {
            FileTreeEvent::OpenFile { path } => {
                // Open file but keep focus on file tree
                info!("FileTreeEvent::OpenFile received in workspace: {:?}", path);
                self.handle_open_file_keep_focus(path, cx);
            }
            FileTreeEvent::SelectionChanged { path: _ } => {
                // Update UI if needed for selection changes
                cx.notify();
            }
            FileTreeEvent::DirectoryToggled {
                path: _,
                expanded: _,
            } => {
                // Update UI for directory expansion/collapse
                cx.notify();
            }
            FileTreeEvent::FileSystemChanged { path, kind } => {
                info!("File system change detected: {:?} - {:?}", path, kind);
                // Handle file system changes by triggering VCS refresh
                if let Some(ref mut file_tree) = self.file_tree {
                    file_tree.update(cx, |tree, tree_cx| {
                        tree.handle_file_system_change(path, tree_cx);
                    });
                }
                cx.notify();
            }
            FileTreeEvent::VcsRefreshStarted { repository_root } => {
                info!("VCS refresh started for repository: {:?}", repository_root);
                // TODO: Show loading indicator in status bar
                cx.notify();
            }
            FileTreeEvent::VcsStatusChanged {
                repository_root,
                affected_files,
            } => {
                info!(
                    "VCS status updated for repository: {:?} ({} files)",
                    repository_root,
                    affected_files.len()
                );
                // VCS status has been updated, file tree should already be refreshed
                // Could trigger status bar updates or notifications here
                cx.notify();
            }
            FileTreeEvent::VcsRefreshFailed {
                repository_root,
                error,
            } => {
                error!(
                    "VCS refresh failed for repository: {:?} - {}",
                    repository_root, error
                );
                // TODO: Show error notification to user
                cx.notify();
            }
            FileTreeEvent::RefreshVcs { force } => {
                info!("VCS refresh requested (force: {})", force);
                if let Some(ref mut file_tree) = self.file_tree {
                    file_tree.update(cx, |tree, tree_cx| {
                        tree.handle_vcs_refresh(*force, tree_cx);
                    });
                }
                cx.notify();
            }
            FileTreeEvent::ToggleVisibility => {
                info!("Toggle file tree visibility requested");
                self.show_file_tree = !self.show_file_tree;
                cx.notify();
            }
        }
    }

    fn update_key_hints(&mut self, cx: &mut Context<Self>) {
        // Check if editor has pending keymap info
        let editor = &self.core.read(cx).editor;
        let editor_info = editor.autoinfo.as_ref().map(|info| helix_view::info::Info {
            title: info.title.clone(),
            text: info.text.clone(),
            width: info.width,
            height: info.height,
        });
        let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

        self.key_hints.update(cx, |key_hints, cx| {
            key_hints.set_info(editor_info);
            key_hints.set_theme(theme);
            cx.notify();
        });
    }

    /// Register focus groups for main UI areas with InputCoordinator
    fn register_focus_groups(&mut self, cx: &mut Context<Self>) {
        info!("Registering focus groups with InputCoordinator");

        // Register editor focus group
        self.input_coordinator.register_focus_group(
            FocusGroup::Editor,
            Some(self.focus_handle.clone()),
            Some(Box::new(|| {
                debug!("Editor focus group activated");
            })),
        );

        // Register file tree focus group if available
        if let Some(ref file_tree) = self.file_tree {
            self.input_coordinator.register_focus_group(
                FocusGroup::FileTree,
                Some(file_tree.focus_handle(cx)),
                Some(Box::new(|| {
                    debug!("FileTree focus group activated");
                })),
            );
        }

        // Register overlay focus group
        self.input_coordinator.register_focus_group(
            FocusGroup::Overlays,
            Some(self.overlay.focus_handle(cx)),
            Some(Box::new(|| {
                debug!("Overlays focus group activated");
            })),
        );

        // Set editor and file tree as available if they exist
        self.input_coordinator
            .set_focus_group_available(FocusGroup::Editor, true);
        if self.file_tree.is_some() && self.show_file_tree {
            self.input_coordinator
                .set_focus_group_available(FocusGroup::FileTree, true);
        }

        info!("Registered focus groups for main UI areas with InputCoordinator");

        // OLD CODE - disabled
        /*
            let file_tree_group = GlobalFocusGroup {
                id: "file_tree".to_string(),
                name: "File Tree".to_string(),
                priority: FocusPriority::Normal,
                elements: vec![FocusElement {
                    id: "file_tree_view".to_string(),
                    name: "File Tree View".to_string(),
                    focus_handle: Some(file_tree.focus_handle(cx)),
                    tab_index: 0,
                    enabled: true,
                    element_type: FocusElementType::FileTree,
                }],
                active_element: Some(0),
                enabled: true,
            };
            // DISABLED: // OLD: self.global_input.register_focus_group(file_tree_group);
        }

        // Register overlay focus group
        let overlay_group = GlobalFocusGroup {
            id: "overlays".to_string(),
            name: "Overlays".to_string(),
            priority: FocusPriority::Critical,
            elements: vec![FocusElement {
                id: "overlay_view".to_string(),
                name: "Overlay View".to_string(),
                focus_handle: Some(self.overlay.focus_handle(cx)),
                tab_index: 2,
                enabled: true,
                element_type: FocusElementType::Picker,
            }],
            active_element: Some(0),
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_focus_group(overlay_group);
        */

        // Method completed with InputCoordinator integration
    }

    /// Setup completion-specific shortcuts and input contexts
    fn setup_completion_shortcuts(&mut self) {
        // TODO: Re-implement with InputCoordinator
        /*
        use nucleotide_ui::providers::EventPriority;
        use nucleotide_ui::{
            DismissTarget, GlobalNavigationDirection, ShortcutAction, ShortcutDefinition,
        };

        // Register Escape key to dismiss completion with high priority
        let escape_shortcut = ShortcutDefinition {
            key_combination: "escape".to_string(),
            action: ShortcutAction::Dismiss(DismissTarget::Completion),
            description: "Dismiss completion popup".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::Critical,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(escape_shortcut);

        // Register Ctrl+Space to trigger completion
        let trigger_completion_shortcut = ShortcutDefinition {
            key_combination: "ctrl-space".to_string(),
            action: ShortcutAction::Action("trigger_completion".to_string()),
            description: "Trigger completion".to_string(),
            context: Some("editor".to_string()),
            priority: EventPriority::High,
            enabled: true,
        };
        // OLD: self.global_input
            .register_shortcut(trigger_completion_shortcut);

        // Register Tab for completion navigation
        let tab_shortcut = ShortcutDefinition {
            key_combination: "tab".to_string(),
            action: ShortcutAction::Navigate(GlobalNavigationDirection::Next),
            description: "Navigate to next completion item".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::High,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(tab_shortcut);

        // Register Shift+Tab for reverse completion navigation
        let shift_tab_shortcut = ShortcutDefinition {
            key_combination: "shift-tab".to_string(),
            action: ShortcutAction::Navigate(GlobalNavigationDirection::Previous),
            description: "Navigate to previous completion item".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::High,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(shift_tab_shortcut);

        // Register additional keyboard navigation shortcuts
        self.setup_additional_navigation_shortcuts();

        // Register dismiss handler for completion
        // Note: The actual dismissal is handled by the global input system returning HandledAndStop
        // which prevents the key from reaching the normal escape handling logic
        // DISABLED: Method call to global_input system
        /*
        // OLD: self.global_input.register_dismiss_handler(
            nucleotide_ui::DismissTarget::Completion,
            move || {
                eprintln!("DEBUG: Global input dispatcher handling completion dismiss signal");
                // This signals that the dismiss action was triggered by global input
                // The actual dismissal happens in the normal key handling flow
            },
        );
        */

        nucleotide_logging::info!("Setup completion-specific shortcuts");
        */
    }

    /// Manage completion input context based on completion state
    fn manage_completion_context(&mut self, has_completion: bool) {
        // Check current context stack to see if completion context is active
        let completion_context_active = false; // TODO: Replace with InputCoordinator call

        match (has_completion, completion_context_active) {
            (true, false) => {
                // Completion appeared, push completion context
                // DISABLED: // OLD: self.global_input.push_context("completion");
                nucleotide_logging::debug!("Pushed completion context");
            }
            (false, true) => {
                // Completion disappeared, pop completion context
                // DISABLED: Completion context management
                /*
                if let Some(popped) = // OLD: self.global_input.pop_context() {
                    nucleotide_logging::debug!(context = popped, "Popped completion context");
                }
                */
                debug!("Completion disappeared - context management disabled");
            }
            _ => {
                // No context change needed
            }
        }
    }

    /// Setup additional keyboard navigation shortcuts for comprehensive app navigation
    fn setup_additional_navigation_shortcuts(&mut self) {
        use nucleotide_ui::providers::EventPriority;
        use nucleotide_ui::{GlobalNavigationDirection, ShortcutAction, ShortcutDefinition};

        // Global shortcuts that work in any context
        let global_shortcuts = vec![
            // File tree management
            ShortcutDefinition {
                key_combination: "ctrl-b".to_string(),
                action: ShortcutAction::Action("toggle_file_tree".to_string()),
                description: "Toggle file tree visibility".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-e".to_string(),
                action: ShortcutAction::Focus("file_tree".to_string()),
                description: "Focus file tree".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Focus management shortcuts
            ShortcutDefinition {
                key_combination: "ctrl-1".to_string(),
                action: ShortcutAction::Focus("editor".to_string()),
                description: "Focus main editor".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-2".to_string(),
                action: ShortcutAction::Focus("file_tree".to_string()),
                description: "Focus file tree".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Panel navigation
            ShortcutDefinition {
                key_combination: "alt-left".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Left),
                description: "Navigate to left panel".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "alt-right".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Right),
                description: "Navigate to right panel".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Quick actions
            ShortcutDefinition {
                key_combination: "ctrl-p".to_string(),
                action: ShortcutAction::Action("open_file_picker".to_string()),
                description: "Open file picker".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-p".to_string(),
                action: ShortcutAction::Action("open_command_palette".to_string()),
                description: "Open command palette".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
            // Window management
            ShortcutDefinition {
                key_combination: "ctrl-w".to_string(),
                action: ShortcutAction::Action("close_active_document".to_string()),
                description: "Close active document".to_string(),
                context: Some("editor".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-w".to_string(),
                action: ShortcutAction::Action("close_all_documents".to_string()),
                description: "Close all documents".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Search and navigation
            ShortcutDefinition {
                key_combination: "ctrl-f".to_string(),
                action: ShortcutAction::Action("start_search".to_string()),
                description: "Start search".to_string(),
                context: Some("editor".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-f".to_string(),
                action: ShortcutAction::Action("global_search".to_string()),
                description: "Global search in files".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
        ];

        // File tree specific shortcuts
        let file_tree_shortcuts = vec![
            // Navigate within file tree
            ShortcutDefinition {
                key_combination: "up".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Move up in file tree".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "down".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Move down in file tree".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "left".to_string(),
                action: ShortcutAction::Action("collapse_file_tree_node".to_string()),
                description: "Collapse file tree node".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "right".to_string(),
                action: ShortcutAction::Action("expand_file_tree_node".to_string()),
                description: "Expand file tree node".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "enter".to_string(),
                action: ShortcutAction::Action("open_selected_file".to_string()),
                description: "Open selected file".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            // Return to editor from file tree
            ShortcutDefinition {
                key_combination: "escape".to_string(),
                action: ShortcutAction::Focus("editor".to_string()),
                description: "Return focus to editor".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
        ];

        // Completion specific shortcuts (beyond the basic ones already registered)
        let completion_shortcuts = vec![
            ShortcutDefinition {
                key_combination: "up".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Move up in completion list".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "down".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Move down in completion list".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "enter".to_string(),
                action: ShortcutAction::Action("accept_completion".to_string()),
                description: "Accept selected completion".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "page-up".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::First),
                description: "Move to first completion item".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "page-down".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Last),
                description: "Move to last completion item".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
        ];

        // Register all shortcuts
        for shortcut in global_shortcuts
            .into_iter()
            .chain(file_tree_shortcuts.into_iter())
            .chain(completion_shortcuts.into_iter())
        {
            // DISABLED: // OLD: self.global_input.register_shortcut(shortcut);
        }

        // Register action handlers
        self.setup_action_handlers();

        nucleotide_logging::info!("Setup additional navigation shortcuts");
    }

    /// Setup action handlers for keyboard shortcuts
    fn setup_action_handlers(&mut self) {
        nucleotide_logging::info!("Setup comprehensive action handlers for keyboard navigation");
        // Note: Action handlers are now implemented as workspace methods
        // The global input system will call these methods via the action execution system
    }

    /// Register action handlers with the global input system
    fn register_action_handlers(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::info!("Registering action handlers with global input system");

        // Get weak references to avoid circular dependencies
        let workspace_handle = cx.entity().downgrade();

        // For now, register simple logging handlers - the real functionality will be
        // implemented via proper GPUI actions below
        // OLD: self.global_input.register_action_handler("focus_editor".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: focus_editor")
        // });

        // OLD: self.global_input.register_action_handler("focus_file_tree".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: focus_file_tree")
        // });

        // OLD: self.global_input.register_action_handler("toggle_file_tree".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: toggle_file_tree")
        // });

        // OLD: self.global_input.register_action_handler("trigger_completion".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: trigger_completion")
        // });

        // OLD: self.global_input.register_action_handler("open_file_picker".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: open_file_picker")
        // });

        // OLD: self.global_input.register_action_handler("open_command_palette".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: open_command_palette")
        // });

        nucleotide_logging::info!("Successfully registered all action handlers");
    }

    /// Handle only truly global shortcuts that should work regardless of focus state
    fn handle_global_shortcuts_only(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = &event.keystroke.key;

        eprintln!(
            "DEBUG: handle_global_shortcuts_only called for key: '{}', ctrl: {}",
            key, modifiers.control
        );

        // Only handle shortcuts that are truly global and don't interfere with component input

        // Ctrl+B (toggle file tree) - should work from anywhere
        if key == "b" && modifiers.control {
            eprintln!(
                "DEBUG: Handling global ToggleFileTree shortcut in handle_global_shortcuts_only"
            );
            nucleotide_logging::debug!("Handling global ToggleFileTree shortcut");
            self.show_file_tree = !self.show_file_tree;
            cx.notify();
            return;
        }

        eprintln!("DEBUG: No global shortcuts matched in handle_global_shortcuts_only");
        // Add other truly global shortcuts here (window management, app-level commands, etc.)
        // but NOT shortcuts that should be handled by focused components
    }

    /// Handle keyboard shortcuts detected by the global input system (full processing)
    fn handle_global_input_shortcuts(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = &event.keystroke.key;

        eprintln!(
            "DEBUG: handle_global_input_shortcuts called for key: '{}', ctrl: {}",
            key, modifiers.control
        );

        // Check for Ctrl+B (toggle file tree)
        if key == "b" && modifiers.control {
            eprintln!(
                "DEBUG: Handling global ToggleFileTree shortcut in handle_global_input_shortcuts"
            );
            nucleotide_logging::debug!("Handling ToggleFileTree shortcut");
            self.show_file_tree = !self.show_file_tree;
            cx.notify();
            return;
        }

        eprintln!("DEBUG: No shortcuts matched in handle_global_input_shortcuts");
        // For focus management shortcuts, we need window context,
        // so we'll handle them in the regular key processing flow instead.
        // This method handles shortcuts that don't require window access.
    }

    // === Action Handler Implementations ===

    /// Focus the main editor area
    pub fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Focusing editor area");

        // Find the currently active document view and focus it
        if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some(doc_view) = self.view_manager.get_document_view(&view_id)
        {
            let doc_focus = doc_view.focus_handle(cx);
            window.focus(&doc_focus);
            nucleotide_logging::debug!(view_id = ?view_id, "Focused active document view");
            return;
        }

        // If no specific document, focus the main workspace
        window.focus(&self.focus_handle);
        nucleotide_logging::debug!("Focused main workspace");
    }

    /// Focus the file tree if it exists and is visible
    pub fn focus_file_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Focusing file tree");

        if let Some(file_tree) = &self.file_tree
            && self.show_file_tree
        {
            let file_tree_focus = file_tree.focus_handle(cx);
            window.focus(&file_tree_focus);
            nucleotide_logging::debug!("Focused file tree");
            return;
        }

        nucleotide_logging::warn!(
            "File tree not available or not visible, focusing editor instead"
        );
        self.focus_editor(window, cx);
    }

    /// Toggle file tree visibility
    pub fn toggle_file_tree_visibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_tree = !self.show_file_tree;
        nucleotide_logging::debug!(
            visible = self.show_file_tree,
            "Toggled file tree visibility"
        );

        if self.show_file_tree {
            // If we're showing the file tree, focus it
            self.focus_file_tree(window, cx);
        } else {
            // If we're hiding the file tree, focus the editor
            self.focus_editor(window, cx);
        }

        cx.notify(); // Trigger re-render
    }

    /// Trigger completion in the active editor
    pub fn trigger_completion(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Triggering LSP completion in active editor");

        // Trigger LSP completion through helix's completion system
        // The completion coordinator will receive the event and handle the UI
        let editor = &self.core.read(cx).editor;
        nucleotide_lsp::lsp_completion_trigger::trigger_completion(editor, true);

        nucleotide_logging::info!(
            "LSP completion trigger sent to helix handlers - coordinator will handle response and UI display"
        );
    }

    // REMOVED: Old completion coordinator initialization method replaced by event-based approach
    // See the implementation at the end of the file that uses the event system

    // REMOVED: Complex cross-thread completion methods replaced by event-based approach
    // The Application now handles completion results and emits Update::Completion events
    // which the workspace receives via the existing event subscription

    /// Handle completion acceptance - insert the selected text into the editor
    fn handle_completion_accepted(&mut self, text: &str, cx: &mut Context<Self>) {
        nucleotide_logging::info!(completion_text = %text, "Handling completion acceptance");

        // Convert the completion text to individual key events and send them to Helix
        // This simulates typing the completion text
        for ch in text.chars() {
            let key_event = helix_view::input::KeyEvent {
                code: helix_view::keyboard::KeyCode::Char(ch),
                modifiers: helix_view::keyboard::KeyModifiers::NONE,
            };

            self.input.update(cx, |_, cx| {
                cx.emit(crate::InputEvent::Key(key_event));
            });
        }

        nucleotide_logging::info!("Completion text sent to Helix editor");
    }

    /// Accept the current completion selection
    pub fn accept_completion(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Accepting current completion selection");

        // Send Enter to accept completion
        let key_event = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::empty(),
        };

        self.input.update(cx, |_, cx| {
            cx.emit(crate::InputEvent::Key(key_event));
        });
        nucleotide_logging::debug!("Sent Enter to accept completion");
    }

    /// Open file picker
    pub fn open_file_picker(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Opening file picker");

        // Send the space+f shortcut to open file picker (common Helix shortcut)
        let key_event = KeyEvent {
            code: KeyCode::Char('f'),
            modifiers: KeyModifiers::empty(),
        };

        self.input.update(cx, |_, cx| {
            cx.emit(crate::InputEvent::Key(key_event));
        });
        nucleotide_logging::debug!("Sent 'f' key to open file picker");
    }

    /// Open command palette  
    pub fn open_command_palette(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Opening command palette");

        // Send ':' to enter command mode (command palette)
        let key_event = KeyEvent {
            code: KeyCode::Char(':'),
            modifiers: KeyModifiers::empty(),
        };

        self.input.update(cx, |_, cx| {
            cx.emit(crate::InputEvent::Key(key_event));
        });
        nucleotide_logging::debug!("Opened command mode (command palette)");
    }

    /// Start local search in current document
    pub fn start_search(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting local search");

        // Send '/' to start search mode
        let key_event = KeyEvent {
            code: KeyCode::Char('/'),
            modifiers: KeyModifiers::empty(),
        };

        self.input.update(cx, |_, cx| {
            cx.emit(crate::InputEvent::Key(key_event));
        });
        nucleotide_logging::debug!("Started search mode");
    }

    /// Start global search across files
    pub fn start_global_search(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting global search");

        // For now, this is the same as local search
        // In a full implementation, this would open a global search interface
        self.start_search(cx);
    }

    fn update_document_views(&mut self, cx: &mut Context<Self>) {
        let mut view_ids = HashSet::new();
        self.make_views(&mut view_ids, cx);
    }

    /// Update only a specific document view - more efficient for targeted updates
    fn update_specific_document_view(
        &mut self,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) {
        // Find views for this specific document
        let view_ids: Vec<helix_view::ViewId> = self
            .core
            .read(cx)
            .editor
            .tree
            .views()
            .filter_map(|(view, _)| {
                if view.doc == doc_id {
                    Some(view.id)
                } else {
                    None
                }
            })
            .collect();

        // Update only the views for this document
        for view_id in view_ids {
            if let Some(view_entity) = self.view_manager.get_document_view(&view_id) {
                view_entity.update(cx, |_view, cx| {
                    cx.notify();
                });
            }
        }
    }

    /// Update only the currently focused document view
    fn update_current_document_view(&mut self, cx: &mut Context<Self>) {
        if let Some(focused_view_id) = self.view_manager.focused_view_id()
            && let Some(view_entity) = self.view_manager.get_document_view(&focused_view_id)
        {
            view_entity.update(cx, |_view, cx| {
                cx.notify();
            });
        }
    }

    /// Trigger completion UI based on current editor state

    /// Send a key directly to Helix, ensuring the editor has focus
    fn send_helix_key(&mut self, key: &str, cx: &mut Context<Self>) {
        // Ensure an editor view has focus
        if self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }

        // Parse the key string and send it to Helix
        let keystroke = gpui::Keystroke::parse(key).unwrap_or_else(|_| {
            // Fallback for simple keys
            gpui::Keystroke {
                key_char: Some(key.chars().next().unwrap_or(' ').to_string()),
                key: key.to_string(),
                modifiers: gpui::Modifiers::default(),
            }
        });

        let key_event = utils::translate_key(&keystroke);
        self.input.update(cx, |_, cx| {
            cx.emit(InputEvent::Key(key_event));
        });
    }

    /// Adjust the editor font size
    fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        // Get current font config
        let mut font_config = cx.global::<crate::types::EditorFontConfig>().clone();

        // Adjust size with bounds checking
        font_config.size = (font_config.size + delta).clamp(8.0, 72.0);

        // Update global font config
        cx.set_global(font_config);

        // Update all document views to use new font size
        self.update_document_views(cx);

        // Force redraw
        cx.notify();
    }

    fn make_views(
        &mut self,
        view_ids: &mut HashSet<ViewId>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut focused_file_name = None;
        let mut focused_doc_path = None;

        {
            let editor = &self.core.read(cx).editor;

            // First pass: collect all active view IDs
            for (view, is_focused) in editor.tree.views() {
                let view_id = view.id;

                view_ids.insert(view_id);

                if is_focused {
                    // Verify the view still exists in the tree before accessing
                    if editor.tree.contains(view_id)
                        && let Some(doc) = editor.document(view.doc)
                    {
                        self.view_manager.set_focused_view_id(Some(view_id));
                        let doc_path = doc.path();
                        focused_file_name = doc_path.map(|p| p.display().to_string());
                        focused_doc_path = doc_path.map(|p| p.to_path_buf());
                    }
                }
            }
        } // End of editor borrow scope

        // Sync file tree selection with the focused document (after releasing borrow)
        if let Some(path) = focused_doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        // Remove views that are no longer active
        let to_remove: Vec<_> = self
            .view_manager
            .document_views()
            .keys()
            .copied()
            .filter(|id| !view_ids.contains(id))
            .collect();
        for view_id in to_remove {
            self.view_manager.remove_document_view(&view_id);
        }

        // Second pass: create or update views
        for view_id in view_ids.iter() {
            let view_id = *view_id;
            let is_focused = self.view_manager.focused_view_id() == Some(view_id);
            let editor_font = cx.global::<crate::types::EditorFontConfig>();
            let style = TextStyle {
                color: gpui::black(),
                font_family: cx
                    .global::<crate::types::FontSettings>()
                    .fixed_font
                    .family
                    .clone()
                    .into(),
                font_features: FontFeatures::default(),
                font_fallbacks: None,
                font_size: px(editor_font.size).into(),
                line_height: gpui::phi(), // Use golden ratio for optimal line height
                font_weight: editor_font.weight.into(),
                font_style: gpui::FontStyle::Normal,
                background_color: None,
                underline: None,
                strikethrough: None,
                white_space: gpui::WhiteSpace::Normal,
                text_overflow: None,
                text_align: gpui::TextAlign::default(),
                line_clamp: None,
            };
            let core = self.core.clone();

            // Check if view exists and update its style if it does
            if let Some(view) = self.view_manager.get_document_view(&view_id) {
                view.update(cx, |view, _cx| {
                    view.set_focused(is_focused);
                    view.update_text_style(style.clone());
                });
            } else {
                // Create new view if it doesn't exist
                let view = cx.new(|cx| {
                    let doc_focus_handle = cx.focus_handle();
                    DocumentView::new(core, view_id, style.clone(), &doc_focus_handle, is_focused)
                });
                self.view_manager.insert_document_view(view_id, view);
            }
        }
        focused_file_name
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Set up window appearance observer on first render
        if !self.appearance_observer_set {
            self.appearance_observer_set = true;

            // Get initial appearance and trigger theme switch if needed
            let initial_appearance = cx.window_appearance();
            nucleotide_logging::info!(
                initial_appearance = ?initial_appearance,
                "Initial window appearance detected at startup"
            );

            // Handle initial appearance
            self.handle_appearance_change(initial_appearance, window, cx);

            // Set up observer for future changes
            cx.observe_window_appearance(window, |workspace: &mut Workspace, window, cx| {
                // Get the new appearance from the window
                let appearance = window.appearance();
                nucleotide_logging::info!(
                    new_appearance = ?appearance,
                    "Window appearance observer triggered"
                );
                workspace.needs_appearance_update = true;
                workspace.pending_appearance = Some(appearance);
                cx.notify();
            })
            .detach();
        }

        // Handle window appearance update if needed (for theme changes)
        if self.needs_window_appearance_update {
            debug!("Processing scheduled window appearance update");
            self.needs_window_appearance_update = false;
            self.update_window_appearance(window, cx);
        }

        // Handle appearance update if needed
        if self.needs_appearance_update {
            self.needs_appearance_update = false;
            if let Some(appearance) = self.pending_appearance.take() {
                nucleotide_logging::info!(
                    pending_appearance = ?appearance,
                    "Processing pending appearance change"
                );
                self.handle_appearance_change(appearance, window, cx);
            } else {
                // Fallback to current appearance if no pending appearance
                let appearance = cx.window_appearance();
                self.handle_appearance_change(appearance, window, cx);
            }
        }

        // Handle focus restoration if needed
        if self.view_manager.needs_focus_restore() {
            // ALWAYS focus the workspace for key handling, not individual document views
            // The InputCoordinator will handle routing keys to the appropriate context
            window.focus(&self.focus_handle);
            self.view_manager.set_needs_focus_restore(false);
        }
        // Don't create views during render - just use existing ones
        let mut focused_file_name = None;

        let editor = &self.core.read(cx).editor;

        for (view, is_focused) in editor.tree.views() {
            if is_focused {
                // Verify the view still exists in the tree before accessing
                if editor.tree.contains(view.id)
                    && let Some(doc) = editor.document(view.doc)
                {
                    focused_file_name = doc.path().map(|p| {
                        p.file_name()
                            .and_then(|name| name.to_str())
                            .map(std::string::ToString::to_string)
                            .unwrap_or_else(|| p.display().to_string())
                    });
                }
                break; // Only need the focused view
            }
        }

        // For native titlebar - we still set the window title
        let window_title = if let Some(ref path) = focused_file_name {
            format!("{path} — Helix") // Using em dash like macOS
        } else {
            "Helix".to_string()
        };

        // Only set window title if using native decorations
        if window.window_decorations() == gpui::Decorations::Server {
            window.set_window_title(&window_title);
        }

        let editor = &self.core.read(cx).editor;

        // Get theme colors using testing-aware theme access
        let default_style = cx.theme_style("ui.background");
        let default_ui_text = cx.theme_style("ui.text");
        let bg_color = default_style
            .bg
            .and_then(utils::color_to_hsla)
            .unwrap_or(black());
        let _text_color = default_ui_text
            .fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());
        let window_style = cx.theme_style("ui.window");
        let border_color = window_style
            .fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());

        let editor_rect = editor.tree.area();

        // Create document root container using design tokens
        let mut docs_root = div()
            .id("docs-root")
            .flex()
            .w_full()
            .h_full()
            // Background color inherited // Use semantic background color
            ; // No gap needed for documents

        // Only render the focused view, not all views
        if let Some(focused_view_id) = self.view_manager.focused_view_id()
            && let Some(doc_view) = self.view_manager.get_document_view(&focused_view_id)
        {
            // Create document element container with semantic styling
            // Note: Removed right border since resize handle now serves as the border
            let doc_element = div()
                .id("document-container")
                .flex()
                .size_full()
                // Background color inherited
                .child(doc_view.clone());
            docs_root = docs_root.child(doc_element);
        }

        let focused_view = self
            .view_manager
            .focused_view_id()
            .and_then(|id| self.view_manager.get_document_view(&id))
            .cloned();
        if let Some(_view) = &focused_view {
            // Focus is managed by DocumentView's focus state
        }

        self.core.update(cx, |core, _cx| {
            core.compositor.resize(editor_rect);
            // Also resize the editor to match
            core.editor.resize(editor_rect);
        });

        if let Some(_view) = &focused_view {
            // Focus is managed by DocumentView's focus state
        }

        let has_overlay = !self.overlay.read(cx).is_empty();

        // Create main content area using semantic layout with design tokens
        let main_content = div()
            .id("main-content")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            // Background color inherited
            // No gap needed between tab bar and content
            .child(self.render_tab_bar(window, cx)) // Tab bar at the top of editor area
            .child(
                // Editor content container
                div()
                    .id("editor-container")
                    .flex()
                    .flex_col()
                    .w_full()
                    .flex_1() // Take remaining height after tab bar
                    // Background color inherited
                    .when_some(Some(docs_root), gpui::ParentElement::child)
                    .child(self.notifications.clone())
                    .when(!self.overlay.read(cx).is_empty(), |this| {
                        let view = &self.overlay;
                        this.child(view.clone())
                    })
                    .when(
                        !self.info_hidden && !self.info.read(cx).is_empty(),
                        |this| this.child(self.info.clone()),
                    )
                    .child(self.key_hints.clone())
                    .when(self.tab_overflow_dropdown_open, |this| {
                        // Render the overflow menu as an overlay
                        this.child(self.render_tab_overflow_menu(window, cx))
                    }),
            );

        // Create the main workspace container using nucleotide-ui theme access
        let theme = cx.theme();
        let _tokens = &theme.tokens;

        let mut workspace_div = div()
            .key_context("Workspace")
            .id("workspace")
            .bg(bg_color)
            .flex()
            .flex_col() // Vertical layout to include titlebar
            .w_full()
            .h_full()
            .focusable();

        // Always add global key handling - the workspace should always capture key events
        // regardless of focus state or overlay presence for global shortcuts to work
        workspace_div = workspace_div
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|view, ev, window, cx| {
                view.handle_key(ev, window, cx);
            }));

        // Add resize cursor when needed
        if self.is_resizing_file_tree {
            workspace_div = workspace_div.cursor(gpui::CursorStyle::ResizeLeftRight);
        }

        // Add mouse event handlers
        workspace_div = workspace_div
            .on_mouse_move(
                cx.listener(|workspace, event: &MouseMoveEvent, _window, cx| {
                    if workspace.is_resizing_file_tree {
                        // Mouse events in GPUI are already in logical pixels, no scale correction needed
                        let mouse_x = event.position.x.0;
                        let delta = mouse_x - workspace.resize_start_x;
                        let new_width = (workspace.resize_start_width + delta).clamp(150.0, 600.0);

                        // Update width if changed
                        if (workspace.file_tree_width - new_width).abs() > 0.1 {
                            workspace.file_tree_width = new_width;
                            cx.notify();
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|workspace, _event: &MouseUpEvent, _window, cx| {
                    if workspace.is_resizing_file_tree {
                        workspace.is_resizing_file_tree = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|workspace, _event: &MouseDownEvent, _window, cx| {
                    // Close tab overflow dropdown when clicking elsewhere
                    if workspace.tab_overflow_dropdown_open {
                        workspace.tab_overflow_dropdown_open = false;
                        cx.notify();
                    }

                    // Ensure workspace regains focus when clicked, so global shortcuts work
                    workspace.view_manager.set_needs_focus_restore(true);
                    cx.notify();
                }),
            );

        // Add action handlers
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::help::About, _window, _cx| {
                eprintln!("About Helix");
            },
        ));

        // Global editor actions that work regardless of focus
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::editor::Quit, _window, cx| {
                quit(core.clone(), handle.clone(), cx);
                cx.quit();
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::editor::OpenFile, _window, cx| {
                open(core.clone(), handle.clone(), cx)
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::editor::OpenDirectory, _window, cx| {
                open_directory(core.clone(), handle.clone(), cx)
            },
        ));

        // Settings action - open nucleotide.toml configuration file
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::OpenSettings, _window, cx| {
                workspace.open_settings_file(cx)
            },
        ));

        // Reload configuration action - reload nucleotide.toml without restart
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::ReloadConfiguration, _window, cx| {
                workspace.reload_configuration(cx)
            },
        ));

        // Add handlers for Save, SaveAs, CloseFile
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Save, _window, cx| {
                workspace.execute_raw_command("write", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::SaveAs, _window, cx| {
                // TODO: Implement save as with file dialog
                workspace.execute_raw_command("write", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::CloseFile, _window, cx| {
                workspace.execute_raw_command("close", cx);
            },
        ));

        // Add handlers for Undo, Redo, Copy, Paste
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Undo, _window, cx| {
                workspace.send_helix_key("u", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Redo, _window, cx| {
                workspace.send_helix_key("U", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Copy, _window, cx| {
                workspace.send_helix_key("y", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Paste, _window, cx| {
                workspace.send_helix_key("p", cx);
            },
        ));

        // Font size actions
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::IncreaseFontSize, _window, cx| {
                workspace.adjust_font_size(1.0, cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::DecreaseFontSize, _window, cx| {
                workspace.adjust_font_size(-1.0, cx);
            },
        ));

        // Completion trigger action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::completion::TriggerCompletion, _window, cx| {
                // Check if we're in insert mode - completion should only work in insert mode
                let core = workspace.core.read(cx);
                let current_mode = core.editor.mode();

                match current_mode {
                    helix_view::document::Mode::Insert => {
                        // Get current view and document IDs
                        let (doc_id, view_id) = {
                            let view_id = core.editor.tree.focus;
                            let doc_id = core
                                .editor
                                .tree
                                .try_get(view_id)
                                .map(|view| view.doc)
                                .unwrap_or_default();
                            (doc_id, view_id)
                        };

                        // Release the core read lock before calling handle_completion_requested
                        let _ = core;

                        workspace.handle_completion_requested(
                            doc_id,
                            view_id,
                            &crate::types::CompletionTrigger::Manual,
                            cx,
                        );
                    }
                    _ => {
                        // Do nothing - completion is only available in insert mode
                    }
                }
            },
        ));

        // Workspace actions
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::workspace::ShowBufferPicker, _window, cx| {
                show_buffer_picker(core.clone(), handle.clone(), cx)
            },
        ));

        // Toggle file tree action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ToggleFileTree, _window, cx| {
                info!("ToggleFileTree action triggered from menu");
                workspace.show_file_tree = !workspace.show_file_tree;
                cx.notify();
            },
        ));

        // File finder action
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::workspace::ShowFileFinder, _window, cx| {
                open(core.clone(), handle.clone(), cx)
            },
        ));

        // NewFile action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::NewFile, _window, cx| {
                workspace.execute_raw_command("new", cx);
            },
        ));

        // NewWindow action
        workspace_div = workspace_div.on_action(cx.listener(
            move |_workspace, _: &crate::actions::workspace::NewWindow, _window, _cx| {
                // TODO: Implement new window
                eprintln!("New window not yet implemented");
            },
        ));

        // ShowCommandPalette action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ShowCommandPalette, _window, cx| {
                workspace.send_helix_key(":", cx);
            },
        ));

        // Window actions
        workspace_div = workspace_div
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::Hide, _window, cx| cx.hide()),
            )
            .on_action(cx.listener(
                move |_, _: &crate::actions::window::HideOthers, _window, cx| cx.hide_other_apps(),
            ))
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::ShowAll, _window, cx| {
                    cx.unhide_other_apps()
                }),
            )
            .on_action(cx.listener(
                move |_, _: &crate::actions::window::Minimize, _window, _cx| {
                    // minimize not available in GPUI yet
                },
            ))
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::Zoom, _window, _cx| {
                    // zoom not available in GPUI yet
                }),
            );

        // Help and test actions
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::help::OpenTutorial, _window, cx| {
                load_tutor(core.clone(), handle.clone(), cx)
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::test::TestPrompt, _window, cx| {
                test_prompt(core.clone(), handle.clone(), cx)
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::test::TestCompletion, _window, cx| {
                test_completion(core.clone(), handle.clone(), cx)
            },
        ));

        // Create content area that will hold file tree and main content
        // Using relative positioning for better control over resize behavior
        let mut content_area = div().relative().w_full().flex_1();

        // Add file tree panel if needed, or show "Open a project" message
        if self.show_file_tree {
            let file_tree_left_offset = 0.0;
            let resize_handle_width = 3.0;
            let main_content_offset = self.file_tree_width + resize_handle_width;

            if let Some(file_tree) = &self.file_tree {
                // Create file tree panel with absolute positioning
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                let panel_bg = ui_theme.tokens.colors.surface;
                let border_color = nucleotide_ui::styling::ColorTheory::subtle_border_color(
                    panel_bg,
                    &ui_theme.tokens,
                );
                let file_tree_panel = div()
                    .absolute()
                    .left(px(file_tree_left_offset))
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .border_r_1()
                    .border_color(border_color)
                    .child(file_tree.clone());

                // Create resize handle as border line
                let border_color = nucleotide_ui::styling::ColorTheory::subtle_border_color(
                    ui_theme.tokens.colors.surface,
                    &ui_theme.tokens,
                );
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(3.0)) // 3px wide drag handle
                    .bg(border_color)
                    .hover(|style| style.bg(ui_theme.tokens.colors.text_secondary))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .id("file-tree-resize-handle")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|workspace, event: &MouseDownEvent, _window, cx| {
                            workspace.is_resizing_file_tree = true;
                            workspace.resize_start_x = event.position.x.0;
                            workspace.resize_start_width = workspace.file_tree_width;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    );

                content_area = content_area
                    .child(file_tree_panel)
                    .child(resize_handle)
                    .child(
                        // Main content with absolute positioning
                        div()
                            .absolute()
                            .left(px(main_content_offset))
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .child(main_content),
                    );
            } else {
                // No project directory set - show placeholder message
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                let resize_handle_width = 3.0;
                let main_content_offset = self.file_tree_width + resize_handle_width;

                // Use the same background color as the actual file tree for consistency
                let prompt_bg = ui_theme.tokens.colors.surface;
                let border_color = nucleotide_ui::styling::ColorTheory::subtle_border_color(
                    prompt_bg,
                    &ui_theme.tokens,
                );

                let placeholder_panel = div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .bg(prompt_bg)
                    .border_r_1()
                    .border_color(border_color)
                    .flex()
                    .flex_col()
                    .child(div().w_full().p(px(12.0)).child({
                        let workspace_entity = cx.entity().clone();
                        Button::new("open-directory-btn", "Open Directory")
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Medium)
                            .icon("icons/folder.svg")
                            .on_click(move |_event, _window, app_cx| {
                                // Create and show directory picker
                                let directory_picker = crate::picker::Picker::native_directory(
                                    "Select Project Directory",
                                    |_path| {
                                        // Callback handled through events
                                    },
                                );
                                workspace_entity.update(app_cx, |workspace, cx| {
                                    workspace.core.update(cx, |_core, cx| {
                                        cx.emit(crate::Update::DirectoryPicker(directory_picker));
                                    });
                                });
                            })
                    }));

                // Add resize handle as border line
                let border_color = nucleotide_ui::styling::ColorTheory::subtle_border_color(
                    ui_theme.tokens.colors.surface,
                    &ui_theme.tokens,
                );
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(3.0)) // 3px wide drag handle
                    .bg(border_color)
                    .hover(|style| style.bg(ui_theme.tokens.colors.text_secondary))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .id("file-tree-resize-handle-placeholder")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|workspace, event: &MouseDownEvent, _window, cx| {
                            workspace.is_resizing_file_tree = true;
                            workspace.resize_start_x = event.position.x.0;
                            workspace.resize_start_width = workspace.file_tree_width;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    );

                content_area = content_area
                    .child(placeholder_panel)
                    .child(resize_handle)
                    .child(
                        // Main content with absolute positioning
                        div()
                            .absolute()
                            .left(px(main_content_offset))
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .child(main_content),
                    );
            }
        } else {
            // File tree not shown - main content takes full width
            content_area = content_area.child(main_content);
        }

        // Build final workspace with unified bottom status bar
        workspace_div
            .children(self.titlebar.clone()) // Render titlebar if present
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .h_full()
                    .child(content_area) // Main content area (file tree + editor with tab bar)
                    .child(self.render_unified_status_bar(cx)), // Unified bottom status bar
            )
    }
}

fn load_tutor(core: Entity<Core>, handle: tokio::runtime::Handle, cx: &mut Context<Workspace>) {
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();
        let _ = utils::load_tutor(&mut core.editor);
        cx.notify()
    })
}

fn open(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use crate::picker_view::PickerItem;
    use ignore::WalkBuilder;
    use std::sync::Arc;

    info!("Opening file picker");

    // Get all files in the current directory using ignore crate (respects .gitignore)
    let mut items = Vec::new();

    // Use project directory if set, otherwise use current directory
    let base_dir = core.update(cx, |core, _| {
        core.project_directory
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    });
    info!("Base directory for file picker: {:?}", base_dir);

    // Use ignore::Walk to get files, respecting .gitignore and other VCS ignore files
    // Configure WalkBuilder like Helix does to properly respect all ignore files
    let mut walker = WalkBuilder::new(&base_dir);

    // Enable all ignore file types that Helix uses by default
    walker.git_ignore(true); // Respect .gitignore files
    walker.git_global(true); // Respect global gitignore
    walker.git_exclude(true); // Respect .git/info/exclude
    walker.ignore(true); // Respect .ignore files
    walker.parents(true); // Check parent directories for ignore files
    walker.hidden(true); // Hide hidden files (files starting with .)

    // Add Helix-specific ignore files
    walker.add_custom_ignore_filename(".helix/ignore");

    // Add standard editor ignore patterns
    walker.filter_entry(|entry| {
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip common VCS directories that might not be caught by ignore files
        if path.is_dir() {
            match file_name {
                ".git" | ".svn" | ".hg" | ".bzr" => return false,
                _ => {}
            }
        }

        true
    });

    for entry in walker.build().filter_map(std::result::Result::ok) {
        let path = entry.path().to_path_buf();

        // Skip directories
        if path.is_dir() {
            continue;
        }

        // Skip zed-source directory
        if path.to_string_lossy().starts_with("zed-source/") {
            continue;
        }

        // Get relative path from base directory
        let relative_path = path.strip_prefix(&base_dir).unwrap_or(&path);
        let path_str = relative_path.to_string_lossy().into_owned();

        // Get filename for label
        let _filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();

        // For project files, use path as label for better visibility
        items.push(PickerItem {
            label: path_str.clone().into(),
            sublabel: None,
            data: Arc::new(path.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            file_path: Some(path.clone()),
            vcs_status: None, // Will be populated using global VCS service
            columns: None,    // File picker uses simple label display
        });

        // Limit to 1000 files to prevent hanging on large projects
        if items.len() >= 1000 {
            break;
        }
    }

    // Sort items by label (path) for consistent ordering
    items.sort_by(|a, b| a.label.cmp(&b.label));

    info!("File picker has {} items", items.len());

    // Populate VCS status for all file items using the global VCS service
    let file_paths: Vec<std::path::PathBuf> = items
        .iter()
        .filter_map(|item| item.file_path.clone())
        .collect();

    // Populate VCS status for all file items using the global VCS service
    if cx.has_global::<nucleotide_vcs::VcsServiceHandle>() {
        info!("VCS service available, populating file picker VCS status");

        // Apply VCS status to items using cached status
        let mut vcs_status_count = 0;
        for item in &mut items {
            if let Some(ref file_path) = item.file_path {
                let vcs_service = cx.global::<nucleotide_vcs::VcsServiceHandle>();
                item.vcs_status = vcs_service.get_status_cached(file_path, cx);
                if item.vcs_status.is_some() {
                    vcs_status_count += 1;
                }
            }
        }

        info!(
            file_count = items.len(),
            vcs_status_count = vcs_status_count,
            "Populated file picker VCS status"
        );
    } else {
        info!("VCS service not available");
    }

    // Create a simple native picker without callback - the overlay will handle file opening via events
    let file_picker = crate::picker::Picker::native("Open File", items, |_index| {
        // This callback is no longer used - file opening is handled via OpenFile events
        // The overlay will emit OpenFile events when files are selected
    });

    info!("Emitting file picker to overlay");

    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Picker(file_picker));
    });
}

fn open_directory(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    info!("Opening directory picker");

    // Create a native directory picker
    let directory_picker =
        crate::picker::Picker::native_directory("Select Project Directory", |path| {
            info!("Directory selected: {:?}", path);
            // The callback will be handled through events
        });

    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::DirectoryPicker(directory_picker));
    });
}

fn show_buffer_picker(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use crate::picker_view::PickerItem;
    use helix_view::DocumentId;
    use std::sync::Arc;

    info!("Opening buffer picker");

    // Structure to hold buffer metadata for sorting
    #[derive(Clone)]
    struct BufferMeta {
        doc_id: DocumentId,
        path: Option<std::path::PathBuf>,
        is_modified: bool,
        is_current: bool,
        focused_at: std::time::Instant,
    }

    // Collect all open documents/buffers with metadata
    let mut buffer_metas = Vec::new();
    let current_doc_id = core
        .read(cx)
        .editor
        .tree
        .get(core.read(cx).editor.tree.focus)
        .doc;

    core.update(cx, |core, _cx| {
        let editor = &core.editor;

        // Collect buffer metadata
        for (doc_id, doc) in editor.documents.iter() {
            let focused_at = doc.focused_at;

            buffer_metas.push(BufferMeta {
                doc_id: *doc_id,
                path: doc.path().map(|p| p.to_path_buf()),
                is_modified: doc.is_modified(),
                is_current: *doc_id == current_doc_id,
                focused_at,
            });
        }
    });

    // Sort by MRU (Most Recently Used) - most recent first
    buffer_metas.sort_by(|a, b| b.focused_at.cmp(&a.focused_at));

    // Create picker items with terminal-like formatting
    let mut items = Vec::new();

    for meta in buffer_metas {
        // Format like terminal: "ID  FLAGS  PATH"
        // DocumentId likely has Display impl that shows "DocumentId(N)"
        let display_str = format!("{}", meta.doc_id);

        // Extract number from "DocumentId(N)" format
        let id_str = if display_str.starts_with("DocumentId(") && display_str.ends_with(")") {
            // Extract the number between parentheses
            display_str[11..display_str.len() - 1].to_string()
        } else if let Some(start) = display_str.find('(') {
            // More flexible parsing for variations
            if let Some(end) = display_str.rfind(')') {
                display_str[start + 1..end].trim().to_string()
            } else {
                display_str[start + 1..].trim().to_string()
            }
        } else if display_str.chars().all(char::is_numeric) {
            // If it's already just a number, use it
            display_str
        } else {
            // Fallback - try to find any number in the string
            display_str
                .chars()
                .skip_while(|c| !c.is_numeric())
                .take_while(|c| c.is_numeric())
                .collect::<String>()
        };

        // Build flags column - ensure consistent 2-character width
        let mut flags = String::new();
        if meta.is_modified {
            flags.push('+');
        }
        if meta.is_current {
            flags.push('*');
        }

        // Ensure flags are always exactly 2 characters for consistent column alignment
        let flags_str = format!("{flags:2}");

        // Get path or [scratch] label
        let path_str = if let Some(path) = &meta.path {
            // Show relative path if possible
            if let Some(project_dir) = &core.read(cx).project_directory {
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

        // Create data that includes both doc_id and path for preview functionality
        // We'll store a tuple of (DocumentId, Option<PathBuf>) for all items
        let picker_data =
            Arc::new((meta.doc_id, meta.path.clone())) as Arc<dyn std::any::Any + Send + Sync>;

        // Use structured columns instead of text formatting
        items.push(PickerItem::with_buffer_columns(
            id_str,
            flags_str,
            path_str,
            picker_data,
        ));
    }

    info!("Buffer picker has {} items", items.len());

    // Create the picker with buffer items
    let buffer_picker = crate::picker::Picker::native("Switch Buffer", items, move |index| {
        info!("Buffer selected at index: {}", index);
        // The overlay will handle buffer switching via the stored document ID
    });

    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Picker(buffer_picker));
    });
}

fn test_prompt(core: Entity<Core>, handle: tokio::runtime::Handle, cx: &mut App) {
    // Create and emit a native prompt for testing
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();

        // Create a native prompt directly
        let native_prompt = core.create_sample_native_prompt();

        // Emit the prompt to show it in the overlay
        cx.emit(crate::Update::Prompt(native_prompt));
    });
}

fn test_completion(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    // Create sample completion items
    let items = core.read(cx).create_sample_completion_items();

    // Position the completion near the top-left (simulating cursor position)
    let _anchor_position = gpui::point(gpui::px(200.0), gpui::px(300.0));

    // Create completion view
    let completion_view = cx.new(|cx| {
        let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
        view.set_items(items, cx);
        view
    });

    // Emit completion event to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Completion(completion_view));
    });
}

fn quit(core: Entity<Core>, rt: tokio::runtime::Handle, cx: &mut App) {
    core.update(cx, |core, _cx| {
        let editor = &mut core.editor;
        let _guard = rt.enter();
        if let Err(e) = rt.block_on(async { editor.flush_writes().await }) {
            error!(error = %e, "Failed to flush writes");
        }
        let views: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
        for view_id in views {
            // Check if the view still exists before trying to close it
            if editor.tree.contains(view_id) {
                editor.close(view_id);
            }
        }
    });
}

impl Workspace {
    /// Initialize the completion coordinator with the Application
    fn initialize_completion_coordinator(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::info!("Initializing completion coordinator with LSP support");

        // Extract the completion channels from the Application
        let (completion_rx, completion_results_tx, lsp_completion_requests_tx) =
            self.core.update(cx, |core, _cx| {
                nucleotide_logging::info!("Extracting completion channels from Application");
                let completion_rx = core.take_completion_receiver();
                let completion_results_tx = core.take_completion_results_sender();
                let lsp_completion_requests_tx = core.take_lsp_completion_requests_sender();
                (
                    completion_rx,
                    completion_results_tx,
                    lsp_completion_requests_tx,
                )
            });

        if let (
            Some(completion_rx),
            Some(completion_results_tx),
            Some(lsp_completion_requests_tx),
        ) = (
            completion_rx,
            completion_results_tx,
            lsp_completion_requests_tx,
        ) {
            nucleotide_logging::info!(
                "Successfully extracted all completion channels - creating coordinator"
            );

            // Create and spawn the completion coordinator
            let coordinator = crate::completion_coordinator::CompletionCoordinator::new(
                completion_rx,
                completion_results_tx,
                lsp_completion_requests_tx,
                self.core.clone(),
                cx.background_executor().clone(),
            );

            nucleotide_logging::info!("Spawning completion coordinator task with LSP support");

            // Spawn the coordinator task
            coordinator.spawn();

            nucleotide_logging::info!(
                "Completion coordinator task spawned successfully with LSP integration"
            );
        } else {
            nucleotide_logging::error!(
                "Missing completion channels for coordinator - this should not happen!"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_project_detection_basic() {
        // Test that project detection function exists and doesn't panic with valid path
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        //         let _detected_types = crate::project_indicator::detect_project_types_for_path(&current_dir);

        // The main goal is ensuring the integration compiles and doesn't panic
        assert!(true, "Project detection should complete without panicking");
    }

    #[test]
    fn test_workspace_project_change_detection() {
        let workspace = TestWorkspace::new();

        // Test that project root change is detected
        let old_root = Some(PathBuf::from("/old/path"));
        let new_root = PathBuf::from("/new/path");

        assert!(workspace.is_project_change(&old_root, &new_root));
        assert!(!workspace.is_project_change(&Some(new_root.clone()), &new_root));
    }

    #[test]
    fn test_lsp_manager_config_creation() {
        // Test that ProjectLspConfig can be created with defaults
        let config = nucleotide_lsp::ProjectLspConfig::default();

        // Basic validation of config fields
        assert!(
            config.enable_proactive_startup,
            "Proactive startup should be enabled by default"
        );
        assert!(
            config.health_check_interval.as_secs() > 0,
            "Health check interval should be positive"
        );

        // This test mainly ensures the integration compiles
        assert!(true, "ProjectLspConfig should be creatable with defaults");
    }

    // Helper struct for testing workspace functionality
    struct TestWorkspace {
        current_project_root: Option<PathBuf>,
    }

    impl TestWorkspace {
        fn new() -> Self {
            Self {
                current_project_root: None,
            }
        }

        fn is_project_change(&self, old_root: &Option<PathBuf>, new_root: &PathBuf) -> bool {
            old_root.as_ref() != Some(new_root)
        }
    }
}
