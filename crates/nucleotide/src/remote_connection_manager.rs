// ABOUTME: GPUI remote connection manager for opening SSH and WSL workspaces
// ABOUTME: Provides protocol selection, server autocomplete and remote home browsing

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, Window,
    anchored, deferred, div, point, px,
};
use nucleotide_core::{EditorStatus, Severity};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::actions::remote_connection_manager::{
    Accept as AcceptRemoteConnection, Cancel as CancelRemoteConnection,
    CollapseSelected as CollapseSelectedRemoteDirectory,
    ExpandSelected as ExpandSelectedRemoteDirectory, SelectNext as SelectNextRemoteItem,
    SelectPrevious as SelectPreviousRemoteItem, ToggleProtocol as ToggleRemoteProtocol,
};
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, CheckboxSize, FileIcon, IconPosition, ListItem,
    ListItemSpacing, ListItemVariant, MenuCheckSide, PopupMenu, TextInput, TextInputEvent,
    TextInputFocusStyle, ThemedContext,
};
use nucleotide_workspace::{
    DirectoryListing, FileKind, SshWorkspaceTarget, WorkspaceBackendHandle,
    classify_workspace_location, ssh_display_path, wsl_display_path,
};

use crate::application::workspace_backend_for_project_directory_with_options_progress_and_startup_context;
use crate::file_tree::icons::chevron_icon;
use crate::remote_connections::{RemoteConnectionStore, target_to_string, valid_connection_name};

const REMOTE_CONNECTION_MANAGER_CONTEXT: &str = "RemoteConnectionManager";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "enter",
            AcceptRemoteConnection,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "escape",
            CancelRemoteConnection,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "tab",
            ToggleRemoteProtocol,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "up",
            SelectPreviousRemoteItem,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "down",
            SelectNextRemoteItem,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "left",
            CollapseSelectedRemoteDirectory,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
        KeyBinding::new(
            "right",
            ExpandSelectedRemoteDirectory,
            Some(REMOTE_CONNECTION_MANAGER_CONTEXT),
        ),
    ]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemoteConnectionProtocol {
    Ssh,
    Wsl,
}

impl RemoteConnectionProtocol {
    fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Wsl => "WSL",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Ssh => Self::Wsl,
            Self::Wsl => Self::Ssh,
        }
    }
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json, no_register)]
struct SelectRemoteProtocol {
    protocol: RemoteConnectionProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteServerSuggestion {
    protocol: RemoteConnectionProtocol,
    insert_text: String,
    display_text: String,
    description: String,
    source: RemoteSuggestionSource,
    target_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RemoteSuggestionSource {
    Saved,
    Recent,
    SshConfig,
    KnownHost,
    WslDistro,
    Manual,
}

impl RemoteSuggestionSource {
    fn label(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Recent => "recent",
            Self::SshConfig => "ssh config",
            Self::KnownHost => "known host",
            Self::WslDistro => "wsl distro",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone)]
struct RemoteDirectoryNode {
    name: String,
    path: PathBuf,
    expanded: bool,
    loading: bool,
    loaded: bool,
    children: Vec<RemoteDirectoryNode>,
}

#[derive(Debug, Clone)]
struct RemoteDirectoryTreeRow {
    name: String,
    path: PathBuf,
    depth: usize,
    expanded: bool,
    loading: bool,
}

#[derive(Clone)]
struct RemoteBrowseSession {
    backend: WorkspaceBackendHandle,
    selected_path: PathBuf,
    selected_tree_path: Option<PathBuf>,
    root: RemoteDirectoryNode,
}

struct RemoteBrowseConnection {
    backend: WorkspaceBackendHandle,
    home_path: PathBuf,
    selected_path: Option<PathBuf>,
    listing: DirectoryListing,
}

#[derive(Debug, Clone)]
enum RemoteConnectTarget {
    Ssh {
        target: SshWorkspaceTarget,
        selected_path: Option<PathBuf>,
    },
    Wsl {
        distro: String,
        selected_path: Option<PathBuf>,
    },
}

enum RemoteManagerTaskEvent {
    Progress {
        generation: u64,
        message: String,
    },
    Finished {
        generation: u64,
        result: Result<RemoteBrowseConnection>,
    },
}

#[derive(Debug, Default)]
struct ConnectionAttemptState {
    generation: u64,
    active: bool,
    startup: Option<nucleotide_remote::RemoteStartupAttempt>,
}

impl ConnectionAttemptState {
    fn begin(
        &mut self,
        timeout: Duration,
    ) -> Option<(u64, nucleotide_remote::RemoteStartupContext)> {
        if self.active {
            return None;
        }

        self.generation = self.generation.wrapping_add(1).max(1);
        self.active = true;
        let startup = nucleotide_remote::RemoteStartupAttempt::new(timeout);
        let context = startup.context();
        self.startup = Some(startup);
        Some((self.generation, context))
    }

