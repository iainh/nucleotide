// ABOUTME: Focus management utilities for nucleotide-ui components
// ABOUTME: Provides focus tracking, focus groups, and accessibility helpers

use gpui::{App, ElementId, SharedString};
use std::collections::HashMap;

/// Focus manager for handling component focus state
static mut FOCUS_MANAGER: Option<FocusManager> = None;
static INIT_FOCUS: std::sync::Once = std::sync::Once::new();

/// Initialize the focus management system
pub fn init_focus_management(_cx: &mut App) {
    INIT_FOCUS.call_once(|| unsafe {
        FOCUS_MANAGER = Some(FocusManager::new());
    });
}

/// Get access to the global focus manager
pub fn with_focus_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut FocusManager) -> R,
{
    unsafe { FOCUS_MANAGER.as_mut().map(f) }
}

/// Focus direction for navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Forward,
    Backward,
    Up,
    Down,
    Left,
    Right,
}

/// Focus group configuration
#[derive(Debug, Clone)]
pub struct FocusGroup {
    pub id: SharedString,
    pub elements: Vec<ElementId>,
    pub wrap_navigation: bool,
    pub orientation: FocusOrientation,
    pub active_index: Option<usize>,
}

/// Focus group orientation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOrientation {
    Horizontal,
    Vertical,
    Grid { columns: usize },
}

impl FocusGroup {
    /// Create a new focus group
    pub fn new(id: impl Into<SharedString>, orientation: FocusOrientation) -> Self {
        Self {
            id: id.into(),
            elements: Vec::new(),
            wrap_navigation: false,
            orientation,
            active_index: None,
        }
    }

    /// Add an element to the focus group
    pub fn add_element(&mut self, element_id: ElementId) {
        if !self.elements.contains(&element_id) {
            self.elements.push(element_id);
        }
    }

    /// Remove an element from the focus group
    pub fn remove_element(&mut self, element_id: &ElementId) {
        if let Some(pos) = self.elements.iter().position(|id| id == element_id) {
            self.elements.remove(pos);

            // Adjust active index if necessary
            if let Some(active) = self.active_index {
                if active >= pos && active > 0 {
                    self.active_index = Some(active - 1);
                } else if self.elements.is_empty() {
                    self.active_index = None;
                }
            }
        }
    }

    /// Set the active element
    pub fn set_active(&mut self, element_id: &ElementId) -> bool {
        if let Some(index) = self.elements.iter().position(|id| id == element_id) {
            self.active_index = Some(index);
            true
        } else {
            false
        }
    }

    /// Get the next element in the given direction
    pub fn next_element(&self, direction: FocusDirection) -> Option<&ElementId> {
        let current_index = self.active_index?;
        let next_index = self.calculate_next_index(current_index, direction)?;
        self.elements.get(next_index)
    }

    /// Calculate the next index based on direction and orientation
    fn calculate_next_index(&self, current: usize, direction: FocusDirection) -> Option<usize> {
        if self.elements.is_empty() {
            return None;
        }

        let len = self.elements.len();

        match (self.orientation, direction) {
            // Linear navigation (horizontal/vertical)
            (FocusOrientation::Horizontal, FocusDirection::Forward)
            | (FocusOrientation::Horizontal, FocusDirection::Right)
            | (FocusOrientation::Vertical, FocusDirection::Forward)
            | (FocusOrientation::Vertical, FocusDirection::Down) => {
                if current + 1 < len {
                    Some(current + 1)
                } else if self.wrap_navigation {
                    Some(0)
                } else {
                    None
                }
            }

            (FocusOrientation::Horizontal, FocusDirection::Backward)
            | (FocusOrientation::Horizontal, FocusDirection::Left)
            | (FocusOrientation::Vertical, FocusDirection::Backward)
            | (FocusOrientation::Vertical, FocusDirection::Up) => {
                if current > 0 {
                    Some(current - 1)
                } else if self.wrap_navigation {
                    Some(len - 1)
                } else {
                    None
                }
            }

            // Grid navigation
            (FocusOrientation::Grid { columns }, FocusDirection::Right) => {
                if (current + 1) % columns != 0 && current + 1 < len {
                    Some(current + 1)
                } else if self.wrap_navigation {
                    let row = current / columns;
                    Some(row * columns)
                } else {
                    None
                }
            }

            (FocusOrientation::Grid { columns }, FocusDirection::Left) => {
                if current % columns != 0 {
                    Some(current - 1)
                } else if self.wrap_navigation {
                    let row = current / columns;
                    let row_end = ((row + 1) * columns - 1).min(len - 1);
                    Some(row_end)
                } else {
                    None
                }
            }

            (FocusOrientation::Grid { columns }, FocusDirection::Down) => {
                let next_row_index = current + columns;
                if next_row_index < len {
                    Some(next_row_index)
                } else if self.wrap_navigation {
                    Some(current % columns)
                } else {
                    None
                }
            }

            (FocusOrientation::Grid { columns }, FocusDirection::Up) => {
                if current >= columns {
                    Some(current - columns)
                } else if self.wrap_navigation {
                    let col = current % columns;
                    let last_row_start = ((len - 1) / columns) * columns;
                    Some((last_row_start + col).min(len - 1))
                } else {
                    None
                }
            }

            _ => None,
        }
    }
}

