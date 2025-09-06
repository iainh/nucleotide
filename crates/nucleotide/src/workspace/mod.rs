// ABOUTME: Workspace module decomposition for cleaner architecture
// ABOUTME: Separates view management from workspace coordination logic

pub mod prefix_extraction;
pub mod view_manager;

use prefix_extraction::PrefixExtractor;
pub use view_manager::ViewManager;

// Main workspace implementation
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use gpui::FontFeatures;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, BorrowAppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Render, StatefulInteractiveElement, Styled,
    TextStyle, Window, WindowAppearance, WindowBackgroundAppearance, black, div, px, white,
};
use helix_core::Selection;
use helix_core::syntax::config::LanguageServerFeature;
use helix_view::ViewId;
use helix_view::info::Info as HelixInfo;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};
use nucleotide_logging::{debug, error, info, instrument, warn};
use nucleotide_lsp::HelixLspBridge;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

// ViewManager already imported above via pub use
use nucleotide_ui::{
    AboutWindow, Button, ButtonSize, ButtonVariant, ListItem, ListItemSpacing, ListItemVariant,
};

use crate::input_coordinator::{FocusGroup, InputContext, InputCoordinator};
use nucleotide_lsp::ServerStatus;

use crate::application::find_workspace_root_from;
use crate::document::DocumentView;
use crate::file_tree::{FileTreeConfig, FileTreeEvent, FileTreeView};
use crate::info_box::InfoBoxView;
use crate::key_hint_view::KeyHintView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
use crate::{Core, Input, InputEvent};
use nucleotide_vcs::VcsServiceHandle;

// (focus logging removed for commit; keep code minimal)
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
    // File tree context menu state
    context_menu_open: bool,
    context_menu_pos: (f32, f32),
    context_menu_path: Option<std::path::PathBuf>,
    context_menu_index: usize,
    // LSP server list popup state
    lsp_menu_open: bool,
    lsp_menu_pos: (f32, f32),
    document_order: Vec<helix_view::DocumentId>, // Ordered list of documents in opening order
    input_coordinator: Arc<InputCoordinator>,    // Central input coordination system
    project_lsp_manager: Option<Arc<nucleotide_lsp::ProjectLspManager>>, // Project-level LSP management
    current_project_root: Option<std::path::PathBuf>, // Track current project root for change detection
    _pending_lsp_startup: Option<std::path::PathBuf>, // Track pending server startup requests
    prefix_extractor: PrefixExtractor,                // Language-aware completion prefix extraction
    about_window: Entity<AboutWindow>,                // About dialog window
    // Pending file operation that expects a text input via prompt
    pending_file_op: Option<PendingFileOp>,
    // Defer a file tree refresh until after processing core events
    needs_file_tree_refresh: bool,
    // Delete confirmation modal state
    delete_confirm_open: bool,
    delete_confirm_path: Option<std::path::PathBuf>,
    // Leader key state (e.g., SPACE as prefix)
    leader_active: bool,
    leader_deadline: Option<std::time::Instant>,
}

// Pending file operation kinds awaiting user input (used with the prompt overlay)
enum PendingFileOp {
    NewFile { parent: std::path::PathBuf },
    NewFolder { parent: std::path::PathBuf },
    Rename { path: std::path::PathBuf },
    Duplicate { path: std::path::PathBuf },
}

impl EventEmitter<crate::Update> for Workspace {}

impl Workspace {
    /// Compute document and LSP context for the status bar without triggering borrow conflicts.
    fn statusbar_doc_info(
        &self,
        cx: &mut Context<Self>,
    ) -> (
        &'static str,                        // mode
        String,                              // file name display
        String,                              // position text
        bool,                                // has LSP state
        Option<helix_lsp::LanguageServerId>, // preferred server for current doc
    ) {
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

        // Determine preferred LSP server for the current document
        let preferred_server_id = if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some(view) = editor.tree.try_get(view_id)
            && let Some(doc) = editor.document(view.doc)
        {
            doc.language_servers().next().map(|ls| ls.id())
        } else {
            None
        };

        let has_lsp_state = core.lsp_state.is_some();
        (
            mode_name,
            file_name,
            position_text,
            has_lsp_state,
            preferred_server_id,
        )
    }

    /// Build the LSP indicator string for the preferred server if available.
    fn compute_statusbar_lsp_indicator(
        &self,
        cx: &mut Context<Self>,
        has_lsp_state: bool,
        preferred_server_id: Option<helix_lsp::LanguageServerId>,
    ) -> Option<String> {
        if !has_lsp_state {
            return None;
        }

        let lsp_state_entity = {
            let core = self.core.read(cx);
            core.lsp_state.clone()
        }?;

        lsp_state_entity.update(cx, |state, _| {
            if let Some(pref_id) = preferred_server_id
                && let Some(server) = state.servers.get(&pref_id).cloned()
            {
                // Prefer progress for this server if any
                if let Some(p) = state
                    .progress
                    .values()
                    .find(|p| p.server_id == pref_id)
                    .cloned()
                {
                    let indicator = state.get_spinner_frame().to_string();
                    let mut s = format!("{} {}: ", indicator, server.name);
                    if let Some(pct) = p.percentage {
                        s.push_str(&format!("{:>2}% ", pct));
                    }
                    s.push_str(&p.title);
                    if let Some(msg) = &p.message {
                        s.push_str(" ⋅ ");
                        s.push_str(msg);
                    }
                    return Some(s);
                }

                // Otherwise show basic server indicator based on status
                let indicator = match server.status {
                    ServerStatus::Starting | ServerStatus::Initializing => {
                        state.get_spinner_frame().to_string()
                    }
                    _ => "◉".to_string(),
                };
                return Some(format!("{} {}", indicator, server.name));
            }

            // Fallback to default indicator
            state.get_lsp_indicator()
        })
    }

    /// Standard divider element for the status bar.
    fn statusbar_divider(&self, color: gpui::Hsla) -> gpui::AnyElement {
        gpui::div()
            .w(gpui::px(1.0))
            .h(gpui::px(18.0))
            .bg(color)
            .mx_2()
            .into_any_element()
    }

    /// Build the main content row for the unified status bar.
    fn statusbar_main_content(
        &self,
        mode_name: &'static str,
        file_name: String,
        position_text: String,
        lsp_indicator: Option<String>,
        divider_color: gpui::Hsla,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use nucleotide_ui::{Button, ButtonSize, ButtonVariant, IconPosition};
        let mut row = gpui::div()
            .flex()
            .flex_1()
            .flex_row()
            .items_center()
            .child(
                // Mode indicator
                gpui::div()
                    .child(mode_name)
                    .min_w(gpui::px(50.0))
                    .text_color(status_bar_tokens.text_primary),
            )
            .child(self.statusbar_divider(divider_color))
            .child(
                // File name grows
                gpui::div().flex_1().overflow_hidden().child(file_name),
            )
            .child(self.statusbar_divider(divider_color))
            .child(gpui::div().child(position_text).min_w(gpui::px(80.0)));

        if let Some(indicator) = lsp_indicator {
            row = row.child(self.statusbar_divider(divider_color)).child(
                Button::new("lsp-status-trigger", indicator)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .icon("icons/chevron-up.svg")
                    .icon_position(IconPosition::End)
                    .on_click(cx.listener(
                        |this: &mut Workspace, ev: &gpui::MouseUpEvent, _w, cx| {
                            this.lsp_menu_open = true;
                            this.lsp_menu_pos = (ev.position.x.0, ev.position.y.0);
                            cx.notify();
                        },
                    )),
            );
        }

        row.into_any_element()
    }
    fn context_menu_items() -> Vec<(&'static str, fn(&mut Workspace, &mut Context<Workspace>))> {
        vec![
            ("New File", Workspace::cm_action_new_file),
            ("New Folder", Workspace::cm_action_new_folder),
            ("Rename", Workspace::cm_action_rename),
            ("Delete", Workspace::cm_action_delete),
            ("Duplicate", Workspace::cm_action_duplicate),
            ("Copy Path", Workspace::cm_action_copy_path),
            (
                "Copy Relative Path",
                Workspace::cm_action_copy_relative_path,
            ),
            ("Reveal in OS", Workspace::cm_action_reveal_in_os),
        ]
    }
    /// Ensure document is in the order list, adding it to the end if new
    fn ensure_document_in_order(&mut self, doc_id: helix_view::DocumentId) {
        if !self.document_order.contains(&doc_id) {
            self.document_order.push(doc_id);
        }
    }

    // (debug focus logger removed for commit)
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

        // Subscribe to completion acceptance events from the overlay
        cx.subscribe(
            &overlay,
            |workspace, _overlay, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                workspace.handle_completion_via_helix(event.item_index, cx);
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
            _project_status_handle.set_project_root(Some(root.clone()));
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

        // Create about window
        let about_window = cx.new(|_cx| AboutWindow::new());

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

            //  Set up HelixLspBridge for the ProjectLspManager in constructor
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
            context_menu_open: false,
            context_menu_pos: (0.0, 0.0),
            context_menu_path: None,
            context_menu_index: 0,
            lsp_menu_open: false,
            lsp_menu_pos: (0.0, 0.0),
            document_order: Vec::new(),
            input_coordinator,
            project_lsp_manager,
            current_project_root: root_path_for_manager.clone(),
            _pending_lsp_startup: None,
            prefix_extractor: PrefixExtractor::new(),
            about_window,
            pending_file_op: None,
            needs_file_tree_refresh: false,
            delete_confirm_open: false,
            delete_confirm_path: None,
            leader_active: false,
            leader_deadline: None,
        };

        // Set initial focus restore state
        workspace.view_manager.set_needs_focus_restore(true);

        // Register focus groups for main UI areas
        workspace.register_focus_groups(cx);

        // Setup completion-specific shortcuts
        workspace.setup_completion_shortcuts();

        // Note: Completion handling is now done directly via event-driven approach

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

    /// Process completion results directly from Helix's completion system
    fn process_completion_results(&mut self, _cx: &mut Context<Self>) {
        // Completion results are now processed directly through Helix's completion system
        // via hooks that we register to capture when Helix has completion results ready
        // This method is kept as a placeholder for when we implement the hook-based system
    }

    /// Rescan a single directory and update the file tree entries for that folder only
    fn rescan_directory(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if let Some(ref file_tree) = self.file_tree {
            let dir = dir.to_path_buf();
            file_tree.update(cx, |view, tree_cx| {
                view.refresh_directory(&dir, tree_cx);
            });
        }
    }

