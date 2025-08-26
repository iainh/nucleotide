use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, Styled, Window,
    div, px,
};

use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::completion_v2::CompletionView;
use nucleotide_ui::picker::Picker;
use nucleotide_ui::picker_view::{PickerItem, PickerView};
use nucleotide_ui::prompt::{Prompt, PromptElement};
use nucleotide_ui::prompt_view::PromptView;
use nucleotide_ui::theme_manager::HelixThemedContext;

pub struct OverlayView {
    prompt: Option<Prompt>,
    native_picker_view: Option<Entity<PickerView>>,
    native_prompt_view: Option<Entity<PromptView>>,
    completion_view: Option<Entity<CompletionView>>,
    focus: FocusHandle,
    core: gpui::WeakEntity<crate::Core>,
}

impl OverlayView {
    pub fn new(focus: &FocusHandle, core: &Entity<crate::Core>) -> Self {
        Self {
            prompt: None,
            native_picker_view: None,
            native_prompt_view: None,
            completion_view: None,
            focus: focus.clone(),
            core: core.downgrade(),
        }
    }

    pub fn is_empty(&self) -> bool {
        let empty = self.prompt.is_none()
            && self.native_picker_view.is_none()
            && self.native_prompt_view.is_none()
            && self.completion_view.is_none();

        if !empty && self.completion_view.is_some() {
            println!("COMP: Overlay is NOT empty - has completion view");
        }

        empty
    }

    pub fn has_completion(&self) -> bool {
        self.completion_view.is_some()
    }

    pub fn has_picker(&self) -> bool {
        self.native_picker_view.is_some()
    }

    pub fn has_prompt(&self) -> bool {
        self.native_prompt_view.is_some() || self.prompt.is_some()
    }

    pub fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion_view.is_some() {
            self.completion_view = None;
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        // Clean up picker before clearing
        if let Some(picker) = &self.native_picker_view {
            picker.update(cx, |picker, cx| {
                picker.cleanup(cx);
            });
        }
        // Clear all overlay components
        self.prompt = None;
        self.native_picker_view = None;
        self.native_prompt_view = None;
        self.completion_view = None;

        cx.notify();
    }

