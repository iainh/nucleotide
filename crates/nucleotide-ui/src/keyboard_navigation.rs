// ABOUTME: Keyboard navigation utilities for list components
// ABOUTME: Provides arrow key navigation, home/end shortcuts, and search functionality

use crate::list_item::{SelectionMode, SelectionState};
use gpui::KeyDownEvent;

/// Navigation direction for keyboard events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Keyboard navigation action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationAction {
    Move(NavigationDirection),
    Select(NavigationDirection),
    ToggleSelection,
    SelectAll,
    ClearSelection,
    Search(String),
}

/// Keyboard navigation handler for lists
pub struct KeyboardNavigationHandler {
    /// Current focused item index
    pub focused_index: Option<usize>,
    /// Total number of items in the list
    pub item_count: usize,
    /// Selection mode
    pub selection_mode: SelectionMode,
    /// Selected items (for multiple selection)
    pub selected_indices: std::collections::HashSet<usize>,
    /// Search buffer for incremental search
    search_buffer: String,
    /// Last search time for buffer reset
    last_search_time: std::time::Instant,
}

impl KeyboardNavigationHandler {
    /// Create a new keyboard navigation handler
    pub fn new(item_count: usize, selection_mode: SelectionMode) -> Self {
        Self {
            focused_index: None,
            item_count,
            selection_mode,
            selected_indices: std::collections::HashSet::new(),
            search_buffer: String::new(),
            last_search_time: std::time::Instant::now(),
        }
    }

    /// Update the item count (when list changes)
    pub fn set_item_count(&mut self, count: usize) {
        self.item_count = count;

        // Adjust focused index if it's out of bounds
        if let Some(focused) = self.focused_index {
            if focused >= count {
                self.focused_index = if count > 0 { Some(count - 1) } else { None };
            }
        }

        // Remove selected indices that are out of bounds
        self.selected_indices.retain(|&index| index < count);
    }

