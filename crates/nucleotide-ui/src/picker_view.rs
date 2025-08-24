// ABOUTME: GPUI-native picker component for fuzzy searching and selection
// ABOUTME: Uses proper GPUI uniform_list for scrollable content like Zed

use crate::common::{FocusableModal, ModalStyle, SearchInput};
use crate::{VcsIcon, VcsStatus};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, DismissEvent, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, Render, Result, SharedString, Size, Styled, Task,
    UniformListScrollHandle, div, hsla, px, uniform_list,
};
use gpui::{Context, ScrollStrategy, Window};
use helix_view::DocumentId;
use nucleo::Nucleo;
use nucleotide_core::preview_tracker::PreviewTracker;
use nucleotide_logging::warn;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug)]
pub enum ColumnData {
    BufferColumns {
        id: String,
        flags: String,
        path: String,
    },
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: SharedString,
    pub sublabel: Option<SharedString>,
    pub data: Arc<dyn std::any::Any + Send + Sync>,
    /// Optional file path for VCS status lookup and icon rendering
    pub file_path: Option<std::path::PathBuf>,
    /// Optional VCS status for this item
    pub vcs_status: Option<VcsStatus>,
    /// Optional structured column data for table-like display
    pub columns: Option<ColumnData>,
}

impl PickerItem {
    /// Create a new PickerItem for a file with path information
    pub fn from_file_path(
        label: impl Into<SharedString>,
        file_path: std::path::PathBuf,
        data: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            label: label.into(),
            sublabel: None,
            data,
            file_path: Some(file_path),
            vcs_status: None,
            columns: None,
        }
    }

    /// Create a new PickerItem for a file with path and VCS status
    pub fn from_file_path_with_vcs(
        label: impl Into<SharedString>,
        file_path: std::path::PathBuf,
        vcs_status: Option<VcsStatus>,
        data: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            label: label.into(),
            sublabel: None,
            data,
            file_path: Some(file_path),
            vcs_status,
            columns: None,
        }
    }

    /// Create a new PickerItem with sublabel and file path
    pub fn with_sublabel_and_path(
        label: impl Into<SharedString>,
        sublabel: impl Into<SharedString>,
        file_path: std::path::PathBuf,
        data: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            label: label.into(),
            sublabel: Some(sublabel.into()),
            data,
            file_path: Some(file_path),
            vcs_status: None,
            columns: None,
        }
    }

    /// Create a new PickerItem with buffer columns for table display
    pub fn with_buffer_columns(
        id: impl Into<String>,
        flags: impl Into<String>,
        path: impl Into<String>,
        data: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        let path_str = path.into();
        Self {
            label: path_str.clone().into(), // Use path as fallback label
            sublabel: None,
            data,
            file_path: None, // Buffer items don't need file path for icons
            vcs_status: None,
            columns: Some(ColumnData::BufferColumns {
                id: id.into(),
                flags: flags.into(),
                path: path_str,
            }),
        }
    }
}

// Type aliases for callbacks
type PickerSelectCallback = Box<dyn FnMut(&PickerItem, &mut Context<PickerView>) + 'static>;
type PickerCancelCallback = Box<dyn FnMut(&mut Context<PickerView>) + 'static>;

pub struct PickerView {
    // Core picker state
    query: SharedString,
    cursor_position: usize,
    items: Vec<PickerItem>,
    filtered_indices: Vec<u32>,
    selected_index: usize,

    // Fuzzy matcher
    matcher: Option<Nucleo<PickerItem>>,

    // UI state
    focus_handle: FocusHandle,
    list_scroll_handle: UniformListScrollHandle,

    // Preview state
    show_preview: bool,
    preview_content: Option<String>,
    preview_loading: bool,
    preview_doc_id: Option<DocumentId>,
    preview_view_id: Option<helix_view::ViewId>,
    // TODO: Replace with capability trait
    // core: Option<WeakEntity<crate::Core>>,
    initial_preview_loaded: bool,
    preview_task: Option<Task<()>>,

    // Callbacks
    on_select: Option<PickerSelectCallback>,
    on_cancel: Option<PickerCancelCallback>,

    // Styling
    style: PickerStyle,

