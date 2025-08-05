use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use helix_core::Selection;
use helix_view::ViewId;
use log::{info, warn};

use crate::document::DocumentView;
use crate::file_tree::{FileTreeView, FileTreeEvent, FileTreeConfig};
use crate::info_box::InfoBoxView;
use crate::key_hint_view::KeyHintView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
use crate::{Core, Input, InputEvent};
use crate::application::find_workspace_root_from;

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
}

impl EventEmitter<crate::Update> for Workspace {}

impl Workspace {
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
        cx.subscribe(&overlay, |workspace, _overlay, _event: &DismissEvent, cx| {
            println!("🎯 Workspace received DismissEvent from overlay");
            // Mark that we need to restore focus in the next render
            workspace.needs_focus_restore = true;
            cx.notify();
        }).detach();
        
        // Subscribe to core (Application) events to receive Update events
        cx.subscribe(&core, |workspace, _core, event: &crate::Update, cx| {
            info!("Workspace: Received Update event from core: {:?}", event);
            workspace.handle_event(event, cx);
        }).detach();
        
        let key_hints = cx.new(|_cx| KeyHintView::new());
        
        // Initialize file tree if we can find a workspace root
        let root_path = core.read(cx).project_directory.clone()
            .or_else(|| {
                // Try to find workspace root from current working directory
                std::env::current_dir().ok()
                    .map(|cwd| find_workspace_root_from(&cwd))
            });
        
        let file_tree = root_path.map(|root_path| {
            cx.new(|cx| {
                let config = FileTreeConfig::default();
                FileTreeView::new(root_path, config, cx)
            })
        });
        
        // Subscribe to file tree events if we have a file tree
        if let Some(ref file_tree) = file_tree {
            info!("Workspace: Subscribing to file tree events");
            cx.subscribe(file_tree, |workspace, _file_tree, event, cx| {
                info!("Workspace: Received file tree event: {:?}", event);
                workspace.handle_file_tree_event(event, cx);
            }).detach();
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
        };
        // Initialize document views
        workspace.update_document_views(cx);
        // Focus the workspace by default (focus will be managed by render)
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

    // Removed - views are created in main.rs and passed in

    // Removed - views are created in main.rs and passed in