    /// Handle keyboard input and return the appropriate action
    pub fn handle_key_down(&mut self, event: &KeyDownEvent) -> Option<NavigationAction> {
        let modifiers = &event.keystroke.modifiers;

        match event.keystroke.key.as_str() {
            // Arrow navigation
            "up" | "ArrowUp" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::Up))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::Up))
                }
            }
            "down" | "ArrowDown" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::Down))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::Down))
                }
            }

            // Home/End navigation
            "home" | "Home" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::Home))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::Home))
                }
            }
            "end" | "End" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::End))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::End))
                }
            }

            // Page navigation
            "pageup" | "PageUp" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::PageUp))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::PageUp))
                }
            }
            "pagedown" | "PageDown" => {
                if modifiers.shift && self.selection_mode == SelectionMode::Range {
                    Some(NavigationAction::Select(NavigationDirection::PageDown))
                } else {
                    Some(NavigationAction::Move(NavigationDirection::PageDown))
                }
            }

            // Selection commands
            " " | "space" => {
                if self.selection_mode != SelectionMode::None {
                    Some(NavigationAction::ToggleSelection)
                } else {
                    None
                }
            }

            // Select all (Ctrl+A / Cmd+A)
            "a" if (modifiers.control || modifiers.platform)
                && self.selection_mode == SelectionMode::Multiple =>
            {
                Some(NavigationAction::SelectAll)
            }

            // Clear selection (Escape)
            "escape" | "Escape" => {
                if !self.selected_indices.is_empty() || self.focused_index.is_some() {
                    Some(NavigationAction::ClearSelection)
                } else {
                    None
                }
            }

            // Character input for search
            _ => {
                if let Some(char) = event.keystroke.key.to_string().chars().next() {
                    if char.is_alphanumeric() || char.is_whitespace() {
                        self.handle_search_input(char)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Handle search character input
    fn handle_search_input(&mut self, char: char) -> Option<NavigationAction> {
        let now = std::time::Instant::now();

        // Reset search buffer if too much time has passed
        if now.duration_since(self.last_search_time).as_millis() > 500 {
            self.search_buffer.clear();
        }

        self.search_buffer.push(char);
        self.last_search_time = now;

        Some(NavigationAction::Search(self.search_buffer.clone()))
    }

    /// Apply a navigation action and return the new focused/selected indices
    pub fn apply_action(&mut self, action: NavigationAction) -> NavigationResult {
        match action {
            NavigationAction::Move(direction) => {
                let new_focus = self.calculate_new_focus(direction);
                let old_focus = self.focused_index;
                self.focused_index = new_focus;

                NavigationResult {
                    old_focused_index: old_focus,
                    new_focused_index: new_focus,
                    selection_changed: false,
                    selected_indices: self.selected_indices.clone(),
                }
            }

            NavigationAction::Select(direction) => {
                let old_focus = self.focused_index;
                let new_focus = self.calculate_new_focus(direction);

                // Handle range selection
                if self.selection_mode == SelectionMode::Range {
                    self.handle_range_selection(old_focus, new_focus);
                }

                self.focused_index = new_focus;

                NavigationResult {
                    old_focused_index: old_focus,
                    new_focused_index: new_focus,
                    selection_changed: true,
                    selected_indices: self.selected_indices.clone(),
                }
            }

            NavigationAction::ToggleSelection => {
                if let Some(focused) = self.focused_index {
                    if self.selected_indices.contains(&focused) {
                        self.selected_indices.remove(&focused);
                    } else {
                        self.selected_indices.insert(focused);
                    }

                    NavigationResult {
                        old_focused_index: self.focused_index,
                        new_focused_index: self.focused_index,
                        selection_changed: true,
                        selected_indices: self.selected_indices.clone(),
                    }
                } else {
                    NavigationResult::no_change()
                }
            }

            NavigationAction::SelectAll => {
                self.selected_indices = (0..self.item_count).collect();

                NavigationResult {
                    old_focused_index: self.focused_index,
                    new_focused_index: self.focused_index,
                    selection_changed: true,
                    selected_indices: self.selected_indices.clone(),
                }
            }

            NavigationAction::ClearSelection => {
                let had_selection = !self.selected_indices.is_empty();
                let had_focus = self.focused_index.is_some();

                self.selected_indices.clear();
                self.focused_index = None;

                NavigationResult {
                    old_focused_index: if had_focus { self.focused_index } else { None },
                    new_focused_index: None,
                    selection_changed: had_selection,
                    selected_indices: std::collections::HashSet::new(),
                }
            }

            NavigationAction::Search(_query) => {
                // Search implementation would be handled by the list component
                // This just returns the current state
                NavigationResult::no_change()
            }
        }
    }

    /// Calculate new focus index based on direction
    fn calculate_new_focus(&self, direction: NavigationDirection) -> Option<usize> {
        if self.item_count == 0 {
            return None;
        }

        let current = self.focused_index.unwrap_or(0);

        match direction {
            NavigationDirection::Up => {
                if current > 0 {
                    Some(current - 1)
                } else {
                    Some(current) // Stay at top
                }
            }
            NavigationDirection::Down => {
                if current < self.item_count - 1 {
                    Some(current + 1)
                } else {
                    Some(current) // Stay at bottom
                }
            }
            NavigationDirection::Home => Some(0),
            NavigationDirection::End => Some(self.item_count - 1),
            NavigationDirection::PageUp => {
                let page_size = 10; // Could be configurable
                Some(current.saturating_sub(page_size))
            }
            NavigationDirection::PageDown => {
                let page_size = 10; // Could be configurable
                Some((current + page_size).min(self.item_count - 1))
            }
        }
    }

    /// Handle range selection between two indices
    fn handle_range_selection(&mut self, start: Option<usize>, end: Option<usize>) {
        if let (Some(start), Some(end)) = (start, end) {
            let min = start.min(end);
            let max = start.max(end);

            // Add all indices in the range to selection
            for i in min..=max {
                if i < self.item_count {
                    self.selected_indices.insert(i);
                }
            }
        }
    }

    /// Get the current selection state for a specific index
    pub fn get_selection_state(&self, index: usize) -> SelectionState {
        SelectionState {
            mode: self.selection_mode,
            selected: self.selected_indices.contains(&index),
            focused: self.focused_index == Some(index),
            selection_index: Some(index),
            group_id: None,
        }
    }
}

/// Result of applying a navigation action
#[derive(Debug, Clone)]
pub struct NavigationResult {
    pub old_focused_index: Option<usize>,
    pub new_focused_index: Option<usize>,
    pub selection_changed: bool,
    pub selected_indices: std::collections::HashSet<usize>,
}

impl NavigationResult {
    fn no_change() -> Self {
        Self {
            old_focused_index: None,
            new_focused_index: None,
            selection_changed: false,
            selected_indices: std::collections::HashSet::new(),
        }
    }
}

/// List virtualization helper for large datasets
pub struct ListVirtualization {
    /// Total number of items in the list
    pub total_items: usize,
    /// Height of each item in pixels
    pub item_height: f32,
    /// Height of the visible area
    pub viewport_height: f32,
    /// Current scroll offset
    pub scroll_offset: f32,
}

impl ListVirtualization {
    /// Create a new virtualization helper
    pub fn new(total_items: usize, item_height: f32, viewport_height: f32) -> Self {
        Self {
            total_items,
            item_height,
            viewport_height,
            scroll_offset: 0.0,
        }
    }

    /// Calculate which items should be rendered
    pub fn visible_range(&self) -> (usize, usize) {
        if self.total_items == 0 {
            return (0, 0);
        }

        let start_index = (self.scroll_offset / self.item_height).floor() as usize;
        let visible_count = (self.viewport_height / self.item_height).ceil() as usize + 1; // +1 for partial items
        let end_index = (start_index + visible_count).min(self.total_items);

        (start_index, end_index)
    }

    /// Get the total height of all items
    pub fn total_height(&self) -> f32 {
        self.total_items as f32 * self.item_height
    }

    /// Update scroll position
    pub fn set_scroll_offset(&mut self, offset: f32) {
        self.scroll_offset = offset
            .max(0.0)
            .min(self.total_height() - self.viewport_height);
    }

    /// Scroll to make a specific item visible
    pub fn scroll_to_item(&mut self, item_index: usize) {
        if item_index >= self.total_items {
            return;
        }

        let item_top = item_index as f32 * self.item_height;
        let item_bottom = item_top + self.item_height;
        let viewport_bottom = self.scroll_offset + self.viewport_height;

        if item_top < self.scroll_offset {
            // Item is above viewport, scroll up
            self.scroll_offset = item_top;
        } else if item_bottom > viewport_bottom {
            // Item is below viewport, scroll down
            self.scroll_offset = item_bottom - self.viewport_height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyboard_navigation_basic() {
        let mut handler = KeyboardNavigationHandler::new(5, SelectionMode::Single);

        // Test initial state
        assert_eq!(handler.focused_index, None);

        // Test moving down
        handler.focused_index = Some(0);
        let new_focus = handler.calculate_new_focus(NavigationDirection::Down);
        assert_eq!(new_focus, Some(1));

        // Test moving up from top
        handler.focused_index = Some(0);
        let new_focus = handler.calculate_new_focus(NavigationDirection::Up);
        assert_eq!(new_focus, Some(0)); // Should stay at top

        // Test home/end
        let home_focus = handler.calculate_new_focus(NavigationDirection::Home);
        assert_eq!(home_focus, Some(0));

        let end_focus = handler.calculate_new_focus(NavigationDirection::End);
        assert_eq!(end_focus, Some(4));
    }

    #[test]
    fn test_selection_state() {
        let mut handler = KeyboardNavigationHandler::new(5, SelectionMode::Multiple);
        handler.focused_index = Some(2);
        handler.selected_indices.insert(1);
        handler.selected_indices.insert(2);

        let state_0 = handler.get_selection_state(0);
        assert!(!state_0.selected && !state_0.focused);

        let state_1 = handler.get_selection_state(1);
        assert!(state_1.selected && !state_1.focused);

        let state_2 = handler.get_selection_state(2);
        assert!(state_2.selected && state_2.focused);
    }

    #[test]
    fn test_virtualization() {
        let mut virt = ListVirtualization::new(100, 25.0, 300.0);

        // Test visible range at top
        let (start, end) = virt.visible_range();
        assert_eq!(start, 0);
        assert!(end <= 13); // Should be around 12-13 items visible

        // Test scrolling
        virt.set_scroll_offset(250.0); // Scroll down
        let (start, _end) = virt.visible_range();
        assert_eq!(start, 10); // 250 / 25 = 10

        // Test scroll to item
        virt.scroll_to_item(50);
        assert!(virt.scroll_offset > 0.0);
    }
}
