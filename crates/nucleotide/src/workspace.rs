use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::FontFeatures;
use gpui::{
    black, div, hsla, px, svg, transparent_black, white, App, AppContext, BorrowAppContext,
    Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, TextStyle, Window, WindowAppearance,
    WindowBackgroundAppearance,
};
use helix_core::Selection;
use helix_view::ViewId;
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};
use nucleotide_logging::{debug, error, info, instrument, warn};
use nucleotide_ui::{Button, ButtonSize, ButtonVariant};

use crate::application::find_workspace_root_from;
use crate::document::DocumentView;
use crate::file_tree::{FileTreeConfig, FileTreeEvent, FileTreeView};
use crate::info_box::InfoBoxView;
use crate::key_hint_view::KeyHintView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
use crate::vcs_service::VcsServiceHandle;
use crate::{Core, Input, InputEvent};

pub struct Workspace {
    core: Entity<Core>,
    input: Entity<Input>,
    focused_view_id: Option<ViewId>,
    documents: HashMap<ViewId, Entity<DocumentView>>,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    info: Entity<InfoBoxView>,
    info_hidden: bool,
    key_hints: Entity<KeyHintView>,
    notifications: Entity<NotificationView>,
    focus_handle: FocusHandle,
    needs_focus_restore: bool,
    file_tree: Option<Entity<FileTreeView>>,
    show_file_tree: bool,
    file_tree_width: f32,
    is_resizing_file_tree: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    titlebar: Option<gpui::AnyView>,
    appearance_observer_set: bool,
    needs_appearance_update: bool,
}

impl EventEmitter<crate::Update> for Workspace {}

impl Workspace {
    pub fn current_filename(&self, cx: &App) -> Option<String> {
        let editor = &self.core.read(cx).editor;

        // Get the currently focused view
        for (view, is_focused) in editor.tree.views() {
            if is_focused {
                if let Some(doc) = editor.document(view.doc) {
                    return doc.path().map(|p| {
                        p.file_name()
                            .and_then(|name| name.to_str())
                            .map(std::string::ToString::to_string)
                            .unwrap_or_else(|| p.display().to_string())
                    });
                }
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
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Subscribe to overlay dismiss events to restore focus
        cx.subscribe(
            &overlay,
            |workspace, _overlay, _event: &DismissEvent, cx| {
                // Mark that we need to restore focus in the next render
                workspace.needs_focus_restore = true;
                cx.notify();
            },
        )
        .detach();

        // Subscribe to core (Application) events to receive Update events
        cx.subscribe(&core, |workspace, _core, event: &crate::Update, cx| {
            info!("Workspace: Received Update event from core: {:?}", event);
            workspace.handle_event(event, cx);
        })
        .detach();

        // Note: Window appearance observation needs to be set up after window creation
        // It will be handled in the render method when window is available

        let key_hints = cx.new(|_cx| KeyHintView::new());

        // Initialize file tree only if project directory is explicitly set
        let root_path = core.read(cx).project_directory.clone();

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
                info!("Workspace: Received file tree event: {:?}", event);
                workspace.handle_file_tree_event(event, cx);
            })
            .detach();
        } else {
            info!("Workspace: No file tree to subscribe to");
        }

        let mut workspace = Self {
            core,
            input,
            focused_view_id: None,
            documents: HashMap::new(),
            handle,
            overlay,
            info,
            info_hidden: true,
            key_hints,
            notifications,
            focus_handle,
            needs_focus_restore: false,
            file_tree,
            show_file_tree: true,
            file_tree_width: 250.0, // Default width
            is_resizing_file_tree: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            titlebar: None,
            appearance_observer_set: false,
            needs_appearance_update: false,
        };
        // Initialize document views
        workspace.update_document_views(cx);

        // Auto-focus the first document view on startup
        if workspace.focused_view_id.is_some() {
            workspace.needs_focus_restore = true;
        }

        workspace
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
        self.core.update(cx, |core, _cx| {
            core.project_directory = Some(dir.clone());
        });

