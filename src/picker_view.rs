// ABOUTME: GPUI-native picker component for fuzzy searching and selection  
// ABOUTME: Uses proper GPUI uniform_list for scrollable content like Zed

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui::{Context, Window, ScrollStrategy};
use nucleo::Nucleo;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub label: SharedString,
    pub sublabel: Option<SharedString>,
    pub data: Arc<dyn std::any::Any + Send + Sync>,
}

pub struct PickerView {
    // Core picker state
    query: SharedString,
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

    // Callbacks
    on_select: Option<Box<dyn FnMut(&PickerItem, &mut Context<Self>) + 'static>>,
    on_cancel: Option<Box<dyn FnMut(&mut Context<Self>) + 'static>>,

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
    pub background: Hsla,
    pub text: Hsla,
    pub selected_background: Hsla,
    pub selected_text: Hsla,
    pub border: Hsla,
    pub prompt_text: Hsla,
}

impl Default for PickerStyle {
    fn default() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.1, 1.0),
            text: hsla(0.0, 0.0, 0.9, 1.0),
            selected_background: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            selected_text: hsla(0.0, 0.0, 1.0, 1.0),
            border: hsla(0.0, 0.0, 0.3, 1.0),
            prompt_text: hsla(0.0, 0.0, 0.7, 1.0),
        }
    }
}

impl PickerView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            query: SharedString::default(),
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected_index: 0,
            matcher: None,
            focus_handle: cx.focus_handle(),
            list_scroll_handle: UniformListScrollHandle::new(),
            show_preview: true,
            preview_content: None,
            preview_loading: false,
            on_select: None,
            on_cancel: None,
            style: PickerStyle::default(),
            cached_dimensions: None,
        }
    }

    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.items = items;
        self.filtered_indices = (0..self.items.len() as u32).collect();
        // Reset matcher when items change
        self.matcher = None;
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

    pub fn on_select(mut self, callback: impl FnMut(&PickerItem, &mut Context<Self>) + 'static) -> Self {
        self.on_select = Some(Box::new(callback));
        self
    }

    pub fn on_cancel(mut self, callback: impl FnMut(&mut Context<Self>) + 'static) -> Self {
        self.on_cancel = Some(Box::new(callback));
        self
    }

    pub fn set_query(&mut self, query: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.query = query.into();
        self.filter_items(cx);
        self.selected_index = 0;
        // Scroll to top when query changes
        self.list_scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        self.load_preview_for_selected_item(cx);
        cx.notify();
    }

    fn filter_items(&mut self, _cx: &mut Context<Self>) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.items.len() as u32).collect();
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
                        if let Some(q_char) = current_char {
                            if item_char == q_char {
                                current_char = query_chars.next();
                                if current_char.is_none() {
                                    return true; // All query chars found
                                }
                            }
                        }
                    }
                    
                    current_char.is_none() // True if all chars were matched
                })
                .map(|(idx, _)| idx as u32)
                .collect();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let old_index = self.selected_index;
        let new_index = if delta > 0 {
            (self.selected_index + delta as usize).min(self.filtered_indices.len() - 1)
        } else {
            self.selected_index.saturating_sub((-delta) as usize)
        };

        self.selected_index = new_index;
        println!("🎯 Selection moved from {} to {} (delta: {})", old_index, new_index, delta);

        // Scroll to keep selection visible - GPUI handles this automatically!
        self.list_scroll_handle.scroll_to_item(self.selected_index, ScrollStrategy::Top);

        // Load preview for newly selected item
        self.load_preview_for_selected_item(cx);

        cx.notify();
    }

    fn confirm_selection(&mut self, cx: &mut Context<Self>) {
        println!("🎯 confirm_selection called with selected_index: {}", self.selected_index);
        println!("🎯 filtered_indices length: {}", self.filtered_indices.len());
        
        if let Some(idx) = self.filtered_indices.get(self.selected_index) {
            println!("🎯 Found filtered index: {}", idx);
            if let Some(item) = self.items.get(*idx as usize) {
                println!("🎯 Found item: {}", item.label);
                if let Some(on_select) = &mut self.on_select {
                    println!("🎯 Calling on_select callback");
                    on_select(item, cx);
                } else {
                    println!("🚫 No on_select callback set");
                }
            } else {
                println!("🚫 Item not found at index {}", idx);
            }
        } else {
            println!("🚫 No filtered index found for selected_index {}", self.selected_index);
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(on_cancel) = &mut self.on_cancel {
            on_cancel(cx);
        }
    }
    
    fn calculate_dimensions(&self, window_size: Size<Pixels>) -> CachedDimensions {
        let min_width_for_preview = 800.0;
        let window_width = window_size.width.0 as f64;
        let _window_height = window_size.height.0 as f64;
        
        let show_preview = self.show_preview && window_width > min_width_for_preview;
        
        // Calculate fixed dimensions to prevent size changes
        let total_width = px(800.0); // Fixed width
        let max_height = px(500.0);   // Fixed height
        
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
        if let Some(path_buf) = item.data.downcast_ref::<std::path::PathBuf>() {
            self.load_file_preview(path_buf.clone(), cx);
        } else {
            self.preview_content = Some(format!("Preview not available for: {}", item.label));
            cx.notify();
        }
    }

    fn load_file_preview(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.preview_loading {
            return;
        }

        self.preview_loading = true;
        self.preview_content = Some("Loading...".to_string());
        cx.notify();

        // When spawning from Context<T>, the closure gets WeakEntity<T> as first param
        cx.spawn(async move |view_weak, cx| {
            let content = if path.is_dir() {
                // Load directory listing
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut content = format!("Directory: {}\n\n", path.display());
                        let mut items: Vec<_> = entries.collect::<Result<Vec<_>, _>>()
                            .unwrap_or_default();
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
                            content.push_str(&format!("\n... and {} more items\n", items.len() - 100));
                        }
                        content
                    }
                    Err(e) => format!("Error reading directory: {}", e),
                }
            } else {
                // Load file content
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        if content.len() > 10000 {
                            format!("{}\n\n[File truncated - showing first 10KB of {}KB total]", 
                                &content[..10000], content.len() / 1024)
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
                            Err(e) => format!("Error reading file: {}", e),
                        }
                    }
                }
            };

            _ = view_weak.update(cx, |this, cx| {
                this.preview_loading = false;
                this.preview_content = Some(content);
                cx.notify();
            });
        })
        .detach();
    }

}