    pub fn theme(editor: &Entity<Core>, cx: &mut Context<Self>) -> helix_view::Theme {
        editor.read(cx).editor.theme.clone()
    }

    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        info!("handling event {ev:?}");
        match ev {
            crate::Update::EditorEvent(ev) => {
                use helix_view::editor::EditorEvent;
                match ev {
                    EditorEvent::Redraw => cx.notify(),
                    EditorEvent::LanguageServerMessage(_) => { /* handled by notifications */ }
                    _ => {
                        info!("editor event {ev:?} not handled");
                    }
                }
            }
            crate::Update::EditorStatus(_) => {}
            crate::Update::Redraw => {
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
            crate::Update::Prompt(_) | crate::Update::Picker(_) | crate::Update::DirectoryPicker(_) | crate::Update::Completion(_) => {
                // When a picker, prompt, or completion appears, auto-dismiss the info box
                self.info_hidden = true;
                
                // Focus will be handled by the overlay components
                cx.notify();
            }
            crate::Update::OpenFile(path) => {
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
                    match core.editor.open(&path, action) {
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
                            cx.emit(crate::Update::EditorEvent(helix_view::editor::EditorEvent::Redraw));
                            
                            // Set cursor to beginning of file without selecting content
                            let view_id = editor.tree.focus;
                            
                            // Check if the view exists before attempting operations
                            if editor.tree.contains(view_id) {
                                // Get the current document id from the view
                                let view_doc_id = editor.tree.get(view_id).doc;
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
                
                // Force a redraw
                cx.notify();
            }
            crate::Update::OpenDirectory(path) => {
                // Set the project directory
                info!("Setting project directory: {path:?}");
                self.core.update(cx, |core, cx| {
                    core.project_directory = Some(path.clone());
                    
                    // Find the workspace root from this directory
                    let workspace_root = find_workspace_root_from(&path);
                    info!("Found workspace root: {workspace_root:?}");
                    
                    // Update the editor's working directory
                    // This will affect file picker and other operations
                    if let Err(e) = std::env::set_current_dir(&workspace_root) {
                        warn!("Failed to change working directory: {}", e);
                    }
                    
                    cx.notify();
                });
                
                // Update the file tree with the new directory
                let new_file_tree = cx.new(|cx| {
                    let config = FileTreeConfig::default();
                    FileTreeView::new(path.clone(), config, cx)
                });
                
                // Subscribe to file tree events
                cx.subscribe(&new_file_tree, |workspace, _file_tree, event, cx| {
                    info!("Workspace: Received file tree event from new tree: {:?}", event);
                    workspace.handle_file_tree_event(event, cx);
                }).detach();
                
                self.file_tree = Some(new_file_tree);
                
                // Make sure file tree is visible
                self.show_file_tree = true;
                cx.notify();
                
                // Show status message about the new project directory
                self.core.update(cx, |core, cx| {
                    core.editor.set_status(format!("Project directory set to: {}", path.display()));
                    cx.notify();
                });
            }
            crate::Update::FileTreeEvent(event) => {
                self.handle_file_tree_event(event, cx);
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
            crate::Update::CommandSubmitted(command) => {
                println!("🎯 Workspace received command: {command}");
                // Execute the command through helix's command system
                let core = self.core.clone();
                let handle = self.handle.clone();
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
                                    core.editor.set_error("Failed to parse arguments".to_string());
                                }
                            }
                        } else {
                            // Execute regular command
                            match helix_term::commands::TYPABLE_COMMAND_MAP.get(cmd_name) {
                                Some(cmd) => {
                                    // Parse args for the command
                                    let parsed_args = helix_core::command_line::Args::parse(
                                        args,
                                        cmd.signature,
                                        true,
                                        |token| helix_view::expansion::expand(comp_ctx.editor, token).map_err(|err| err.into()),
                                    );
                                    
                                    match parsed_args {
                                        Ok(parsed_args) => {
                                            if let Err(err) = (cmd.fun)(
                                                &mut comp_ctx,
                                                parsed_args,
                                                helix_term::ui::PromptEvent::Validate,
                                            ) {
                                                core.editor.set_error(format!("'{cmd_name}': {err}"));
                                            }
                                        }
                                        Err(err) => {
                                            core.editor.set_error(format!("'{cmd_name}': {err}"));
                                        }
                                    }
                                }
                                None => {
                                    core.editor.set_error(format!("no such command: '{cmd_name}'"));
                                }
                            }
                        }
                    }
                    
                    // Check if the theme has changed after command execution
                    let current_theme = core.editor.theme.clone();
                    let theme_manager = cx.global::<crate::theme_manager::ThemeManager>();
                    
                    // If the theme has changed, update the ThemeManager and UI theme
                    if theme_manager.helix_theme().name() != current_theme.name() {
                        // Update the global ThemeManager
                        cx.update_global(|theme_manager: &mut crate::theme_manager::ThemeManager, _cx| {
                            theme_manager.set_theme(current_theme.clone());
                        });
                        
                        // Update the global UI theme
                        let new_ui_theme = cx.global::<crate::theme_manager::ThemeManager>().ui_theme().clone();
                        cx.update_global(|_ui_theme: &mut crate::ui::Theme, _cx| {
                            *_ui_theme = new_ui_theme;
                        });
                        
                        // Force a full redraw to update all components
                        cx.notify();
                        
                        // Send theme change event to Helix
                        crate::gpui_to_helix_bridge::send_gpui_event_to_helix(
                            crate::gpui_to_helix_bridge::GpuiToHelixEvent::ThemeChanged {
                                theme_name: current_theme.name().to_string(),
                            }
                        );
                    }
                    
                    // Check if we should quit after command execution
                    if core.editor.should_close() {
                        cx.emit(crate::Update::ShouldQuit);
                    }
                    
                    cx.notify();
                });
            }
            // Helix event bridge - respond to automatic Helix events
            crate::Update::DocumentChanged { doc_id } => {
                // Document content changed - update specific document view
                self.update_specific_document_view(*doc_id, cx);
                cx.notify();
            }
            crate::Update::SelectionChanged { doc_id, view_id } => {
                // Selection/cursor moved - update status and specific view
                info!("Selection changed in doc {:?}, view {:?}", doc_id, view_id);
                self.update_specific_document_view(*doc_id, cx);
                cx.notify();
            }
            crate::Update::ModeChanged { old_mode, new_mode } => {
                // Editor mode changed - update status line and current view
                info!("Mode changed from {:?} to {:?}", old_mode, new_mode);
                self.update_current_document_view(cx);
                cx.notify();
            }
            crate::Update::DiagnosticsChanged { doc_id } => {
                // LSP diagnostics changed - update specific document view
                self.update_specific_document_view(*doc_id, cx);
                cx.notify();
            }
            crate::Update::DocumentOpened { doc_id } => {
                // New document opened - the view will be created automatically
                info!("Document opened: {:?}", doc_id);
                cx.notify();
            }
            crate::Update::DocumentClosed { doc_id } => {
                // Document closed - the view will be cleaned up automatically
                info!("Document closed: {:?}", doc_id);
                cx.notify();
            }
            crate::Update::ViewFocused { view_id } => {
                // View focus changed - just update focus state
                info!("View focused: {:?}", view_id);
                self.focused_view_id = Some(*view_id);
                cx.notify();
            }
            crate::Update::LanguageServerInitialized { server_id } => {
                // LSP server initialized - update status
                info!("Language server initialized: {:?}", server_id);
                cx.notify();
            }
            crate::Update::LanguageServerExited { server_id } => {
                // LSP server exited - update status
                info!("Language server exited: {:?}", server_id);
                cx.notify();
            }
            crate::Update::CompletionRequested { doc_id, view_id, trigger } => {
                // Completion was requested - trigger completion UI
                info!("Completion requested for doc {:?}, view {:?}, trigger: {:?}", doc_id, view_id, trigger);
                
                // Only show completion for certain triggers (not every character)
                match trigger {
                    crate::event_bridge::CompletionTrigger::Manual => {
                        // Always show for manual triggers
                        self.trigger_completion(cx);
                    }
                    crate::event_bridge::CompletionTrigger::CharacterTyped(c) => {
                        // Only trigger for certain characters that typically start identifiers
                        if c.is_alphabetic() || *c == '_' || *c == '.' {
                            self.trigger_completion(cx);
                        }
                    }
                    crate::event_bridge::CompletionTrigger::Filter => {
                        // Re-filter existing completion
                        self.trigger_completion(cx);
                    }
                }
                
                cx.notify();
            }
        }
    }

    fn handle_file_tree_event(&mut self, event: &FileTreeEvent, cx: &mut Context<Self>) {
        match event {
            FileTreeEvent::OpenFile { path } => {
                // Emit an OpenFile event to trigger file opening
                info!("FileTreeEvent::OpenFile received in workspace: {:?}", path);
                cx.emit(crate::Update::OpenFile(path.clone()));
            }
            FileTreeEvent::SelectionChanged { path: _ } => {
                // Update UI if needed for selection changes
                cx.notify();
            }
            FileTreeEvent::DirectoryToggled { path: _, expanded: _ } => {
                // Update UI for directory expansion/collapse
                cx.notify();
            }
            FileTreeEvent::FileSystemChanged { path, kind } => {
                info!("File system change detected: {:?} - {:?}", path, kind);
                // Handle file system changes
                // Could refresh the file tree or update document views
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
                    log::debug!("File tree has focus, not forwarding key to editor");
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
            log::error!("Panic in key handler: {e:?}");
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
        let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme().clone();
        
        
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
    fn update_specific_document_view(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // Find views for this specific document
        let view_ids: Vec<helix_view::ViewId> = self.core.read(cx).editor.tree.views()
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
            let anchor_position = gpui::Point::new(gpui::Pixels(100.0), gpui::Pixels(100.0));
            
            // Create completion view as an entity
            let completion_view = cx.new(|cx| {
                crate::completion::CompletionView::new(items, anchor_position, cx)
            });
            
            cx.emit(crate::Update::Completion(completion_view));
        });
    }
    
    fn make_views(
        &mut self,
        view_ids: &mut HashSet<ViewId>,
        right_borders: &mut HashSet<ViewId>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let editor = &self.core.read(cx).editor;
        let mut focused_file_name = None;

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
                        focused_file_name = doc.path().map(|p| p.display().to_string());
                    }
                }
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
            let editor_font = cx.global::<crate::EditorFontConfig>();
            let style = TextStyle {
                font_family: cx.global::<crate::FontSettings>().fixed_font.family.clone(),
                font_size: px(editor_font.size).into(),
                font_weight: editor_font.weight,
                ..Default::default()
            };
            let core = self.core.clone();
            let input = self.input.clone();
            let view = self.documents.entry(view_id).or_insert_with(|| {
                cx.new(|cx| {
                    let doc_focus_handle = cx.focus_handle();
                    DocumentView::new(
                        core,
                        input,
                        view_id,
                        style.clone(),
                        &doc_focus_handle,
                        is_focused,
                    )
                })
            });
            
            view.update(cx, |view, _cx| {
                view.set_focused(is_focused);
                // Focus is managed by the view's render method
            });
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
        // Handle focus restoration if needed
        if self.needs_focus_restore {
            if let Some(view_id) = self.focused_view_id {
                if let Some(doc_view) = self.documents.get(&view_id) {
                    println!("🔄 Restoring focus to document view: {view_id:?}");
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
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| p.display().to_string())
                        });
                    }
                }
            }
        }
        
        // Set the native window title (macOS convention: filename — appname)
        let window_title = if let Some(ref path) = focused_file_name {
            format!("{path} — Helix")  // Using em dash like macOS
        } else {
            "Helix".to_string()
        };
        window.set_window_title(&window_title);

        
        let editor = &self.core.read(cx).editor;

        // Get theme from ThemeManager instead of editor directly
        let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
        let default_style = theme.get("ui.background");
        let default_ui_text = theme.get("ui.text");
        let bg_color = default_style.bg
            .and_then(utils::color_to_hsla)
            .unwrap_or(black());
        let _text_color = default_ui_text.fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());
        let window_style = theme.get("ui.window");
        let border_color = window_style.fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());

        let editor_rect = editor.tree.area();

        let editor = &self.core.read(cx).editor;
        let mut docs_root = div()
            .flex()
            .w_full()
            .h_full();

        for (view, _) in editor.tree.views() {
            let view_id = view.id;
            if let Some(doc_view) = self.documents.get(&view_id) {
                let has_border = right_borders.contains(&view_id);
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

        // Don't remove views during render - handle this in update_document_views
        // let to_remove = self
        //     .documents
        //     .keys()
        //     .copied()
        //     .filter(|id| !view_ids.contains(id))
        //     .collect::<Vec<_>>();
        // for view_id in to_remove {
        //     if let Some(_view) = self.documents.remove(&view_id) {
        //         // Views are automatically cleaned up when no longer referenced in GPUI
        //     }
        // }

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
        
        // Create main content area (documents + notifications + overlays)
        let main_content = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .when_some(Some(docs_root), |this, docs| this.child(docs))
            .child(self.notifications.clone())
            .when(!self.overlay.read(cx).is_empty(), |this| {
                let view = &self.overlay;
                this.child(view.clone())
            })
            .when(
                !self.info_hidden && !self.info.read(cx).is_empty(),
                |this| this.child(self.info.clone()),
            )
            .child(self.key_hints.clone());

        div()
            .key_context("Workspace")
            .when(!has_overlay, |this| {
                this.track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(|view, ev, window, cx| {
                        view.handle_key(ev, window, cx);
                    }))
            })
            .when(self.is_resizing_file_tree, |this| {
                this.cursor(gpui::CursorStyle::ResizeLeftRight)
            })
            .on_mouse_move(cx.listener(|workspace, event: &MouseMoveEvent, _window, cx| {
                if workspace.is_resizing_file_tree {
                    let delta = event.position.x.0 - workspace.resize_start_x;
                    let new_width = (workspace.resize_start_width + delta).clamp(150.0, 600.0);
                    workspace.file_tree_width = new_width;
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|workspace, _event: &MouseUpEvent, _window, cx| {
                if workspace.is_resizing_file_tree {
                    workspace.is_resizing_file_tree = false;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(move |_, _: &crate::actions::help::About, _window, _cx| {
                eprintln!("About Helix");
            }))
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::editor::Quit, _window, cx| {
                    quit(core.clone(), handle.clone(), cx);
                    cx.quit();
                })
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::editor::OpenFile, _window, cx| {
                    open(core.clone(), handle.clone(), cx)
                })
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::editor::OpenDirectory, _window, cx| {
                    open_directory(core.clone(), handle.clone(), cx)
                })
            })
            .on_action(cx.listener(move |_, _: &crate::actions::window::Hide, _window, cx| {
                cx.hide()
            }))
            .on_action(cx.listener(move |_, _: &crate::actions::window::HideOthers, _window, cx| {
                cx.hide_other_apps()
            }))
            .on_action(cx.listener(move |_, _: &crate::actions::window::ShowAll, _window, cx| {
                cx.unhide_other_apps()
            }))
            .on_action(cx.listener(move |_, _: &crate::actions::window::Minimize, _window, _cx| {
                // minimize not available in GPUI yet
            }))
            .on_action(cx.listener(move |_, _: &crate::actions::window::Zoom, _window, _cx| {
                // zoom not available in GPUI yet
            }))
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::help::OpenTutorial, _window, cx| {
                    load_tutor(core.clone(), handle.clone(), cx)
                })
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::test::TestPrompt, _window, cx| {
                    test_prompt(core.clone(), handle.clone(), cx)
                })
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, _: &crate::actions::test::TestCompletion, _window, cx| {
                    test_completion(core.clone(), handle.clone(), cx)
                })
            })
            .id("workspace")
            .bg(bg_color)
            .flex()
            .flex_row() // Changed to horizontal layout
            .w_full()
            .h_full()
            .focusable()
            // Add file tree as left panel
            .when(self.show_file_tree && self.file_tree.is_some(), |this| {
                if let Some(file_tree) = &self.file_tree {
                    this.child(
                        div()
                            .w(px(self.file_tree_width))
                            .h_full()
                            .child(file_tree.clone())
                    )
                    // Add resize handle
                    .child(
                        div()
                            .w(px(4.0))
                            .h_full()
                            .bg(transparent_black())
                            .hover(|style| style.bg(hsla(0.0, 0.0, 0.5, 0.3)))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .id("file-tree-resize-handle")
                            .on_mouse_down(MouseButton::Left, cx.listener(|workspace, event: &MouseDownEvent, _window, cx| {
                                workspace.is_resizing_file_tree = true;
                                workspace.resize_start_x = event.position.x.0;
                                workspace.resize_start_width = workspace.file_tree_width;
                                cx.stop_propagation();
                                cx.notify();
                            }))
                    )
                } else {
                    this
                }
            })
            // Add main content area
            .child(main_content)
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
    use std::sync::Arc;
    use ignore::WalkBuilder;
    
    info!("Opening file picker");
    
    // Get all files in the current directory using ignore crate (respects .gitignore)
    let mut items = Vec::new();
    
    // Use project directory if set, otherwise use current directory
    let base_dir = core.update(cx, |core, _| {
        core.project_directory.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    });
    info!("Base directory for file picker: {:?}", base_dir);
    
    // Use ignore::Walk to get files, respecting .gitignore
    let mut walker = WalkBuilder::new(&base_dir);
    walker.add_custom_ignore_filename(".helix/ignore");
    walker.hidden(false); // Show hidden files like .gitignore
    
    for entry in walker.build().filter_map(|e| e.ok()) {
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
        let _filename = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        
        // For project files, use path as label for better visibility
        items.push(PickerItem {
            label: path_str.clone().into(),
            sublabel: None,
            data: Arc::new(path)
                as Arc<dyn std::any::Any + Send + Sync>,
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
    let file_picker = crate::picker::Picker::native(
        "Open File",
        items,
        |_index| {
            // This callback is no longer used - file opening is handled via OpenFile events
            // The overlay will emit OpenFile events when files are selected
        }
    );
    
    info!("Emitting file picker to overlay");
    
    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Picker(file_picker));
    });
}

fn open_directory(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    info!("Opening directory picker");
    
    // Create a native directory picker
    let directory_picker = crate::picker::Picker::native_directory(
        "Select Project Directory",
        |path| {
            info!("Directory selected: {:?}", path);
            // The callback will be handled through events
        }
    );
    
    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::DirectoryPicker(directory_picker));
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
    let anchor_position = gpui::point(gpui::px(200.0), gpui::px(300.0));
    
    // Create completion view
    let completion_view = cx.new(|cx| {
        crate::completion::CompletionView::new(
            items,
            anchor_position,
            cx
        )
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
            log::error!("Failed to flush writes: {e}");
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