    fn invalidate(&mut self) {
        if let Some(mut startup) = self.startup.take() {
            startup.cancel();
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.active = false;
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    fn is_active(&self, generation: u64) -> bool {
        self.active && self.is_current(generation)
    }

    fn finish(&mut self, generation: u64) -> bool {
        if !self.is_active(generation) {
            return false;
        }

        self.active = false;
        if let Some(mut startup) = self.startup.take() {
            startup.disarm();
        }
        true
    }
}

pub struct RemoteConnectionManagerView {
    focus_handle: FocusHandle,
    save_connection_focus_handle: FocusHandle,
    core: gpui::WeakEntity<crate::Core>,
    handle: tokio::runtime::Handle,
    backend_options: nucleotide_remote::RemoteWorkspaceBackendOptions,

    protocol: RemoteConnectionProtocol,
    protocol_menu_open: bool,
    protocol_menu: Option<Entity<PopupMenu>>,
    protocol_menu_subscription: Option<Subscription>,
    server_input_view: Entity<TextInput>,
    server_input: String,
    suggestions: Vec<RemoteServerSuggestion>,
    suggestion_selection: usize,
    suggestions_scroll_handle: ScrollHandle,
    suggestions_scrollbar_state: ScrollbarState,
    accepted_suggestion: bool,

    browse_session: Option<RemoteBrowseSession>,
    pending_selected_path: Option<PathBuf>,
    directory_scroll_handle: ScrollHandle,
    directory_scrollbar_state: ScrollbarState,
    status: Option<EditorStatus>,
    connection_attempt: ConnectionAttemptState,
    save_on_open: bool,
}

impl RemoteConnectionManagerView {
    fn new_server_input(cx: &mut Context<Self>) -> Entity<TextInput> {
        let input = cx.new(|cx| {
            TextInput::new("remote-server-input", cx)
                .size(nucleotide_ui::InputSize::Medium)
                .focus_style(TextInputFocusStyle::Chrome)
                .placeholder("Host, distro, alias or saved connection")
        });
        cx.subscribe(&input, Self::handle_server_input_event)
            .detach();
        input
    }

    pub fn new(
        core: gpui::WeakEntity<crate::Core>,
        handle: tokio::runtime::Handle,
        backend_options: nucleotide_remote::RemoteWorkspaceBackendOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let server_input_view = Self::new_server_input(cx);
        let focus_handle = server_input_view.read(cx).focus_handle(cx);
        let save_connection_focus_handle = cx.focus_handle().tab_index(20).tab_stop(true);
        let suggestions_scroll_handle = ScrollHandle::new();
        let suggestions_scrollbar_state = ScrollbarState::new(suggestions_scroll_handle.clone());
        let directory_scroll_handle = ScrollHandle::new();
        let directory_scrollbar_state = ScrollbarState::new(directory_scroll_handle.clone());

        let view = Self {
            focus_handle,
            save_connection_focus_handle,
            core,
            handle,
            backend_options,
            protocol: RemoteConnectionProtocol::Ssh,
            protocol_menu_open: false,
            protocol_menu: None,
            protocol_menu_subscription: None,
            server_input_view,
            server_input: String::new(),
            suggestions: load_remote_server_suggestions(),
            suggestion_selection: 0,
            suggestions_scroll_handle,
            suggestions_scrollbar_state,
            accepted_suggestion: false,
            browse_session: None,
            pending_selected_path: None,
            directory_scroll_handle,
            directory_scrollbar_state,
            status: None,
            connection_attempt: ConnectionAttemptState::default(),
            save_on_open: false,
        };
        view.load_wsl_distribution_suggestions(cx);
        view
    }

    fn invalidate_connection_attempt(&mut self) {
        self.connection_attempt.invalidate();
    }

    fn load_wsl_distribution_suggestions(&self, cx: &mut Context<Self>) {
        let task = self.handle.spawn_blocking(wsl_distributions);
        cx.spawn(async move |this, cx| {
            let distributions = match task.await {
                Ok(distributions) => distributions,
                Err(error) => {
                    nucleotide_logging::warn!(
                        error = %error,
                        "WSL distribution discovery task failed"
                    );
                    return;
                }
            };

            _ = this.update(cx, |view, cx| {
                view.suggestions = with_wsl_distribution_suggestions(
                    std::mem::take(&mut view.suggestions),
                    distributions,
                );
                cx.notify();
            });
        })
        .detach();
    }

    fn filtered_suggestions(&self) -> Vec<RemoteServerSuggestion> {
        let query = self.server_input.trim().to_ascii_lowercase();
        let mut suggestions = self
            .suggestions
            .iter()
            .filter(|suggestion| suggestion.protocol == self.protocol)
            .filter(|suggestion| {
                query.is_empty()
                    || suggestion.insert_text.to_ascii_lowercase().contains(&query)
                    || suggestion
                        .display_text
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .take(8)
            .cloned()
            .collect::<Vec<_>>();

        if !self.server_input.trim().is_empty()
            && !suggestions
                .iter()
                .any(|suggestion| suggestion.insert_text == self.server_input)
        {
            suggestions.push(RemoteServerSuggestion {
                protocol: self.protocol,
                insert_text: self.server_input.trim().to_string(),
                display_text: self.server_input.trim().to_string(),
                description: "Use typed target".to_string(),
                source: RemoteSuggestionSource::Manual,
                target_path: None,
            });
        }

        suggestions
    }

    fn set_protocol(&mut self, protocol: RemoteConnectionProtocol, cx: &mut Context<Self>) {
        if self.protocol != protocol {
            self.invalidate_connection_attempt();
            self.protocol = protocol;
            self.close_protocol_menu(cx);
            self.suggestion_selection = 0;
            self.accepted_suggestion = false;
            self.browse_session = None;
            self.pending_selected_path = None;
            self.status = None;

            if !self.server_input.trim().is_empty()
                && self.target_from_current_input().is_err()
                && !self
                    .filtered_suggestions()
                    .iter()
                    .any(|suggestion| suggestion.insert_text == self.server_input)
            {
                self.set_server_input("", cx);
            }
        }

        cx.notify();
    }

    fn handle_server_input_event(
        &mut self,
        _input: Entity<TextInput>,
        event: &TextInputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed(value) => {
                self.invalidate_connection_attempt();
                self.server_input = value.to_string();
                self.suggestion_selection = 0;
                self.accepted_suggestion = false;
                self.browse_session = None;
                self.pending_selected_path = None;
                self.status = None;
                cx.notify();
            }
            TextInputEvent::Submitted(_) => self.accept_or_connect(cx),
            TextInputEvent::Cancelled => {
                if self.protocol_menu_open {
                    self.close_protocol_menu(cx);
                } else {
                    self.cancel(cx);
                }
            }
        }
    }

    fn set_server_input(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let value = value.into();
        self.server_input = value.clone();
        self.server_input_view.update(cx, |input, cx| {
            input.set_value_silent(value, cx);
        });
    }

    fn close_protocol_menu(&mut self, cx: &mut Context<Self>) {
        self.protocol_menu_open = false;
        self.protocol_menu = None;
        self.protocol_menu_subscription = None;
        cx.notify();
    }

    fn build_protocol_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        if let Some(menu) = self.protocol_menu.clone() {
            return menu;
        }

        let current_protocol = self.protocol;
        let action_context = self.focus_handle.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            menu.action_context(action_context)
                .check_side(MenuCheckSide::Right)
                .min_w(px(118.0))
                .menu_with_check_and_disabled(
                    "SSH",
                    current_protocol == RemoteConnectionProtocol::Ssh,
                    Box::new(SelectRemoteProtocol {
                        protocol: RemoteConnectionProtocol::Ssh,
                    }),
                    false,
                )
                .menu_with_check_and_disabled(
                    "WSL",
                    current_protocol == RemoteConnectionProtocol::Wsl,
                    Box::new(SelectRemoteProtocol {
                        protocol: RemoteConnectionProtocol::Wsl,
                    }),
                    false,
                )
        });

        self.protocol_menu_subscription = Some(cx.subscribe(
            &menu,
            |this, _menu, _event: &DismissEvent, cx| {
                this.close_protocol_menu(cx);
            },
        ));
        self.protocol_menu = Some(menu.clone());
        menu
    }

    fn toggle_protocol_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.protocol_menu_open {
            self.close_protocol_menu(cx);
            return;
        }

        self.protocol_menu_open = true;
        let menu = self.build_protocol_menu(window, cx);
        menu.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn select_remote_protocol(
        &mut self,
        action: &SelectRemoteProtocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_protocol(action.protocol, cx);
        self.close_protocol_menu(cx);
        self.focus_handle.focus(window, cx);
        cx.stop_propagation();
    }

    fn apply_suggestion(&mut self, suggestion: RemoteServerSuggestion, cx: &mut Context<Self>) {
        self.invalidate_connection_attempt();
        self.protocol = suggestion.protocol;
        self.set_server_input(suggestion.insert_text, cx);
        self.suggestion_selection = 0;
        self.accepted_suggestion = true;
        self.close_protocol_menu(cx);

        if let Some(target_path) = suggestion.target_path {
            self.pending_selected_path = Some(target_path);
        }

        cx.notify();
    }