    // Cached dimensions to prevent resizing on key presses
    cached_dimensions: Option<CachedDimensions>,
}

#[derive(Clone, Copy, Debug)]
struct CachedDimensions {
    window_size: Size<Pixels>,
    total_width: Pixels,
    max_height: Pixels,
    list_width: Pixels,
    preview_width: Pixels,
    show_preview: bool,
}

#[derive(Clone)]
pub struct PickerStyle {
    pub modal_style: ModalStyle,
    pub preview_background: Hsla,
    pub cursor: Hsla,
}

impl Default for PickerStyle {
    fn default() -> Self {
        Self {
            modal_style: ModalStyle::default(),
            preview_background: hsla(0.0, 0.0, 0.05, 1.0),
            cursor: hsla(0.0, 0.0, 0.9, 1.0),
        }
    }
}

impl PickerStyle {
    /// Create PickerStyle from helix theme using appropriate theme keys
    pub fn from_theme(theme: &helix_view::Theme) -> Self {
        use crate::theme_utils::color_to_hsla;

        let modal_style = ModalStyle::from_theme(theme);

        // Get preview background - slightly different from main background
        let preview_background = theme
            .get("ui.background.separator")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.background").bg.and_then(color_to_hsla))
            .unwrap_or(modal_style.background);

        // Get cursor color from theme
        let cursor = theme
            .get("ui.cursor")
            .fg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.cursor.primary").fg.and_then(color_to_hsla))
            .or_else(|| theme.get("ui.cursor").bg.and_then(color_to_hsla))
            .unwrap_or(modal_style.text);

        Self {
            modal_style,
            preview_background,
            cursor,
        }
    }
}