impl Focusable for PickerView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PickerView {}

impl Render for PickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure the picker has focus when rendered
        if !self.focus_handle.is_focused(window) {
            println!("🔍 Picker not focused, requesting focus");
            self.focus_handle.focus(window);
        }
        
        let font = cx.global::<crate::FontSettings>().fixed_font.clone();
        let window_size = window.viewport_size();
        
        // Check if we need to recalculate dimensions
        let dimensions = if let Some(cached) = self.cached_dimensions {
            // Only recalculate if window size changed
            if cached.window_size != window_size {
                println!("📐 Window size changed, recalculating picker dimensions");
                self.calculate_dimensions(window_size)
            } else {
                cached
            }
        } else {
            println!("📐 Initial picker dimension calculation");
            self.calculate_dimensions(window_size)
        };
        
        // Update cache
        self.cached_dimensions = Some(dimensions);
        
        let total_width = dimensions.total_width;
        let max_height = dimensions.max_height;
        let list_width = dimensions.list_width;
        let preview_width = dimensions.preview_width;
        let show_preview = dimensions.show_preview;

        div().flex().flex_col()
            .key_context("Picker")  // Set proper key context for picker
            .absolute()  // Use absolute positioning
            .w(total_width)
            .h(max_height)  // Use fixed height instead of max_h to prevent size changes
            .bg(self.style.background)
            .border_1()
            .border_color(self.style.border)
            .rounded_md()
            .shadow_lg()
            .font(font)
            .text_size(px(14.))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            // Use GPUI actions instead of direct key handling
            .on_action(cx.listener(|this, _: &crate::actions::picker::SelectPrev, _window, cx| {
                println!("⬆️ SelectPrev action triggered");
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::picker::SelectNext, _window, cx| {
                println!("⬇️ SelectNext action triggered");
                this.move_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::picker::SelectFirst, _window, cx| {
                println!("⏫ SelectFirst action triggered");
                this.move_selection(-(this.selected_index as isize), cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::picker::SelectLast, _window, cx| {
                println!("⏬ SelectLast action triggered");
                let last_index = this.filtered_indices.len().saturating_sub(1);
                let delta = last_index as isize - this.selected_index as isize;
                this.move_selection(delta, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::picker::ConfirmSelection, _window, cx| {
                println!("✅ ConfirmSelection action triggered");
                this.confirm_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::picker::DismissPicker, _window, cx| {
                println!("❌ DismissPicker action triggered");
                this.cancel(cx);
            }))
            .child(
                // Search input (full width)
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .h_10()  // Fixed height for search input
                    .border_b_1()
                    .border_color(self.style.border)
                    .child(
                        div()
                            .flex_1()
                            .child(format!("🔍 {}", self.query))
                            .text_color(self.style.prompt_text)
                    )
            )
            .child(
                // Main content area - horizontal split
                div().flex()
                    .h_full()  // Use full height of remaining space
                    .overflow_hidden()
                    .child(
                        // File list using proper GPUI uniform_list
                        div().flex().flex_col()
                            .w(list_width)
                            .h_full()  // Use fixed height instead of flex_1
                            .overflow_hidden()  // Ensure overflow is hidden
                            .when(show_preview, |this| this.border_r_1().border_color(self.style.border))
                            .when(self.filtered_indices.is_empty(), |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .h_24()
                                        .text_color(self.style.prompt_text)
                                        .child("No matches found")
                                )
                            })
                            .when(!self.filtered_indices.is_empty(), |this| {
                                this.child(
                                    uniform_list(
                                        "picker-items",
                                        self.filtered_indices.len(),
                                        cx.processor(move |picker, visible_range: Range<usize>, _window, _cx| {
                                            visible_range
                                                .map(|visible_idx| {
                                                    let item_idx = picker.filtered_indices[visible_idx] as usize;
                                                    let item = &picker.items[item_idx];
                                                    let is_selected = visible_idx == picker.selected_index;

                                                    div()
                                                        .id(("picker-item", visible_idx))
                                                        .flex()
                                                        .flex_col()
                                                        .px_3()
                                                        .min_h_8()  // Ensure minimum height for items
                                                        .justify_center()
                                                        .cursor_pointer()
                                                        .when(is_selected, |this| {
                                                            this.bg(picker.style.selected_background)
                                                                .text_color(picker.style.selected_text)
                                                        })
                                                        .when(!is_selected, |this| this.text_color(picker.style.text))
                                                        .child(
                                                            div()
                                                                .overflow_hidden()
                                                                .text_ellipsis()
                                                                .child(item.label.clone())
                                                        )
                                                        .when_some(item.sublabel.as_ref(), |this, sublabel| {
                                                            this.child(
                                                                div()
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .text_size(px(12.))
                                                                    .text_color(picker.style.prompt_text)
                                                                    .child(sublabel.clone())
                                                            )
                                                        })
                                                })
                                                .collect()
                                        })
                                    )
                                    .h_full()  // Use fixed height instead of flex_1
                                    .track_scroll(self.list_scroll_handle.clone())
                                )
                            })
                    )
                    .when(show_preview, |this| {
                        this.child(
                            // Preview panel (right side)
                            div().flex().flex_col()
                                .w(preview_width)
                                .h_full()  // Use full height instead of flex_1
                                .overflow_hidden()  // Ensure overflow is hidden
                                .bg(hsla(0.0, 0.0, 0.05, 1.0))
                                .child(
                                    // Preview header
                                    div()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(self.style.border)
                                        .text_size(px(12.))
                                        .text_color(self.style.prompt_text)
                                        .child("Preview")
                                )
                                .child(
                                    // Preview content
                                    div()
                                        .h_full()  // Use full height instead of flex_1
                                        .overflow_y_hidden()  // Hide overflow for preview content
                                        .px_3()
                                        .py_2()
                                        .text_size(px(12.))
                                        .text_color(self.style.text)
                                        .font_family("monospace")
                                        .child(
                                            match &self.preview_content {
                                                Some(content) => content.clone(),
                                                None => "Select a file to preview".to_string()
                                            }
                                        )
                                )
                        )
                    })
            )
    }
}