    fn target_from_current_input(&self) -> Result<RemoteConnectTarget> {
        let input = self.server_input.trim();
        if input.is_empty() {
            return Err(anyhow!("Enter an SSH host or WSL distribution"));
        }

        match self.protocol {
            RemoteConnectionProtocol::Ssh => {
                if let nucleotide_workspace::WorkspaceLocation::Ssh {
                    target,
                    original_path,
                    ..
                } = classify_workspace_location(Path::new(input))
                {
                    return Ok(RemoteConnectTarget::Ssh {
                        target,
                        selected_path: Some(original_path),
                    });
                }

                let target = parse_ssh_server_input(input)
                    .ok_or_else(|| anyhow!("Enter an SSH host, alias, or ssh:// target"))?;
                Ok(RemoteConnectTarget::Ssh {
                    target,
                    selected_path: None,
                })
            }
            RemoteConnectionProtocol::Wsl => {
                if let nucleotide_workspace::WorkspaceLocation::Wsl {
                    distro,
                    original_path,
                    ..
                } = classify_workspace_location(Path::new(input))
                {
                    return Ok(RemoteConnectTarget::Wsl {
                        distro,
                        selected_path: Some(original_path),
                    });
                }

                Ok(RemoteConnectTarget::Wsl {
                    distro: input.to_string(),
                    selected_path: None,
                })
            }
        }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let target = match self.target_from_current_input() {
            Ok(target) => target,
            Err(error) => {
                self.status = Some(EditorStatus {
                    status: error.to_string(),
                    severity: Severity::Error,
                });
                cx.notify();
                return;
            }
        };

        let Some((generation, startup_context)) = self
            .connection_attempt
            .begin(nucleotide_remote::DEFAULT_REMOTE_STARTUP_TIMEOUT)
        else {
            return;
        };
        self.status = Some(EditorStatus {
            status: "Connecting to remote target".to_string(),
            severity: Severity::Info,
        });
        self.browse_session = None;
        let options = self.backend_options.clone();
        let handle = self.handle.clone();
        let browse_handle = handle.clone();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress_tx = event_tx.clone();

        let join = handle.spawn_blocking(move || {
            connect_browse_session(
                target,
                options,
                &startup_context,
                &browse_handle,
                |message| {
                    let _ = progress_tx.send(RemoteManagerTaskEvent::Progress {
                        generation,
                        message,
                    });
                },
            )
        });

        handle.spawn(async move {
            let result = match join.await {
                Ok(result) => result,
                Err(error) => Err(anyhow!("remote browse task failed: {error}")),
            };
            let _ = event_tx.send(RemoteManagerTaskEvent::Finished { generation, result });
        });

        cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                let Some(this) = this.upgrade() else {
                    break;
                };

                let should_break = this.update(cx, |view, cx| match event {
                    RemoteManagerTaskEvent::Progress {
                        generation: event_generation,
                        message,
                    } => {
                        if view.connection_attempt.is_active(event_generation) {
                            view.status = Some(EditorStatus {
                                status: message,
                                severity: Severity::Info,
                            });
                            cx.notify();
                        }
                        false
                    }
                    RemoteManagerTaskEvent::Finished {
                        generation: event_generation,
                        result,
                    } => {
                        if !view.connection_attempt.finish(event_generation) {
                            return true;
                        }

                        match result {
                            Ok(connection) => {
                                let root = remote_directory_root_from_listing(
                                    connection.home_path.clone(),
                                    &connection.listing,
                                );
                                let selected_path = view
                                    .pending_selected_path
                                    .take()
                                    .or(connection.selected_path.clone())
                                    .unwrap_or_else(|| connection.home_path.clone());
                                let selected_tree_path =
                                    remote_tree_contains_path(&root, &selected_path)
                                        .then(|| selected_path.clone());

                                view.browse_session = Some(RemoteBrowseSession {
                                    backend: connection.backend,
                                    selected_path,
                                    selected_tree_path,
                                    root,
                                });
                                view.status = Some(EditorStatus {
                                    status: "Loaded remote home directory".to_string(),
                                    severity: Severity::Info,
                                });
                            }
                            Err(error) => {
                                view.status = Some(EditorStatus {
                                    status: format!("Could not connect: {error:#}"),
                                    severity: Severity::Error,
                                });
                            }
                        }
                        cx.notify();
                        true
                    }
                });

                if should_break {
                    break;
                }
            }
        })
        .detach();