impl PickerView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            query: SharedString::default(),
            cursor_position: 0,
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected_index: 0,
            matcher: None,
            focus_handle: cx.focus_handle(),
            list_scroll_handle: UniformListScrollHandle::new(),
            show_preview: true,
            preview_content: None,
            preview_loading: false,
            preview_doc_id: None,
            preview_view_id: None,
            // core: None, // TODO: Replace with capability trait
            initial_preview_loaded: false,
            preview_task: None,
            on_select: None,
            on_cancel: None,
            style: PickerStyle::default(),
            cached_dimensions: None,
        }
    }

    /// Create a new PickerView with theme-based styling
    pub fn new_with_theme(theme: &helix_view::Theme, cx: &mut Context<Self>) -> Self {
        let style = PickerStyle::from_theme(theme);

        Self {
            query: SharedString::default(),
            cursor_position: 0,
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected_index: 0,
            matcher: None,
            focus_handle: cx.focus_handle(),
            list_scroll_handle: UniformListScrollHandle::new(),
            show_preview: true,
            preview_content: None,
            preview_loading: false,
            preview_doc_id: None,
            preview_view_id: None,
            // core: None, // TODO: Replace with capability trait
            initial_preview_loaded: false,
            preview_task: None,
            on_select: None,
            on_cancel: None,
            style,
            cached_dimensions: None,
        }
    }

    // TODO: Replace with capability trait
    // pub fn with_core(mut self, core: WeakEntity<crate::Core>) -> Self {
    //     self.core = Some(core);
    //     self
    // }

    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.items = items;
        // Reasonable assumption: pickers won't have more than u32::MAX items
        let item_count = u32::try_from(self.items.len()).unwrap_or(u32::MAX);
        self.filtered_indices = (0..item_count).collect();
        // Reset matcher when items change
        self.matcher = None;
        // Reset initial preview flag so preview loads for new items
        self.initial_preview_loaded = false;
        // Clear any existing preview document IDs (cleanup happens elsewhere)
        self.preview_doc_id = None;
        self.preview_view_id = None;
        self.preview_content = None;
        // VCS status will be fetched from global service as needed
        self
    }

    pub fn with_style(mut self, style: PickerStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_preview(mut self, show_preview: bool) -> Self {
        self.show_preview = show_preview;
        self
    }

    pub fn on_select(
        mut self,
        callback: impl FnMut(&PickerItem, &mut Context<Self>) + 'static,
    ) -> Self {
        self.on_select = Some(Box::new(callback));
        self
    }

    pub fn on_cancel(mut self, callback: impl FnMut(&mut Context<Self>) + 'static) -> Self {
        self.on_cancel = Some(Box::new(callback));
        self
    }

    pub fn set_query(&mut self, query: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.query = query.into();
        self.cursor_position = self.query.len();
        self.filter_items(cx);
        self.selected_index = 0;
        // Scroll to top when query changes
        self.list_scroll_handle
            .scroll_to_item(0, ScrollStrategy::Top);
        self.load_preview_for_selected_item(cx);
        cx.notify();
    }

    fn filter_items(&mut self, _cx: &mut Context<Self>) {
        if self.query.is_empty() {
            // Reasonable assumption: pickers won't have more than u32::MAX items
            let item_count = u32::try_from(self.items.len()).unwrap_or(u32::MAX);
            self.filtered_indices = (0..item_count).collect();
        } else {
            // Simple fuzzy matching for now
            // TODO: Properly integrate nucleo when API is stable
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    // Basic fuzzy matching: check if all query characters appear in order
                    let item_lower = item.label.to_lowercase();
                    let query_lower = self.query.to_lowercase();

                    if query_lower.is_empty() {
                        return true;
                    }

                    let mut query_chars = query_lower.chars();
                    let mut current_char = query_chars.next();

                    if current_char.is_none() {
                        return true;
                    }

                    for item_char in item_lower.chars() {
                        if let Some(q_char) = current_char
                            && item_char == q_char
                        {
                            current_char = query_chars.next();
                            if current_char.is_none() {
                                return true; // All query chars found
                            }
                        }
                    }

                    current_char.is_none() // True if all chars were matched
                })
                .filter_map(|(idx, _)| u32::try_from(idx).ok())
                .collect();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let _old_index = self.selected_index;
        let new_index = if delta > 0 {
            let delta_usize = usize::try_from(delta).unwrap_or(0);
            (self.selected_index + delta_usize).min(self.filtered_indices.len() - 1)
        } else {
            let delta_usize = usize::try_from(-delta).unwrap_or(0);
            self.selected_index.saturating_sub(delta_usize)
        };

        self.selected_index = new_index;

        // Scroll to keep selection visible - GPUI handles this automatically!
        self.list_scroll_handle
            .scroll_to_item(self.selected_index, ScrollStrategy::Top);

        // Load preview for newly selected item
        self.load_preview_for_selected_item(cx);

        cx.notify();
    }

    fn confirm_selection(&mut self, cx: &mut Context<Self>) {
        // Clean up preview document before confirming selection
        self.cleanup_preview_document(cx);

        if let Some(idx) = self.filtered_indices.get(self.selected_index)
            && let Some(item) = self.items.get(*idx as usize)
            && let Some(on_select) = &mut self.on_select
        {
            on_select(item, cx);
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        // Clean up preview document before cancelling
        self.cleanup_preview_document(cx);

        if let Some(on_cancel) = &mut self.on_cancel {
            on_cancel(cx);
        }
    }

    fn insert_char(&mut self, ch: char, cx: &mut Context<Self>) {
        let mut query = self.query.to_string();
        let chars: Vec<char> = query.chars().collect();

        // Calculate byte position from character position
        let mut byte_pos = 0;
        for (i, c) in chars.iter().enumerate() {
            if i >= self.cursor_position {
                break;
            }
            byte_pos += c.len_utf8();
        }

        query.insert(byte_pos, ch);
        self.cursor_position += 1; // Move cursor by one character position
        self.query = query.into();
        self.filter_items(cx);
        self.selected_index = 0;
        self.list_scroll_handle
            .scroll_to_item(0, ScrollStrategy::Top);
        self.load_preview_for_selected_item(cx);
        cx.notify();
    }

    fn delete_char(&mut self, cx: &mut Context<Self>) {
        if self.cursor_position > 0 {
            let mut query = self.query.to_string();
            let char_pos = self.cursor_position.saturating_sub(1);
            let char_count = query.chars().count();
            if char_pos < char_count {
                // Find the byte position for the character position
                let mut byte_pos = 0;
                for (i, ch) in query.chars().enumerate() {
                    if i == char_pos {
                        break;
                    }
                    byte_pos += ch.len_utf8();
                }
                // Safe access to character at position
                if let Some(ch) = query.chars().nth(char_pos) {
                    let ch_len = ch.len_utf8();
                    query.drain(byte_pos..byte_pos + ch_len);
                } else {
                    warn!(
                        char_pos = char_pos,
                        "Attempted to delete character at invalid position"
                    );
                }
                self.query = query.into();
                self.cursor_position = char_pos;
                self.filter_items(cx);
                self.selected_index = 0;
                self.list_scroll_handle
                    .scroll_to_item(0, ScrollStrategy::Top);
                self.load_preview_for_selected_item(cx);
                cx.notify();
            }
        }
    }

    fn calculate_dimensions(&self, window_size: Size<Pixels>) -> CachedDimensions {
        let min_width_for_preview = 800.0;
        let window_width = f64::from(window_size.width.0);
        let window_height = window_size.height;

        let show_preview = self.show_preview && window_width > min_width_for_preview;

        // Calculate fixed dimensions to prevent size changes
        let total_width = px(800.0); // Fixed width

        // Calculate height based on items with max 60% of window
        let item_height = px(32.0); // Approximate height per item (including padding)
        let header_footer_height = px(80.0); // Space for search bar, footer, etc.

        // Use filtered items if available, otherwise use all items
        let item_count = if self.filtered_indices.is_empty() && self.query.is_empty() {
            self.items.len()
        } else {
            self.filtered_indices.len()
        };

        let items_height = item_height * item_count.min(20) as f32; // Cap at 20 visible items
        let content_height = items_height + header_footer_height;

        // Limit to 60% of window height
        let max_allowed_height = window_height * 0.6;
        let max_height = content_height.min(max_allowed_height).max(px(200.0)); // Min height of 200px

        let (list_width, preview_width) = if show_preview {
            (px(400.0), px(400.0))
        } else {
            (total_width, px(0.0))
        };

        CachedDimensions {
            window_size,
            total_width,
            max_height,
            list_width,
            preview_width,
            show_preview,
        }
    }

    fn load_preview_for_selected_item(&mut self, cx: &mut Context<Self>) {
        if !self.show_preview {
            return;
        }

        let Some(selected_idx) = self.filtered_indices.get(self.selected_index) else {
            self.preview_content = None;
            return;
        };

        let Some(item) = self.items.get(*selected_idx as usize) else {
            self.preview_content = None;
            return;
        };

        // Try to extract path from item data
        // Handle buffer picker items (DocumentId, Option<PathBuf>) first
        if let Some((_doc_id, _path_opt)) = item
            .data
            .downcast_ref::<(helix_view::DocumentId, Option<std::path::PathBuf>)>()
        {
            // For buffer picker items, always use the existing document content
            // Don't create a new document for preview
            // TODO: Replace with capability trait
            // if let Some(core_weak) = &self.core {
            //     if let Some(core) = core_weak.upgrade() {
            //         let content = core
            //             .read(cx)
            //             .editor
            //             .document(*doc_id)
            //             .map(|doc| {
            //                 let text = doc.text();
            //                 let content = text.to_string();
            //                 // Limit preview to first 500 lines for performance
            //                 let lines: Vec<&str> = content.lines().take(500).collect();
            //                 lines.join("\n")
            //             })
            //             .unwrap_or_else(|| "Unable to load buffer content".to_string());

            //         self.preview_content = Some(content);
            //         self.preview_loading = false;
            //         // Store the doc_id so we can use syntax highlighting
            //         self.preview_doc_id = Some(*doc_id);
            //         cx.notify();
            //     }
            // }
        }
        // Try standalone PathBuf (for file picker)
        else if let Some(path_buf) = item.data.downcast_ref::<std::path::PathBuf>() {
            self.load_file_preview(path_buf.clone(), cx);
        } else {
            // Debug: check what type we actually have
            warn!(
                type_id = ?item.data.type_id(),
                "Preview not available for item with type_id"
            );
            self.preview_content = Some("Preview not available".to_string());
            cx.notify();
        }
    }

    fn load_file_preview(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.preview_loading {
            return;
        }

        // Clean up previous preview document if any
        self.cleanup_preview_document(cx);

        self.preview_loading = true;
        self.preview_content = Some("Loading...".to_string());
        cx.notify();

        // TODO: Replace with capability trait
        // let core_weak = self.core.clone();
        let _core_weak = None::<gpui::WeakEntity<()>>;
        // When spawning from Context<T>, the closure gets WeakEntity<T> as first param
        self.preview_task = Some(cx.spawn(async move |view_weak, cx| {
            let content = if path.is_dir() {
                // Load directory listing
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut content = format!("Directory: {}\n\n", path.display());
                        let mut items: Vec<_> =
                            entries.collect::<Result<Vec<_>, _>>().unwrap_or_default();
                        items.sort_by(|a, b| {
                            let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            match (a_is_dir, b_is_dir) {
                                (true, false) => std::cmp::Ordering::Less,
                                (false, true) => std::cmp::Ordering::Greater,
                                _ => a.file_name().cmp(&b.file_name()),
                            }
                        });

                        for entry in items.iter().take(100) {
                            let name = entry.file_name();
                            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            content.push_str(&format!(
                                "{}{}\n",
                                if is_dir { "📁 " } else { "📄 " },
                                name.to_string_lossy()
                            ));
                        }
                        if items.len() > 100 {
                            content
                                .push_str(&format!("\n... and {} more items\n", items.len() - 100));
                        }
                        content
                    }
                    Err(e) => format!("Error reading directory: {e}"),
                }
            } else {
                // Load file content
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        if content.len() > 10000 {
                            format!(
                                "{}\n\n[File truncated - showing first 10KB of {}KB total]",
                                &content[..10000],
                                content.len() / 1024
                            )
                        } else {
                            content
                        }
                    }
                    Err(_) => {
                        // Try to read as binary and show info
                        match std::fs::metadata(&path) {
                            Ok(meta) => format!(
                                "Binary file: {}\nSize: {} bytes\nModified: {:?}",
                                path.display(),
                                meta.len(),
                                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                            ),
                            Err(e) => format!("Error reading file: {e}"),
                        }
                    }
                }
            };

            _ = view_weak.update(cx, |this, cx| {
                // For directories, just show the content as before
                if path.is_dir() {
                    this.preview_loading = false;
                    this.preview_content = Some(content);
                    cx.notify();
                    return;
                }

                // TODO: Replace with capability trait
                // For files, we would normally create a document for syntax highlighting
                // but that requires Core access which we don't have in the UI crate
                // For now, just display the plain text

                this.preview_loading = false;
                cx.notify();
            });
        }));
    }

    /// Clean up preview document - public method for external cleanup
    pub fn cleanup(&mut self, cx: &mut Context<Self>) {
        self.cleanup_preview_document(cx);
    }

    fn cleanup_preview_document(&mut self, cx: &mut Context<Self>) {
        // Cancel any pending preview task
        self.preview_task = None;

        // Only clean up if we have both doc_id and view_id
        // (view_id indicates we created a new document for preview)
        if let (Some(doc_id), Some(view_id)) =
            (self.preview_doc_id.take(), self.preview_view_id.take())
        {
            // TODO: Replace with capability trait
            // if let Some(core_weak) = &self.core {
            //     if let Some(core) = core_weak.upgrade() {
            //         core.update(cx, |core, _cx| {
            //             // Close the view first, but only if it still exists
            //             if core.editor.tree.contains(view_id) {
            //                 core.editor.close(view_id);
            //             }
            //             // Then close the document without saving
            //             let _ = core.editor.close_document(doc_id, false);
            //
            //             // Note: Unregistering from preview tracker happens in the outer scope
            //         });
            //
            //         // Unregister from preview tracker
            //         if let Some(tracker) = cx.try_global::<PreviewTracker>() {
            //             tracker.unregister(doc_id, view_id);
            //         }
            //     }
            // }

            // Unregister from preview tracker even without Core
            if let Some(tracker) = cx.try_global::<PreviewTracker>() {
                tracker.unregister(doc_id, view_id);
            }
        } else {
            // If we only have doc_id (no view_id), it means we're showing an existing buffer
            // Just clear the reference, don't close the document
            self.preview_doc_id = None;
        }
    }
}

