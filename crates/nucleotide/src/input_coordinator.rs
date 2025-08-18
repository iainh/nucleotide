// ABOUTME: Central hub for keyboard input routing and focus management in Nucleotide
// ABOUTME: Replaces fragmented input handling with unified GPUI-native action system

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{Context, Entity, FocusHandle, KeyDownEvent, Window, actions};
use nucleotide_logging::{debug, error, info, instrument, warn};

// Import for Helix integration
use crate::{Core, Input, InputEvent, utils};
use helix_view::input::KeyEvent;

/// Result of input coordinator processing
#[derive(Debug)]
pub enum InputResult {
    /// Key was not handled, continue normal processing
    NotHandled,
    /// Key was handled, no further processing needed
    Handled,
    /// Key should be sent to Helix editor
    SendToHelix(KeyEvent),
    /// A workspace action should be executed
    WorkspaceAction(String),
}

/// Central coordinator for all keyboard input routing and focus management
#[derive(Clone)]
pub struct InputCoordinator {
    /// Current active input context
    active_context: Arc<RwLock<InputContext>>,
    /// Context priority stack for modal behavior
    context_stack: Arc<RwLock<Vec<InputContext>>>,
    /// Action handler registry by context
    action_handlers: Arc<RwLock<HashMap<InputContext, ContextActionHandlers>>>,
    /// Focus group management
    focus_groups: Arc<RwLock<FocusGroupManager>>,
    /// Current active focus group
    active_focus_group: Arc<RwLock<Option<FocusGroup>>>,
}

/// Input contexts that determine which shortcuts are active
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputContext {
    /// Normal editing mode
    Normal,
    /// Completion popup is active
    Completion,
    /// File tree is focused
    FileTree,
    /// Picker/fuzzy finder is open
    Picker,
    /// Modal dialog or overlay is active
    Modal,
    /// Command prompt is active
    Prompt,
}

/// Context priority levels for resolving conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextPriority {
    Background = 0,
    Normal = 1,
    Component = 2,
    Modal = 3,
    Critical = 4,
}

/// Focus groups for Tab navigation between major UI areas
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusGroup {
    /// Text editor area
    Editor,
    /// File tree sidebar
    FileTree,
    /// Status bar and bottom panel
    StatusBar,
    /// Overlays and modal dialogs
    Overlays,
}

/// Action handlers for a specific context
#[derive(Default)]
pub struct ContextActionHandlers {
    /// Global action handlers that work in this context
    global_handlers: HashMap<String, Arc<dyn Fn() + Send + Sync>>,
    /// Context-specific action handlers
    context_handlers: HashMap<String, Arc<dyn Fn() + Send + Sync>>,
}

/// Focus group manager for Tab navigation
#[derive(Default)]
pub struct FocusGroupManager {
    /// Available focus groups and their states
    groups: HashMap<FocusGroup, FocusGroupState>,
    /// Current navigation order
    navigation_order: Vec<FocusGroup>,
    /// Currently active group
    active_group: Option<FocusGroup>,
}

/// State of a focus group
pub struct FocusGroupState {
    /// Whether this group is available for navigation
    available: bool,
    /// Focus handle for this group (if any)
    focus_handle: Option<FocusHandle>,
    /// Callback to activate this group
    activate_callback: Option<Box<dyn Fn() + Send + Sync>>,
}

// Define global actions for the input coordinator
actions!(
    input_coordinator,
    [
        // Global navigation
        ToggleFileTree,
        ShowFileFinder,
        ShowCommandPalette,
        ShowBufferPicker,
        // Focus group navigation
        NextFocusGroup,
        PrevFocusGroup,
        FocusEditor,
        FocusFileTree,
        FocusStatusBar,
        // Modal handling
        Escape,
        // Quick navigation
        QuickNav1,
        QuickNav2,
        QuickNav3,
        QuickNav0,
        // Editor actions
        SaveFile,
        CloseFile,
        NewFile,
        Quit,
    ]
);

impl InputCoordinator {
    /// Create a new input coordinator
    pub fn new() -> Self {
        debug!("Creating new InputCoordinator");

        let coordinator = Self {
            active_context: Arc::new(RwLock::new(InputContext::Normal)),
            context_stack: Arc::new(RwLock::new(Vec::new())),
            action_handlers: Arc::new(RwLock::new(HashMap::new())),
            focus_groups: Arc::new(RwLock::new(FocusGroupManager::new())),
            active_focus_group: Arc::new(RwLock::new(None)),
        };

        coordinator.setup_default_handlers();
        coordinator
    }