        cx.notify();
    }

    fn select_tree_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(session) = &mut self.browse_session else {
            return;
        };

        if !remote_tree_contains_path(&session.root, &path) {
            return;
        }

        session.selected_tree_path = Some(path.clone());
        session.selected_path = path;
        cx.notify();
    }

    fn toggle_selected_directory(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .browse_session
            .as_ref()
            .and_then(|session| session.selected_tree_path.clone())
        else {
            return;
        };

        self.toggle_tree_directory(path, cx);
    }

    fn collapse_selected_directory(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.browse_session else {
            return;
        };
        let Some(path) = session.selected_tree_path.clone() else {
            return;
        };

        if let Some(node) = remote_tree_node_mut(&mut session.root, &path)
            && node.expanded
        {
            node.expanded = false;
            cx.notify();
            return;
        }

        if let Some(parent) = remote_display_parent(&path)
            && remote_tree_contains_path(&session.root, &parent)
        {
            session.selected_tree_path = Some(parent.clone());
            session.selected_path = parent;
            cx.notify();
        }
    }

    fn expand_selected_directory(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .browse_session
            .as_ref()
            .and_then(|session| session.selected_tree_path.clone())
        else {
            return;
        };

        let first_child_path = self.browse_session.as_ref().and_then(|session| {
            remote_tree_node(&session.root, &path).and_then(|node| {
                node.expanded
                    .then(|| node.children.first().map(|child| child.path.clone()))
                    .flatten()
            })
        });

        if let Some(first_child_path) = first_child_path {
            self.select_tree_path(first_child_path, cx);
        } else {
            self.expand_tree_directory(path, cx);
        }
    }

    fn toggle_tree_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let expanded = self
            .browse_session
            .as_ref()
            .and_then(|session| remote_tree_node(&session.root, &path))
            .is_some_and(|node| node.expanded);

        if expanded {
            if let Some(session) = &mut self.browse_session
                && let Some(node) = remote_tree_node_mut(&mut session.root, &path)
            {
                node.expanded = false;
                cx.notify();
            }
        } else {
            self.expand_tree_directory(path, cx);
        }
    }

    fn expand_tree_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(session) = &mut self.browse_session else {
            return;
        };
        let backend = session.backend.clone();
        let generation = self.connection_attempt.generation();

        let Some(node) = remote_tree_node_mut(&mut session.root, &path) else {
            return;
        };
        if node.expanded || node.loading {
            return;
        }
        if node.loaded {
            node.expanded = true;
            cx.notify();
            return;
        }

        node.loading = true;
        self.status = Some(EditorStatus {
            status: format!("Loading {}", path.display()),
            severity: Severity::Info,
        });

        let path_for_listing = path.clone();

        cx.spawn(async move |this, cx| {
            let listing = backend.list_dir(&path_for_listing).await;
            _ = this.update(cx, |view, cx| {
                if !view.connection_attempt.is_current(generation) {
                    return;
                }

                match listing {
                    Ok(listing) => {
                        if let Some(session) = &mut view.browse_session
                            && let Some(node) = remote_tree_node_mut(&mut session.root, &path)
                        {
                            node.children = remote_directory_nodes_from_listing(&listing);
                            node.loaded = true;
                            node.loading = false;
                            node.expanded = true;
                        }
                        view.status = None;
                    }
                    Err(error) => {
                        if let Some(session) = &mut view.browse_session
                            && let Some(node) = remote_tree_node_mut(&mut session.root, &path)
                        {
                            node.loading = false;
                        }
                        view.status = Some(EditorStatus {
                            status: format!("Could not load directory: {error}"),
                            severity: Severity::Error,
                        });
                    }
                }

                cx.notify();
            });
        })
        .detach();

        cx.notify();
    }

    fn open_selected_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .browse_session
            .as_ref()
            .map(|session| session.selected_path.clone())
        else {
            self.status = Some(EditorStatus {
                status: "Choose a workspace root first".to_string(),
                severity: Severity::Warning,
            });
            cx.notify();
            return;
        };

        if self.save_on_open {
            self.save_connection(&path);
        }

        if let Some(core) = self.core.upgrade() {
            let target = target_to_string(&path);
            let options = self.backend_options.clone();
            core.update(cx, |_core, cx| {
                cx.emit(crate::Update::OpenRemoteWithOptions {
                    input: target,
                    options,
                });
            });
        }

        cx.emit(DismissEvent);
    }

    fn save_connection(&mut self, path: &Path) {
        let mut store = RemoteConnectionStore::load_default().unwrap_or_default();
        let name = generated_connection_name(self.protocol, &self.server_input, path);
        store.save_named(name, target_to_string(path));
        if let Err(error) = store.save_default() {
            self.status = Some(EditorStatus {
                status: format!("Could not save remote connection: {error:#}"),
                severity: Severity::Warning,
            });
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.invalidate_connection_attempt();
        cx.emit(DismissEvent);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(session) = &mut self.browse_session {
            let rows = visible_remote_directory_rows(&session.root);
            if rows.is_empty() {
                cx.notify();
                return;
            }

            let current_index = session
                .selected_tree_path
                .as_ref()
                .and_then(|path| rows.iter().position(|row| &row.path == path))
                .unwrap_or(0);
            let next_index = moved_index(current_index, rows.len(), delta);
            let next_path = rows[next_index].path.clone();
            session.selected_tree_path = Some(next_path.clone());
            session.selected_path = next_path;
        } else {
            let len = self.filtered_suggestions().len();
            self.suggestion_selection = moved_index(self.suggestion_selection, len, delta);
        }
        cx.notify();
    }

    fn accept_or_connect(&mut self, cx: &mut Context<Self>) {
        if self.browse_session.is_some() {
            self.toggle_selected_directory(cx);
            return;
        }

        let suggestions = self.filtered_suggestions();
        if let Some(suggestion) = suggestions.get(self.suggestion_selection).cloned()
            && suggestion.source != RemoteSuggestionSource::Manual
            && !self.accepted_suggestion
        {
            self.apply_suggestion(suggestion, cx);
            return;
        }

        self.connect(cx);
    }

    fn accept_action(
        &mut self,
        _: &AcceptRemoteConnection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.accept_or_connect(cx);
        cx.stop_propagation();
    }

    fn cancel_action(
        &mut self,
        _: &CancelRemoteConnection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.protocol_menu_open {
            self.close_protocol_menu(cx);
        } else {
            self.cancel(cx);
        }
        cx.stop_propagation();
    }

    fn toggle_protocol_action(
        &mut self,
        _: &ToggleRemoteProtocol,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_protocol(self.protocol.toggled(), cx);
        cx.stop_propagation();
    }

    fn select_previous_action(
        &mut self,
        _: &SelectPreviousRemoteItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(-1, cx);
        cx.stop_propagation();
    }

    fn select_next_action(
        &mut self,
        _: &SelectNextRemoteItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(1, cx);
        cx.stop_propagation();
    }

    fn collapse_selected_action(
        &mut self,
        _: &CollapseSelectedRemoteDirectory,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapse_selected_directory(cx);
        cx.stop_propagation();
    }

    fn expand_selected_action(
        &mut self,
        _: &ExpandSelectedRemoteDirectory,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expand_selected_directory(cx);
        cx.stop_propagation();
    }

    fn render_protocol_dropdown(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let manager = cx.entity().clone();
        let protocol_menu = self
            .protocol_menu_open
            .then(|| self.build_protocol_menu(window, cx));

        div()
            .relative()
            .flex_shrink_0()
            .child(
                Button::new("remote-protocol-button", self.protocol.label())
                    .variant(ButtonVariant::Secondary)
                    .size(ButtonSize::Medium)
                    .icon("icons/chevron-down.svg")
                    .icon_position(IconPosition::End)
                    .activate_on_mouse_down()
                    .on_click(move |_event, window, cx| {
                        manager.update(cx, |this, cx| {
                            this.toggle_protocol_menu(window, cx);
                        });
                    }),
            )
            .when_some(protocol_menu, |this, menu| {
                this.child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            .offset(point(px(0.0), px(4.0)))
                            .snap_to_window_with_margin(px(8.0))
                            .child(div().occlude().child(menu)),
                    )
                    .with_priority(500),
                )
            })
    }

    fn render_input_field(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_w(px(0.0))
            .child(self.server_input_view.clone())
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let list_border = cx.theme().tokens.picker_tokens().border;
        let suggestions = self.filtered_suggestions();
        let rows = suggestions
            .into_iter()
            .enumerate()
            .map(|(index, suggestion)| {
                let selected = index == self.suggestion_selection;
                self.render_suggestion_row(index, suggestion, selected, cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .border_1()
            .border_color(list_border)
            .rounded_md()
            .overflow_hidden()
            .child(
                div()
                    .id("remote-suggestions-scroll")
                    .size_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.suggestions_scroll_handle)
                    .child(div().flex().flex_col().children(rows)),
            )
            .when_some(
                Scrollbar::vertical(self.suggestions_scrollbar_state.clone()),
                |container, scrollbar| {
                    container.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(SCROLLBAR_THICKNESS)
                            .child(scrollbar),
                    )
                },
            )
    }

    fn render_suggestion_row(
        &self,
        index: usize,
        suggestion: RemoteServerSuggestion,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let picker_tokens = tokens.picker_tokens();
        let background = if selected {
            tokens.chrome.surface_active
        } else {
            picker_tokens.item_background
        };
        let hover_background = picker_tokens.item_background_hover;
        let text_color = if selected {
            tokens.chrome.text_on_chrome
        } else {
            picker_tokens.item_text
        };
        let secondary_text = if selected {
            tokens.chrome.text_chrome_secondary
        } else {
            picker_tokens.item_text_secondary
        };
        let text_sm = tokens.sizes.text_sm;
        let suggestion_for_click = suggestion.clone();
        let click_listener = cx.listener(move |this, _event, _window, cx| {
            this.apply_suggestion(suggestion_for_click.clone(), cx);
        });

        ListItem::new(("remote-suggestion", index))
            .variant(ListItemVariant::Ghost)
            .spacing(ListItemSpacing::Default)
            .focusable(false)
            .background(background)
            .text_color(text_color)
            .hover_background(if selected {
                background
            } else {
                hover_background
            })
            .with_listener(move |item| {
                item.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, click_listener)
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.0))
                    .child(div().child(suggestion.display_text.clone()))
                    .child(
                        div()
                            .text_size(text_sm)
                            .text_color(secondary_text)
                            .child(suggestion.description.clone()),
                    ),
            )
            .end_slot(
                div()
                    .text_size(text_sm)
                    .text_color(secondary_text)
                    .child(suggestion.source.label()),
            )
    }

    fn render_browse_session(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (chrome_secondary_text, list_border, input_border, input_background, editor_background) = {
            let theme = cx.theme();
            let tokens = &theme.tokens;
            (
                tokens.chrome.text_chrome_secondary,
                tokens.picker_tokens().border,
                tokens.input_tokens().border,
                tokens.input_tokens().background,
                tokens.editor.background,
            )
        };
        let Some(session) = self.browse_session.as_ref() else {
            return div().into_any_element();
        };
        let rows = visible_remote_directory_rows(&session.root);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .gap_3()
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(chrome_secondary_text)
                    .child("Directories"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_1()
                    .border_color(list_border)
                    .rounded_md()
                    .bg(editor_background)
                    .overflow_hidden()
                    .child(
                        div()
                            .id("remote-directory-scroll")
                            .size_full()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .track_scroll(&self.directory_scroll_handle)
                            .child(
                                div().flex().flex_col().children(
                                    rows.into_iter()
                                        .enumerate()
                                        .map(|(index, row)| {
                                            let selected = session.selected_tree_path.as_ref()
                                                == Some(&row.path);
                                            self.render_directory_row(index, row, selected, cx)
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                            ),
                    )
                    .when_some(
                        Scrollbar::vertical(self.directory_scrollbar_state.clone()),
                        |container, scrollbar| {
                            container.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(SCROLLBAR_THICKNESS)
                                    .child(scrollbar),
                            )
                        },
                    ),
            )
            .when(session.root.children.is_empty(), |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_color(chrome_secondary_text)
                        .child("No child directories"),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("Workspace root")
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(input_border)
                            .bg(input_background)
                            .child(short_display_path(&session.selected_path)),
                    ),
            )
            .into_any_element()
    }

    fn render_directory_row(
        &self,
        index: usize,
        row: RemoteDirectoryTreeRow,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let picker_tokens = tokens.picker_tokens();
        let row_path = row.path.clone();
        let row_name = row.name.clone();
        let icon_color = if selected {
            tokens.chrome.text_on_chrome
        } else {
            tokens.chrome.text_chrome_secondary
        };
        let chevron_color = if selected {
            tokens.chrome.text_on_chrome
        } else if row.loading {
            tokens.chrome.text_chrome_secondary
        } else {
            tokens.chrome.text_chrome_disabled
        };
        let text_color = if selected {
            tokens.chrome.text_on_chrome
        } else {
            picker_tokens.item_text
        };
        let background = if selected {
            tokens.chrome.surface_active
        } else {
            picker_tokens.item_background
        };
        let hover_background = picker_tokens.item_background_hover;
        let depth = row.depth;
        let expanded = row.expanded;
        let click_listener = cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
            this.select_tree_path(row_path.clone(), cx);
            this.toggle_tree_directory(row_path.clone(), cx);
        });

        ListItem::new(("remote-directory", index))
            .variant(ListItemVariant::Ghost)
            .spacing(ListItemSpacing::Default)
            .focusable(false)
            .background(background)
            .text_color(text_color)
            .hover_background(if selected {
                background
            } else {
                hover_background
            })
            .with_listener(move |item| {
                item.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, click_listener)
            })
            .start_slot(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(depth as f32 * 16.0)).flex_shrink_0())
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                chevron_icon(if expanded { "down" } else { "right" })
                                    .size(px(14.0))
                                    .text_color(chevron_color),
                            ),
                    )
                    .child(
                        FileIcon::directory(expanded)
                            .size(16.0)
                            .text_color(icon_color),
                    ),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .ml(px(4.0))
                    .whitespace_nowrap()
                    .child(row_name),
            )
    }

    fn render_save_connection_checkbox(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = cx.entity().clone();

        Checkbox::new("remote-save-connection", "Save this connection")
            .checked(self.save_on_open)
            .size(CheckboxSize::Medium)
            .focus_handle(self.save_connection_focus_handle.clone())
            .on_change(move |checked, _window, cx| {
                manager.update(cx, |this, cx| {
                    this.save_on_open = checked;
                    cx.notify();
                });
            })
    }

    fn render_text_button<F>(
        &self,
        label: &'static str,
        icon: &'static str,
        cx: &mut Context<Self>,
        listener: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut Self, &mut Context<Self>) + 'static,
    {
        let manager = cx.entity().clone();
        Button::new(label, label)
            .variant(ButtonVariant::Secondary)
            .size(ButtonSize::Medium)
            .icon(icon)
            .activate_on_mouse_down()
            .on_click(move |_event, _window, cx| {
                manager.update(cx, |this, cx| {
                    listener(this, cx);
                });
            })
    }

    fn render_status(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let Some(status) = &self.status else {
            return div().into_any_element();
        };

        let color = match status.severity {
            Severity::Error => tokens.editor.error,
            Severity::Warning => tokens.editor.warning,
            Severity::Hint => tokens.editor.text_secondary,
            Severity::Info => tokens.editor.text_secondary,
        };

        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(tokens.chrome.surface_hover)
            .text_color(color)
            .child(status.status.clone())
            .into_any_element()
    }
}