    pub fn subscribe(&self, editor: &Entity<crate::Core>, cx: &mut Context<Self>) {
        cx.subscribe(editor, |this, _, ev, cx| {
            this.handle_event(ev, cx);
        })
        .detach()
    }

    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        match ev {
            crate::Update::Prompt(prompt) => {
                match prompt {
                    Prompt::Native {
                        prompt: prompt_text,
                        initial_input,
                        on_submit,
                        on_cancel,
                    } => {
                        let prompt_text = prompt_text.clone();
                        let initial_input = initial_input.clone();
                        let on_submit = on_submit.clone();
                        let on_cancel = on_cancel.clone();

                        let prompt_view = cx.new(|cx| {
                            let is_search_prompt = prompt_text.starts_with("search")
                                || prompt_text.starts_with("rsearch");

                            let mut view = PromptView::new(prompt_text.clone(), cx);

                            // Apply theme styling using testing-aware theme access
                            let style = Self::create_prompt_style_from_context(cx);
                            view = view.with_style(style);

                            if !initial_input.is_empty() {
                                view.set_text(&initial_input, cx);
                            }

                            // Only set up completion for command prompts, not search prompts
                            if !is_search_prompt {
                                // Set up completion function for command mode using our centralized completions module
                                // Try to get actual settings from the editor
                                let settings_cache = if let Some(core) = self.core.upgrade() {
                                    // Use the Helix setting completer to get all available settings
                                    // Pass an empty string to get all settings
                                    let all_settings = helix_term::ui::completers::setting(
                                        &core.read(cx).editor,
                                        "",
                                    );
                                    let settings: Vec<String> = all_settings
                                        .into_iter()
                                        .map(|(_, span)| span.content.to_string())
                                        .collect();
                                    settings
                                } else {
                                    // Fallback to hardcoded list
                                    crate::completions::SETTINGS_KEYS.to_vec()
                                };

                                view = view.with_completion_fn(move |input| {
                                    // Strip leading colon if present
                                    let input = input.strip_prefix(':').unwrap_or(input);

                                    // Use cached settings for better completions
                                    crate::completions::get_command_completions_with_cache(
                                        input,
                                        Some(settings_cache.to_vec()),
                                    )
                                });
                            }

                            // Set up the submit callback with command/search execution
                            let core_weak_submit = self.core.clone();
                            view = view.on_submit(move |input: &str, cx| {
                                use nucleotide_logging::info;
                                info!(input = %input, input_len = input.len(), is_search = is_search_prompt, "Overlay on_submit received input");
                                // Emit appropriate event based on prompt type
                                if let Some(core) = core_weak_submit.upgrade() {
                                    core.update(cx, |_core, cx| {
                                        if is_search_prompt {
                                            cx.emit(crate::Update::SearchSubmitted(
                                                input.to_string(),
                                            ));
                                        } else {
                                            info!(command = %input, "Emitting CommandSubmitted event");
                                            cx.emit(crate::Update::CommandSubmitted(
                                                input.to_string(),
                                            ));
                                        }
                                    });
                                }

                                // Also call the original callback
                                (on_submit)(input);

                                // Dismiss the prompt after submission
                                cx.emit(DismissEvent);
                            });

                            // Set up the cancel callback if provided
                            if let Some(cancel_fn) = on_cancel {
                                view = view.on_cancel(move |cx| {
                                    (cancel_fn)();
                                    cx.emit(DismissEvent);
                                });
                            } else {
                                view = view.on_cancel(move |cx| {
                                    cx.emit(DismissEvent);
                                });
                            }

                            view
                        });

                        // Subscribe to dismiss events from the prompt view
                        cx.subscribe(
                            &prompt_view,
                            |this, _prompt_view, _event: &DismissEvent, cx| {
                                this.native_prompt_view = None;
                                // Emit dismiss event to notify workspace
                                cx.emit(DismissEvent);
                                cx.notify();
                            },
                        )
                        .detach();

                        // Focus will be handled by the prompt view's render method

                        self.native_prompt_view = Some(prompt_view);
                    }
                    Prompt::Legacy(_) => {
                        // For legacy prompts, store them as-is
                        self.prompt = Some(prompt.clone());
                        self.native_prompt_view = None;
                    }
                }

                cx.notify();
            }
            crate::Update::Completion(completion_view) => {
                nucleotide_logging::info!(
                    "🎨 OVERLAY RECEIVED COMPLETION VIEW: Setting up completion overlay"
                );

                // Set up completion view with event subscription
                self.completion_view = Some(completion_view.clone());

                // Subscribe to dismiss events from completion view
                cx.subscribe(
                    &completion_view,
                    |this, _completion_view, _event: &DismissEvent, cx| {
                        this.completion_view = None;
                        cx.emit(DismissEvent);
                        cx.notify();
                    },
                )
                .detach();

                // Subscribe to the new completion acceptance event
                cx.subscribe(
                    &completion_view,
                    |this, _completion_view, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                        nucleotide_logging::info!(
                            item_index = event.item_index,
                            "Completion accepted via Helix transaction system"
                        );
                        // The completion view will handle the actual acceptance through Helix
                        // This event is just for notification/coordination
                        cx.emit(DismissEvent);
                    },
                )
                .detach();

                cx.notify();
            }
            crate::Update::Picker(picker) => {
                // Clean up any existing picker before creating a new one
                if let Some(existing_picker) = &self.native_picker_view {
                    existing_picker.update(cx, |picker, cx| {
                        picker.cleanup(cx);
                    });
                    self.native_picker_view = None;
                }

                match picker {
                    Picker::Native {
                        title: _,
                        items,
                        on_select,
                    } => {
                        let items = items.clone();
                        let on_select = on_select.clone();
                        let core_weak = self.core.clone();
                        let _items_count = items.len();

                        let picker_view = cx.new(|cx| {
                            // Use testing-aware theme constructor
                            let mut view = Self::create_picker_view_with_context(cx);
                            let items_for_callback = items.clone();

                            // Enable preview by default, especially for buffer picker
                            view = view.with_preview(true);

                            view = view.with_items(items);

                            // Set up the selection callback
                            view = view.on_select(move |selected_item: &PickerItem, picker_cx| {
                                // Find the index of the selected item
                                if let Some(index) = items_for_callback
                                    .iter()
                                    .position(|item| std::ptr::eq(item, selected_item))
                                {
                                    // Call the original on_select callback with the index
                                    (on_select)(index);
                                }

                                // Check if it's a buffer picker item (DocumentId, Option<PathBuf>)
                                if let Some((doc_id, _path)) = selected_item.data.downcast_ref::<(
                                    helix_view::DocumentId,
                                    Option<std::path::PathBuf>,
                                )>(
                                ) {
                                    // Switch to the selected buffer
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            core.editor.switch(
                                                *doc_id,
                                                helix_view::editor::Action::Replace,
                                            );
                                        });
                                    }
                                }
                                // Extract the file path from the selected item for opening
                                else if let Some(path) =
                                    selected_item.data.downcast_ref::<std::path::PathBuf>()
                                {
                                    // Emit OpenFile event to actually open the file
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |_core, core_cx| {
                                            core_cx.emit(crate::Update::OpenFile(path.clone()));
                                        });
                                    }
                                }
                                // Check if it's a document ID for buffer switching (legacy)
                                else if let Some(doc_id) =
                                    selected_item.data.downcast_ref::<helix_view::DocumentId>()
                                {
                                    // Switch to the selected buffer
                                    if let Some(core) = core_weak.upgrade() {
                                        core.update(picker_cx, |core, _cx| {
                                            // Switch to the selected document
                                            core.editor.switch(
                                                *doc_id,
                                                helix_view::editor::Action::Replace,
                                            );
                                        });
                                    }
                                }

                                // Dismiss the picker after selection
                                picker_cx.emit(gpui::DismissEvent);
                            });