    /// Set up default action handlers for common shortcuts
    fn setup_default_handlers(&self) {
        debug!("Setting up default action handlers");

        // Initialize context handlers for all contexts
        let mut handlers = self.action_handlers.write().unwrap();
        for context in [
            InputContext::Normal,
            InputContext::Completion,
            InputContext::FileTree,
            InputContext::Picker,
            InputContext::Modal,
            InputContext::Prompt,
        ] {
            handlers.insert(context, ContextActionHandlers::default());
        }
    }

    /// Switch to a new input context
    #[instrument(skip(self))]
    pub fn switch_context(&self, new_context: InputContext) {
        let mut active = self.active_context.write().unwrap();
        let old_context = *active;
        *active = new_context;

        info!(
            old_context = ?old_context,
            new_context = ?new_context,
            "Switched input context"
        );
    }

    /// Push a new context onto the context stack (for modal behavior)
    #[instrument(skip(self))]
    pub fn push_context(&self, context: InputContext) {
        let mut stack = self.context_stack.write().unwrap();
        stack.push(*self.active_context.read().unwrap());
        drop(stack);

        self.switch_context(context);

        debug!(context = ?context, "Pushed context onto stack");
    }

    /// Pop the most recent context from the stack
    #[instrument(skip(self))]
    pub fn pop_context(&self) -> Option<InputContext> {
        let mut stack = self.context_stack.write().unwrap();
        if let Some(previous_context) = stack.pop() {
            drop(stack);
            self.switch_context(previous_context);
            debug!(context = ?previous_context, "Popped context from stack");
            Some(previous_context)
        } else {
            debug!("No context to pop, switching to Normal");
            self.switch_context(InputContext::Normal);
            None
        }
    }

    /// Get the current active context
    pub fn current_context(&self) -> InputContext {
        *self.active_context.read().unwrap()
    }

    /// Register an action handler for a specific context
    pub fn register_action(
        &self,
        context: InputContext,
        action_name: impl Into<String>,
        handler: impl Fn() + Send + Sync + 'static,
    ) {
        let action_name = action_name.into();
        let mut handlers = self.action_handlers.write().unwrap();

        if let Some(context_handlers) = handlers.get_mut(&context) {
            context_handlers
                .global_handlers
                .insert(action_name.clone(), Arc::new(handler));

            debug!(
                action = %action_name,
                context = ?context,
                "Registered action handler"
            );
        }
    }

    /// Register a global action handler that works in any context
    pub fn register_global_action(
        &self,
        action_name: impl Into<String>,
        handler: impl Fn() + Send + Sync + 'static,
    ) {
        let action_name = action_name.into();
        let handler = Arc::new(handler);

        // Register for all contexts
        for context in [
            InputContext::Normal,
            InputContext::Completion,
            InputContext::FileTree,
            InputContext::Picker,
            InputContext::Modal,
            InputContext::Prompt,
        ] {
            let mut handlers = self.action_handlers.write().unwrap();
            if let Some(context_handlers) = handlers.get_mut(&context) {
                context_handlers
                    .global_handlers
                    .insert(action_name.clone(), handler.clone());
            }
        }

        debug!(action = %action_name, "Registered global action handler");
    }

    /// Handle a key down event - main entry point for input processing
    #[instrument(skip(self, event, window))]
    pub fn handle_key_event(&self, event: &KeyDownEvent, window: &Window) -> InputResult {
        let current_context = self.current_context();

        debug!(
            key = %event.keystroke,
            context = ?current_context,
            "Processing key event"
        );

        // Handle special cases first
        if let Some(result) = self.handle_escape(event) {
            return result;
        }

        if let Some(result) = self.handle_focus_group_navigation(event) {
            return result;
        }

        // Delegate to context-specific handling
        match current_context {
            InputContext::Normal => self.handle_normal_context(event, window),
            InputContext::Completion => self.handle_completion_context(event, window),
            InputContext::FileTree => self.handle_file_tree_context(event, window),
            InputContext::Picker => self.handle_picker_context(event, window),
            InputContext::Modal => self.handle_modal_context(event, window),
            InputContext::Prompt => self.handle_prompt_context(event, window),
        }
    }

