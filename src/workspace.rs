use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use helix_core::Selection;
use helix_view::ViewId;
use log::info;

use crate::document::DocumentView;
use crate::info_box::InfoBoxView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
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
    notifications: Entity<NotificationView>,
    focus_handle: FocusHandle,
    needs_focus_restore: bool,
}

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
        
        let mut workspace = Self {
            core,
            input,
            focused_view_id: None,
            documents: HashMap::new(),
            handle,
            overlay,
            info,
            info_hidden: true,
            notifications,
            focus_handle,
            needs_focus_restore: false,
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
        info!("handling event {:?}", ev);
        match ev {
            crate::Update::EditorEvent(ev) => {
                use helix_view::editor::EditorEvent;
                match ev {
                    EditorEvent::Redraw => cx.notify(),
                    EditorEvent::LanguageServerMessage(_) => { /* handled by notifications */ }
                    _ => {
                        info!("editor event {:?} not handled", ev);
                    }
                }
            }
            crate::Update::EditorStatus(_) => {}
            crate::Update::Redraw => {
                // Update views when editor state changes
                self.update_document_views(cx);
                
                if let Some(view) = self.focused_view_id.and_then(|id| self.documents.get(&id)) {
                    view.update(cx, |_view, cx| {
                        cx.notify();
                    })
                }
                cx.notify();
            }
            crate::Update::Prompt(_) | crate::Update::Picker(_) | crate::Update::Completion(_) => {
                // When a picker, prompt, or completion appears, auto-dismiss the info box
                self.info_hidden = true;
                
                // Focus will be handled by the overlay components
                cx.notify();
            }
            crate::Update::OpenFile(path) => {
                // Open the specified file in the editor
                info!("Opening file: {:?}", path);
                self.core.update(cx, |core, cx| {
                    let _guard = self.handle.enter();
                    let editor = &mut core.editor;
                    
                    // Open the file in the editor
                    match editor.open(&path, helix_view::editor::Action::Replace) {
                        Err(e) => {
                            eprintln!("Failed to open file {:?}: {}", path, e);
                        }
                        Ok(doc_id) => {
                            info!("Successfully opened file: {:?}", path);
                            
                            // Set cursor to beginning of file without selecting content
                            let view_id = editor.tree.focus;
                            
                            // Check if the view exists before attempting operations
                            if editor.tree.contains(view_id) {
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
                // Update document views after opening file
                self.update_document_views(cx);
                cx.notify();
            }
            crate::Update::Info(_) => {
                self.info_hidden = false;
                // handled by the info box view
            }
            crate::Update::ShouldQuit => {
                info!("ShouldQuit event received - triggering application quit");
                cx.quit();
            }
            crate::Update::CommandSubmitted(command) => {
                println!("🎯 Workspace received command: {}", command);
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
                    let (cmd_name, args, _) = helix_core::command_line::split(&command);
                    
                    if !cmd_name.is_empty() {
                        // Check if it's a line number
                        if cmd_name.parse::<usize>().is_ok() && args.trim().is_empty() {
                            // Handle goto line
                            if let Some(cmd) = helix_term::commands::TYPABLE_COMMAND_MAP.get("goto") {
                                // Parse args manually since we can't use execute_command
                                let parsed_args = helix_core::command_line::Args::parse(
                                    cmd_name,
                                    cmd.signature.clone(),
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
                                        cmd.signature.clone(),
                                        true,
                                        |token| helix_view::expansion::expand(&comp_ctx.editor, token).map_err(|err| err.into()),
                                    );
                                    
                                    match parsed_args {
                                        Ok(parsed_args) => {
                                            if let Err(err) = (cmd.fun)(
                                                &mut comp_ctx,
                                                parsed_args,
                                                helix_term::ui::PromptEvent::Validate,
                                            ) {
                                                core.editor.set_error(format!("'{}': {}", cmd_name, err));
                                            }
                                        }
                                        Err(err) => {
                                            core.editor.set_error(format!("'{}': {}", cmd_name, err));
                                        }
                                    }
                                }
                                None => {
                                    core.editor.set_error(format!("no such command: '{}'", cmd_name));
                                }
                            }
                        }
                    }
                    
                    // Check if we should quit after command execution
                    if core.editor.should_close() {
                        cx.emit(crate::Update::ShouldQuit);
                    }
                    
                    cx.notify();
                });
            }
        }
    }

    fn handle_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        // Wrap the entire key handling in a catch to prevent panics from propagating to FFI
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Check if we should dismiss the info box first
            if ev.keystroke.key == "escape" && !self.info_hidden {
                self.info_hidden = true;
                cx.notify();
                return; // Don't pass escape to editor when dismissing info box
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
            })
        })) {
            log::error!("Panic in key handler: {:?}", e);
        }
    }

    fn update_document_views(&mut self, cx: &mut Context<Self>) {
        let mut view_ids = HashSet::new();
        let mut right_borders = HashSet::new();
        self.make_views(&mut view_ids, &mut right_borders, cx);
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
            let style = TextStyle {
                font_family: cx.global::<crate::FontSettings>().fixed_font.family.clone(),
                font_size: px(14.0).into(),
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
                    println!("🔄 Restoring focus to document view: {:?}", view_id);
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
            format!("{} — Helix", path)  // Using em dash like macOS
        } else {
            "Helix".to_string()
        };
        window.set_window_title(&window_title);

        
        let editor = &self.core.read(cx).editor;

        let default_style = editor.theme.get("ui.background");
        let default_ui_text = editor.theme.get("ui.text");
        let bg_color = default_style.bg
            .and_then(|c| utils::color_to_hsla(c))
            .unwrap_or(black());
        let _text_color = default_ui_text.fg
            .and_then(|c| utils::color_to_hsla(c))
            .unwrap_or(white());
        let window_style = editor.theme.get("ui.window");
        let border_color = window_style.fg
            .and_then(|c| utils::color_to_hsla(c))
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
        
        div()
            .key_context("Workspace")
            .when(!has_overlay, |this| {
                this.track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(|view, ev, _window, cx| {
                        view.handle_key(ev, cx);
                    }))
            })
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
            .flex_col()
            .w_full()
            .h_full()
            .focusable()
            .when_some(Some(docs_root), |this, docs| this.child(docs))
            .child(self.notifications.clone())
            .when(!self.overlay.read(cx).is_empty(), |this| {
                let view = &self.overlay;
                // TODO: Implement focus for OverlayView
                this.child(view.clone())
            })
            .when(
                !self.info_hidden && !self.info.read(cx).is_empty(),
                |this| this.child(self.info.clone()),
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
    use std::sync::Arc;
    use ignore::WalkBuilder;
    
    // Get all files in the current directory using ignore crate (respects .gitignore)
    let mut items = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_default();
    
    // Use ignore::Walk to get files, respecting .gitignore
    let mut walker = WalkBuilder::new(&cwd);
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
        
        // Get relative path from current directory
        let relative_path = path.strip_prefix(&cwd).unwrap_or(&path);
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
    
    // Sort items by path for consistent ordering
    items.sort_by(|a, b| a.sublabel.cmp(&b.sublabel));
    
    // Create a simple native picker without callback - the overlay will handle file opening via events
    let file_picker = crate::picker::Picker::native(
        "Open File",
        items,
        |_index| {
            // This callback is no longer used - file opening is handled via OpenFile events
            // The overlay will emit OpenFile events when files are selected
        }
    );
    
    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Picker(file_picker));
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
            log::error!("Failed to flush writes: {}", e);
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