/// Global focus manager
#[derive(Debug)]
pub struct FocusManager {
    current_focus: Option<ElementId>,
    focus_groups: HashMap<SharedString, FocusGroup>,
    element_to_group: HashMap<ElementId, SharedString>,
    focus_history: Vec<ElementId>,
    trap_focus: Option<SharedString>,
}

impl FocusManager {
    /// Create a new focus manager
    pub fn new() -> Self {
        Self {
            current_focus: None,
            focus_groups: HashMap::new(),
            element_to_group: HashMap::new(),
            focus_history: Vec::new(),
            trap_focus: None,
        }
    }

    /// Set the currently focused element
    pub fn set_focus(&mut self, element_id: ElementId) {
        // Add to history if different from current
        if self.current_focus != Some(element_id.clone()) {
            if let Some(current) = &self.current_focus {
                self.focus_history.push(current.clone());

                // Keep history limited
                if self.focus_history.len() > 10 {
                    self.focus_history.remove(0);
                }
            }
        }

        self.current_focus = Some(element_id.clone());

        // Update focus group active element
        if let Some(group_id) = self.element_to_group.get(&element_id) {
            if let Some(group) = self.focus_groups.get_mut(group_id) {
                group.set_active(&element_id);
            }
        }

        nucleotide_logging::debug!(
            element_id = ?element_id,
            "Focus set to element"
        );
    }

    /// Get the currently focused element
    pub fn current_focus(&self) -> Option<&ElementId> {
        self.current_focus.as_ref()
    }

    /// Clear focus
    pub fn clear_focus(&mut self) {
        if let Some(current) = self.current_focus.take() {
            self.focus_history.push(current);
        }

        nucleotide_logging::debug!("Focus cleared");
    }

    /// Return focus to the previous element
    pub fn focus_previous(&mut self) -> bool {
        if let Some(previous) = self.focus_history.pop() {
            self.current_focus = Some(previous);
            true
        } else {
            false
        }
    }

    /// Register a focus group
    pub fn register_group(&mut self, group: FocusGroup) {
        let group_id = group.id.clone();

        // Update element-to-group mapping
        for element_id in &group.elements {
            self.element_to_group
                .insert(element_id.clone(), group_id.clone());
        }

        self.focus_groups.insert(group_id, group);
    }

    /// Unregister a focus group
    pub fn unregister_group(&mut self, group_id: &str) {
        if let Some(group) = self.focus_groups.remove(group_id) {
            // Remove element mappings
            for element_id in &group.elements {
                self.element_to_group.remove(element_id);
            }
        }
    }

    /// Navigate focus in a direction
    pub fn navigate(&mut self, direction: FocusDirection) -> bool {
        let current_element = match self.current_focus.as_ref() {
            Some(element) => element,
            None => return false,
        };
        let group_id = match self.element_to_group.get(current_element) {
            Some(id) => id,
            None => return false,
        };
        let group = match self.focus_groups.get(group_id) {
            Some(group) => group,
            None => return false,
        };

        // Check focus trap
        if let Some(trap_group) = &self.trap_focus {
            if trap_group != group_id {
                return false; // Can't navigate outside trapped group
            }
        }

        if let Some(next_element) = group.next_element(direction) {
            self.set_focus(next_element.clone());
            true
        } else {
            false
        }
    }

    /// Trap focus within a specific group
    pub fn trap_focus(&mut self, group_id: impl Into<SharedString>) {
        self.trap_focus = Some(group_id.into());
    }

    /// Release focus trap
    pub fn release_trap(&mut self) {
        self.trap_focus = None;
    }

    /// Check if focus is trapped
    pub fn is_trapped(&self) -> bool {
        self.trap_focus.is_some()
    }

    /// Add element to existing group
    pub fn add_to_group(&mut self, group_id: &str, element_id: ElementId) {
        if let Some(group) = self.focus_groups.get_mut(group_id) {
            group.add_element(element_id.clone());
            self.element_to_group
                .insert(element_id, group_id.to_string().into());
        }
    }

    /// Remove element from its group
    pub fn remove_element(&mut self, element_id: &ElementId) {
        if let Some(group_id) = self.element_to_group.remove(element_id) {
            if let Some(group) = self.focus_groups.get_mut(&group_id) {
                group.remove_element(element_id);
            }
        }

        // Clear focus if this element was focused
        if self.current_focus.as_ref() == Some(element_id) {
            self.clear_focus();
        }

        // Remove from history
        self.focus_history.retain(|id| id != element_id);
    }