impl Focusable for PickerView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PickerView {}

impl Drop for PickerView {
    fn drop(&mut self) {
        // Clean up preview document when picker is closed
        // Note: We can't use update() in drop, so cleanup happens via cleanup_preview_document
        // when the picker is dismissed or a new file is selected
    }
}

impl FocusableModal for PickerView {}

impl PickerView {
    fn render_picker_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Ensure the picker has focus when rendered
        self.ensure_focus(window, &self.focus_handle);

        // Load initial preview if not already loaded
        if !self.initial_preview_loaded && !self.filtered_indices.is_empty() {
            self.initial_preview_loaded = true;
            self.load_preview_for_selected_item(cx);
        }

        let font = cx
            .global::<nucleotide_types::FontSettings>()
            .var_font
            .clone()
            .into();
        let window_size = window.viewport_size();

        // Check if we need to recalculate dimensions
        let dimensions = if let Some(cached) = self.cached_dimensions {
            // Only recalculate if window size changed
            if cached.window_size != window_size {
                self.calculate_dimensions(window_size)
            } else {
                cached
            }
        } else {
            self.calculate_dimensions(window_size)
        };

        // Update cache
        self.cached_dimensions = Some(dimensions);

        let total_width = dimensions.total_width;
        let max_height = dimensions.max_height;
        let list_width = dimensions.list_width;
        let preview_width = dimensions.preview_width;
        let show_preview = dimensions.show_preview;