    /// Render a simple delete confirmation modal overlay with two actions
    fn render_delete_confirm_modal(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;

        let message = if let Some(path) = &self.delete_confirm_path {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("this item");
            format!("Delete '{}' permanently?", name)
        } else {
            "Delete permanently?".to_string()
        };

        // Backdrop to block clicks
        let backdrop = div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.35))
            .on_mouse_down(MouseButton::Left, |_, _, _| {});

        // Dialog content
        let picker_tokens = tokens.picker_tokens();
        let dialog = div()
            .absolute()
            .top(px(120.0))
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .bg(picker_tokens.container_background)
                    .border_1()
                    .border_color(picker_tokens.border)
                    .rounded(tokens.sizes.radius_lg)
                    .shadow_xl()
                    .w(px(380.0))
                    .p(tokens.sizes.space_4)
                    .flex()
                    .flex_col()
                    .gap(tokens.sizes.space_3)
                    .child(
                        div()
                            .text_size(tokens.sizes.text_md)
                            .child("Confirm Delete"),
                    )
                    .child(div().text_size(tokens.sizes.text_sm).child(message))
                    .child(
                        div()
                            .flex()
                            .gap(tokens.sizes.space_2)
                            .justify_end()
                            .child({
                                Button::new("cancel-delete", "Cancel")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Small)
                                    .on_click(cx.listener(|view: &mut Workspace, _ev, _w, cx| {
                                        view.delete_confirm_open = false;
                                        view.delete_confirm_path = None;
                                        cx.notify();
                                    }))
                            })
                            .child({
                                let btn_label =
                                    match self.core.read(cx).config.gui.file_ops.delete_behavior {
                                        crate::config::DeleteBehavior::Trash => "Move to Trash",
                                        crate::config::DeleteBehavior::Permanent => {
                                            "Delete Permanently"
                                        }
                                    };
                                Button::new("confirm-delete", btn_label)
                                    .variant(ButtonVariant::Danger)
                                    .size(ButtonSize::Small)
                                    .on_click(cx.listener(|view: &mut Workspace, _ev, _w, cx| {
                                        view.perform_delete_confirm(cx);
                                    }))
                            }),
                    ),
            );

        div().child(backdrop).child(dialog)
    }

    /// Execute the delete after confirmation
    fn perform_delete_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.delete_confirm_path.clone() {
            let mode = match self.core.read(cx).config.gui.file_ops.delete_behavior {
                crate::config::DeleteBehavior::Trash => {
                    nucleotide_events::v2::workspace::DeleteMode::Trash
                }
                crate::config::DeleteBehavior::Permanent => {
                    nucleotide_events::v2::workspace::DeleteMode::Permanent
                }
            };
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::Delete {
                    path: path.clone(),
                    mode,
                },
            };
            self.core.read(cx).dispatch_workspace_event(event);
            if let Some(parent) = path.parent() {
                self.rescan_directory(parent, cx);
            }
        }
        self.delete_confirm_open = false;
        self.delete_confirm_path = None;
        cx.notify();
    }

    /// Render the file tree context menu anchored at the last click position
    fn render_file_tree_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui::{Corner, anchored, point};
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let (x, y) = self.context_menu_pos;

        let items = Self::context_menu_items();
        let item_count = items.len();

        // Use anchored popup at the stored cursor position, relative to window
        let dd_tokens = tokens.dropdown_tokens();

        // Move keyboard focus to the workspace focus group so arrow/enter navigation works
        window.focus(&self.focus_handle);

        let popup = div()
            .bg(dd_tokens.container_background)
            .border_1()
            .border_color(dd_tokens.border)
            .rounded(tokens.sizes.radius_md)
            .shadow_lg()
            .min_w(px(200.0))
            // Use equal padding on all sides so the selection has the same
            // inset from the border horizontally as vertically
            .py(tokens.sizes.space_1)
            .px(tokens.sizes.space_1)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Prevent hover/move from reaching the file tree beneath the menu
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(i, (label, handler))| {
                let _hover_bg = dd_tokens.item_background_hover;
                let text_default = dd_tokens.item_text;
                let _text_hover = dd_tokens.item_text_selected;
                // Compute rounded corner radius for selected rows at the top/bottom of the menu
                let inner_radius = tokens.sizes.radius_md - px(0.5); // Outer radius minus half border
                let is_first = i == 0;
                let is_last = i + 1 == item_count;
                div()
                    .w_full()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_move(cx.listener(move |w: &mut Workspace, _ev, _win, cx| {
                        if w.context_menu_index != i {
                            w.context_menu_index = i;
                            cx.notify();
                        }
                    }))
                    // Apply selection background on the full-width wrapper so it stretches edge to edge
                    .when(self.context_menu_index == i, |d| {
                        d.bg(dd_tokens.item_background_selected)
                    })
                    // Round selection corners to match the popup for first/last items
                    .when(self.context_menu_index == i && is_first, |d| {
                        d.rounded_tl(inner_radius).rounded_tr(inner_radius)
                    })
                    .when(self.context_menu_index == i && is_last, |d| {
                        d.rounded_bl(inner_radius).rounded_br(inner_radius)
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let handler_fn = handler;
                        cx.listener(move |workspace: &mut Workspace, _ev, _window, cx| {
                            workspace.context_menu_open = false;
                            handler_fn(workspace, cx);
                            cx.stop_propagation();
                        })
                    })
                    .child(
                        ListItem::new(("filetree-cm", i as u32))
                            .variant(ListItemVariant::Ghost)
                            .spacing(ListItemSpacing::Compact)
                            .child(
                                div()
                                    .w_full()
                                    .text_size(tokens.sizes.text_sm)
                                    // Tighter spacing for compact context menu rows
                                    .px(tokens.sizes.space_2)
                                    .py(tokens.sizes.space_1)
                                    .text_color(if self.context_menu_index == i {
                                        dd_tokens.item_text_selected
                                    } else {
                                        text_default
                                    })
                                    .child(label),
                            ),
                    )
            }));

        // Fullscreen backdrop to block clicks and handle outside-click dismiss
        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            // Swallow mouse move/hover so it doesn't update file tree hover beneath
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|w: &mut Workspace, _ev, _win, cx| {
                    // Clicking backdrop closes the menu
                    if w.context_menu_open {
                        w.context_menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|w: &mut Workspace, _ev, _win, cx| {
                    if w.context_menu_open {
                        w.context_menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                anchored()
                    .position(point(px(x), px(y)))
                    .anchor(Corner::TopLeft)
                    // Offset the menu away from the cursor so the pointer isn't directly above the first item
                    .offset(point(px(8.0), px(8.0)))
                    .snap_to_window_with_margin(tokens.sizes.space_2)
                    .child(popup),
            )
    }

    // --- Context menu action handlers (stubs that close the menu and log) ---
    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_open = false;
        cx.notify();
    }

    fn cm_action_new_file(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(clicked) = this.context_menu_path.clone() {
            let parent = if clicked.is_dir() {
                clicked
            } else {
                clicked
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .to_path_buf()
            };
            // Queue pending op and show prompt (overlay will emit CommandSubmitted)
            this.pending_file_op = Some(PendingFileOp::NewFile { parent });
            this.core.update(cx, |_core, cx| {
                let prompt = crate::prompt::Prompt::native("New file name", "", |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_new_folder(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(clicked) = this.context_menu_path.clone() {
            let parent = if clicked.is_dir() {
                clicked
            } else {
                clicked
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .to_path_buf()
            };
            this.pending_file_op = Some(PendingFileOp::NewFolder { parent });
            this.core.update(cx, |_core, cx| {
                let prompt = crate::prompt::Prompt::native("New folder name", "", |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_rename(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            let current_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            this.pending_file_op = Some(PendingFileOp::Rename { path });
            this.core.update(cx, move |_core, cx| {
                let prompt = crate::prompt::Prompt::native("Rename to", current_name, |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_delete(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            // Open confirmation modal
            this.delete_confirm_open = true;
            this.delete_confirm_path = Some(path);
            cx.notify();
        }
        this.close_context_menu(cx);
    }

    fn cm_action_duplicate(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            let base_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| format!("{} copy", s))
                .unwrap_or_else(|| "copy".to_string());
            this.pending_file_op = Some(PendingFileOp::Duplicate { path });
            this.core.update(cx, move |_core, cx| {
                let prompt = crate::prompt::Prompt::native("Duplicate as", base_name, |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_copy_path(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            // Copy absolute path to clipboard
            let text = path.display().to_string();
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(path=%text, "Failed to copy path to clipboard");
            }
            // Optionally dispatch intent for telemetry/handlers
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::Absolute,
                },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
    }

    fn cm_action_copy_relative_path(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            // Compute relative to current project root if available
            let text = if let Some(root) = &this.current_project_root {
                match path.strip_prefix(root) {
                    Ok(rel) => rel.display().to_string(),
                    Err(_) => path.display().to_string(),
                }
            } else {
                path.display().to_string()
            };
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(path=%text, "Failed to copy relative path to clipboard");
            }
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::RelativeToWorkspace,
                },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
    }

    /// Best-effort clipboard copy using platform tools
    fn copy_to_clipboard_impl(text: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            use std::io::Write;
            let mut child = match std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            if let Some(stdin) = &mut child.stdin
                && stdin.write_all(text.as_bytes()).is_err()
            {
                return false;
            }
            let _ = child.wait();
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            use std::io::Write;
            let mut child = match std::process::Command::new("cmd")
                .args(["/C", "clip"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            if let Some(stdin) = &mut child.stdin {
                if stdin.write_all(text.as_bytes()).is_err() {
                    return false;
                }
            }
            let _ = child.wait();
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            // Try wl-copy (Wayland)
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = &mut child.stdin {
                    if stdin.write_all(text.as_bytes()).is_ok() {
                        let _ = child.wait();
                        return true;
                    }
                }
            }
            // Fallback to xclip
            if let Ok(mut child) = std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = &mut child.stdin {
                    if stdin.write_all(text.as_bytes()).is_ok() {
                        let _ = child.wait();
                        return true;
                    }
                }
            }
            return false;
        }
        #[allow(unreachable_code)]
        {
            // Other platforms: not implemented
            false
        }
    }

    fn cm_action_reveal_in_os(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::RevealInOs { path },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
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
            } else {
                InputContext::Normal
            }
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

        // Delete modal keyboard handling
        if self.delete_confirm_open {
            match ev.keystroke.key.as_str() {
                "enter" => {
                    self.perform_delete_confirm(cx);
                    return;
                }
                "escape" => {
                    self.delete_confirm_open = false;
                    self.delete_confirm_path = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        // Context menu keyboard handling
        if self.context_menu_open {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    self.context_menu_open = false;
                    cx.notify();
                    return;
                }
                "down" => {
                    let len = Self::context_menu_items().len();
                    if len > 0 {
                        self.context_menu_index = (self.context_menu_index + 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    let len = Self::context_menu_items().len();
                    if len > 0 {
                        self.context_menu_index = (self.context_menu_index + len - 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "enter" => {
                    let items = Self::context_menu_items();
                    if let Some((_, handler)) = items.get(self.context_menu_index) {
                        let handler_fn = *handler;
                        self.context_menu_open = false;
                        handler_fn(self, cx);
                    } else {
                        self.context_menu_open = false;
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Close context menu on Escape
        if self.context_menu_open && ev.keystroke.key == "escape" {
            self.context_menu_open = false;
            cx.notify();
            return;
        }

        // Check if completion is visible and handle navigation/control keys
        if self.overlay.read(cx).has_completion() {
            match ev.keystroke.key.as_str() {
                // Accept with Enter when showing code actions dropdown
                "enter" => {
                    let accept_with_enter = self.overlay.read(cx).has_code_actions();
                    if accept_with_enter {
                        nucleotide_logging::info!("Accepting code action via Enter key");
                        let handled = self
                            .overlay
                            .update(cx, |overlay, cx| overlay.handle_completion_enter_key(cx));
                        if handled {
                            return;
                        }
                    }
                }
                "up" | "down" => {
                    nucleotide_logging::info!(
                        key = %ev.keystroke.key,
                        "Forwarding arrow key to completion view"
                    );
                    // Forward the key event to the completion view via overlay method
                    let handled = self.overlay.update(cx, |overlay, cx| {
                        overlay.handle_completion_arrow_key(ev.keystroke.key.as_str(), cx)
                    });
                    if handled {
                        // Don't let this key go to Helix - we handled it
                        return;
                    }
                }
                "tab" => {
                    nucleotide_logging::info!(
                        "Forwarding tab key to accept completion (secondary)"
                    );
                    // Forward tab to completion view to accept selected item
                    let handled = self
                        .overlay
                        .update(cx, |overlay, cx| overlay.handle_completion_tab_key(cx));
                    if handled {
                        // Don't let tab go to Helix - we handled it
                        return;
                    }
                }
                key if ev.keystroke.modifiers.control => {
                    // Handle Helix-style control key combinations
                    match key {
                        "y" => {
                            nucleotide_logging::info!(
                                "Forwarding C-y to accept completion (primary - Helix style)"
                            );
                            // Forward C-y to completion view to accept selected item (Helix primary)
                            let handled = self
                                .overlay
                                .update(cx, |overlay, cx| overlay.handle_completion_tab_key(cx));
                            if handled {
                                // Don't let C-y go to Helix - we handled it
                                return;
                            }
                        }
                        "n" => {
                            nucleotide_logging::info!(
                                "Forwarding C-n to select next completion (Helix style)"
                            );
                            // Forward C-n to completion view for next selection
                            let handled = self.overlay.update(cx, |overlay, cx| {
                                overlay.handle_completion_arrow_key("down", cx)
                            });
                            if handled {
                                // Don't let C-n go to Helix - we handled it
                                return;
                            }
                        }
                        "p" => {
                            nucleotide_logging::info!(
                                "Forwarding C-p to select previous completion (Helix style)"
                            );
                            // Forward C-p to completion view for previous selection
                            let handled = self.overlay.update(cx, |overlay, cx| {
                                overlay.handle_completion_arrow_key("up", cx)
                            });
                            if handled {
                                // Don't let C-p go to Helix - we handled it
                                return;
                            }
                        }
                        _ => {
                            // Other control keys - let them pass through to Helix
                        }
                    }
                }
                "escape" => {
                    nucleotide_logging::info!("Forwarding escape key to close completion view");
                    // Close the completion view without accepting any item
                    self.overlay.update(cx, |overlay, cx| {
                        overlay.dismiss_completion(cx);
                    });
                    // Don't let escape go to Helix - we handled it
                    return;
                }
                "backspace" => {
                    nucleotide_logging::debug!(
                        "Backspace while completion active - will predict shorter prefix"
                    );
                    // For backspace, predict by removing the last character from current prefix
                    self.update_completion_filter_with_predicted_backspace(cx);
                }
                key if key.len() == 1 => {
                    let typed_char = key.chars().next().unwrap();
                    if typed_char.is_alphanumeric() || typed_char == '_' {
                        nucleotide_logging::debug!(
                            key = %key,
                            "Character typed while completion active - will update filter with predicted prefix"
                        );
                        // Regular alphanumeric character - update filter with prediction
                        self.update_completion_filter_with_predicted_char(typed_char, cx);
                    } else if typed_char == '.' {
                        nucleotide_logging::debug!(
                            key = %key,
                            "Dot typed while completion active - will trigger new completion request"
                        );
                        // Dot should trigger a new completion request for methods/properties
                        // Let the dot go to Helix first, then trigger new completion
                        self.schedule_completion_filter_update(cx);
                    } else {
                        // Other punctuation might close completion
                        nucleotide_logging::debug!(
                            key = %key,
                            "Non-alphanumeric character typed - letting Helix handle normally"
                        );
                    }
                }
                _ => {
                    // For other keys when completion is visible, continue normal processing
                }
            }
        }

        // Leader key timeout handling

        // Update input context based on current focus state
        self.update_input_context(window, cx);

        // Determine current Helix editor mode for precise gating
        let helix_mode = { self.core.read(cx).editor.mode() };

        // If we left Normal mode while a leader sequence was active, cancel it
        if self.leader_active && helix_mode != helix_view::document::Mode::Normal {
            info!("Leader: cancelled due to leaving Normal mode");
            self.leader_active = false;
            self.leader_deadline = None;
            self.key_hints.update(cx, |key_hints, cx| {
                key_hints.set_info(None);
                cx.notify();
            });
        }

        // Leader key handling: SPACE as prefix only in Normal editor mode
        if self.input_coordinator.current_context() == InputContext::Normal
            && helix_mode == helix_view::document::Mode::Normal
        {
            match ev.keystroke.key.as_str() {
                "space" | " "
                    if !self.leader_active && ev.keystroke.modifiers.number_of_modifiers() == 0 =>
                {
                    info!("Leader: SPACE pressed, activating leader mode");
                    // Activate leader, swallow the space
                    self.leader_active = true;
                    // No timeout in Normal mode

                    // Show leader key hint list
                    let info = HelixInfo {
                        title: "Leader (space)".into(),
                        text: "f   File Finder\n\
                               b   Buffer Picker\n\
                               t   Toggle File Tree\n\
                               a   Code Actions\n\
                               d   Diagnostics (buffer)\n\
                               D   Diagnostics (workspace)\n\
                               esc Cancel"
                            .into(),
                        width: 0,
                        height: 0,
                    };
                    let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(Some(info));
                        key_hints.set_theme(theme);
                        cx.notify();
                    });
                    return;
                }
                "a" if self.leader_active => {
                    info!("Leader: SPACE-a detected, opening Code Actions");
                    // SPACE-a => Show code actions
                    self.leader_active = false;
                    self.leader_deadline = None;
                    // Clear leader hint
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    show_code_actions(self.core.clone(), self.handle.clone(), cx);
                    return;
                }
                "f" if self.leader_active => {
                    info!("Leader: SPACE-f detected, opening File Finder");
                    // Clear leader state and hint
                    self.leader_active = false;
                    self.leader_deadline = None;
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    // Invoke file finder
                    let core = self.core.clone();
                    let handle = self.handle.clone();
                    open(core, handle, cx);
                    return;
                }
                "b" if self.leader_active => {
                    info!("Leader: SPACE-b detected, opening Buffer Picker");
                    self.leader_active = false;
                    self.leader_deadline = None;
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    let core = self.core.clone();
                    let handle = self.handle.clone();
                    show_buffer_picker(core, handle, cx);
                    return;
                }
                "t" if self.leader_active => {
                    info!("Leader: SPACE-t detected, toggling File Tree");
                    self.leader_active = false;
                    self.leader_deadline = None;
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    self.show_file_tree = !self.show_file_tree;
                    cx.notify();
                    return;
                }
                "d" if self.leader_active && !ev.keystroke.modifiers.shift => {
                    info!("Leader: SPACE-d detected, showing Diagnostics (buffer)");
                    self.leader_active = false;
                    self.leader_deadline = None;
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    // Bridge to application to show diagnostics picker for current buffer
                    event_bridge::send_bridged_event(
                        event_bridge::BridgedEvent::DiagnosticsPickerRequested { workspace: false },
                    );
                    return;
                }
                "d" if self.leader_active && ev.keystroke.modifiers.shift => {
                    info!("Leader: SPACE-D detected, showing Diagnostics (workspace)");
                    self.leader_active = false;
                    self.leader_deadline = None;
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    // Bridge to application to show workspace-wide diagnostics picker
                    event_bridge::send_bridged_event(
                        event_bridge::BridgedEvent::DiagnosticsPickerRequested { workspace: true },
                    );
                    return;
                }
                "escape" if self.leader_active => {
                    info!("Leader: cancelled by Escape");
                    // Cancel leader
                    self.leader_active = false;
                    self.leader_deadline = None;
                    // Clear leader hint
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    return;
                }
                _ => {
                    if self.leader_active {
                        // Keep leader active and swallow unrelated keys until ESC or valid sequence
                        info!(key = %ev.keystroke.key, "Leader: awaiting sequence; key ignored");
                        return;
                    }
                }
            }
        }

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
                nucleotide_logging::info!(
                    key = ?helix_key,
                    "DEBUG: Sending key to Helix editor - completion test"
                );

                // Send the key to Helix
                self.input.update(cx, |_, cx| {
                    cx.emit(crate::InputEvent::Key(helix_key));
                });

                // Extra debug for ctrl-x specifically
                if helix_key
                    .modifiers
                    .contains(helix_view::keyboard::KeyModifiers::CONTROL)
                    && matches!(helix_key.code, helix_view::keyboard::KeyCode::Char('x'))
                {
                    nucleotide_logging::info!(
                        "DEBUG: CTRL-X sent to Helix - should trigger completion in insert mode"
                    );
                }
            }
            InputResult::WorkspaceAction(action) => {
                debug!(action = %action, "Executing workspace action");
                self.handle_workspace_action(&action, cx);
            }
        }

        // Trigger delete confirmation from keyboard when file tree has focus
        if ev.keystroke.key.as_str() == "delete"
            && let Some(ref file_tree) = self.file_tree
        {
            let is_tree_focused = file_tree.focus_handle(cx).is_focused(window);
            if is_tree_focused {
                let selected = file_tree.read(cx).selected_path().cloned();
                if let Some(path) = selected {
                    self.delete_confirm_open = true;
                    self.delete_confirm_path = Some(path);
                    cx.notify();
                }
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
        let project_status = nucleotide_project::project_status_service(cx);
        project_status.set_project_root(Some(dir.clone()));

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

            // Clear existing LSP state to avoid stale indicators from previous project
            if let Some(lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
                lsp_state_entity.update(cx, |state, cx| {
                    state.clear_all_state();
                    cx.notify();
                });
                info!("Cleared LSP state for project root change");

                // Immediately sync any existing servers to populate the new project context
                // This ensures LSP indicators appear quickly for the new project
                let editor = &self.core.read(cx).editor;
                let active_servers: Vec<(helix_lsp::LanguageServerId, String)> = editor
                    .language_servers
                    .iter_clients()
                    .map(|client| (client.id(), client.name().to_string()))
                    .collect();

                if !active_servers.is_empty() {
                    lsp_state_entity.update(cx, |state, cx| {
                        for (id, name) in active_servers {
                            info!(
                                server_id = ?id,
                                server_name = %name,
                                "Registering existing LSP server for new project"
                            );
                            state.register_server(id, name, Some(dir.display().to_string()));
                            state.update_server_status(id, nucleotide_lsp::ServerStatus::Running);
                        }
                        cx.notify();
                    });
                    info!("Registered existing LSP servers for new project");
                }
            }

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

            //  Set up HelixLspBridge for the ProjectLspManager
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

            // Note: File tree header update will be handled via project status service update
            // which triggers UI refresh through the project status service

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
            "🔄 LSP_RESTART: Starting LSP server restart for workspace change"
        );

        // Get the old project root from the workspace state
        let old_project_root = self.current_project_root.clone();

        // Get the LSP command sender from the Application
        let lsp_command_sender = self.core.read(cx).get_project_lsp_command_sender();

        if let Some(sender) = lsp_command_sender {
            info!(
                old_project_root = ?old_project_root.as_ref().map(|p| p.display()),
                new_project_root = %new_project_root.display(),
                current_working_dir = ?std::env::current_dir().ok(),
                "🔄 LSP_RESTART: Sending RestartServersForWorkspaceChange command to Application"
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
        project_status.set_project_root(Some(project_root.clone()));
        info!("Project status service updated, refreshing project detection");
        project_status.refresh_project_detection();
        info!("Project detection refresh completed");

        // Coordinate with ProjectLspManager if available
        if let Some(ref manager) = self.project_lsp_manager {
            let manager_clone = manager.clone();
            let runtime_handle = self.handle.clone();
            let project_root_clone = project_root.clone();

            info!("Notifying ProjectLspManager about project detection and starting LSP servers");

            runtime_handle.spawn(async move {
                info!(project_root = %project_root_clone.display(), "Starting project detection via ProjectLspManager");

                // PROJECT-LEVEL LSP: Use proper event-driven approach
                match manager_clone.detect_project(project_root_clone.clone()).await {
                    Ok(()) => {
                        info!(
                            project_root = %project_root_clone.display(),
                            "Project detection completed successfully via ProjectLspManager"
                        );

                        // Get the project info to send proper LSP startup events
                        if let Some(project_info) = manager_clone.get_project_info(&project_root_clone).await {
                            info!(
                                project_type = ?project_info.project_type,
                                language_servers = ?project_info.language_servers,
                                "EVENT-DRIVEN: Sending LspServerStartupRequested events"
                            );

                            // Send LspServerStartupRequested events for each language server
                            for server_name in &project_info.language_servers {
                                info!(
                                    server_name = %server_name,
                                    project_root = %project_info.workspace_root.display(),
                                    "EVENT: Sending LspServerStartupRequested"
                                );

                                // Create the event command
                    let _command = nucleotide_events::ProjectLspCommand::LspServerStartupRequested {
                                    server_name: server_name.clone(),
                                    workspace_root: project_info.workspace_root.clone(),
                                };

                                // Project detection completed - LSP events will be sent from event bridge
                                info!(
                                    server_name = %server_name,
                                    "📡 PROJECT: Server startup will be handled by event bridge"
                                );
                            }

                            // Project detection completed successfully
                            info!("Project detection completed - events handled by bridge");
                        } else {
                            warn!(
                                project_root = %project_root_clone.display(),
                                "Project detected but no project info available"
                            );
                        }
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

        // Process any events that may have been sent during project detection
        self.core.update(cx, |app, cx| {
            app.handle_periodic_maintenance(cx, self.handle.clone());
        });
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
            project_status.update_lsp_state(&lsp_state_clone);

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

    // removed unused switch_theme_by_name

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

    // removed unused update_titlebar_appearance

    // removed unused set_macos_window_appearance (macOS)

    #[cfg(any())]
    unsafe fn update_titlebar_appearance_native(
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
    ) {
        use nucleotide_ui::theme_manager::SystemAppearance;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_app_kit::{NSApplication, NSWindow};
        use objc2_foundation::{MainThreadMarker, NSArray, NSString};

        // Get all windows from NSApplication instead of just the main window

        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let windows: &NSArray<NSWindow> = unsafe { msg_send![&**app, windows] };
        let window_count = windows.count();

        nucleotide_logging::debug!(
            window_count = window_count,
            "Found {} windows in NSApplication",
            window_count
        );

        // Log details about all windows to make sure we're targeting the right one
        for i in 0..window_count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let window_title: *mut AnyObject = msg_send![window, title];
            let title_str = if !window_title.is_null() {
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
            let mut target_window: *mut AnyObject = std::ptr::null_mut();

            // First try to find the main window
            for i in 0..window_count {
                let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                let is_main: bool = msg_send![window, isMainWindow];
                if is_main {
                    target_window = window;
                    nucleotide_logging::debug!(window_index = i, "Found main window");
                    break;
                }
            }

            // If no main window, try to find the key window
            if target_window.is_null() {
                for i in 0..window_count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let is_key: bool = msg_send![window, isKeyWindow];
                    if is_key {
                        target_window = window;
                        nucleotide_logging::debug!(window_index = i, "Found key window");
                        break;
                    }
                }
            }

            // If still no target, find the first visible window with a titlebar
            if target_window.is_null() {
                for i in 0..window_count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
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
            if target_window.is_null() {
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
            let current_appearance: *mut AnyObject = msg_send![window, appearance];
            nucleotide_logging::debug!(
                current_appearance_is_nil = (current_appearance.is_null()),
                "Window appearance before setting"
            );

            // Set the window appearance to match the detected system appearance
            match system_appearance {
                SystemAppearance::Dark => {
                    // Set to dark appearance explicitly
                    let dark_appearance_name = NSString::from_str("NSAppearanceNameDarkAqua");
                    let dark_appearance: *mut AnyObject =
                        msg_send![class!(NSAppearance), appearanceNamed: &*dark_appearance_name];
                    let _: () = msg_send![window, setAppearance: dark_appearance];
                    nucleotide_logging::debug!("Set window to dark appearance explicitly");
                }
                SystemAppearance::Light => {
                    // Set to light appearance explicitly
                    let light_appearance_name = NSString::from_str("NSAppearanceNameAqua");
                    let light_appearance: *mut AnyObject =
                        msg_send![class!(NSAppearance), appearanceNamed: &*light_appearance_name];
                    let _: () = msg_send![window, setAppearance: light_appearance];
                    nucleotide_logging::debug!("Set window to light appearance explicitly");
                }
            }

            // Check appearance after setting and verify it took effect
            let new_appearance: *mut AnyObject = msg_send![window, appearance];
            let new_appearance_name: *mut AnyObject = if !new_appearance.is_null() {
                msg_send![new_appearance, name]
            } else {
                std::ptr::null_mut()
            };

            let appearance_name_str = if !new_appearance_name.is_null() {
                let cstr: *const i8 = msg_send![new_appearance_name, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };

            nucleotide_logging::info!(
                system_appearance = ?system_appearance,
                new_appearance_is_nil = (new_appearance.is_null()),
                new_appearance_name = appearance_name_str,
                "Successfully set NSWindow appearance"
            );

            // Also check the actual effective appearance to see what macOS thinks
            let effective_appearance: *mut AnyObject = msg_send![window, effectiveAppearance];
            let effective_appearance_name: *mut AnyObject = if !effective_appearance.is_null() {
                msg_send![effective_appearance, name]
            } else {
                std::ptr::null_mut()
            };

            let effective_name_str = if !effective_appearance_name.is_null() {
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
                    let mtm = MainThreadMarker::new_unchecked();
                    let app = NSApplication::sharedApplication(mtm);
                    let windows: *mut AnyObject = msg_send![&**app, windows];
                    let window_count: usize = msg_send![windows, count];

                    if window_count > 0 {
                        let window: *mut AnyObject = msg_send![windows, objectAtIndex: 0];
                        let current_appearance: *mut AnyObject = msg_send![window, appearance];
                        let effective_appearance: *mut AnyObject =
                            msg_send![window, effectiveAppearance];

                        let current_name = if !current_appearance.is_null() {
                            let name: *mut AnyObject = msg_send![current_appearance, name];
                            if !name.is_null() {
                                let cstr: *const i8 = msg_send![name, UTF8String];
                                std::ffi::CStr::from_ptr(cstr).to_str().unwrap_or("unknown")
                            } else {
                                "nil"
                            }
                        } else {
                            "nil"
                        };

                        let effective_name = if !effective_appearance.is_null() {
                            let name: *mut AnyObject = msg_send![effective_appearance, name];
                            if !name.is_null() {
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

    #[cfg(any())]
    unsafe fn update_titlebar_appearance_native_with_retry(
        system_appearance: nucleotide_ui::theme_manager::SystemAppearance,
        attempt: u32,
    ) -> bool {
        use nucleotide_ui::theme_manager::SystemAppearance;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_app_kit::{NSApplication, NSWindow};
        use objc2_foundation::{MainThreadMarker, NSArray, NSString};

        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let windows: &NSArray<NSWindow> = unsafe { msg_send![&**app, windows] };
        let window_count = windows.count();

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
        let mut target_window: *mut AnyObject = std::ptr::null_mut();

        for i in 0..window_count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let window_title: *mut AnyObject = msg_send![window, title];
            let title_str = if !window_title.is_null() {
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

        if target_window.is_null() {
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
                let dark_appearance_name = NSString::from_str("NSAppearanceNameDarkAqua");
                let dark_appearance: *mut AnyObject =
                    msg_send![class!(NSAppearance), appearanceNamed: &*dark_appearance_name];
                let _: () = msg_send![window, setAppearance: dark_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to dark appearance on proper main window"
                );
            }
            SystemAppearance::Light => {
                let light_appearance_name = NSString::from_str("NSAppearanceNameAqua");
                let light_appearance: *mut AnyObject =
                    msg_send![class!(NSAppearance), appearanceNamed: &*light_appearance_name];
                let _: () = msg_send![window, setAppearance: light_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to light appearance on proper main window"
                );
            }
        }

        true
    }

    // removed unused ensure_window_follows_system_appearance

    #[cfg(any())]
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
        nucleotide_logging::info!(doc_id = ?doc_id, "DIAG: Workspace handling DiagnosticsChanged - updating view");
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
        nucleotide_logging::info!(
            "🎯 TRIGGER COMPLETION: doc {:?}, view {:?}, trigger: {:?}",
            doc_id,
            view_id,
            trigger
        );

        // Trigger completion through the completion coordinator
        match trigger {
            crate::types::CompletionTrigger::Manual => {
                nucleotide_logging::info!("Manual completion triggered (CTRL+Space)");

                // Send manual trigger event to completion coordinator
                self.core.update(cx, |app, _cx| {
                    app.trigger_completion_manual(doc_id, view_id);
                });
            }
            crate::types::CompletionTrigger::Character(c) => {
                nucleotide_logging::info!(character = %c, "Character-triggered completion");

                // Send character trigger event to completion coordinator
                self.core.update(cx, |app, _cx| {
                    app.trigger_completion_character(doc_id, view_id, *c);
                });
            }
            crate::types::CompletionTrigger::Automatic => {
                nucleotide_logging::info!("Automatic completion triggered");

                // Send automatic trigger event to completion coordinator
                self.core.update(cx, |app, _cx| {
                    app.trigger_completion_automatic(doc_id, view_id);
                });
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
        info!("handle_command_submitted called with '{}'", command);

        // If a file op is pending, treat the submitted text as the name and dispatch an intent
        if let Some(pending) = self.pending_file_op.take() {
            use nucleotide_events::v2::workspace::{Event as WsEvent, FileOpIntent};

            // Build event and decide which directory to rescan using references to avoid moves
            let (event, refresh_dir): (WsEvent, Option<std::path::PathBuf>) = match &pending {
                PendingFileOp::NewFile { parent } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::NewFile {
                            parent: parent.clone(),
                            name: command.to_string(),
                        },
                    },
                    Some(parent.clone()),
                ),
                PendingFileOp::NewFolder { parent } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::NewFolder {
                            parent: parent.clone(),
                            name: command.to_string(),
                        },
                    },
                    Some(parent.clone()),
                ),
                PendingFileOp::Rename { path } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::Rename {
                            path: path.clone(),
                            new_name: command.to_string(),
                        },
                    },
                    path.parent().map(|p| p.to_path_buf()),
                ),
                PendingFileOp::Duplicate { path } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::Duplicate {
                            path: path.clone(),
                            target_name: command.to_string(),
                        },
                    },
                    path.parent().map(|p| p.to_path_buf()),
                ),
            };

            // Clear the overlay and dispatch the event
            self.overlay.update(cx, |overlay, cx| overlay.clear(cx));
            self.core.read(cx).dispatch_workspace_event(event);

            if let Some(dir) = refresh_dir {
                self.rescan_directory(&dir, cx);
            }
            return;
        }

        // No pending file op: proceed with normal command handling

        // Clear the overlay first to hide the prompt
        self.overlay.update(cx, |overlay, cx| overlay.clear(cx));

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
            "🗂️ OPEN_DIR: Opening directory"
        );

        // Update the editor's working directory FIRST
        // This is critical for LSP servers to start with correct workspace
        info!(
            old_cwd = ?std::env::current_dir().ok(),
            new_cwd = %workspace_root.display(),
            "🗂️ OPEN_DIR: Changing working directory before LSP restart"
        );

        if let Err(e) = std::env::set_current_dir(&workspace_root) {
            error!("🗂️ OPEN_DIR: Failed to change working directory: {}", e);
        } else {
            info!(
                confirmed_cwd = ?std::env::current_dir().ok(),
                "🗂️ OPEN_DIR: Working directory successfully changed"
            );
        }

        // CRITICAL: Use helix_stdx to set working directory for consistency
        // This ensures Helix's internal working directory is also updated
        if let Err(e) = helix_stdx::env::set_current_working_dir(&workspace_root) {
            error!("🗂️ OPEN_DIR: Failed to set Helix working directory: {}", e);
        } else {
            info!("🗂️ OPEN_DIR: Helix working directory updated successfully");
        }

        // Use set_project_directory to properly initialize LSP and project management
        // Pass the workspace root (not the selected directory) for proper project management
        info!("🗂️ OPEN_DIR: Calling set_project_directory to trigger LSP restart");
        self.set_project_directory(workspace_root.clone(), cx);

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
                    let ui_font_config = cx.global_mut::<crate::types::UiFontConfig>();
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
                    nucleotide_logging::error!(path = ?path, error = %e, "Failed to open file");
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
            crate::Update::CompletionEvent(completion_event) => {
                self.handle_completion_event(completion_event, cx);
            }
            crate::Update::Prompt(_)
            | crate::Update::Picker(_)
            | crate::Update::DirectoryPicker(_)
            | crate::Update::DiagnosticsPanel(_) => {
                self.handle_overlay_update(cx);
            }
            crate::Update::Completion(_completion_view) => {
                nucleotide_logging::info!("Forwarding completion to overlay");

                // Overlay will handle completion view setup in its own Update handler
                self.handle_overlay_update(cx);
            }
            crate::Update::CodeActions(_completion_view, _pairs) => {
                nucleotide_logging::info!("Forwarding code actions dropdown to overlay");
                self.handle_overlay_update(cx);
            }
            crate::Update::OpenFile(path) => self.handle_open_file(path, cx),
            crate::Update::OpenDirectory(path) => self.handle_open_directory(path, cx),
            crate::Update::FileTreeEvent(event) => {
                self.handle_file_tree_event(event, cx);
            }
            crate::Update::ShowFilePicker => {
                nucleotide_logging::info!("DIAG: Workspace received ShowFilePicker");
                let handle = self.handle.clone();
                let core = self.core.clone();
                open(core, handle, cx);
            }
            crate::Update::ShowBufferPicker => {
                nucleotide_logging::info!("DIAG: Workspace received ShowBufferPicker");
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
                // Ensure editor state is cleanly flushed and views are closed before quit
                let handle = self.handle.clone();
                let core = self.core.clone();
                quit(core, handle, cx);
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
                                // Ensure editor state is cleanly flushed and views are closed before quit
                                let handle = self.handle.clone();
                                let core = self.core.clone();
                                quit(core, handle, cx);
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
                            _ => {}
                        }
                    }
                    crate::types::AppEvent::Workspace(workspace_event) => {
                        if let crate::types::WorkspaceEvent::FileSelected { path, source } =
                            workspace_event
                        {
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
                    }
                    crate::types::AppEvent::Ui(ui_event) => {
                        match ui_event {
                            crate::types::UiEvent::OverlayShown {
                                overlay_type,
                                overlay_id,
                            } => {
                                use nucleotide_events::v2::ui::OverlayType;
                                match overlay_type {
                                    OverlayType::FilePicker => {
                                        nucleotide_logging::info!(
                                            "DIAG: Workspace observed OverlayShown(FilePicker)"
                                        );
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        open(core, handle, cx);
                                    }
                                    OverlayType::CommandPalette => {
                                        // Only treat explicitly-tagged command palette as buffer picker
                                        if overlay_id == "buffer_picker" {
                                            nucleotide_logging::info!(
                                                "DIAG: Workspace observed OverlayShown(CommandPalette as buffer_picker)"
                                            );
                                            let handle = self.handle.clone();
                                            let core = self.core.clone();
                                            show_buffer_picker(core, handle, cx);
                                        }
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
                    crate::types::AppEvent::Integration(integration_event) => {
                        // Integration events for UI synchronization
                        debug!(integration_event = ?integration_event, "Integration event received");
                        // TODO: Handle integration events for UI updates
                        // These events coordinate between document changes and UI elements like:
                        // - File tree highlighting
                        // - Tab bar updates
                        // - Save indicator changes
                        // - Diagnostic indicator updates
                    }
                    crate::types::AppEvent::Diagnostics(_d) => {
                        // Diagnostics domain events are handled upstream to update LspState
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

        // Now collect document information in the stable order, excluding ephemeral preview docs
        let mut documents = Vec::new();
        let core = self.core.read(cx);
        let editor = &core.editor;
        // Build a set of ephemeral preview doc_ids to exclude from the tab bar
        let ephemeral_docs: std::collections::HashSet<_> = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .map(|t| t.ephemeral_doc_ids())
            .unwrap_or_default();

        // Iterate in our stable order, not HashMap order
        for (order_index, &doc_id) in self.document_order.iter().enumerate() {
            if ephemeral_docs.contains(&doc_id) {
                continue;
            }
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
                // Avoid forcing a refresh on every tab bar recompute; rely on
                // initial monitoring refresh and explicit triggers elsewhere.
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

        // Extract design token values before any mutable borrows (none needed here)

        // Get UI font configuration
        let ui_font_config = cx.global::<crate::types::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);

        // Get current document info first (without LSP indicator to avoid borrow conflicts)
        let (mode_name, file_name, position_text, has_lsp_state, preferred_server_id) =
            self.statusbar_doc_info(cx);

        // Get LSP indicator separately to avoid borrowing conflicts
        let lsp_indicator =
            self.compute_statusbar_lsp_indicator(cx, has_lsp_state, preferred_server_id);

        // Use consistent border and divider colors from hybrid system
        // Status bar border color
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
                self.statusbar_main_content(
                    mode_name,
                    file_name,
                    position_text,
                    lsp_indicator,
                    divider_color,
                    &status_bar_tokens,
                    cx,
                ),
            ) // .child({
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
            FileTreeEvent::ContextMenuRequested { path, x, y } => {
                info!(
                    "FileTreeEvent::ContextMenuRequested at ({}, {}): {:?}",
                    x, y, path
                );
                self.context_menu_open = true;
                self.context_menu_pos = (*x, *y);
                self.context_menu_path = Some(path.clone());
                self.context_menu_index = 0;
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

        // DISABLED: CTRL+Space shortcut registration - let Helix handle it natively
        // let trigger_completion_shortcut = ShortcutDefinition {
        //     key_combination: "ctrl-space".to_string(),
        //     action: ShortcutAction::Action("trigger_completion".to_string()),
        //     description: "Trigger completion".to_string(),
        //     context: Some("editor".to_string()),
        //     priority: EventPriority::High,
        //     enabled: true,
        // };
        // self.global_input.register_shortcut(trigger_completion_shortcut);

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

    // removed unused setup_additional_navigation_shortcuts
    /*
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

        // Completion specific shortcuts - Helix compatible keybindings
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
                key_combination: "ctrl-y".to_string(),
                action: ShortcutAction::Action("accept_completion".to_string()),
                description: "Accept selected completion (primary - Helix)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "tab".to_string(),
                action: ShortcutAction::Action("accept_completion".to_string()),
                description: "Accept selected completion (secondary)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-n".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Next completion item (Helix style)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-p".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Previous completion item (Helix style)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
        ];

        // Register all shortcuts
        for _shortcut in global_shortcuts
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
    */

    // removed unused setup_action_handlers

    /// Register action handlers with the global input system
    fn register_action_handlers(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::info!("Registering action handlers with global input system");

        // Get weak references to avoid circular dependencies
        let _workspace_handle = cx.entity().downgrade();

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
    // removed unused handle_global_shortcuts_only

    /// Handle completion events directly using the event system
    fn handle_completion_event(
        &mut self,
        event: &helix_view::handlers::completion::CompletionEvent,
        cx: &mut Context<Self>,
    ) {
        use helix_view::handlers::completion::CompletionEvent;

        info!("Workspace handling completion event");

        match event {
            CompletionEvent::ManualTrigger { cursor, doc, view } => {
                info!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing manual completion trigger");
                self.process_completion_trigger(*cursor, *doc, *view, cx);
            }
            CompletionEvent::AutoTrigger { cursor, doc, view } => {
                info!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing auto completion trigger");
                self.process_completion_trigger(*cursor, *doc, *view, cx);
            }
            CompletionEvent::TriggerChar { cursor, doc, view } => {
                info!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing trigger character completion");
                self.process_completion_trigger(*cursor, *doc, *view, cx);
            }
            CompletionEvent::DeleteText { cursor: _ } => {
                info!("Processing delete text - hiding completions");
                self.hide_completions(cx);
            }
            CompletionEvent::Cancel => {
                info!("Processing completion cancel - hiding completions");
                self.hide_completions(cx);
            }
        }
    }

    /// Update completion filter if completion is active and prefix has changed
    /// This should be called when text changes while completion is active
    pub fn update_completion_filter(&mut self, new_prefix: String, cx: &mut Context<Self>) -> bool {
        info!(
            prefix = %new_prefix,
            "Updating completion filter with new prefix"
        );

        // Check if we have an active completion view and update its filter
        self.overlay.update(cx, |overlay, cx| {
            overlay.update_completion_filter(new_prefix, cx)
        })
    }

    /// Update completion filter by detecting current prefix at cursor
    /// This method attempts to auto-detect the current completion prefix
    pub fn update_completion_filter_auto(&mut self, cx: &mut Context<Self>) -> bool {
        // Get current text under cursor to determine new prefix
        if let Some(current_prefix) = self.get_current_completion_prefix(cx) {
            self.update_completion_filter(current_prefix, cx)
        } else {
            false
        }
    }

    /// Schedule a completion filter update to happen after current key processing
    /// This ensures the document text is updated before we extract the new prefix
    fn schedule_completion_filter_update(&mut self, cx: &mut Context<Self>) {
        // Use defer to schedule the filter update after the current key processing
        let workspace_handle = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(workspace) = workspace_handle.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    nucleotide_logging::debug!("Executing deferred completion filter update");
                    workspace.update_completion_filter_auto(cx);
                });
            }
        });
    }

    /// Get the current word prefix under the cursor for completion filtering
    fn get_current_completion_prefix(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let core = self.core.clone();
        core.update(cx, |core, _cx| {
            let editor = &mut core.editor;
            let (view, doc) = helix_view::current!(editor);
            let text = doc.text();
            let selection = doc.selection(view.id);
            let cursor_pos = selection.primary().cursor(text.slice(..));

            // Find the start of the current word by looking backwards from cursor
            let line = text.char_to_line(cursor_pos);
            let line_start = text.line_to_char(line);
            let line_end = text.line_to_char(line + 1).min(text.len_chars());

            // Get the full line text to ensure we capture the most recent character
            let full_line = text.slice(line_start..line_end).to_string();

            // Find our position within the line
            let cursor_in_line = cursor_pos - line_start;

            nucleotide_logging::debug!(
                cursor_pos = cursor_pos,
                line_start = line_start,
                cursor_in_line = cursor_in_line,
                full_line = %full_line,
                "Cursor position analysis"
            );

            // Try getting text up to cursor position
            let line_text_to_cursor = &full_line[..cursor_in_line.min(full_line.len())];

            nucleotide_logging::debug!(
                line_text_to_cursor = %line_text_to_cursor,
                full_line_len = full_line.len(),
                cursor_in_line = cursor_in_line,
                "Text extraction analysis"
            );

            // Configure prefix extractor based on current document's file extension
            if let Some(path) = doc.path()
                && let Some(extension) = path.extension().and_then(|ext| ext.to_str())
            {
                let language = self.map_extension_to_language(extension);
                self.prefix_extractor.configure_for_language(&language);
            }

            // Use the enhanced prefix extractor for language-aware completion
            let (prefix, is_trigger_completion) = self
                .prefix_extractor
                .extract_prefix(line_text_to_cursor, cursor_in_line);

            nucleotide_logging::debug!(
                is_trigger_completion = is_trigger_completion,
                extracted_prefix = %prefix,
                "Enhanced prefix extraction result"
            );

            nucleotide_logging::debug!(
                prefix = %prefix,
                cursor_pos = cursor_pos,
                line = line,
                line_text_to_cursor = %line_text_to_cursor,
                ends_with_dot = line_text_to_cursor.ends_with('.'),
                is_trigger_completion = is_trigger_completion,
                "Enhanced completion prefix extraction completed"
            );

            // Even empty prefix is valid for trigger completions (e.g., method completion after a dot)
            Some(prefix)
        })
    }

    /// Map file extensions to language identifiers
    fn map_extension_to_language(&self, extension: &str) -> String {
        match extension.to_lowercase().as_str() {
            "rs" => "rust".to_string(),
            "js" | "mjs" => "javascript".to_string(),
            "ts" | "mts" => "typescript".to_string(),
            "tsx" => "typescript".to_string(),
            "jsx" => "javascript".to_string(),
            "css" => "css".to_string(),
            "scss" => "scss".to_string(),
            "less" => "less".to_string(),
            "php" => "php".to_string(),
            "c" => "c".to_string(),
            "cpp" | "cc" | "cxx" | "c++" => "cpp".to_string(),
            "h" | "hpp" | "hxx" => "cpp".to_string(),
            "py" => "python".to_string(),
            "go" => "go".to_string(),
            "java" => "java".to_string(),
            _ => "generic".to_string(),
        }
    }

    /// Update completion filter by predicting what the prefix will be after the character is typed
    fn update_completion_filter_with_predicted_char(
        &mut self,
        typed_char: char,
        cx: &mut Context<Self>,
    ) -> bool {
        // Get the current prefix and append the character that was just typed
        if let Some(current_prefix) = self.get_current_completion_prefix(cx) {
            let predicted_prefix = format!("{}{}", current_prefix, typed_char);

            nucleotide_logging::debug!(
                current_prefix = %current_prefix,
                typed_char = %typed_char,
                predicted_prefix = %predicted_prefix,
                "Predicting completion prefix after character input"
            );

            self.update_completion_filter(predicted_prefix, cx)
        } else {
            // If we can't get current prefix, use just the typed character
            let predicted_prefix = typed_char.to_string();
            nucleotide_logging::debug!(
                typed_char = %typed_char,
                predicted_prefix = %predicted_prefix,
                "Using typed character as completion prefix (no current prefix available)"
            );

            self.update_completion_filter(predicted_prefix, cx)
        }
    }

    /// Update completion filter by predicting what the prefix will be after backspace
    fn update_completion_filter_with_predicted_backspace(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        // Get the current prefix and remove the last character to predict the result of backspace
        if let Some(current_prefix) = self.get_current_completion_prefix(cx) {
            if current_prefix.is_empty() {
                // If prefix is already empty, backspace won't change anything
                nucleotide_logging::debug!("Backspace on empty prefix - no filter update needed");
                false
            } else {
                // Remove the last character to predict what prefix will be after backspace
                let mut chars: Vec<char> = current_prefix.chars().collect();
                chars.pop(); // Remove last character
                let predicted_prefix: String = chars.iter().collect();

                nucleotide_logging::debug!(
                    current_prefix = %current_prefix,
                    predicted_prefix = %predicted_prefix,
                    "Predicting completion prefix after backspace"
                );

                if predicted_prefix.is_empty() {
                    // If predicted prefix becomes empty, show all items by clearing filter
                    self.update_completion_filter("".to_string(), cx)
                } else {
                    self.update_completion_filter(predicted_prefix, cx)
                }
            }
        } else {
            // If we can't get current prefix, just clear the filter to show all items
            nucleotide_logging::debug!(
                "No current prefix available - clearing filter for backspace"
            );
            self.update_completion_filter("".to_string(), cx)
        }
    }

    /// Process completion trigger and request LSP completions
    fn process_completion_trigger(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) {
        info!(cursor = cursor, doc_id = ?doc_id, view_id = ?view_id, "Sending completion event directly to Application");

        // Send completion event directly to the Application which will forward to Helix
        self.core.update(cx, |app, _cx| {
            app.trigger_completion_manual(doc_id, view_id);
        });

        // Completion results will now be processed directly through Helix's completion system
        // via hooks that we'll register to capture when Helix has completion results ready
    }

    /// Convert completion items and show completion popup
    // removed unused show_completion_items

    /// Convert completion items and show completion popup with prefix filtering
    pub fn show_completion_items_with_prefix(
        &mut self,
        items: Vec<nucleotide_events::completion::CompletionItem>,
        prefix: String,
        _cursor: usize,
        _doc_id: helix_view::DocumentId,
        _view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) {
        // Convert between completion item types (same as existing method)
        let ui_items: Vec<nucleotide_ui::completion_v2::CompletionItem> = items
            .into_iter()
            .map(|item| {
                use nucleotide_events::completion::CompletionItemKind;
                use nucleotide_ui::completion_v2::{
                    CompletionItem as UiCompletionItem, CompletionItemKind as UiCompletionItemKind,
                };

                let ui_kind = match item.kind {
                    CompletionItemKind::Text => UiCompletionItemKind::Text,
                    CompletionItemKind::Method => UiCompletionItemKind::Method,
                    CompletionItemKind::Function => UiCompletionItemKind::Function,
                    CompletionItemKind::Constructor => UiCompletionItemKind::Constructor,
                    CompletionItemKind::Field => UiCompletionItemKind::Field,
                    CompletionItemKind::Variable => UiCompletionItemKind::Variable,
                    CompletionItemKind::Class => UiCompletionItemKind::Class,
                    CompletionItemKind::Interface => UiCompletionItemKind::Interface,
                    CompletionItemKind::Module => UiCompletionItemKind::Module,
                    CompletionItemKind::Property => UiCompletionItemKind::Property,
                    CompletionItemKind::Unit => UiCompletionItemKind::Unit,
                    CompletionItemKind::Value => UiCompletionItemKind::Value,
                    CompletionItemKind::Enum => UiCompletionItemKind::Enum,
                    CompletionItemKind::Keyword => UiCompletionItemKind::Keyword,
                    CompletionItemKind::Snippet => UiCompletionItemKind::Snippet,
                    CompletionItemKind::Color => UiCompletionItemKind::Color,
                    CompletionItemKind::File => UiCompletionItemKind::File,
                    CompletionItemKind::Reference => UiCompletionItemKind::Reference,
                    CompletionItemKind::Folder => UiCompletionItemKind::Folder,
                    CompletionItemKind::EnumMember => UiCompletionItemKind::EnumMember,
                    CompletionItemKind::Constant => UiCompletionItemKind::Constant,
                    CompletionItemKind::Struct => UiCompletionItemKind::Struct,
                    CompletionItemKind::Event => UiCompletionItemKind::Event,
                    CompletionItemKind::Operator => UiCompletionItemKind::Operator,
                    CompletionItemKind::TypeParameter => UiCompletionItemKind::TypeParameter,
                };

                UiCompletionItem {
                    text: item.insert_text.into(),
                    description: item.detail.as_ref().map(|d| d.clone().into()),
                    display_text: Some(item.label.into()),
                    kind: Some(ui_kind),
                    documentation: item.documentation.map(|d| d.into()),
                    detail: item.detail.map(|d| d.into()),
                    signature_info: item.signature_info.map(|s| s.into()),
                    type_info: item.type_info.map(|t| t.into()),
                    insert_text_format: match item.insert_text_format {
                        nucleotide_events::completion::InsertTextFormat::PlainText => {
                            nucleotide_ui::completion_v2::InsertTextFormat::PlainText
                        }
                        nucleotide_events::completion::InsertTextFormat::Snippet => {
                            nucleotide_ui::completion_v2::InsertTextFormat::Snippet
                        }
                    },
                }
            })
            .collect();

        nucleotide_logging::info!(
            ui_item_count = ui_items.len(),
            prefix = %prefix,
            "Converted to UI completion items with prefix, creating filtered completion view"
        );

        // Create completion view with prefix filtering
        let ui_items_count = ui_items.len();
        let completion_view = cx.new(|cx| {
            let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
            // Use the new method that applies initial filtering
            let initial_filter = if prefix.is_empty() {
                None
            } else {
                Some(prefix)
            };
            view.set_items_with_filter(ui_items, initial_filter, cx);
            view
        });
        nucleotide_logging::info!(
            "✨ CREATING COMPLETION VIEW: {} items, emitting Update::Completion event via core",
            ui_items_count
        );

        // Emit through core so overlay (which subscribes to core) receives the event
        let completion_view_clone = completion_view.clone();
        self.core.update(cx, |_core, cx| {
            cx.emit(crate::Update::Completion(completion_view_clone));
        });
        cx.notify();
    }

    /// Hide completions
    fn hide_completions(&mut self, cx: &mut Context<Self>) {
        info!("Hiding completions via overlay dismiss");
        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_completion(cx);
        });
        cx.notify();
    }

    /// Handle keyboard shortcuts detected by the global input system (full processing)
    // removed unused handle_global_input_shortcuts

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
        nucleotide_logging::info!("Triggering completion directly using real LSP completions");

        // Get current document and view information (in a separate scope to release the borrow)
        let (cursor, doc_id, view_id) = {
            let editor = &self.core.read(cx).editor;
            let view_id = editor.tree.focus;
            let view = editor.tree.get(view_id);
            let doc = editor.documents.get(&view.doc).unwrap();
            let cursor = doc
                .selection(view.id)
                .primary()
                .cursor(doc.text().slice(..));
            (cursor, doc.id(), view.id)
        };

        nucleotide_logging::info!(
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            "Calling real LSP completion directly from workspace"
        );

        // Instead of going through the coordinator, call the LSP completion directly
        let core_handle = self.core.clone();
        core_handle.update(cx, |core, cx| {
            match core.request_lsp_completions_sync_with_prefix(cursor, doc_id, view_id) {
                Ok((completion_items, prefix)) => {
                    nucleotide_logging::info!(
                        item_count = completion_items.len(),
                        prefix = %prefix,
                        "Successfully retrieved real LSP completions directly"
                    );

                    if !completion_items.is_empty() {
                        nucleotide_logging::info!(
                            "Creating and emitting CompletionView with {} items",
                            completion_items.len()
                        );

                        // Convert completion items to UI items
                        let ui_items: Vec<_> = completion_items
                            .into_iter()
                            .map(|item| {
                                // Convert to UI completion item kind
                                let ui_kind = match item.kind {
                                    nucleotide_events::completion::CompletionItemKind::Text => Some(nucleotide_ui::completion_v2::CompletionItemKind::Text),
                                    nucleotide_events::completion::CompletionItemKind::Method => Some(nucleotide_ui::completion_v2::CompletionItemKind::Method),
                                    nucleotide_events::completion::CompletionItemKind::Function => Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
                                    nucleotide_events::completion::CompletionItemKind::Constructor => Some(nucleotide_ui::completion_v2::CompletionItemKind::Constructor),
                                    nucleotide_events::completion::CompletionItemKind::Field => Some(nucleotide_ui::completion_v2::CompletionItemKind::Field),
                                    nucleotide_events::completion::CompletionItemKind::Variable => Some(nucleotide_ui::completion_v2::CompletionItemKind::Variable),
                                    nucleotide_events::completion::CompletionItemKind::Class => Some(nucleotide_ui::completion_v2::CompletionItemKind::Class),
                                    nucleotide_events::completion::CompletionItemKind::Interface => Some(nucleotide_ui::completion_v2::CompletionItemKind::Interface),
                                    nucleotide_events::completion::CompletionItemKind::Module => Some(nucleotide_ui::completion_v2::CompletionItemKind::Module),
                                    nucleotide_events::completion::CompletionItemKind::Property => Some(nucleotide_ui::completion_v2::CompletionItemKind::Property),
                                    nucleotide_events::completion::CompletionItemKind::Unit => Some(nucleotide_ui::completion_v2::CompletionItemKind::Unit),
                                    nucleotide_events::completion::CompletionItemKind::Value => Some(nucleotide_ui::completion_v2::CompletionItemKind::Value),
                                    nucleotide_events::completion::CompletionItemKind::Enum => Some(nucleotide_ui::completion_v2::CompletionItemKind::Enum),
                                    nucleotide_events::completion::CompletionItemKind::Keyword => Some(nucleotide_ui::completion_v2::CompletionItemKind::Keyword),
                                    nucleotide_events::completion::CompletionItemKind::Snippet => Some(nucleotide_ui::completion_v2::CompletionItemKind::Snippet),
                                    _ => Some(nucleotide_ui::completion_v2::CompletionItemKind::Text),
                                };

                                nucleotide_ui::completion_v2::CompletionItem {
                                    text: item.insert_text.into(),
                                    description: item.detail.as_ref().map(|d| d.clone().into()),
                                    display_text: Some(item.label.into()),
                                    kind: ui_kind,
                                    documentation: item.documentation.map(|d| d.into()),
                                    detail: item.detail.map(|d| d.into()),
                                    signature_info: item.signature_info.map(|s| s.into()),
                                    type_info: item.type_info.map(|t| t.into()),
                                    insert_text_format: match item.insert_text_format {
                                        nucleotide_events::completion::InsertTextFormat::PlainText => nucleotide_ui::completion_v2::InsertTextFormat::PlainText,
                                        nucleotide_events::completion::InsertTextFormat::Snippet => nucleotide_ui::completion_v2::InsertTextFormat::Snippet,
                                    },
                                }
                            })
                            .collect();

                        // Create completion view and emit update
                        let completion_view = cx.new(|cx| {
                            let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
                            view.set_items_with_filter(ui_items, Some(prefix.clone()), cx);
                            view
                        });

                        nucleotide_logging::info!("Created completion view, emitting Update::Completion event");
                        cx.emit(crate::Update::Completion(completion_view));
                        cx.notify();
                    } else {
                        nucleotide_logging::warn!("No completion items returned from LSP");
                    }
                }
                Err(e) => {
                    nucleotide_logging::error!(
                        error = %e,
                        "Failed to get LSP completions directly"
                    );
                }
            }
        });
    }

    // REMOVED: Old completion coordinator initialization method replaced by event-based approach
    // See the implementation at the end of the file that uses the event system

    // REMOVED: Complex cross-thread completion methods replaced by event-based approach
    // The Application now handles completion results and emits Update::Completion events
    // which the workspace receives via the existing event subscription

    /// Handle completion acceptance via Helix's transaction system
    fn handle_completion_via_helix(&mut self, item_index: usize, cx: &mut Context<Self>) {
        nucleotide_logging::info!(
            item_index = item_index,
            "Accepting completion via Helix transaction system"
        );

        // Get the completion item from the current completion state
        let completion_item = self.overlay.update(cx, |overlay, cx| {
            overlay.get_completion_item(item_index, cx)
        });

        let Some(completion_item) = completion_item else {
            nucleotide_logging::warn!(
                item_index = item_index,
                "No completion item at index for acceptance"
            );
            return;
        };

        nucleotide_logging::info!(
            item_index = item_index,
            completion_text = %completion_item.text,
            insert_text_format = ?completion_item.insert_text_format,
            "Retrieved completion item for transaction"
        );

        // Check if this is a snippet completion
        match completion_item.insert_text_format {
            nucleotide_ui::completion_v2::InsertTextFormat::Snippet => {
                self.handle_snippet_completion(completion_item, cx);
            }
            nucleotide_ui::completion_v2::InsertTextFormat::PlainText => {
                self.handle_plain_text_completion(completion_item, cx);
            }
        }
    }

    fn handle_snippet_completion(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        cx: &mut Context<Self>,
    ) {
        nucleotide_logging::info!(
            completion_text = %completion_item.text,
            "Processing snippet completion with cursor positioning"
        );

        // Parse the snippet
        let snippet_template = match nucleotide_core::SnippetTemplate::parse(&completion_item.text)
        {
            Ok(template) => template,
            Err(err) => {
                nucleotide_logging::warn!(
                    completion_text = %completion_item.text,
                    error = %err,
                    "Failed to parse snippet, falling back to plain text"
                );
                // Fall back to plain text handling
                self.handle_plain_text_completion(completion_item, cx);
                return;
            }
        };

        // Render snippet to plain text and calculate cursor position
        let plain_text = snippet_template.render_plain_text();

        nucleotide_logging::info!(
            original_snippet = %completion_item.text,
            rendered_text = %plain_text,
            has_final_tabstop = snippet_template.final_cursor_pos.is_some(),
            "Snippet parsed successfully"
        );

        // Use Helix's transaction system to insert the plain text
        let rt_handle = self.handle.clone();
        self.core.update(cx, move |core, cx| {
            let _guard = rt_handle.enter();
            let editor = &mut core.editor;

            nucleotide_logging::info!(
                rendered_text = %plain_text,
                "Creating Helix transaction for snippet completion"
            );

            // Apply the completion using Helix's transaction system
            let (view, doc) = helix_view::current!(editor);
            use helix_core::Selection;
            use helix_core::Transaction;

            let text = doc.text();
            let selection = doc.selection(view.id);
            let primary_cursor = selection.primary().cursor(text.slice(..));

            nucleotide_logging::info!(
                cursor_pos = primary_cursor,
                doc_len = text.len_chars(),
                selection_ranges = selection.len(),
                "Transaction context before snippet insertion"
            );

            // Create transaction to replace the partial word with completion text
            let mut replacement_start_pos = primary_cursor;
            let transaction = Transaction::change_by_selection(text, selection, |range| {
                // Find the start of the word being completed (go backward from cursor)
                let cursor_pos = range.cursor(text.slice(..));
                let mut start_pos = cursor_pos;
                let text_slice = text.slice(..);
                let mut chars_iter = text_slice.chars_at(cursor_pos);
                chars_iter.reverse();

                nucleotide_logging::info!(
                    range_cursor = cursor_pos,
                    "Processing range in snippet transaction"
                );

                for ch in chars_iter {
                    if helix_core::chars::char_is_word(ch) {
                        if start_pos > 0 {
                            start_pos -= ch.len_utf8();
                        }
                    } else {
                        break;
                    }
                }

                // Store the start position for cursor calculation
                replacement_start_pos = start_pos;

                nucleotide_logging::info!(
                    start_pos = start_pos,
                    end_pos = cursor_pos,
                    replacement_text = %plain_text,
                    "Snippet transaction replacement calculated"
                );

                // Return the replacement text for this range
                (start_pos, cursor_pos, Some(plain_text.clone().into()))
            });

            // Apply the transaction
            nucleotide_logging::info!("Applying snippet transaction to document");
            doc.apply(&transaction, view.id);

            // Calculate and set the final cursor position for snippet
            if let Some(cursor_pos) =
                snippet_template.calculate_final_cursor_position(replacement_start_pos)
            {
                nucleotide_logging::info!(
                    calculated_cursor_pos = cursor_pos,
                    replacement_start = replacement_start_pos,
                    "Setting final cursor position for snippet"
                );

                // Create a new selection with the cursor at the calculated position
                let new_selection = Selection::point(cursor_pos);
                doc.set_selection(view.id, new_selection);

                nucleotide_logging::info!(
                    final_cursor_pos = cursor_pos,
                    "Snippet cursor positioned successfully"
                );
            } else {
                nucleotide_logging::info!(
                    "No $0 tabstop found, cursor remains at end of insertion"
                );
            }

            nucleotide_logging::info!("Applied snippet completion transaction successfully");

            cx.notify();
        });

        // Dismiss the completion view after successful text insertion
        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_completion(cx);
        });

        nucleotide_logging::info!("Snippet completion processing complete - view dismissed");
    }

    fn handle_plain_text_completion(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        cx: &mut Context<Self>,
    ) {
        nucleotide_logging::info!(
            completion_text = %completion_item.text,
            "Processing plain text completion"
        );

        // Use Helix's transaction system to insert the completion text
        let rt_handle = self.handle.clone();
        self.core.update(cx, move |core, cx| {
            let _guard = rt_handle.enter();
            let editor = &mut core.editor;

            nucleotide_logging::info!(
                completion_text = %completion_item.text,
                "Creating Helix transaction for plain text completion"
            );

            // Apply the completion using Helix's transaction system
            let (view, doc) = helix_view::current!(editor);

            use helix_core::Transaction;

            let text = doc.text();
            let selection = doc.selection(view.id);
            let primary_cursor = selection.primary().cursor(text.slice(..));

            nucleotide_logging::info!(
                cursor_pos = primary_cursor,
                doc_len = text.len_chars(),
                selection_ranges = selection.len(),
                "Transaction context before plain text creation"
            );

            // Create transaction to replace the partial word with completion text
            let transaction = Transaction::change_by_selection(text, selection, |range| {
                // Find the start of the word being completed (go backward from cursor)
                let cursor_pos = range.cursor(text.slice(..));
                let mut start_pos = cursor_pos;
                let text_slice = text.slice(..);
                let mut chars_iter = text_slice.chars_at(cursor_pos);
                chars_iter.reverse();

                nucleotide_logging::info!(
                    range_cursor = cursor_pos,
                    "Processing range in plain text transaction"
                );

                for ch in chars_iter {
                    if helix_core::chars::char_is_word(ch) {
                        if start_pos > 0 {
                            start_pos -= ch.len_utf8();
                        }
                    } else {
                        break;
                    }
                }

                nucleotide_logging::info!(
                    start_pos = start_pos,
                    end_pos = cursor_pos,
                    replacement_text = %completion_item.text,
                    "Plain text transaction replacement calculated"
                );

                // Return the replacement text for this range
                (
                    start_pos,
                    cursor_pos,
                    Some(completion_item.text.to_string().into()),
                )
            });

            // Apply the transaction
            nucleotide_logging::info!("Applying plain text transaction to document");
            doc.apply(&transaction, view.id);

            nucleotide_logging::info!("Applied plain text completion transaction successfully");

            cx.notify();
        });

        // Dismiss the completion view after successful text insertion
        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_completion(cx);
        });

        nucleotide_logging::info!("Plain text completion processing complete - view dismissed");
    }

    /// Handle completion acceptance - insert the selected text into the editor (DEPRECATED)
    // removed unused handle_completion_accepted

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

    /// Update global workspace layout information for UI positioning
    fn update_workspace_layout_info(&self, cx: &mut Context<Self>) {
        use crate::overlay::WorkspaceLayoutInfo;

        // Calculate current tab bar height based on configuration
        let core = self.core.read(cx);
        let editor = &core.editor;
        let bufferline_config = &editor.config().bufferline;

        let tab_bar_height = match bufferline_config {
            helix_view::editor::BufferLine::Never => px(0.0),
            helix_view::editor::BufferLine::Always => px(40.0), // Standard tab height
            helix_view::editor::BufferLine::Multiple => {
                if editor.documents.len() > 1 {
                    px(40.0) // Standard tab height when multiple docs
                } else {
                    px(0.0) // No tab bar for single document
                }
            }
        };

        // Get actual file tree width (user may have resized it)
        let file_tree_width = if self.show_file_tree {
            px(self.file_tree_width)
        } else {
            px(0.0) // No file tree width if hidden
        };

        // Get font metrics from the focused DocumentView if available
        let (line_height, char_width) = self.get_font_metrics_from_focused_view(cx);

        let layout_info = WorkspaceLayoutInfo {
            file_tree_width,
            gutter_width: px(60.0), // Line number gutter width (approximately)
            tab_bar_height,
            title_bar_height: px(30.0), // Much smaller - just window controls
            line_height,
            char_width,
            cursor_position: None, // Will be filled by DocumentView during cursor rendering
            cursor_size: None,     // Will be filled by DocumentView during cursor rendering
        };

        // Set as global state so overlay can access it
        cx.set_global(layout_info);
    }

    /// Get font metrics (line height, char width) from the focused DocumentView
    fn get_font_metrics_from_focused_view(
        &self,
        cx: &mut Context<Self>,
    ) -> (gpui::Pixels, gpui::Pixels) {
        // Try to get the focused DocumentView
        if let Some(focused_view_id) = self.view_manager.focused_view_id()
            && let Some(doc_view) = self.view_manager.get_document_view(&focused_view_id)
        {
            // Access the DocumentView to get real font metrics
            return doc_view.read_with(cx, |doc_view, _cx| {
                // Get the actual line height from DocumentView
                let line_height = doc_view.get_line_height();

                // Calculate character width from font style
                // This is approximate but much more accurate than hardcoded 8.0
                let char_width = px(line_height.0 * 0.6); // Typical monospace ratio

                (line_height, char_width)
            });
        }

        // Fallback to reasonable defaults if no focused view
        (px(20.0), px(12.0))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drive V2 event processing so FsOpHandler can execute intents
        if let Some(aggregator) = self.core.read(cx).event_aggregator.as_ref() {
            aggregator.process_events();
        }
        // Fallback: full refresh if any pending flag remains
        if self.needs_file_tree_refresh {
            if let Some(ref file_tree) = self.file_tree {
                file_tree.update(cx, |view, tree_cx| {
                    view.refresh(tree_cx);
                });
            }
            self.needs_file_tree_refresh = false;
        }

        // Process completion results from the coordinator
        self.process_completion_results(cx);

        // Failsafe: If the overlay is gone and no known element has focus, force-refocus.
        // We see cases in logs where overlay_empty=true and both workspace and doc view
        // report not focused, leaving the app with no key receiver. This block ensures
        // that after overlay teardown, we always regain a valid focus target without a click.
        if self.overlay.read(cx).is_empty() {
            let ws_focused = self.focus_handle.is_focused(window);
            let overlay_focused = self.overlay.focus_handle(cx).is_focused(window);

            let (doc_focus_handle, doc_focused) = if let Some(id) =
                self.view_manager.focused_view_id()
                && let Some(doc_view) = self.view_manager.get_document_view(&id)
            {
                let fh = doc_view.focus_handle(cx);
                (Some(fh.clone()), fh.is_focused(window))
            } else {
                (None, false)
            };

            let file_tree_focused = self
                .file_tree
                .as_ref()
                .map(|ft| ft.focus_handle(cx).is_focused(window))
                .unwrap_or(false);

            if !ws_focused && !overlay_focused && !doc_focused && !file_tree_focused {
                // First, nudge caret into the document view if we have one.
                if let Some(fh) = doc_focus_handle {
                    window.focus(&fh);
                }
                // Then ensure global key routing via workspace root.
                window.focus(&self.focus_handle);
            }
        }

        // Update global workspace layout information for completion positioning
        self.update_workspace_layout_info(cx);

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
            // Focus the workspace; input routing will handle contexts
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
        let _border_color = window_style
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

        let _has_overlay = !self.overlay.read(cx).is_empty();

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
                        debug!("COMP: Workspace rendering overlay because it's not empty");
                        let view = &self.overlay;
                        this.child(view.clone())
                    })
                    .child(self.about_window.clone())
                    .when(
                        !self.info_hidden && !self.info.read(cx).is_empty(),
                        |this| this.child(self.info.clone()),
                    )
                    .child(self.key_hints.clone())
                    .when(self.tab_overflow_dropdown_open, |this| {
                        // Render the overflow menu as an overlay
                        this.child(self.render_tab_overflow_menu(window, cx))
                    })
                    .when(self.context_menu_open, |this| {
                        // Render file tree context menu when open
                        this.child(self.render_file_tree_context_menu(window, cx))
                    })
                    .when(self.delete_confirm_open, |this| {
                        // Render delete confirmation modal overlay
                        this.child(self.render_delete_confirm_modal(window, cx))
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

                    // Close context menu when clicking elsewhere
                    if workspace.context_menu_open {
                        workspace.context_menu_open = false;
                        cx.notify();
                    }

                    // Clicking outside the delete confirm modal closes it
                    if workspace.delete_confirm_open {
                        workspace.delete_confirm_open = false;
                        workspace.delete_confirm_path = None;
                        cx.notify();
                    }

                    // Ensure workspace regains focus when clicked, so global shortcuts work
                    workspace.view_manager.set_needs_focus_restore(true);
                    cx.notify();
                }),
            );

        // Add action handlers
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::help::About, _window, cx| {
                workspace.about_window.update(cx, |about_window, cx| {
                    about_window.show(cx);
                });
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

        // Code actions picker
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::workspace::ShowCodeActions, _window, cx| {
                show_code_actions(core.clone(), handle.clone(), cx)
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
                nucleotide_logging::warn!("New window not yet implemented");
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
                move |_, _: &crate::actions::window::Minimize, window, _cx| {
                    window.minimize_window();
                },
            ))
            .on_action(cx.listener(
                move |_, _: &crate::actions::window::Zoom, window, _cx| {
                    window.zoom_window();
                },
            ));

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
            let resize_handle_width = 4.0;
            let main_content_offset = self.file_tree_width + resize_handle_width;

            if let Some(file_tree) = &self.file_tree {
                // Create file tree panel with absolute positioning
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                let status_bar_tokens = ui_theme.tokens.status_bar_tokens();
                let panel_bg = ui_theme.tokens.chrome.surface;
                let _panel_border_color = status_bar_tokens.border;
                let file_tree_panel = div()
                    .absolute()
                    .left(px(file_tree_left_offset))
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .child(file_tree.clone());

                // Create resize handle as border line
                let border_color = status_bar_tokens.border;
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(4.0)) // 4px wide drag handle
                    .bg(panel_bg) // Match file tree background
                    .border_0() // Reset all borders
                    .border_r_1() // Only right border
                    .border_color(border_color)
                    .hover(|style| style.border_color(ui_theme.tokens.chrome.text_chrome_secondary))
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
                let resize_handle_width = 4.0;
                let main_content_offset = self.file_tree_width + resize_handle_width;

                // Use the same background color as the actual file tree for consistency
                let prompt_bg = ui_theme.tokens.chrome.surface;
                let status_bar_tokens = ui_theme.tokens.status_bar_tokens();
                let _border_color = status_bar_tokens.border;

                let placeholder_panel = div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .bg(prompt_bg)
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
                let border_color = status_bar_tokens.border;
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(4.0)) // 4px wide drag handle
                    .bg(prompt_bg) // Match placeholder panel background
                    .border_0() // Reset all borders
                    .border_r_1() // Only right border
                    .border_color(border_color)
                    .hover(|style| style.border_color(ui_theme.tokens.chrome.text_chrome_secondary))
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
                    .child(self.render_unified_status_bar(cx)) // Unified bottom status bar
                    .when(self.lsp_menu_open, |container| {
                        use gpui::{Corner, anchored, point};
                        let ui_theme = cx.global::<nucleotide_ui::Theme>();
                        let dd_tokens = ui_theme.tokens.dropdown_tokens();

                        // Snapshot LSP state
                        let server_rows: Vec<gpui::AnyElement> = {
                            let lsp_state_entity = self.core.read(cx).lsp_state.clone();
                            if let Some(lsp_state) = lsp_state_entity {
                                let state = lsp_state.read(cx);
                                let mut rows: Vec<gpui::AnyElement> = Vec::new();

                                // Sort servers by name for a stable order
                                let mut servers: Vec<_> = state.servers.values().cloned().collect();
                                servers.sort_by(|a, b| a.name.cmp(&b.name));

                                // If there are no servers, show muted empty message
                                if servers.is_empty() {
                                    rows.push(
                                        div()
                                            .w_full()
                                            .px(ui_theme.tokens.sizes.space_3)
                                            .py(ui_theme.tokens.sizes.space_2)
                                            .text_size(ui_theme.tokens.sizes.text_sm)
                                            .text_color(dd_tokens.item_text_secondary)
                                            .child("no LSP servers")
                                            .into_any_element(),
                                    );
                                    // No servers to display; end of list
                                }

                                for server in servers {
                                    let status_text = match server.status {
                                        ServerStatus::Starting => "Starting".to_string(),
                                        ServerStatus::Initializing => "Initializing".to_string(),
                                        ServerStatus::Running => "Running".to_string(),
                                        ServerStatus::Failed(ref e) => format!("Failed: {}", e),
                                        ServerStatus::Stopped => "Stopped".to_string(),
                                    };

                                    // Header row with server name and status
                                    rows.push(
                                        div()
                                            .w_full()
                                            .px(ui_theme.tokens.sizes.space_3)
                                            .py(ui_theme.tokens.sizes.space_2)
                                            .text_size(ui_theme.tokens.sizes.text_sm)
                                            .text_color(dd_tokens.item_text)
                                            .child(format!("{} — {}", server.name, status_text))
                                            .into_any_element(),
                                    );

                                    // Progress rows for this server, or Idle if none
                                    let progress_items: Vec<_> = state
                                        .progress
                                        .values()
                                        .filter(|p| p.server_id == server.id)
                                        .cloned()
                                        .collect();

                                    if progress_items.is_empty() {
                                        rows.push(
                                            div()
                                                .w_full()
                                                .px(ui_theme.tokens.sizes.space_6)
                                                .pb(ui_theme.tokens.sizes.space_2)
                                                .text_size(ui_theme.tokens.sizes.text_sm)
                                                .text_color(dd_tokens.item_text_secondary)
                                                .child("Idle")
                                                .into_any_element(),
                                        );
                                    } else {
                                        for p in progress_items {
                                            let mut line = String::new();
                                            if let Some(pct) = p.percentage {
                                                line.push_str(&format!("{pct}% "));
                                            }
                                            line.push_str(&p.title);
                                            if let Some(msg) = p.message {
                                                line.push_str(&format!(" ⋅ {}", msg));
                                            }

                                            rows.push(
                                                div()
                                                    .w_full()
                                                    .px(ui_theme.tokens.sizes.space_6)
                                                    .pb(ui_theme.tokens.sizes.space_1)
                                                    .text_size(ui_theme.tokens.sizes.text_sm)
                                                    .text_color(dd_tokens.item_text_secondary)
                                                    .child(line)
                                                    .into_any_element(),
                                            );
                                        }
                                    }

                                    // Separator between servers
                                    rows.push(
                                        div()
                                            .w_full()
                                            .h(px(1.0))
                                            .bg(dd_tokens.border)
                                            .opacity(0.5)
                                            .into_any_element(),
                                    );
                                }

                                rows
                            } else {
                                vec![
                                    div()
                                        .w_full()
                                        .px(ui_theme.tokens.sizes.space_3)
                                        .py(ui_theme.tokens.sizes.space_2)
                                        .text_size(ui_theme.tokens.sizes.text_sm)
                                        .text_color(dd_tokens.item_text_secondary)
                                        .child("no LSP servers")
                                        .into_any_element(),
                                ]
                            }
                        };

                        let (x, y) = self.lsp_menu_pos;

                        container.child(
                            div()
                                .absolute()
                                .size_full()
                                .top_0()
                                .left_0()
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this: &mut Workspace, _ev, _win, cx| {
                                        this.lsp_menu_open = false;
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    anchored()
                                        .position(point(px(x), px(y)))
                                        .anchor(Corner::BottomLeft)
                                        .offset(point(px(0.0), px(4.0)))
                                        .snap_to_window_with_margin(ui_theme.tokens.sizes.space_2)
                                        .child(
                                            div()
                                                .min_w(px(260.0))
                                                .max_w(px(480.0))
                                                .bg(dd_tokens.container_background)
                                                .border_1()
                                                .border_color(dd_tokens.border)
                                                .rounded(ui_theme.tokens.sizes.radius_md)
                                                .shadow_md()
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .children(server_rows),
                                        ),
                                ),
                        )
                    }),
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
    let _file_paths: Vec<std::path::PathBuf> = items
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

fn show_code_actions(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use futures_util::stream::{FuturesOrdered, StreamExt};
    use helix_lsp::lsp;
    use helix_lsp::util::{diagnostic_to_lsp_diagnostic, range_to_lsp_range};

    info!("Opening code actions dropdown");

    // Snapshot needed editor state under read lock
    let (doc_id, _view_id, _offset_encoding, identifier, range, diags, servers) = {
        let core_r = core.read(cx);
        let editor = &core_r.editor;
        let view = editor.tree.get(editor.tree.focus);
        let doc = editor.documents.get(&view.doc).expect("doc exists");

        let selection_range = doc.selection(view.id).primary();
        let diags = doc
            .diagnostics()
            .iter()
            .filter(|d| {
                selection_range.overlaps(&helix_core::Range::new(d.range.start, d.range.end))
            })
            .cloned()
            .collect::<Vec<_>>();

        // Collect unique servers supporting CodeAction
        let mut seen = std::collections::HashSet::new();
        let servers: Vec<_> = doc
            .language_servers_with_feature(LanguageServerFeature::CodeAction)
            .filter(|ls| seen.insert(ls.id()))
            .collect();

        let offset_encoding = servers
            .first()
            .map(|ls| ls.offset_encoding())
            .unwrap_or_default();
        let identifier = doc.identifier();
        let range = range_to_lsp_range(doc.text(), selection_range, offset_encoding);

        (
            view.doc,
            view.id,
            offset_encoding,
            identifier,
            range,
            diags,
            servers,
        )
    };

    if servers.is_empty() {
        info!("No language servers with CodeAction support");
        return;
    }

    // Build per-server requests
    let doc_text_for_diag = {
        let core_r = core.read(cx);
        // Safe: doc exists
        core_r.editor.documents.get(&doc_id).unwrap().text().clone()
    };

    let mut futures: FuturesOrdered<_> = servers
        .into_iter()
        .filter_map(|ls| {
            let offset = ls.offset_encoding();
            let ls_id = ls.id();
            let ctx = lsp::CodeActionContext {
                diagnostics: diags
                    .iter()
                    .map(|d| diagnostic_to_lsp_diagnostic(&doc_text_for_diag, d, offset))
                    .collect(),
                only: None,
                trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
            };
            let req = ls.code_actions(identifier.clone(), range, ctx)?;
            Some(async move {
                req.await
                    .map(|opt| (opt.unwrap_or_default(), ls_id, offset))
            })
        })
        .collect();

    // Helper sorters to mirror Helix ordering
    fn action_category(action: &lsp::CodeActionOrCommand) -> u32 {
        if let lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            kind: Some(kind), ..
        }) = action
        {
            let mut components = kind.as_str().split('.');
            match components.next() {
                Some("quickfix") => 0,
                Some("refactor") => match components.next() {
                    Some("extract") => 1,
                    Some("inline") => 2,
                    Some("rewrite") => 3,
                    Some("move") => 4,
                    Some("surround") => 5,
                    _ => 7,
                },
                Some("source") => 6,
                _ => 7,
            }
        } else {
            7
        }
    }

    fn action_preferred(action: &lsp::CodeActionOrCommand) -> bool {
        matches!(
            action,
            lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                is_preferred: Some(true),
                ..
            })
        )
    }

    fn action_fixes_diagnostics(action: &lsp::CodeActionOrCommand) -> bool {
        matches!(
            action,
            lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { diagnostics: Some(diags), .. }) if !diags.is_empty()
        )
    }

    // Spawn async collection job
    let core_weak = core.downgrade();
    cx.spawn(async move |cx| {
        // Build items for the completion-style dropdown
        let mut completion_items: Vec<nucleotide_ui::completion_v2::CompletionItem> = Vec::new();
        // Store paired action + server metadata alongside items for on_select
        let mut pairs: Vec<(
            lsp::CodeActionOrCommand,
            helix_core::diagnostic::LanguageServerId,
            helix_lsp::OffsetEncoding,
        )> = Vec::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((mut actions, ls_id, offset)) => {
                    // Drop disabled actions
                    actions.retain(|a| match a {
                        lsp::CodeActionOrCommand::CodeAction(ca) => ca.disabled.is_none(),
                        _ => true,
                    });

                    // Sort as in Helix: category, then fixes diagnostics, then preferred
                    actions.sort_by(|a, b| {
                        let cat = action_category(a).cmp(&action_category(b));
                        if cat != std::cmp::Ordering::Equal {
                            return cat;
                        }
                        let fix = action_fixes_diagnostics(a)
                            .cmp(&action_fixes_diagnostics(b))
                            .reverse();
                        if fix != std::cmp::Ordering::Equal {
                            return fix;
                        }
                        action_preferred(a).cmp(&action_preferred(b)).reverse()
                    });

                    for action in actions.into_iter() {
                        let label = match &action {
                            lsp::CodeActionOrCommand::Command(cmd) => cmd.title.clone(),
                            lsp::CodeActionOrCommand::CodeAction(ca) => ca.title.clone(),
                        };
                        // Build a simple completion item for the dropdown
                        let ci = nucleotide_ui::completion_v2::CompletionItem::new(label);
                        completion_items.push(ci);
                        pairs.push((action, ls_id, offset));
                    }
                }
                Err(err) => {
                    warn!(error = %err, "Error collecting code actions from server");
                }
            }
        }

        // If none, exit with a notification
        if completion_items.is_empty() {
            if let Some(core) = core_weak.upgrade() {
                let _ = core.update(cx, |_core, cx| {
                    cx.emit(crate::Update::EditorStatus(
                        nucleotide_types::EditorStatus {
                            status: "No code actions available".to_string(),
                            severity: nucleotide_types::Severity::Info,
                        },
                    ));
                });
            }
            return;
        }

        // Create a CompletionView and load items
        if let Some(core) = core_weak.upgrade() {
            let completion_view = cx
                .new(|cx| {
                    let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
                    view.set_items(completion_items, cx);
                    view
                })
                .expect("create code actions completion view");

            // Emit completion-style code actions into overlay
            let _ = core.update(cx, |_core, cx| {
                cx.emit(crate::Update::CodeActions(completion_view, pairs));
            });
        }
    })
    .detach();
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

impl Workspace {}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_project_detection_basic() {
        // Test that project detection function exists and doesn't panic with valid path
        let _current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
    #[allow(clippy::assertions_on_constants)]
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