    /// Get focus group information
    pub fn get_group(&self, group_id: &str) -> Option<&FocusGroup> {
        self.focus_groups.get(group_id)
    }
}

/// Focus helper utilities
pub struct FocusHelpers;

impl FocusHelpers {
    /// Check if an element should be focusable
    pub fn is_focusable(element_id: &ElementId) -> bool {
        with_focus_manager(|manager| manager.element_to_group.contains_key(element_id))
            .unwrap_or(false)
    }

    /// Create a tab order for elements
    pub fn create_tab_order(elements: Vec<ElementId>) -> FocusGroup {
        let mut group = FocusGroup::new("tab_order", FocusOrientation::Horizontal);
        for element in elements {
            group.add_element(element);
        }
        group.wrap_navigation = true;
        group
    }

    /// Create a grid focus group
    pub fn create_grid(
        id: impl Into<SharedString>,
        elements: Vec<ElementId>,
        columns: usize,
    ) -> FocusGroup {
        let mut group = FocusGroup::new(id, FocusOrientation::Grid { columns });
        for element in elements {
            group.add_element(element);
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_group_linear_navigation() {
        let mut group = FocusGroup::new("test", FocusOrientation::Horizontal);
        let elements: Vec<ElementId> = (0..3)
            .map(|i| ElementId::from(SharedString::from(format!("element-{}", i))))
            .collect();

        for element in &elements {
            group.add_element(element.clone());
        }

        group.active_index = Some(0);

        // Test forward navigation
        let next = group.next_element(FocusDirection::Forward);
        assert_eq!(next, elements.get(1));

        // Test backward navigation
        group.active_index = Some(1);
        let prev = group.next_element(FocusDirection::Backward);
        assert_eq!(prev, elements.get(0));
    }

    #[test]
    fn test_focus_group_wrap_navigation() {
        let mut group = FocusGroup::new("test", FocusOrientation::Horizontal);
        group.wrap_navigation = true;

        let elements: Vec<ElementId> = (0..3)
            .map(|i| ElementId::from(SharedString::from(format!("element-{}", i))))
            .collect();
        for element in &elements {
            group.add_element(element.clone());
        }

        // Test wrap from last to first
        group.active_index = Some(2);
        let next = group.next_element(FocusDirection::Forward);
        assert_eq!(next, elements.get(0));

        // Test wrap from first to last
        group.active_index = Some(0);
        let prev = group.next_element(FocusDirection::Backward);
        assert_eq!(prev, elements.get(2));
    }

    #[test]
    fn test_focus_group_grid_navigation() {
        let mut group = FocusGroup::new("test", FocusOrientation::Grid { columns: 2 });
        let elements: Vec<ElementId> = (0..6)
            .map(|i| ElementId::from(SharedString::from(format!("element-{}", i))))
            .collect();

        for element in &elements {
            group.add_element(element.clone());
        }

        // Test right navigation
        group.active_index = Some(0);
        let right = group.next_element(FocusDirection::Right);
        assert_eq!(right, elements.get(1));

        // Test down navigation
        group.active_index = Some(0);
        let down = group.next_element(FocusDirection::Down);
        assert_eq!(down, elements.get(2));

        // Test boundary conditions
        group.active_index = Some(1);
        let right_boundary = group.next_element(FocusDirection::Right);
        assert_eq!(right_boundary, None); // Should not wrap by default
    }

    #[test]
    fn test_focus_manager() {
        let mut manager = FocusManager::new();
        let element1: ElementId = "element1".into();
        let element2: ElementId = "element2".into();

        // Test setting focus
        manager.set_focus(element1.clone());
        assert_eq!(manager.current_focus(), Some(&element1));

        // Test focus history
        manager.set_focus(element2.clone());
        assert_eq!(manager.current_focus(), Some(&element2));

        let returned = manager.focus_previous();
        assert!(returned);
        assert_eq!(manager.current_focus(), Some(&element1));
    }

    #[test]
    fn test_focus_helpers() {
        let elements: Vec<ElementId> = (0..4)
            .map(|i| ElementId::from(SharedString::from(format!("tab-{}", i))))
            .collect();
        let tab_group = FocusHelpers::create_tab_order(elements.clone());

        assert_eq!(tab_group.elements.len(), 4);
        assert_eq!(tab_group.orientation, FocusOrientation::Horizontal);
        assert!(tab_group.wrap_navigation);

        let grid_group = FocusHelpers::create_grid("grid", elements, 2);
        assert!(matches!(
            grid_group.orientation,
            FocusOrientation::Grid { columns: 2 }
        ));
    }
}