impl Focusable for RemoteConnectionManagerView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for RemoteConnectionManagerView {}

impl Drop for RemoteConnectionManagerView {
    fn drop(&mut self) {
        self.invalidate_connection_attempt();
    }
}

impl Render for RemoteConnectionManagerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (
            picker_border,
            picker_background,
            chrome_text,
            chrome_secondary_text,
            text_md,
            text_lg,
            text_sm,
            shadow,
        ) = {
            let theme = cx.theme();
            let tokens = &theme.tokens;
            (
                tokens.picker_tokens().border,
                tokens.picker_tokens().container_background,
                tokens.chrome.text_on_chrome,
                tokens.chrome.text_chrome_secondary,
                tokens.sizes.text_md,
                tokens.sizes.text_lg,
                tokens.sizes.text_sm,
                vec![tokens.chrome.shadow_lg.to_box_shadow(false)],
            )
        };
        let font = gpui::Font {
            family: cx
                .global::<nucleotide_types::UiFontConfig>()
                .family
                .clone()
                .into(),
            features: gpui::FontFeatures::default(),
            weight: cx.global::<nucleotide_types::UiFontConfig>().weight.into(),
            style: gpui::FontStyle::Normal,
            fallbacks: None,
        };

        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }

        div()
            .key_context(REMOTE_CONNECTION_MANAGER_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::accept_action))
            .on_action(cx.listener(Self::cancel_action))
            .on_action(cx.listener(Self::toggle_protocol_action))
            .on_action(cx.listener(Self::select_previous_action))
            .on_action(cx.listener(Self::select_next_action))
            .on_action(cx.listener(Self::collapse_selected_action))
            .on_action(cx.listener(Self::expand_selected_action))
            .on_action(cx.listener(Self::select_remote_protocol))
            .flex()
            .flex_col()
            .gap_4()
            .w(px(720.0))
            .h(px(620.0))
            .max_h(px(620.0))
            .p_4()
            .rounded_md()
            .border_1()
            .border_color(picker_border)
            .bg(picker_background)
            .overflow_hidden()
            .text_color(chrome_text)
            .font(font)
            .text_size(text_md)
            .shadow(shadow)
            .child(
                div().flex().flex_shrink_0().items_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(text_lg)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Remote Connection"),
                        )
                        .child(
                            div()
                                .text_size(text_sm)
                                .text_color(chrome_secondary_text)
                                .child("Choose a host or distro, then browse for a workspace root"),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .child(self.render_protocol_dropdown(window, cx))
                    .child(self.render_input_field())
                    .child(
                        self.render_text_button("Connect", "icons/link.svg", cx, |this, cx| {
                            this.connect(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .when(self.browse_session.is_none(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_hidden()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(text_sm)
                                        .text_color(chrome_secondary_text)
                                        .child(match self.protocol {
                                            RemoteConnectionProtocol::Ssh => "Matching SSH targets",
                                            RemoteConnectionProtocol::Wsl => {
                                                "Matching WSL distributions"
                                            }
                                        }),
                                )
                                .child(self.render_suggestions(cx)),
                        )
                    })
                    .when(self.browse_session.is_some(), |this| {
                        this.child(self.render_browse_session(cx))
                    }),
            )
            .when(self.status.is_some(), |this| {
                this.child(div().flex_shrink_0().child(self.render_status(cx)))
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .child(self.render_save_connection_checkbox(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.render_text_button(
                                "Cancel",
                                "icons/circle-x.svg",
                                cx,
                                |this, cx| {
                                    this.cancel(cx);
                                },
                            ))
                            .child(self.render_text_button(
                                "Open",
                                "icons/folder-open.svg",
                                cx,
                                |this, cx| {
                                    this.open_selected_workspace(cx);
                                },
                            )),
                    ),
            )
    }
}

fn connect_browse_session(
    target: RemoteConnectTarget,
    options: nucleotide_remote::RemoteWorkspaceBackendOptions,
    startup: &nucleotide_remote::RemoteStartupContext,
    runtime: &tokio::runtime::Handle,
    progress: impl Fn(String),
) -> Result<RemoteBrowseConnection> {
    startup.check()?;
    let (display_home, selected_path) = match target {
        RemoteConnectTarget::Ssh {
            target,
            selected_path,
        } => {
            progress(format!("Connecting to SSH host: {}", target.host));
            let home = resolve_ssh_home(&target, &options, startup)?;
            (ssh_display_path(&target, &home), selected_path)
        }
        RemoteConnectTarget::Wsl {
            distro,
            selected_path,
        } => {
            progress(format!("Connecting to WSL distro: {distro}"));
            let home = resolve_wsl_home(&distro, startup)?;
            (wsl_display_path(&distro, &home), selected_path)
        }
    };

    startup.check()?;
    progress("Starting remote browse session".to_string());
    let backend =
        workspace_backend_for_project_directory_with_options_progress_and_startup_context(
            Some(&display_home),
            &options,
            &|p| {
                progress(p.message());
            },
            startup,
        )?;

    startup.check()?;
    progress("Loading remote home directory".to_string());
    let listing = block_on_remote_startup_future(runtime, startup, backend.list_dir(&display_home))
        .with_context(|| format!("failed to list remote home {}", display_home.display()))?;
    startup.check()?;

    Ok(RemoteBrowseConnection {
        backend,
        home_path: display_home,
        selected_path,
        listing,
    })
}

fn resolve_ssh_home(
    target: &SshWorkspaceTarget,
    options: &nucleotide_remote::RemoteWorkspaceBackendOptions,
    startup: &nucleotide_remote::RemoteStartupContext,
) -> Result<PathBuf> {
    startup.check()?;
    let ssh_target = nucleotide_remote::SshTarget {
        host: target.host.clone(),
        user: target.user.clone(),
        port: target.port,
        connect_timeout_secs: options.ssh_connect_timeout_secs,
        extra_args: options.ssh_extra_args.clone(),
        control_path: options.ssh_control_path.clone(),
    };
    let command =
        nucleotide_remote::ssh_non_tty_remote_command(ssh_target, "printf '%s\\n' \"$HOME\"");
    let mut process = command.command();
    process.stdin(std::process::Stdio::null());
    let output = nucleotide_process::output_with_limits_and_cancellation(
        &mut process,
        nucleotide_process::OutputLimits::new(
            startup.cap_timeout(nucleotide_remote::REMOTE_STARTUP_PROBE_TIMEOUT)?,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
        ),
        startup.cancellation().as_atomic_bool(),
    );
    startup.check()?;
    let output = output.with_context(|| format!("failed to run {}", command.display_context()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "SSH home directory probe failed{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }

    let home = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if home.is_empty() {
        return Err(anyhow!("SSH host returned an empty home directory"));
    }

    Ok(PathBuf::from(home))
}

fn resolve_wsl_home(
    distro: &str,
    startup: &nucleotide_remote::RemoteStartupContext,
) -> Result<PathBuf> {
    startup.check()?;
    let mut command = nucleotide_process::command("wsl.exe");
    command.args([
        OsString::from("--distribution"),
        OsString::from(distro),
        OsString::from("--exec"),
        OsString::from("sh"),
        OsString::from("-lc"),
        OsString::from("printf '%s\\n' \"$HOME\""),
    ]);
    command.stdin(std::process::Stdio::null());
    let output = nucleotide_process::output_with_limits_and_cancellation(
        &mut command,
        nucleotide_process::OutputLimits::new(
            startup.cap_timeout(nucleotide_remote::REMOTE_STARTUP_PROBE_TIMEOUT)?,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
        ),
        startup.cancellation().as_atomic_bool(),
    );
    startup.check()?;
    let output = output.with_context(|| "failed to run wsl.exe for home directory discovery")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!(
            "WSL home directory probe failed{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }

    let home = decode_wsl_stdout(&output.stdout).trim().to_string();
    if home.is_empty() {
        return Err(anyhow!("WSL distro returned an empty home directory"));
    }

    Ok(PathBuf::from(home))
}

fn block_on_remote_startup_future<T, E, F>(
    runtime: &tokio::runtime::Handle,
    startup: &nucleotide_remote::RemoteStartupContext,
    future: F,
) -> std::result::Result<T, anyhow::Error>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: Into<anyhow::Error>,
{
    runtime.block_on(async {
        tokio::pin!(future);
        tokio::select! {
            biased;
            error = wait_for_remote_startup_stop(startup) => Err(error),
            result = &mut future => {
                startup.check()?;
                result.map_err(Into::into)
            },
        }
    })
}

async fn wait_for_remote_startup_stop(
    startup: &nucleotide_remote::RemoteStartupContext,
) -> anyhow::Error {
    loop {
        match startup.remaining() {
            Ok(remaining) => {
                tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
            }
            Err(error) => return error,
        }
    }
}

fn load_remote_server_suggestions() -> Vec<RemoteServerSuggestion> {
    let mut suggestions = Vec::new();

    if let Ok(store) = RemoteConnectionStore::load_default() {
        suggestions.extend(store.saved.iter().filter_map(|entry| {
            suggestion_from_target(
                &entry.target,
                RemoteSuggestionSource::Saved,
                Some(entry.name.clone()),
            )
        }));
        suggestions.extend(store.recent.iter().filter_map(|entry| {
            suggestion_from_target(&entry.target, RemoteSuggestionSource::Recent, None)
        }));
    }

    suggestions.extend(
        ssh_config_aliases()
            .into_iter()
            .map(|host| ssh_host_suggestion(host, RemoteSuggestionSource::SshConfig)),
    );
    suggestions.extend(
        known_hosts()
            .into_iter()
            .map(|host| ssh_host_suggestion(host, RemoteSuggestionSource::KnownHost)),
    );
    dedupe_suggestions(suggestions)
}

fn with_wsl_distribution_suggestions(
    mut suggestions: Vec<RemoteServerSuggestion>,
    distributions: Vec<String>,
) -> Vec<RemoteServerSuggestion> {
    suggestions.extend(
        distributions
            .into_iter()
            .map(|distro| RemoteServerSuggestion {
                protocol: RemoteConnectionProtocol::Wsl,
                insert_text: distro.clone(),
                display_text: distro,
                description: "Installed WSL distribution".to_string(),
                source: RemoteSuggestionSource::WslDistro,
                target_path: None,
            }),
    );
    dedupe_suggestions(suggestions)
}

fn suggestion_from_target(
    target: &str,
    source: RemoteSuggestionSource,
    name: Option<String>,
) -> Option<RemoteServerSuggestion> {
    match classify_workspace_location(Path::new(target)) {
        nucleotide_workspace::WorkspaceLocation::Ssh {
            target: ssh_target,
            original_path,
            path,
        } => {
            let server = ssh_server_input(&ssh_target);
            Some(RemoteServerSuggestion {
                protocol: RemoteConnectionProtocol::Ssh,
                insert_text: server.clone(),
                display_text: name.unwrap_or(server),
                description: path.display().to_string(),
                source,
                target_path: Some(original_path),
            })
        }
        nucleotide_workspace::WorkspaceLocation::Wsl {
            distro,
            original_path,
            linux_path,
        } => Some(RemoteServerSuggestion {
            protocol: RemoteConnectionProtocol::Wsl,
            insert_text: distro.clone(),
            display_text: name.unwrap_or(distro),
            description: linux_path.display().to_string(),
            source,
            target_path: Some(original_path),
        }),
        nucleotide_workspace::WorkspaceLocation::Local { .. } => None,
    }
}

fn ssh_host_suggestion(host: String, source: RemoteSuggestionSource) -> RemoteServerSuggestion {
    RemoteServerSuggestion {
        protocol: RemoteConnectionProtocol::Ssh,
        insert_text: host.clone(),
        display_text: host,
        description: "SSH target".to_string(),
        source,
        target_path: None,
    }
}

fn dedupe_suggestions(suggestions: Vec<RemoteServerSuggestion>) -> Vec<RemoteServerSuggestion> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for suggestion in suggestions {
        let key = (
            suggestion.protocol,
            suggestion.insert_text.clone(),
            suggestion.target_path.clone(),
        );
        if seen.insert(key) {
            deduped.push(suggestion);
        }
    }

    deduped.sort_by_key(|suggestion| {
        (
            suggestion.protocol != RemoteConnectionProtocol::Ssh,
            suggestion.source,
            suggestion.display_text.clone(),
        )
    });
    deduped
}

fn ssh_config_aliases() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    std::fs::read_to_string(home.join(".ssh/config"))
        .map(|contents| parse_ssh_config_aliases(&contents))
        .unwrap_or_default()
}

fn known_hosts() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    std::fs::read_to_string(home.join(".ssh/known_hosts"))
        .map(|contents| parse_known_hosts(&contents))
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn wsl_distributions() -> Vec<String> {
    use std::time::Duration;

    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    let mut command = nucleotide_process::command("wsl.exe");
    command
        .args([OsString::from("--list"), OsString::from("--quiet")])
        .stdin(std::process::Stdio::null());
    nucleotide_process::output_with_limits(
        &mut command,
        nucleotide_process::OutputLimits::new(
            DISCOVERY_TIMEOUT,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
            nucleotide_remote::REMOTE_STARTUP_OUTPUT_LIMIT,
        ),
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| parse_wsl_distribution_list(&output.stdout))
    .unwrap_or_default()
}

#[cfg(not(target_os = "windows"))]
fn wsl_distributions() -> Vec<String> {
    Vec::new()
}

fn parse_ssh_server_input(input: &str) -> Option<SshWorkspaceTarget> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let uri = if input.to_ascii_lowercase().starts_with("ssh://") {
        input.to_string()
    } else {
        format!("ssh://{input}/")
    };

    match classify_workspace_location(Path::new(&uri)) {
        nucleotide_workspace::WorkspaceLocation::Ssh { target, .. } => Some(target),
        _ => None,
    }
}

fn ssh_server_input(target: &SshWorkspaceTarget) -> String {
    let mut server = String::new();
    if let Some(user) = &target.user {
        server.push_str(user);
        server.push('@');
    }
    if target.host.contains(':') {
        server.push('[');
        server.push_str(&target.host);
        server.push(']');
    } else {
        server.push_str(&target.host);
    }
    if let Some(port) = target.port {
        server.push(':');
        server.push_str(&port.to_string());
    }
    server
}

fn parse_ssh_config_aliases(contents: &str) -> Vec<String> {
    let mut aliases = BTreeSet::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }

        for alias in rest.split_whitespace() {
            if alias.starts_with('!')
                || alias.contains('*')
                || alias.contains('?')
                || alias.eq_ignore_ascii_case("none")
            {
                continue;
            }
            aliases.insert(alias.to_string());
        }
    }

    aliases.into_iter().collect()
}