        div()
            .flex()
            .flex_col()
            .key_context("Picker") // Set proper key context for picker
            .w(total_width)
            .h(max_height) // Use fixed height instead of max_h to prevent size changes
            .bg(self.style.modal_style.background)
            .border_1()
            .border_color(self.style.modal_style.border)
            .rounded_md()
            .shadow_lg()
            .font(font)
            .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            // Handle keyboard input for filtering
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "backspace" => {
                        this.delete_char(cx);
                    }
                    "enter" => {
                        this.confirm_selection(cx);
                    }
                    "escape" => {
                        if this.query.is_empty() {
                            this.cancel(cx);
                        } else {
                            // Clear the query instead of cancelling
                            this.set_query("", cx);
                        }
                    }
                    "up" => {
                        this.move_selection(-1, cx);
                    }
                    "down" => {
                        this.move_selection(1, cx);
                    }
                    "left" => {
                        if this.cursor_position > 0 {
                            this.cursor_position -= 1;
                            cx.notify();
                        }
                    }
                    "right" => {
                        let char_count = this.query.chars().count();
                        if this.cursor_position < char_count {
                            this.cursor_position += 1;
                            cx.notify();
                        }
                    }
                    "home" => {
                        this.cursor_position = 0;
                        cx.notify();
                    }
                    "end" => {
                        this.cursor_position = this.query.chars().count();
                        cx.notify();
                    }
                    key if key.len() == 1 => {
                        if let Some(ch) = key.chars().next()
                            && (ch.is_alphanumeric()
                                || ch.is_ascii_punctuation()
                                || ch == ' '
                                || ch == '/'
                                || ch == '.'
                                || ch == '-'
                                || ch == '_')
                        {
                            this.insert_char(ch, cx);
                        }
                    }
                    _ => {
                        // Let other keys be handled by actions
                    }
                }
            }))
            // Use GPUI actions instead of direct key handling
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::SelectPrev, _window, cx| {
                    this.move_selection(-1, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::SelectNext, _window, cx| {
                    this.move_selection(1, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::SelectFirst, _window, cx| {
                    this.move_selection(-(this.selected_index as isize), cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::SelectLast, _window, cx| {
                    let last_index = this.filtered_indices.len().saturating_sub(1);
                    let delta = last_index as isize - this.selected_index as isize;
                    this.move_selection(delta, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::ConfirmSelection, _window, cx| {
                    this.confirm_selection(cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::actions::picker::DismissPicker, _window, cx| {
                    this.cancel(cx);
                },
            ))
            .child(
                // Search input with file count display
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .h_10() // Fixed height for search input
                    .border_b_1()
                    .border_color(self.style.modal_style.border)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .child(SearchInput::render(
                                &self.query,
                                self.cursor_position,
                                self.style.cursor,
                                self.style.modal_style.prompt_text,
                                self.focus_handle.is_focused(window),
                            )),
                    )
                    .child(
                        // File count display
                        div()
                            .text_size(px(12.))
                            .text_color(self.style.modal_style.prompt_text)
                            .child(if self.filtered_indices.is_empty() {
                                "0/0".to_string()
                            } else {
                                format!(
                                    "{}/{}",
                                    self.selected_index + 1,
                                    self.filtered_indices.len()
                                )
                            }),
                    ),
            )
            // Add column headers for buffer picker
            .when(
                self.items
                    .first()
                    .map(|item| {
                        // Check if this is a buffer picker by looking at the label format
                        let parts: Vec<&str> = item.label.split_whitespace().collect();
                        // Check if first part looks like an ID (numeric or starts with digit)
                        parts.len() >= 3 && parts[0].chars().next().is_some_and(char::is_numeric)
                    })
                    .unwrap_or(false),
                |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .px_3()
                            .py_1()
                            .border_b_1()
                            .border_color(self.style.modal_style.border)
                            .text_color(self.style.modal_style.prompt_text)
                            .font_family("monospace")
                            .text_size(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        // ID header
                                        div().w(px(50.0)).child("id"),
                                    )
                                    .child(
                                        // Flags header
                                        div().w(px(30.0)).text_center().child("flags"),
                                    )
                                    .child(
                                        // Path header
                                        div().flex_1().child("path"),
                                    ),
                            ),
                    )
                },
            )
            .child(
                // Main content area - horizontal split
                div()
                    .flex()
                    .h_full() // Use full height of remaining space
                    .overflow_hidden()
                    .child(
                        // File list using proper GPUI uniform_list
                        div()
                            .flex()
                            .flex_col()
                            .w(list_width)
                            .h_full() // Use fixed height instead of flex_1
                            .overflow_hidden() // Ensure overflow is hidden
                            .when(show_preview, |this| {
                                this.border_r_1()
                                    .border_color(self.style.modal_style.border)
                            })
                            .when(self.filtered_indices.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .h_24()
                                        .text_color(self.style.modal_style.prompt_text)
                                        .child("No matches found"),
                                )
                            })
                            .when(!self.filtered_indices.is_empty(), |this| {
                                this.child(
                                    uniform_list(
                                        "picker-items",
                                        self.filtered_indices.len(),
                                        cx.processor(
                                            move |picker,
                                                  visible_range: Range<usize>,
                                                  _window,
                                                  _cx| {
                                                visible_range
                                                    .map(|visible_idx| {
                                                        let item_idx = picker.filtered_indices
                                                            [visible_idx]
                                                            as usize;
                                                        let item = &picker.items[item_idx];
                                                        let is_selected =
                                                            visible_idx == picker.selected_index;

                                                        // Full-width wrapper for selection background
                                                        div()
                                                            .id(("picker-item", visible_idx))
                                                            .w_full() // Extend to full width
                                                            .h_8() // Fixed height for items
                                                            .flex() // Make it a flex container
                                                            .items_center() // Center content vertically
                                                            .cursor_pointer()
                                                            .when(is_selected, |this| {
                                                                this.bg(picker
                                                                    .style
                                                                    .modal_style
                                                                    .selected_background)
                                                            })
                                                            .child(
                                                                // Content wrapper with padding
                                                                div()
                                                                    .flex()
                                                                    .items_center()
                                                                    .px_3()
                                                                    .w_full()
                                                                    .when(is_selected, |this| {
                                                                        this.text_color(
                                                                            picker
                                                                                .style
                                                                                .modal_style
                                                                                .selected_text,
                                                                        )
                                                                    })
                                                                    .when(!is_selected, |this| {
                                                                        this.text_color(
                                                                            picker.style.modal_style.text,
                                                                        )
                                                                    })
                                                            .child(
                                                                // Use structured columns if available, fallback to simple label
                                                                match &item.columns {
                                                                    Some(ColumnData::BufferColumns { id, flags, path }) => {
                                                                        // Direct column access for buffer picker
                                                                        div()
                                                                        .flex()
                                                                        .items_center()
                                                                        .gap_2()
                                                                        .font_family("monospace")
                                                                        .child(
                                                                            // ID column
                                                                            div()
                                                                                .w(px(50.0))
                                                                                .flex()
                                                                                .items_center()
                                                                                .overflow_hidden()
                                                                                .text_ellipsis()
                                                                                .child(id.clone())
                                                                        )
                                                                        .child(
                                                                            // Flags column
                                                                            div()
                                                                                .w(px(30.0))
                                                                                .flex()
                                                                                .items_center()
                                                                                .justify_center()
                                                                                .child(flags.clone())
                                                                        )
                                                                        .child(
                                                                            // Path column
                                                                            div()
                                                                                .flex_1()
                                                                                .flex()
                                                                                .items_center()
                                                                                .overflow_hidden()
                                                                                .text_ellipsis()
                                                                                .child(path.clone())
                                                                        )
                                                                    }
                                                                    None => {
                                                                        // File picker or other non-buffer items
                                                                        div()
                                                                            .flex()
                                                                            .items_center()
                                                                            .gap_2()
                                                                            .overflow_hidden()
                                                                            .when_some(
                                                                                item.file_path.as_ref(),
                                                                                |this, file_path| {
                                                                                    // Render VcsIcon for file items
                                                                                    this.child({
                                                                                        // Create VcsIcon with embedded VCS status and proper text color
                                                                                        VcsIcon::from_path(file_path, false)
                                                                                            .size(16.0)
                                                                                            .text_color(picker.style.modal_style.text)
                                                                                            .vcs_status(item.vcs_status)
                                                                                    })
                                                                                }
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .flex_1()
                                                                                    .flex()
                                                                                    .items_center()
                                                                                    .overflow_hidden()
                                                                                    .text_ellipsis()
                                                                                    .font_family("monospace")
                                                                                    .child(item.label.clone())
                                                                            )
                                                                    }
                                                                },
                                                            )
                                                            .when_some(
                                                                item.sublabel.as_ref(),
                                                                |this, sublabel| {
                                                                    this.child(
                                                                        div()
                                                                            .overflow_hidden()
                                                                            .text_ellipsis()
                                                                            .text_size(px(12.))
                                                                            .text_color(
                                                                                picker
                                                                                    .style
                                                                                    .modal_style
                                                                                    .prompt_text,
                                                                            )
                                                                            .child(
                                                                                sublabel.clone(),
                                                                            ),
                                                                    )
                                                                },
                                                            )
                                                            ) // Close content wrapper div
                                                    })
                                                    .collect()
                                            },
                                        ),
                                    )
                                    .h_full() // Use fixed height instead of flex_1
                                    .track_scroll(self.list_scroll_handle.clone()),
                                )
                            }),
                    )
                    .when(show_preview, |this| {
                        this.child(
                            // Preview panel (right side)
                            div()
                                .flex()
                                .flex_col()
                                .w(preview_width)
                                .h_full() // Use full height instead of flex_1
                                .overflow_hidden() // Ensure overflow is hidden
                                .bg(self.style.preview_background)
                                .child(
                                    // Preview content
                                    div()
                                        .h_full() // Use full height instead of flex_1
                                        .overflow_y_hidden() // Hide overflow for preview content
                                        .child({
                                            // TODO: Replace with capability trait - for now just show text preview
                                            // Regular text preview for directories or when no document
                                            div()
                                                .px_3()
                                                .py_2()
                                                .text_size(px(12.))
                                                .text_color(self.style.modal_style.text)
                                                .font_family("monospace")
                                                .child(match &self.preview_content {
                                                    Some(content) => content.clone(),
                                                    None => "Select a file to preview".to_string(),
                                                })
                                                .into_any_element()
                                        }),
                                ),
                        )
                    }),
            )
    }
}

impl Render for PickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The picker view itself is the content that will be wrapped by Overlay
        // We only render the inner content here, not the overlay wrapper
        self.render_picker_content(window, cx)
    }
}