    /// Handle Escape key behavior
    fn handle_escape(&self, event: &KeyDownEvent) -> Option<InputResult> {
        if event.keystroke.key.as_str() == "escape" {
            let current_context = self.current_context();

            // Only consume ESC if we're not in Normal context
            // In Normal context, ESC should go to Helix for mode switching
            match current_context {
                InputContext::Normal => {
                    debug!("ESC in Normal context - passing to Helix for mode switching");
                    return None; // Let it pass through to Helix
                }
                _ => {
                    debug!("ESC in {:?} context - popping context", current_context);
                    self.pop_context();
                    return Some(InputResult::Handled);
                }
            }
        }
        None
    }

    /// Handle focus group navigation (Tab/Shift+Tab)
    fn handle_focus_group_navigation(&self, event: &KeyDownEvent) -> Option<InputResult> {
        if event.keystroke.key.as_str() == "tab" {
            if event.keystroke.modifiers.shift {
                debug!("Shift+Tab pressed, navigating to previous focus group");
                self.navigate_focus_group(false);
            } else {
                debug!("Tab pressed, navigating to next focus group");
                self.navigate_focus_group(true);
            }
            return Some(InputResult::Handled);
        }
        None
    }

    // Context-specific handlers
    fn handle_normal_context(&self, event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling normal context input - sending to Helix editor");

        // Translate GPUI key event to Helix key event
        let helix_key = utils::translate_key(&event.keystroke);

        debug!(
            key = ?helix_key,
            "Sending key to Helix editor"
        );

        InputResult::SendToHelix(helix_key)
    }

    fn handle_completion_context(&self, event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling completion context input");

        // For completion context, we might want to handle some keys specially
        // but for now, also send to Helix
        let helix_key = utils::translate_key(&event.keystroke);
        InputResult::SendToHelix(helix_key)
    }

    fn handle_file_tree_context(&self, _event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling file tree context input");

        // File tree should handle its own input, so don't send to Helix
        InputResult::NotHandled
    }

    fn handle_picker_context(&self, _event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling picker context input");

        // Picker should handle its own input, so don't send to Helix
        InputResult::NotHandled
    }

    fn handle_modal_context(&self, _event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling modal context input");

        // Modal should handle its own input, so don't send to Helix
        InputResult::NotHandled
    }

    fn handle_prompt_context(&self, _event: &KeyDownEvent, _window: &Window) -> InputResult {
        debug!("Handling prompt context input");

        // Prompt should handle its own input, so don't send to Helix
        InputResult::NotHandled
    }

    /// Navigate between focus groups
    fn navigate_focus_group(&self, forward: bool) {
        let mut focus_manager = self.focus_groups.write().unwrap();
        focus_manager.navigate(forward);

        if let Some(new_group) = focus_manager.active_group {
            *self.active_focus_group.write().unwrap() = Some(new_group);
            debug!(group = ?new_group, forward = forward, "Navigated to focus group");
        }
    }

    /// Register a focus group with the coordinator
    pub fn register_focus_group(
        &self,
        group: FocusGroup,
        focus_handle: Option<FocusHandle>,
        activate_callback: Option<Box<dyn Fn() + Send + Sync>>,
    ) {
        let mut focus_manager = self.focus_groups.write().unwrap();
        focus_manager.register_group(group, focus_handle, activate_callback);
        debug!(group = ?group, "Registered focus group");
    }

    /// Set focus group availability
    pub fn set_focus_group_available(&self, group: FocusGroup, available: bool) {
        let mut focus_manager = self.focus_groups.write().unwrap();
        focus_manager.set_group_available(group, available);
        debug!(group = ?group, available = available, "Updated focus group availability");
    }

    /// Get the currently active focus group
    pub fn active_focus_group(&self) -> Option<FocusGroup> {
        *self.active_focus_group.read().unwrap()
    }
}

impl FocusGroupManager {
    /// Create a new focus group manager
    pub fn new() -> Self {
        let mut manager = Self {
            groups: HashMap::new(),
            navigation_order: vec![
                FocusGroup::Editor,
                FocusGroup::FileTree,
                FocusGroup::StatusBar,
                FocusGroup::Overlays,
            ],
            active_group: None,
        };

        // Initialize all groups as unavailable
        for &group in &manager.navigation_order {
            manager.groups.insert(
                group,
                FocusGroupState {
                    available: false,
                    focus_handle: None,
                    activate_callback: None,
                },
            );
        }

        manager
    }