        // Start VCS monitoring for the new directory
        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
        vcs_handle.update(cx, |service, cx| {
            service.start_monitoring(dir, cx);
        });
    }

    // Removed - views are created in main.rs and passed in

    // Removed - views are created in main.rs and passed in

    pub fn theme(editor: &Entity<Core>, cx: &mut Context<Self>) -> helix_view::Theme {
        editor.read(cx).editor.theme.clone()
    }

    fn handle_appearance_change(
        &mut self,
        appearance: WindowAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::config::ThemeMode;
        use nucleotide_ui::theme_manager::SystemAppearance;

        // Update system appearance in theme manager
        let system_appearance = match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
        };

        cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
            theme_manager.set_system_appearance(system_appearance);
        });

        // Check if we should switch themes based on configuration
        let config = self.core.read(cx).config.clone();
        if config.gui.theme.mode == ThemeMode::System {
            // Determine which theme to use
            let theme_name = match system_appearance {
                SystemAppearance::Light => Some(config.gui.theme.get_light_theme()),
                SystemAppearance::Dark => Some(config.gui.theme.get_dark_theme()),
            };

            // Switch to the appropriate theme if specified
            if let Some(theme_name) = theme_name {
                self.switch_theme_by_name(&theme_name, window, cx);
            }
        }

        // Update window appearance if configured
        if config.gui.window.appearance_follows_theme {
            self.update_window_appearance(window, cx);
        }
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
                    *ui_theme = new_ui_theme;
                });

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
        let theme_manager = cx.global::<crate::ThemeManager>();
        let is_dark = theme_manager.is_dark_theme();

        // Set window background appearance based on theme
        let appearance = if is_dark && config.gui.window.blur_dark_themes {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        };

        window.set_background_appearance(appearance);
    }

    fn clear_shaped_lines_cache(&self, cx: &Context<Self>) {
        if let Some(line_cache) = cx.try_global::<nucleotide_editor::LineLayoutCache>() {
            line_cache.clear_shaped_lines();
        }
    }

    // Event handler methods extracted from the main handle_event
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
        if let Some(view) = self.focused_view_id.and_then(|id| self.documents.get(&id)) {
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

        // Sync file tree selection with the newly opened document
        let doc_path = {
            let core = self.core.read(cx);
            core.editor
                .document(doc_id)
                .and_then(|doc| doc.path())
                .map(|p| p.to_path_buf())
        };

        if let Some(path) = doc_path {
            if let Some(file_tree) = &self.file_tree {
                file_tree.update(cx, |tree, cx| {
                    tree.sync_selection_with_file(Some(path.as_path()), cx);
                });
            }
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
        self.focused_view_id = Some(view_id);

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

        if let Some(path) = doc_path {
            if let Some(file_tree) = &self.file_tree {
                file_tree.update(cx, |tree, cx| {
                    tree.sync_selection_with_file(Some(path.as_path()), cx);
                });
            }
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

        // Only show completion for certain triggers (not every character)
        match trigger {
            crate::types::CompletionTrigger::Manual => {
                // Always show for manual triggers
                self.trigger_completion(cx);
            }
            crate::types::CompletionTrigger::Character(c) => {
                // Only trigger for certain characters that typically start identifiers
                if c.is_alphabetic() || *c == '_' || *c == '.' {
                    self.trigger_completion(cx);
                }
            }
            crate::types::CompletionTrigger::Automatic => {
                // Re-filter existing completion
                self.trigger_completion(cx);
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
        use nucleotide_core::{command_system::SplitDirection, Command};

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

            // If the theme has changed, update the ThemeManager and UI theme
            if theme_before != theme_name_after {
                // Update the global ThemeManager
                cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
                    theme_manager.set_theme(current_theme.clone());
                });

                // Update the global UI theme
                let new_ui_theme = cx.global::<crate::ThemeManager>().ui_theme().clone();

                cx.update_global(|_ui_theme: &mut nucleotide_ui::Theme, _cx| {
                    *_ui_theme = new_ui_theme;
                });

                // Clear the shaped lines cache to force re-rendering with new theme colors
                if let Some(line_cache) = cx.try_global::<nucleotide_editor::LineLayoutCache>() {
                    line_cache.clear_shaped_lines();
                }

                // Force a full redraw to update all components
                cx.notify();

                // Send theme change event to Helix
                gpui_to_helix_bridge::send_gpui_event_to_helix(
                    gpui_to_helix_bridge::GpuiToHelixEvent::ThemeChanged {
                        theme_name: theme_name_after.clone(),
                    },
                );
            }

            // Check if we should quit after command execution
            if core.editor.should_close() {
                cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                    crate::types::CoreEvent::ShouldQuit,
                )));
            }

            // Log bufferline config after execution
            let bufferline_after = core.editor.config().bufferline.clone();
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
                cx.emit(crate::Update::EditorEvent(helix_view::editor::EditorEvent::ConfigEvent(
                    helix_view::editor::ConfigEvent::Refresh
                )));
            }
        });

        // Log bufferline config in workspace context after command execution
        let bufferline_after_workspace = &core.read(cx).editor.config().bufferline;
        info!(bufferline_config = ?bufferline_after_workspace, "Bufferline config after command (workspace context)");
    }

    fn handle_open_directory(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Set the project directory
        info!("Setting project directory: {path:?}");
        self.core.update(cx, |core, cx| {
            core.project_directory = Some(path.to_path_buf());

            // Find the workspace root from this directory
            let workspace_root = find_workspace_root_from(path);
            info!("Found workspace root: {workspace_root:?}");

            // Update the editor's working directory
            // This will affect file picker and other operations
            if let Err(e) = std::env::set_current_dir(&workspace_root) {
                warn!("Failed to change working directory: {}", e);
            }

            cx.notify();
        });

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
            info!("About to open file from picker: {path:?} with action: {:?}", action);
            match core.editor.open(path, action) {
                Err(e) => {
                    eprintln!("Failed to open file {path:?}: {e}");
                }
                Ok(doc_id) => {
                    info!("Successfully opened file from picker: {path:?}, doc_id: {doc_id:?}");

                    // Log document info
                    if let Some(doc) = core.editor.document(doc_id) {
                        info!("Document language: {:?}, path: {:?}", doc.language_name(), doc.path());

                        // Check if document has language servers
                        let lang_servers: Vec<_> = doc.language_servers().collect();
                        info!("Document has {} language servers", lang_servers.len());
                        for ls in &lang_servers {
                            info!("  Language server: {}", ls.name());
                        }
                    }

                    // Trigger a redraw event which might help initialize language servers
                    helix_event::request_redraw();

                    // Try to ensure language servers are started for this document
                    // This is a workaround - ideally helix would handle this automatically
                    let editor = &mut core.editor;

                    // Force a refresh of language servers by getting document language config
                    if let Some(doc) = editor.document(doc_id) {
                        if let Some(lang_config) = doc.language_config() {
                            info!("Document has language config: {:?}", lang_config.language_id);
                            // Try to trigger language server initialization
                            // by calling refresh_language_servers (if it exists)
                            info!("Attempting to refresh language servers for document");
                        }
                    }

                    // Force the editor to refresh/check language servers for this document
                    // This is a workaround - ideally helix would do this automatically
                    if let Some(doc) = editor.document(doc_id) {
                        // Try to trigger LSP by getting language configuration
                        if let Some(lang_config) = doc.language_config() {
                            info!("Document has language: {}, checking for language servers", lang_config.language_id);

                            // Check if we need to start language servers
                            let doc_langs: Vec<_> = doc.language_servers().collect();
                            if doc_langs.is_empty() {
                                info!("No language servers attached to document, may need initialization");

                                // Try to trigger initialization by requesting a redraw
                                // which should cause helix to check if LSP needs to be started
                                helix_event::request_redraw();
                            }
                        }
                    }

                    // Emit an editor redraw event which should trigger various checks
                    cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                        crate::types::CoreEvent::RedrawRequested
                    )));

                    // Set cursor to beginning of file without selecting content
                    let view_id = editor.tree.focus;

                    // Check if the view exists before attempting operations
                    if let Some(view) = editor.tree.try_get(view_id) {
                        // Get the current document id from the view
                        let view_doc_id = view.doc;
                        info!("View {:?} has document ID: {:?}, opened doc_id: {:?}", view_id, view_doc_id, doc_id);

                        // Make sure the view is showing the document we just opened
                        if view_doc_id != doc_id {
                            info!("View is showing different document, switching to opened document");
                            editor.switch(doc_id, helix_view::editor::Action::Replace);
                        }

                        // Set the selection and ensure cursor is in view
                        editor.ensure_cursor_in_view(view_id);
                        if let Some(doc) = editor.document_mut(doc_id) {
                            let pos = Selection::point(0);
                            doc.set_selection(view_id, pos);
                        }
                        editor.ensure_cursor_in_view(view_id);
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
        if should_focus && self.focused_view_id.is_some() {
            self.needs_focus_restore = true;
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
                            crate::types::WorkspaceEvent::OpenFile { path } => {
                                self.handle_open_file(path, cx);
                            }
                            crate::types::WorkspaceEvent::OpenDirectory { path } => {
                                self.handle_open_directory(path, cx);
                            }
                            _ => {
                                // Other workspace events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Ui(ui_event) => {
                        match ui_event {
                            crate::types::UiEvent::ShowPicker { picker_type, .. } => {
                                match picker_type {
                                    crate::types::PickerType::File => {
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        open(core, handle, cx);
                                    }
                                    crate::types::PickerType::Buffer => {
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        show_buffer_picker(core, handle, cx);
                                    }
                                    _ => {
                                        // Other picker types not yet implemented
                                    }
                                }
                            }
                            _ => {
                                // Other UI events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Lsp(lsp_event) => {
                        match lsp_event {
                            crate::types::LspEvent::ServerInitialized { server_id } => {
                                self.handle_language_server_initialized(*server_id, cx);
                            }
                            crate::types::LspEvent::ServerExited { server_id } => {
                                self.handle_language_server_exited(*server_id, cx);
                            }
                            _ => {
                                // Other LSP events not yet handled
                            }
                        }
                    }
                }
            }
        }
    }

    /// Render the tab bar showing all open documents
    fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::tab_bar::{DocumentInfo, TabBar};
        use helix_view::editor::BufferLine;

        let core = self.core.read(cx);
        let editor = &core.editor;

        // Check bufferline configuration
        let bufferline_config = &editor.config().bufferline;
        info!(
            "render_tab_bar: bufferline config = {:?}, doc count = {}",
            bufferline_config,
            editor.documents.len()
        );

        let should_show_tabs = match bufferline_config {
            BufferLine::Never => false,
            BufferLine::Always => true,
            BufferLine::Multiple => editor.documents.len() > 1,
        };

        info!(
            should_show_tabs = should_show_tabs,
            match_result = ?bufferline_config,
            "Tab bar visibility decision"
        );

        // If tabs shouldn't be shown, return an empty div with a unique ID
        if !should_show_tabs {
            info!("Tab bar hidden, returning empty div");
            return div()
                .id("tab-bar-hidden")
                .h(px(0.0)) // Explicitly set height to 0 to ensure no space is taken
                .into_any_element();
        }

        info!("Tab bar visible, rendering tabs");

        // Get the currently active document ID
        let active_doc_id = self
            .focused_view_id
            .and_then(|focused_view_id| editor.tree.try_get(focused_view_id))
            .map(|view| view.doc);

        // Get project directory for relative paths first
        let project_directory = core.project_directory.clone();

        // Collect document information first
        let mut documents = Vec::new();
        for (&doc_id, doc) in &editor.documents {
            documents.push(DocumentInfo {
                id: doc_id,
                path: doc.path().map(|p| p.to_path_buf()),
                is_modified: doc.is_modified(),
                focused_at: doc.focused_at,
                git_status: None, // Will be filled in after releasing core borrow
            });
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

        // Update documents with VCS status
        for doc_info in &mut documents {
            if let Some(ref path) = doc_info.path {
                let status = cx.global::<VcsServiceHandle>().get_status(path, cx);
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
        .into_any_element()
    }

    /// Render unified status bar with file tree toggle and status information
    fn render_unified_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_theme = cx.global::<nucleotide_ui::Theme>();
        let helix_theme = cx.global::<crate::ThemeManager>().helix_theme();

        // Use statusline theme colors
        let statusline_style = helix_theme.get("ui.statusline");
        let bg_color = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or_else(|| {
                let base = helix_theme
                    .get("ui.background")
                    .bg
                    .and_then(crate::utils::color_to_hsla)
                    .unwrap_or(ui_theme.background);
                hsla(base.h, base.s, base.l * 0.9, base.a)
            });
        let fg_color = statusline_style
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.text);

        // Get UI font configuration
        let ui_font_config = cx.global::<crate::types::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);

        // Get current document info
        let core = self.core.read(cx);
        let editor = &core.editor;

        let mut mode_name = "NOR";
        let mut file_name = "[no file]".to_string();
        let mut position_text = "1:1".to_string();

        // Get info from focused view if available
        if let Some(view_id) = self.focused_view_id {
            if let Some((view, doc)) = editor
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
        }

        // Create divider color
        let divider_color = Hsla {
            h: fg_color.h,
            s: fg_color.s,
            l: fg_color.l,
            a: 0.3,
        };

        div()
            .h(px(28.0))
            .w_full()
            .bg(bg_color)
            .border_t_1()
            .border_color(ui_theme.border)
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded_md()
                            .hover(|style| style.bg(ui_theme.surface_hover))
                            .cursor(gpui::CursorStyle::PointingHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|workspace, _event, _window, cx| {
                                    info!("Status bar file tree toggle clicked");
                                    workspace.show_file_tree = !workspace.show_file_tree;
                                    cx.notify();
                                }),
                            )
                            .child(svg().path("icons/folder-tree.svg").size_4().text_color(
                                if self.show_file_tree {
                                    fg_color
                                } else {
                                    ui_theme.text_muted
                                },
                            )),
                    ),
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
                    .child(
                        // Mode indicator - absolutely no padding
                        div().child(mode_name).min_w(px(50.)),
                    )
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
                        div().child(position_text).min_w(px(80.)).pr_2(),
                    ),
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

    fn handle_key(&mut self, ev: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        // Wrap the entire key handling in a catch to prevent panics from propagating to FFI
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Check if the file tree has focus - if so, don't consume the event
            if let Some(file_tree) = &self.file_tree {
                if file_tree.focus_handle(cx).is_focused(window) {
                    debug!("File tree has focus, not forwarding key to editor");
                    return; // Let the file tree handle its own key events
                }
            }

            // Check if we should dismiss UI elements on escape
            if ev.keystroke.key == "escape" {
                // First check if we should dismiss key hints (highest priority)
                let has_hints = self.key_hints.read(cx).has_info();
                if has_hints {
                    // Clear key hints
                    self.key_hints.update(cx, |key_hints, cx| {
                        key_hints.set_info(None);
                        cx.notify();
                    });
                    // Also clear the editor's autoinfo
                    self.core.update(cx, |core, _| {
                        core.editor.autoinfo = None;
                    });
                    cx.notify();
                    return; // Don't pass escape to editor when dismissing key hints
                }

                // Then check if we should dismiss the info box
                if !self.info_hidden {
                    self.info_hidden = true;
                    cx.notify();
                    return; // Don't pass escape to editor when dismissing info box
                }
            }

            // Check if overlay has a native component (picker, prompt, completion) - if so, don't consume key events
            // Let GPUI actions bubble up to the native components instead
            let overlay_view = &self.overlay.read(cx);
            if !overlay_view.is_empty() {
                // Skip helix key processing when overlay is not empty
                // The native components (picker, prompt, completion) will handle their own key events via GPUI actions
                return;
            }

            let key = utils::translate_key(&ev.keystroke);

            self.input.update(cx, |_, cx| {
                cx.emit(InputEvent::Key(key));
            });

            // Update key hints after processing the key
            self.update_key_hints(cx);
        })) {
            error!(error = ?e, "Panic in key handler");
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

    fn update_document_views(&mut self, cx: &mut Context<Self>) {
        let mut view_ids = HashSet::new();
        let mut right_borders = HashSet::new();
        self.make_views(&mut view_ids, &mut right_borders, cx);
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
            if let Some(view_entity) = self.documents.get(&view_id) {
                view_entity.update(cx, |_view, cx| {
                    cx.notify();
                });
            }
        }
    }

    /// Update only the currently focused document view
    fn update_current_document_view(&mut self, cx: &mut Context<Self>) {
        if let Some(focused_view_id) = self.focused_view_id {
            if let Some(view_entity) = self.documents.get(&focused_view_id) {
                view_entity.update(cx, |_view, cx| {
                    cx.notify();
                });
            }
        }
    }

    /// Trigger completion UI based on current editor state
    fn trigger_completion(&mut self, cx: &mut Context<Self>) {
        // Create a completion view with sample items for now
        // In a full implementation, this would query the LSP for actual completions
        self.core.update(cx, |core, cx| {
            let items = core.create_sample_completion_items();
            // Create the completion view with a default anchor position
            let _anchor_position = gpui::Point::new(gpui::Pixels(100.0), gpui::Pixels(100.0));

            // Create completion view as an entity
            let completion_view = cx.new(|cx| {
                let mut view = crate::completion::CompletionView::new(cx);
                view.set_items(items);
                view
            });

            cx.emit(crate::Update::Completion(completion_view));
        });
    }

    /// Send a key directly to Helix, ensuring the editor has focus
    fn send_helix_key(&mut self, key: &str, cx: &mut Context<Self>) {
        // Ensure an editor view has focus
        if self.focused_view_id.is_some() {
            self.needs_focus_restore = true;
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
        right_borders: &mut HashSet<ViewId>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut focused_file_name = None;
        let mut focused_doc_path = None;

        {
            let editor = &self.core.read(cx).editor;

            // First pass: collect all active view IDs
            for (view, is_focused) in editor.tree.views() {
                let view_id = view.id;

                if editor
                    .tree
                    .find_split_in_direction(view_id, helix_view::tree::Direction::Right)
                    .is_some()
                {
                    right_borders.insert(view_id);
                }

                view_ids.insert(view_id);

                if is_focused {
                    // Verify the view still exists in the tree before accessing
                    if editor.tree.contains(view_id) {
                        if let Some(doc) = editor.document(view.doc) {
                            self.focused_view_id = Some(view_id);
                            let doc_path = doc.path();
                            focused_file_name = doc_path.map(|p| p.display().to_string());
                            focused_doc_path = doc_path.map(|p| p.to_path_buf());
                        }
                    }
                }
            }
        } // End of editor borrow scope

        // Sync file tree selection with the focused document (after releasing borrow)
        if let Some(path) = focused_doc_path {
            if let Some(file_tree) = &self.file_tree {
                file_tree.update(cx, |tree, cx| {
                    tree.sync_selection_with_file(Some(path.as_path()), cx);
                });
            }
        }

        // Remove views that are no longer active
        let to_remove: Vec<_> = self
            .documents
            .keys()
            .copied()
            .filter(|id| !view_ids.contains(id))
            .collect();
        for view_id in to_remove {
            self.documents.remove(&view_id);
        }

        // Second pass: create or update views
        for view_id in view_ids.iter() {
            let view_id = *view_id;
            let is_focused = self.focused_view_id == Some(view_id);
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
            if let Some(view) = self.documents.get(&view_id) {
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
                self.documents.insert(view_id, view);
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
            self.handle_appearance_change(initial_appearance, window, cx);

            // Set up observer for future changes
            cx.observe_window_appearance(window, |workspace: &mut Workspace, _appearance, cx| {
                // Handle the appearance change in a separate method that can access window
                workspace.needs_appearance_update = true;
                cx.notify();
            })
            .detach();
        }

        // Handle appearance update if needed
        if self.needs_appearance_update {
            self.needs_appearance_update = false;
            let appearance = cx.window_appearance();
            self.handle_appearance_change(appearance, window, cx);
        }

        // Handle focus restoration if needed
        if self.needs_focus_restore {
            if let Some(view_id) = self.focused_view_id {
                if let Some(doc_view) = self.documents.get(&view_id) {
                    let doc_focus = doc_view.focus_handle(cx);
                    window.focus(&doc_focus);
                }
            }
            self.needs_focus_restore = false;
        }
        // Don't create views during render - just use existing ones
        let mut view_ids = HashSet::new();
        let mut right_borders = HashSet::new();
        let mut focused_file_name = None;

        let editor = &self.core.read(cx).editor;

        for (view, is_focused) in editor.tree.views() {
            let view_id = view.id;
            view_ids.insert(view_id);

            if editor
                .tree
                .find_split_in_direction(view_id, helix_view::tree::Direction::Right)
                .is_some()
            {
                right_borders.insert(view_id);
            }

            if is_focused {
                // Verify the view still exists in the tree before accessing
                if editor.tree.contains(view_id) {
                    if let Some(doc) = editor.document(view.doc) {
                        focused_file_name = doc.path().map(|p| {
                            p.file_name()
                                .and_then(|name| name.to_str())
                                .map(std::string::ToString::to_string)
                                .unwrap_or_else(|| p.display().to_string())
                        });
                    }
                }
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

        // Get theme from ThemeManager instead of editor directly
        let theme = cx.global::<crate::ThemeManager>().helix_theme();
        let default_style = theme.get("ui.background");
        let default_ui_text = theme.get("ui.text");
        let bg_color = default_style
            .bg
            .and_then(utils::color_to_hsla)
            .unwrap_or(black());
        let _text_color = default_ui_text
            .fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());
        let window_style = theme.get("ui.window");
        let border_color = window_style
            .fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());

        let editor_rect = editor.tree.area();

        let mut docs_root = div().flex().w_full().h_full();

        // Only render the focused view, not all views
        if let Some(focused_view_id) = self.focused_view_id {
            if let Some(doc_view) = self.documents.get(&focused_view_id) {
                let has_border = right_borders.contains(&focused_view_id);
                let doc_element = div()
                    .flex()
                    .size_full()
                    .child(doc_view.clone())
                    .when(has_border, |this| {
                        this.border_color(border_color).border_r_1()
                    });
                docs_root = docs_root.child(doc_element);
            }
        }

        let focused_view = self
            .focused_view_id
            .and_then(|id| self.documents.get(&id))
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

        // Create main content area (documents + notifications + overlays) with tab bar
        let main_content = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(self.render_tab_bar(cx)) // Tab bar at the top of editor area
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .flex_1() // Take remaining height after tab bar
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
                    .child(self.key_hints.clone()),
            );

        // Create the main workspace div with basic styling first
        let mut workspace_div = div()
            .key_context("Workspace")
            .id("workspace")
            .bg(bg_color)
            .flex()
            .flex_col() // Vertical layout to include titlebar
            .w_full()
            .h_full()
            .focusable();

        // Add focus handling conditionally
        if !has_overlay {
            workspace_div = workspace_div
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|view, ev, window, cx| {
                    view.handle_key(ev, window, cx);
                }));
        }

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
            let resize_handle_width = 4.0;
            let main_content_offset = self.file_tree_width + resize_handle_width;

            if let Some(file_tree) = &self.file_tree {
                // Create file tree panel with absolute positioning
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                let file_tree_panel = div()
                    .absolute()
                    .left(px(file_tree_left_offset))
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .border_r_1()
                    .border_color(ui_theme.border)
                    .child(file_tree.clone());

                // Create resize handle with absolute positioning
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(resize_handle_width))
                    .bg(transparent_black())
                    .hover(|style| style.bg(hsla(0.0, 0.0, 0.5, 0.3)))
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

                // Get the same background color as the file tree
                let prompt_bg = {
                    let helix_theme = cx.global::<crate::ThemeManager>().helix_theme();
                    let popup_style = helix_theme.get("ui.popup");
                    popup_style
                        .bg
                        .and_then(crate::utils::color_to_hsla)
                        .or_else(|| {
                            helix_theme
                                .get("ui.background")
                                .bg
                                .and_then(crate::utils::color_to_hsla)
                        })
                        .unwrap_or(ui_theme.background)
                };

                let placeholder_panel = div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(self.file_tree_width))
                    .bg(prompt_bg)
                    .border_r_1()
                    .border_color(ui_theme.border)
                    .flex()
                    .flex_col()
                    .child(div().w_full().p(px(12.0)).child({
                        let workspace_entity = cx.entity().clone();
                        Button::new("open-directory-btn", "Open Directory")
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Medium)
                            .icon("icons/folder.svg")
                            .on_click(move |_event, app_cx| {
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

                // Add resize handle with absolute positioning
                let resize_handle = div()
                    .absolute()
                    .left(px(self.file_tree_width))
                    .top_0()
                    .bottom_0()
                    .w(px(resize_handle_width))
                    .bg(transparent_black())
                    .hover(|style| style.bg(hsla(0.0, 0.0, 0.5, 0.3)))
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

    // Use ignore::Walk to get files, respecting .gitignore
    let mut walker = WalkBuilder::new(&base_dir);
    walker.add_custom_ignore_filename(".helix/ignore");
    walker.hidden(false); // Show hidden files like .gitignore

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
            data: Arc::new(path) as Arc<dyn std::any::Any + Send + Sync>,
        });

        // Limit to 1000 files to prevent hanging on large projects
        if items.len() >= 1000 {
            break;
        }
    }

    // Sort items by label (path) for consistent ordering
    items.sort_by(|a, b| a.label.cmp(&b.label));

    info!("File picker has {} items", items.len());

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

        // Combine into terminal-like format with proper spacing
        // Pad ID to 4 characters for consistent alignment
        let label = format!("{id_str:<4} {flags_str} {path_str}");

        // Create data that includes both doc_id and path for preview functionality
        // We'll store a tuple of (DocumentId, Option<PathBuf>) for all items
        let picker_data =
            Arc::new((meta.doc_id, meta.path.clone())) as Arc<dyn std::any::Any + Send + Sync>;

        // Store the document ID and path in the picker item data
        items.push(PickerItem {
            label: label.into(),
            sublabel: None, // No sublabel for terminal-style display
            data: picker_data,
        });
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
        let mut view = crate::completion::CompletionView::new(cx);
        view.set_items(items);
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