fn parse_known_hosts(contents: &str) -> Vec<String> {
    let mut hosts = BTreeSet::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('|') {
            continue;
        }

        let Some(hosts_field) = line.split_whitespace().next() else {
            continue;
        };
        for host in hosts_field.split(',') {
            if host.starts_with('|') || host.contains('*') || host.contains('?') {
                continue;
            }
            if let Some(host) = normalize_known_host(host) {
                hosts.insert(host);
            }
        }
    }

    hosts.into_iter().collect()
}

fn normalize_known_host(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }

    if let Some(rest) = value.strip_prefix('[')
        && let Some((host, _port)) = rest.split_once("]:")
    {
        return (!host.is_empty()).then(|| host.to_string());
    }

    Some(value.to_string())
}

#[cfg(any(target_os = "windows", test))]
fn parse_wsl_distribution_list(stdout: &[u8]) -> Vec<String> {
    decode_wsl_stdout(stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(" (Default)").to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn decode_wsl_stdout(stdout: &[u8]) -> String {
    if stdout.len() >= 2 {
        let pairs = stdout.chunks_exact(2).collect::<Vec<_>>();
        let nul_high_bytes = pairs.iter().filter(|pair| pair[1] == 0).count();
        if nul_high_bytes * 2 >= pairs.len() {
            let units = pairs
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            if let Ok(decoded) = String::from_utf16(&units) {
                return decoded
                    .strip_prefix('\u{feff}')
                    .unwrap_or(&decoded)
                    .replace('\r', "");
            }
        }
    }

    let decoded = String::from_utf8_lossy(stdout);
    decoded
        .strip_prefix('\u{feff}')
        .unwrap_or(&decoded)
        .replace(['\0', '\r'], "")
}

fn remote_directory_root_from_listing(
    path: PathBuf,
    listing: &DirectoryListing,
) -> RemoteDirectoryNode {
    RemoteDirectoryNode {
        name: remote_display_name(&path),
        path,
        expanded: true,
        loading: false,
        loaded: true,
        children: remote_directory_nodes_from_listing(listing),
    }
}

fn remote_directory_nodes_from_listing(listing: &DirectoryListing) -> Vec<RemoteDirectoryNode> {
    let mut nodes = listing
        .entries
        .iter()
        .filter(|entry| entry.stat.kind == FileKind::Directory)
        .map(|entry| RemoteDirectoryNode {
            name: entry.name.clone(),
            path: entry.path.clone(),
            expanded: false,
            loading: false,
            loaded: false,
            children: Vec::new(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.name.to_ascii_lowercase());
    nodes
}

fn visible_remote_directory_rows(root: &RemoteDirectoryNode) -> Vec<RemoteDirectoryTreeRow> {
    let mut rows = Vec::new();
    push_visible_remote_directory_rows(root, 0, &mut rows);
    rows
}

fn push_visible_remote_directory_rows(
    node: &RemoteDirectoryNode,
    depth: usize,
    rows: &mut Vec<RemoteDirectoryTreeRow>,
) {
    rows.push(RemoteDirectoryTreeRow {
        name: node.name.clone(),
        path: node.path.clone(),
        depth,
        expanded: node.expanded,
        loading: node.loading,
    });

    if node.expanded {
        for child in &node.children {
            push_visible_remote_directory_rows(child, depth + 1, rows);
        }
    }
}

fn remote_tree_node<'a>(
    node: &'a RemoteDirectoryNode,
    path: &Path,
) -> Option<&'a RemoteDirectoryNode> {
    if node.path == path {
        return Some(node);
    }

    node.children
        .iter()
        .find_map(|child| remote_tree_node(child, path))
}

fn remote_tree_node_mut<'a>(
    node: &'a mut RemoteDirectoryNode,
    path: &Path,
) -> Option<&'a mut RemoteDirectoryNode> {
    if node.path == path {
        return Some(node);
    }

    node.children
        .iter_mut()
        .find_map(|child| remote_tree_node_mut(child, path))
}