                            // Set up the cancel callback
                            view = view.on_cancel(move |cx| {
                                // The PickerView will handle its own dismissal
                                cx.emit(DismissEvent);
                            });

                            view
                        });

                        // Subscribe to dismiss events from the picker view
                        cx.subscribe(
                            &picker_view,
                            |this, picker_view, _event: &DismissEvent, cx| {
                                // Clean up the picker before clearing it
                                picker_view.update(cx, |picker, cx| {
                                    picker.cleanup(cx);
                                });
                                this.native_picker_view = None;
                                // Emit dismiss event to notify workspace
                                cx.emit(DismissEvent);
                                cx.notify();
                            },
                        )
                        .detach();

                        self.native_picker_view = Some(picker_view);
                    }
                }

                cx.notify();
            }
            crate::Update::DirectoryPicker(_picker) => {
                // Use GPUI's native file dialog API
                let core_weak = self.core.clone();
                cx.spawn(async move |this, cx| {
                    // Configure the dialog to only allow directory selection
                    let options = gpui::PathPromptOptions {
                        files: false,      // Don't allow file selection
                        directories: true, // Allow directory selection
                        multiple: false,   // Single directory only
                    };

                    // Open the native directory picker
                    let result = cx.update(|cx| cx.prompt_for_paths(options)).ok();

                    if let Some(receiver) = result {
                        // Wait for the user to select a directory
                        if let Ok(Ok(Some(paths))) = receiver.await {
                            if let Some(path) = paths.first() {
                                // Emit the selected directory through the core entity
                                if let Some(core) = core_weak.upgrade() {
                                    cx.update(|cx| {
                                        core.update(cx, |_core, cx| {
                                            cx.emit(crate::Update::OpenDirectory(path.clone()));
                                        });
                                    })
                                    .ok();
                                }
                                // Dismiss the overlay
                                cx.update(|cx| {
                                    this.update(cx, |_this, cx| {
                                        cx.emit(DismissEvent);
                                    })
                                    .ok();
                                })
                                .ok();
                            }
                        } else {
                            // User cancelled - just dismiss
                            cx.update(|cx| {
                                this.update(cx, |_this, cx| {
                                    cx.emit(DismissEvent);
                                })
                                .ok();
                            })
                            .ok();
                        }
                    }
                })
                .detach();

                cx.notify();
            }
            crate::Update::Redraw => {
                // Don't clear native picker on redraw - let it persist until dismissed by user action
            }
            _ => {}
        }
    }

    /// Create PromptStyle using ThemedContext for consistent theme access
    fn create_prompt_style_from_context(cx: &App) -> nucleotide_ui::prompt_view::PromptStyle {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get ui.menu style with fallback to design tokens
        let ui_menu = cx.theme_style("ui.menu");

        nucleotide_ui::prompt_view::PromptStyle {
            modal_style,
            completion_background: ui_menu
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(tokens.colors.menu_background),
        }
    }

    /// Create ModalStyle using ThemedContext with design token fallbacks
    fn create_modal_style_from_context(cx: &App) -> nucleotide_ui::common::ModalStyle {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get theme styles for specific overrides
        let ui_popup = cx.theme_style("ui.popup");
        let ui_text = cx.theme_style("ui.text");
        let ui_menu_selected = cx.theme_style("ui.menu.selected");

        // Use design tokens as fallbacks - guaranteed to exist
        let background = ui_popup
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.popup_background);
        let text = ui_text
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.text_primary);
        let selected_background = ui_menu_selected
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.surface_selected);
        let selected_text = ui_menu_selected
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.text_primary);
        let border = ui_popup
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.border_default);
        let prompt_text = text;

        nucleotide_ui::common::ModalStyle {
            background,
            text,
            border,
            selected_background,
            selected_text,
            prompt_text,
        }
    }

    /// Calculate cursor position for completion popup placement
    /// TODO: This should get actual cursor position from document view
    /// Calculate cursor-based completion position using exact cursor coordinates when available
    fn calculate_completion_position(&self, cx: &Context<Self>) -> (gpui::Pixels, gpui::Pixels) {
        let layout_info = self.get_workspace_layout_info(cx);

        // Use exact cursor coordinates if available from DocumentView rendering
        if let (Some(cursor_pos), Some(cursor_size)) =
            (layout_info.cursor_position, layout_info.cursor_size)
        {
            println!(
                "DEBUG: Using exact cursor coordinates - pos={:?}, size={:?}",
                cursor_pos, cursor_size
            );
            // cursor_pos is already the top-left of cursor, we want bottom-left for completion
            return (cursor_pos.x, cursor_pos.y + cursor_size.height);
        }

        // Fallback to calculated position if no exact coordinates available
        println!("DEBUG: Using calculated cursor position (fallback)");
        if let Some(core) = self.core.upgrade() {
            let core_read = core.read(cx);
            let editor = &core_read.editor;

            // Get focused view and document
            let focused_view_id = editor.tree.focus;
            let view = editor.tree.get(focused_view_id);
            let doc_id = view.doc;

            match editor.documents.get(&doc_id) {
                Some(document) => {
                    let text = document.text();

                    // Get primary cursor position
                    let primary_selection = document.selection(focused_view_id).primary();
                    let cursor_char_idx = primary_selection.cursor(text.slice(..));

                    // Convert character position to screen coordinates
                    if let Some(cursor_pos) =
                        view.screen_coords_at_pos(document, text.slice(..), cursor_char_idx)
                    {
                        let view_offset = document.view_offset(focused_view_id);

                        // Check if cursor is visible in viewport
                        let viewport_height = view.inner_height();
                        if cursor_pos.row >= view_offset.vertical_offset
                            && cursor_pos.row < view_offset.vertical_offset + viewport_height
                        {
                            // Calculate relative position within viewport (0-based line in visible area)
                            let relative_row =
                                cursor_pos.row.saturating_sub(view_offset.vertical_offset);

                            // Account for UI layout: file tree + gutter + character position
                            let cursor_x = layout_info.file_tree_width
                                + layout_info.gutter_width
                                + px(cursor_pos.col as f32 * layout_info.char_width.0);

                            // Account for UI layout: title bar + tab bar + line position within document area
                            let document_area_y =
                                layout_info.title_bar_height + layout_info.tab_bar_height;
                            let cursor_y = document_area_y
                                + px(relative_row as f32 * layout_info.line_height.0);

                            return (cursor_x, cursor_y + layout_info.line_height); // Position below cursor
                        }
                    }
                }
                None => {}
            }
        }

        // Final fallback positioning
        (
            layout_info.file_tree_width + layout_info.gutter_width + px(10.0),
            layout_info.title_bar_height + layout_info.tab_bar_height + px(20.0),
        )
    }

    /// Get workspace layout information for completion positioning
    /// Attempts to access real workspace dimensions, falls back to reasonable defaults
    fn get_workspace_layout_info(&self, cx: &Context<Self>) -> WorkspaceLayoutInfo {
        // Try to access workspace layout through global state
        if let Some(layout) = cx.try_global::<WorkspaceLayoutInfo>() {
            return *layout;
        }

        // TODO: Alternative approaches to access workspace:
        // 1. Pass layout info when creating completion view
        // 2. Store layout in app-global state
        // 3. Make workspace accessible through entity hierarchy

        // For now, use reasonable defaults that match typical Nucleotide layout
        // These values should be close to the actual defaults but won't reflect user resizing
        WorkspaceLayoutInfo {
            file_tree_width: px(250.0), // Default file tree width (user can resize)
            gutter_width: px(60.0),     // Line number gutter width
            tab_bar_height: px(40.0),   // Tab bar height
            title_bar_height: px(30.0), // Much smaller - just window controls
            line_height: px(20.0),      // Default line height
            char_width: px(12.0),       // Default character width
            cursor_position: None,      // No exact cursor position available
            cursor_size: None,          // No exact cursor size available
        }
    }
}

