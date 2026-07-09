// ABOUTME: Central hub for workspace keyboard context routing in Nucleotide
// ABOUTME: Keeps app/Helix key handoff separate from component-owned GPUI actions

use std::sync::{Arc, RwLock};

use gpui::{KeyDownEvent, Window};
use nucleotide_logging::{debug, info, instrument};

// Import for Helix integration
use crate::utils;
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
}

/// Central coordinator for workspace-level keyboard context routing.
#[derive(Clone)]
pub struct InputCoordinator {
    /// Current active input context
    active_context: Arc<RwLock<InputContext>>,
    /// Context priority stack for modal behavior
    context_stack: Arc<RwLock<Vec<InputContext>>>,
}

/// Input contexts that determine which shortcuts are active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl InputCoordinator {
    /// Create a new input coordinator
    pub fn new() -> Self {
        debug!("Creating new InputCoordinator");

        Self {
            active_context: Arc::new(RwLock::new(InputContext::Normal)),
            context_stack: Arc::new(RwLock::new(Vec::new())),
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
}