    /// Register a focus group
    pub fn register_group(
        &mut self,
        group: FocusGroup,
        focus_handle: Option<FocusHandle>,
        activate_callback: Option<Box<dyn Fn() + Send + Sync>>,
    ) {
        if let Some(state) = self.groups.get_mut(&group) {
            state.focus_handle = focus_handle;
            state.activate_callback = activate_callback;
            state.available = true;
        }
    }

    /// Set focus group availability
    pub fn set_group_available(&mut self, group: FocusGroup, available: bool) {
        if let Some(state) = self.groups.get_mut(&group) {
            state.available = available;
        }
    }

    /// Navigate to the next/previous focus group
    pub fn navigate(&mut self, forward: bool) {
        let available_groups: Vec<_> = self
            .navigation_order
            .iter()
            .filter(|&&group| {
                self.groups
                    .get(&group)
                    .map(|state| state.available)
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        if available_groups.is_empty() {
            return;
        }

        let new_group = if let Some(current) = self.active_group {
            // Find current position and move to next/prev
            if let Some(current_index) = available_groups.iter().position(|&g| g == current) {
                let new_index = if forward {
                    (current_index + 1) % available_groups.len()
                } else {
                    (current_index + available_groups.len() - 1) % available_groups.len()
                };
                available_groups[new_index]
            } else {
                available_groups[0]
            }
        } else {
            available_groups[0]
        };

        self.active_group = Some(new_group);

        // Activate the new group
        if let Some(state) = self.groups.get(&new_group) {
            if let Some(ref callback) = state.activate_callback {
                callback();
            }
        }
    }
}

impl Default for InputCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_coordinator_creation() {
        let coordinator = InputCoordinator::new();
        assert_eq!(coordinator.current_context(), InputContext::Normal);
    }

    #[test]
    fn test_context_switching() {
        let coordinator = InputCoordinator::new();

        // Initial context should be Normal
        assert_eq!(coordinator.current_context(), InputContext::Normal);

        // Switch to FileTree
        coordinator.switch_context(InputContext::FileTree);
        assert_eq!(coordinator.current_context(), InputContext::FileTree);

        // Switch to Modal
        coordinator.switch_context(InputContext::Modal);
        assert_eq!(coordinator.current_context(), InputContext::Modal);
    }

    #[test]
    fn test_context_stack() {
        let coordinator = InputCoordinator::new();

        // Start in Normal
        assert_eq!(coordinator.current_context(), InputContext::Normal);

        // Push FileTree context
        coordinator.push_context(InputContext::FileTree);
        assert_eq!(coordinator.current_context(), InputContext::FileTree);

        // Push Modal context
        coordinator.push_context(InputContext::Modal);
        assert_eq!(coordinator.current_context(), InputContext::Modal);

        // Pop back to FileTree
        let popped = coordinator.pop_context();
        assert_eq!(popped, Some(InputContext::FileTree));
        assert_eq!(coordinator.current_context(), InputContext::FileTree);

        // Pop back to Normal
        let popped = coordinator.pop_context();
        assert_eq!(popped, Some(InputContext::Normal));
        assert_eq!(coordinator.current_context(), InputContext::Normal);

        // Pop from empty stack should return None and stay Normal
        let popped = coordinator.pop_context();
        assert_eq!(popped, None);
        assert_eq!(coordinator.current_context(), InputContext::Normal);
    }

    #[test]
    fn test_action_registration() {
        let coordinator = InputCoordinator::new();

        // This should not panic
        coordinator.register_action(InputContext::Normal, "test_action", || { /* test handler */
        });

        coordinator.register_global_action("global_test_action", || { /* global test handler */ });
    }

    #[test]
    fn test_focus_group_management() {
        let coordinator = InputCoordinator::new();

        // Register focus groups
        coordinator.register_focus_group(FocusGroup::Editor, None, None);
        coordinator.register_focus_group(FocusGroup::FileTree, None, None);

        // Set availability
        coordinator.set_focus_group_available(FocusGroup::Editor, true);
        coordinator.set_focus_group_available(FocusGroup::FileTree, true);

        // Initially no focus group should be active
        assert_eq!(coordinator.active_focus_group(), None);
    }
}