fn remote_tree_contains_path(node: &RemoteDirectoryNode, path: &Path) -> bool {
    remote_tree_node(node, path).is_some()
}

fn remote_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .or_else(|| {
            path.components()
                .next_back()
                .and_then(|component| component.as_os_str().to_str())
        })
        .unwrap_or(".")
        .to_string()
}

fn remote_display_parent(path: &Path) -> Option<PathBuf> {
    match classify_workspace_location(path) {
        nucleotide_workspace::WorkspaceLocation::Ssh {
            target,
            path: native_path,
            ..
        } => native_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| ssh_display_path(&target, parent)),
        nucleotide_workspace::WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => linux_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| wsl_display_path(&distro, parent)),
        nucleotide_workspace::WorkspaceLocation::Local { path } => {
            path.parent().map(Path::to_path_buf)
        }
    }
}

fn short_display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn moved_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }

    if delta < 0 {
        current.saturating_sub(usize::try_from(-delta).unwrap_or(0))
    } else {
        (current + usize::try_from(delta).unwrap_or(0)).min(len - 1)
    }
}

fn generated_connection_name(
    protocol: RemoteConnectionProtocol,
    server_input: &str,
    path: &Path,
) -> String {
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("remote");
    let prefix = match protocol {
        RemoteConnectionProtocol::Ssh => server_input.trim(),
        RemoteConnectionProtocol::Wsl => server_input.trim(),
    };
    let raw = if prefix.is_empty() {
        leaf.to_string()
    } else {
        format!("{prefix}-{leaf}")
    };
    let cleaned = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    if valid_connection_name(&cleaned) {
        cleaned
    } else {
        "remote-project".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_workspace::{DirectoryEntry, FileStat};
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::{Context as TaskContext, Poll};

    struct PendingStartupFuture {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Future for PendingStartupFuture {
        type Output = std::result::Result<(), anyhow::Error>;

        fn poll(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
            self.polled.store(true, Ordering::Release);
            Poll::Pending
        }
    }

    impl Drop for PendingStartupFuture {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn connection_attempt_state_rejects_duplicates_and_stale_completion() {
        let mut state = ConnectionAttemptState::default();

        let (first, first_startup) = state
            .begin(Duration::from_secs(5))
            .expect("first attempt should start");
        assert!(state.begin(Duration::from_secs(5)).is_none());

        state.invalidate();
        assert!(nucleotide_remote::remote_startup_was_cancelled(
            &first_startup.check().unwrap_err()
        ));
        let (replacement, replacement_startup) = state
            .begin(Duration::from_secs(5))
            .expect("replacement attempt should start");
        assert_ne!(replacement, first);
        assert!(!state.finish(first));
        assert!(state.is_active(replacement));
        assert!(replacement_startup.check().is_ok());
        assert!(state.finish(replacement));
        assert!(!state.is_active(replacement));
        assert!(replacement_startup.check().is_ok());
    }

    #[test]
    fn startup_cancellation_drops_blocked_home_listing() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();
        let startup = nucleotide_remote::RemoteStartupContext::new(Duration::from_secs(5));
        let worker_startup = startup.clone();
        let polled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let future = PendingStartupFuture {
            polled: Arc::clone(&polled),
            dropped: Arc::clone(&dropped),
        };
        let worker = std::thread::spawn(move || {
            block_on_remote_startup_future(&handle, &worker_startup, future)
        });
        let started = std::time::Instant::now();
        while !polled.load(Ordering::Acquire) {
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        }

        startup.cancel();
        let error = worker.join().unwrap().unwrap_err();

        assert!(nucleotide_remote::remote_startup_was_cancelled(&error));
        assert!(dropped.load(Ordering::Acquire));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wsl_distributions_merge_without_blocking_suggestion_loading() {
        let initial = vec![ssh_host_suggestion(
            "devbox".to_string(),
            RemoteSuggestionSource::SshConfig,
        )];

        let suggestions = with_wsl_distribution_suggestions(
            initial,
            vec![
                "Ubuntu".to_string(),
                "Ubuntu".to_string(),
                "Debian".to_string(),
            ],
        );

        assert_eq!(
            suggestions
                .iter()
                .filter(|suggestion| suggestion.protocol == RemoteConnectionProtocol::Wsl)
                .map(|suggestion| suggestion.insert_text.as_str())
                .collect::<Vec<_>>(),
            vec!["Debian", "Ubuntu"]
        );
        assert!(suggestions.iter().any(|suggestion| {
            suggestion.protocol == RemoteConnectionProtocol::Ssh
                && suggestion.insert_text == "devbox"
        }));
    }

    #[test]
    fn ssh_config_alias_parser_skips_wildcards_and_negations() {
        let aliases = parse_ssh_config_aliases(
            r#"
            Host devbox *.internal !blocked
              HostName dev.example.com
            Host staging prod
            "#,
        );

        assert_eq!(aliases, vec!["devbox", "prod", "staging"]);
    }

    #[test]
    fn known_hosts_parser_skips_hashed_hosts_and_normalizes_bracket_ports() {
        let hosts = parse_known_hosts(
            r#"
            |1|hashed|value ssh-ed25519 AAAA
            [dev.example.com]:2222 ssh-ed25519 AAAA
            localhost,127.0.0.1 ssh-ed25519 AAAA
            "#,
        );

        assert_eq!(hosts, vec!["127.0.0.1", "dev.example.com", "localhost"]);
    }

    #[test]
    fn wsl_distribution_parser_handles_nul_padded_output() {
        let output = "Ubuntu-24.04\r\nDebian (Default)\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let distros = parse_wsl_distribution_list(&output);

        assert_eq!(distros, vec!["Debian", "Ubuntu-24.04"]);
    }

    #[test]
    fn wsl_distribution_parser_strips_utf16_le_bom() {
        let output = "\u{feff}Ubuntu-24.04\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        let distros = parse_wsl_distribution_list(&output);

        assert_eq!(distros, vec!["Debian", "Ubuntu-24.04"]);
        assert!(distros.iter().all(|distro| !distro.starts_with('\u{feff}')));
    }

    #[test]
    fn wsl_stdout_decoder_strips_nul_and_carriage_return_from_utf8() {
        let decoded = decode_wsl_stdout(b"Ubuntu\0\r\nDebian\r\n");

        assert_eq!(decoded, "Ubuntu\nDebian\n");
    }

    #[test]
    fn parses_ssh_server_input_with_user_and_port() {
        let target = parse_ssh_server_input("me@example.com:2222").unwrap();

        assert_eq!(target.host, "example.com");
        assert_eq!(target.user.as_deref(), Some("me"));
        assert_eq!(target.port, Some(2222));
    }

    #[test]
    fn generated_connection_names_are_store_safe() {
        let name = generated_connection_name(
            RemoteConnectionProtocol::Ssh,
            "me@example.com:2222",
            Path::new("ssh://me@example.com:2222/home/me/project"),
        );

        assert!(valid_connection_name(&name));
        assert_eq!(name, "me-example.com-2222-project");
    }

    #[test]
    fn directory_tree_root_filters_and_sorts_child_directories() {
        let listing = test_listing(
            "/home/me",
            &[
                ("zeta", FileKind::Directory),
                ("readme.md", FileKind::File),
                ("alpha", FileKind::Directory),
            ],
        );

        let root = remote_directory_root_from_listing(PathBuf::from("/home/me"), &listing);
        let rows = visible_remote_directory_rows(&root);

        assert_eq!(root.name, "me");
        assert_eq!(root.children.len(), 2);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, PathBuf::from("/home/me"));
        assert_eq!(rows[1].name, "alpha");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].name, "zeta");
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn directory_tree_visible_rows_follow_expansion_state() {
        let mut root = RemoteDirectoryNode {
            name: "me".to_string(),
            path: PathBuf::from("/home/me"),
            expanded: true,
            loading: false,
            loaded: true,
            children: vec![RemoteDirectoryNode {
                name: "projects".to_string(),
                path: PathBuf::from("/home/me/projects"),
                expanded: true,
                loading: false,
                loaded: true,
                children: vec![RemoteDirectoryNode {
                    name: "nucleotide".to_string(),
                    path: PathBuf::from("/home/me/projects/nucleotide"),
                    expanded: false,
                    loading: false,
                    loaded: false,
                    children: Vec::new(),
                }],
            }],
        };

        let expanded_rows = visible_remote_directory_rows(&root);
        assert_eq!(
            expanded_rows
                .iter()
                .map(|row| (row.name.as_str(), row.depth))
                .collect::<Vec<_>>(),
            vec![("me", 0), ("projects", 1), ("nucleotide", 2)]
        );

        remote_tree_node_mut(&mut root, Path::new("/home/me/projects"))
            .unwrap()
            .expanded = false;
        let collapsed_rows = visible_remote_directory_rows(&root);

        assert_eq!(
            collapsed_rows
                .iter()
                .map(|row| (row.name.as_str(), row.depth))
                .collect::<Vec<_>>(),
            vec![("me", 0), ("projects", 1)]
        );
        assert!(remote_tree_contains_path(
            &root,
            Path::new("/home/me/projects/nucleotide")
        ));
    }

    fn test_listing(path: &str, entries: &[(&str, FileKind)]) -> DirectoryListing {
        let root = PathBuf::from(path);
        DirectoryListing {
            path: root.clone(),
            entries: entries
                .iter()
                .map(|(name, kind)| {
                    let entry_path = root.join(name);
                    DirectoryEntry {
                        name: (*name).to_string(),
                        path: entry_path.clone(),
                        stat: FileStat {
                            path: entry_path,
                            kind: *kind,
                            size: 0,
                            modified: None,
                            readonly: false,
                        },
                        symlink_target: None,
                        target_exists: None,
                        ignored: None,
                    }
                })
                .collect(),
        }
    }
}