/// Layout information for positioning UI elements relative to workspace
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceLayoutInfo {
    pub file_tree_width: gpui::Pixels,
    pub gutter_width: gpui::Pixels,
    pub tab_bar_height: gpui::Pixels,
    pub title_bar_height: gpui::Pixels,
    // Font metrics from the actual DocumentView
    pub line_height: gpui::Pixels,
    pub char_width: gpui::Pixels,
    // Exact cursor coordinates from DocumentView rendering
    pub cursor_position: Option<gpui::Point<Pixels>>,
    pub cursor_size: Option<gpui::Size<Pixels>>,
}

impl gpui::Global for WorkspaceLayoutInfo {}

impl OverlayView {
    /// Create PickerView using ThemedContext with design token fallbacks
    fn create_picker_view_with_context(cx: &mut gpui::Context<PickerView>) -> PickerView {
        use nucleotide_ui::theme_utils::color_to_hsla;

        // Get modal style using ThemedContext
        let modal_style = Self::create_modal_style_from_context(cx);

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get theme styles for specific overrides
        let ui_background_separator = cx.theme_style("ui.background.separator");
        let ui_cursor = cx.theme_style("ui.cursor");

        // Use design tokens as fallbacks - guaranteed to exist
        let preview_background = ui_background_separator
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.surface);
        let cursor = ui_cursor
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(tokens.colors.primary);

        let picker_style = nucleotide_ui::picker_view::PickerStyle {
            modal_style,
            preview_background,
            cursor,
        };

        PickerView::new(cx).with_style(picker_style)
    }
}

impl Focusable for OverlayView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate focus to the active native component, but not completion views
        // Completion views should not steal focus - let the editor maintain focus
        if let Some(picker_view) = &self.native_picker_view {
            picker_view.focus_handle(cx)
        } else if let Some(prompt_view) = &self.native_prompt_view {
            prompt_view.focus_handle(cx)
        } else {
            // Don't delegate to completion_view - let editor keep focus
            self.focus.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for OverlayView {}
impl EventEmitter<nucleotide_ui::completion_v2::CompleteViaHelixEvent> for OverlayView {}

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check what type of overlay we should render
        if let Some(picker_view) = &self.native_picker_view {
            // Render picker with design tokens and consistent overlay patterns
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    // Prevent click-through to elements below
                })
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(picker_view.clone()),
                )
                .into_any_element();
        }

        if let Some(prompt_view) = &self.native_prompt_view {
            // Render prompt with design tokens and consistent overlay patterns
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, _| {
                    // Prevent click-through to elements below
                })
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(prompt_view.clone()),
                )
                .into_any_element();
        }

        if let Some(completion_view) = &self.completion_view {
            use gpui::{Corner, anchored, point};

            // Calculate proper completion position based on cursor location
            let (cursor_x, cursor_y) = self.calculate_completion_position(cx);

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .child(
                    anchored()
                        .position(point(cursor_x, cursor_y))
                        .anchor(Corner::TopLeft) // Anchor top-left of completion to cursor position
                        .offset(point(px(0.0), px(2.0))) // Small offset below cursor
                        .snap_to_window_with_margin(px(8.0))
                        .child(completion_view.clone()),
                )
                .into_any_element();
        }

        // Legacy prompt fallback
        if let Some(prompt) = self.prompt.take() {
            let handle = cx.focus_handle();
            let theme = self
                .core
                .upgrade()
                .map(|core| core.read(cx).editor.theme.clone());
            let prompt_elem = PromptElement {
                prompt,
                focus: handle.clone(),
                theme,
            };
            let theme = cx.theme();
            let tokens = &theme.tokens;

            return div()
                .key_context("Overlay")
                .absolute()
                .size_full()
                .bottom_0()
                .left_0()
                .occlude()
                .child(
                    div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_start()
                        .pt(tokens.sizes.space_8)
                        .child(prompt_elem),
                )
                .into_any_element();
        }

        // Empty overlay using design tokens
        div().size_0().into_any_element()
    }
}
