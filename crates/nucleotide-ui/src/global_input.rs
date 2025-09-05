// ABOUTME: Global input system for centralized keyboard navigation and event routing
// ABOUTME: Manages focus groups, event prioritization, and cross-component communication

use crate::providers::Provider;
use crate::providers::event_provider::{
    EventHandlingProvider, EventPriority, EventResult, KeyboardEventListener,
};
use gpui::{App, FocusHandle, KeyDownEvent};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Global input dispatcher for centralized event management
#[derive(Clone)]
pub struct GlobalInputDispatcher {
    /// Core event provider
    pub event_provider: EventHandlingProvider,
    /// Focus group management
    pub focus_groups: Arc<RwLock<FocusGroupManager>>,
    /// Shortcut registry
    pub shortcuts: Arc<RwLock<ShortcutRegistry>>,
    /// Input context management
    pub contexts: Arc<RwLock<InputContextManager>>,
    /// Dismiss handlers for different targets
    #[allow(clippy::type_complexity)]
    pub dismiss_handlers: Arc<RwLock<HashMap<DismissTarget, Arc<dyn Fn() + Send + Sync>>>>,
    /// Action handlers for named actions
    #[allow(clippy::type_complexity)]
    pub action_handlers: Arc<RwLock<HashMap<String, Arc<dyn Fn() + Send + Sync>>>>,
    /// Focus indicator configuration
    pub focus_indicator_config: Arc<RwLock<FocusIndicatorConfig>>,
}

/// Focus group manager for organizing UI areas
#[derive(Default)]
pub struct FocusGroupManager {
    /// Registered focus groups by ID
    groups: HashMap<String, FocusGroup>,
    /// Current active focus group
    active_group: Option<String>,
    /// Focus group priorities
    priorities: HashMap<String, FocusPriority>,
}

/// Individual focus group
#[derive(Debug, Clone)]
pub struct FocusGroup {
    pub id: String,
    pub name: String,
    pub priority: FocusPriority,
    pub elements: Vec<FocusElement>,
    pub active_element: Option<usize>,
    pub enabled: bool,
}

/// Focus element within a group
#[derive(Debug, Clone)]
pub struct FocusElement {
    pub id: String,
    pub name: String,
    pub focus_handle: Option<FocusHandle>,
    pub tab_index: i32,
    pub enabled: bool,
    pub element_type: FocusElementType,
}

/// Types of focusable elements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusElementType {
    Editor,
    Completion,
    FileTree,
    Picker,
    Prompt,
    Button,
    ListItem,
    Custom,
}

/// Focus group priorities
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FocusPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Shortcut registry for global keyboard shortcuts
#[derive(Default)]
pub struct ShortcutRegistry {
    /// Global shortcuts that work everywhere
    global_shortcuts: HashMap<String, ShortcutDefinition>,
    /// Context-specific shortcuts
    context_shortcuts: HashMap<String, HashMap<String, ShortcutDefinition>>,
}

/// Shortcut definition
#[derive(Debug, Clone)]
pub struct ShortcutDefinition {
    pub key_combination: String,
    pub action: ShortcutAction,
    pub description: String,
    pub context: Option<String>,
    pub priority: EventPriority,
    pub enabled: bool,
}

/// Shortcut action types
#[derive(Clone)]
pub enum ShortcutAction {
    /// Focus a specific element
    Focus(String),
    /// Trigger a named action
    Action(String),
    /// Execute custom function
    Custom(Arc<dyn Fn(&KeyDownEvent) -> EventResult + Send + Sync>),
    /// Navigate within focus group
    Navigate(NavigationDirection),
    /// Dismiss overlay/popup
    Dismiss(DismissTarget),
}

impl std::fmt::Debug for ShortcutAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutAction::Focus(element) => f.debug_tuple("Focus").field(element).finish(),
            ShortcutAction::Action(action) => f.debug_tuple("Action").field(action).finish(),
            ShortcutAction::Custom(_) => f.debug_tuple("Custom").field(&"<function>").finish(),
            ShortcutAction::Navigate(direction) => {
                f.debug_tuple("Navigate").field(direction).finish()
            }
            ShortcutAction::Dismiss(target) => f.debug_tuple("Dismiss").field(target).finish(),
        }
    }
}

/// Navigation directions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Next,
    Previous,
    First,
    Last,
    Up,
    Down,
    Left,
    Right,
}

/// Dismiss targets
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DismissTarget {
    Any,
    Completion,
    Picker,
    Prompt,
    Modal,
}

/// Conflict resolution strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Higher priority wins
    Priority,
    /// Context-specific wins over global
    Context,
    /// First registered wins
    FirstRegistered,
    /// Last registered wins
    LastRegistered,
}

/// Input context manager
#[derive(Default)]
pub struct InputContextManager {
    /// Current active contexts (stack-based)
    context_stack: Vec<InputContext>,
    /// Context definitions
    contexts: HashMap<String, InputContextDefinition>,
}

/// Input context for different UI states
#[derive(Clone)]
pub struct InputContext {
    pub id: String,
    pub name: String,
    pub priority: EventPriority,
    pub blocks_default: bool,
    pub custom_handlers: HashMap<String, KeyboardEventListener>,
}

impl std::fmt::Debug for InputContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputContext")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("priority", &self.priority)
            .field("blocks_default", &self.blocks_default)
            .field(
                "custom_handlers",
                &format!("<{} handlers>", self.custom_handlers.len()),
            )
            .finish()
    }
}

/// Input context definition
#[derive(Debug, Clone)]
pub struct InputContextDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_priority: EventPriority,
    pub key_bindings: HashMap<String, ShortcutAction>,
    pub parent_context: Option<String>,
}

// === Accessibility and Navigation Helper Types ===

/// Focus group information for accessibility
#[derive(Debug, Clone)]
pub struct FocusGroupInfo {
    pub id: String,
    pub name: String,
    pub priority: FocusPriority,
    pub active_element_index: Option<usize>,
    pub total_elements: usize,
    pub enabled: bool,
}

/// Focused element information for accessibility
#[derive(Debug, Clone)]
pub struct FocusedElementInfo {
    pub element_id: String,
    pub element_name: String,
    pub element_type: FocusElementType,
    pub tab_index: i32,
    pub enabled: bool,
    pub group_id: String,
    pub group_name: String,
    pub position_in_group: usize,
    pub total_in_group: usize,
}

/// Available navigation options from current position
#[derive(Debug, Clone)]
pub struct NavigationOptions {
    pub can_navigate_up: bool,
    pub can_navigate_down: bool,
    pub can_navigate_left: bool,
    pub can_navigate_right: bool,
    pub can_navigate_first: bool,
    pub can_navigate_last: bool,
    pub available_groups: Vec<FocusGroupInfo>,
}

/// Shortcut information for help systems
#[derive(Debug, Clone)]
pub struct ShortcutInfo {
    pub key_combination: String,
    pub description: String,
    pub context: Option<String>,
    pub priority: EventPriority,
}

/// Visual focus indicator configuration
#[derive(Debug, Clone)]
pub struct FocusIndicatorConfig {
    pub enabled: bool,
    pub style: FocusIndicatorStyle,
    pub animation_duration: std::time::Duration,
    pub accessibility_high_contrast: bool,
}

/// Focus indicator visual styles
#[derive(Debug, Clone)]
pub enum FocusIndicatorStyle {
    /// Subtle border highlighting
    Border {
        color: Option<gpui::Hsla>,
        width: Option<gpui::Pixels>,
    },
    /// Background color change
    Background {
        color: Option<gpui::Hsla>,
        opacity: Option<f32>,
    },
    /// Outline style focus ring
    Outline {
        color: Option<gpui::Hsla>,
        width: Option<gpui::Pixels>,
        offset: Option<gpui::Pixels>,
    },
    /// Combination of multiple indicators
    Combined {
        border: Option<Box<FocusIndicatorStyle>>,
        background: Option<Box<FocusIndicatorStyle>>,
        outline: Option<Box<FocusIndicatorStyle>>,
    },
}

impl Default for FocusIndicatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            style: FocusIndicatorStyle::Border {
                color: None, // Will use theme colors
                width: None, // Will use default width
            },
            animation_duration: std::time::Duration::from_millis(150),
            accessibility_high_contrast: false,
        }
    }
}

/// Computed focus indicator styles ready for application
#[derive(Debug, Clone)]
pub struct FocusIndicatorStyles {
    pub border_color: Option<gpui::Hsla>,
    pub border_width: Option<gpui::Pixels>,
    pub background_color: Option<gpui::Hsla>,
    pub background_opacity: Option<f32>,
    pub outline_color: Option<gpui::Hsla>,
    pub outline_width: Option<gpui::Pixels>,
    pub outline_offset: Option<gpui::Pixels>,
    pub animation_duration: std::time::Duration,
}

impl FocusIndicatorStyles {
    /// Create empty focus indicator styles (no visual indicators)
    pub fn none() -> Self {
        Self {
            border_color: None,
            border_width: None,
            background_color: None,
            background_opacity: None,
            outline_color: None,
            outline_width: None,
            outline_offset: None,
            animation_duration: std::time::Duration::from_millis(0),
        }
    }

    /// Check if any visual indicators are enabled
    pub fn has_any_indicators(&self) -> bool {
        self.border_color.is_some()
            || self.background_color.is_some()
            || self.outline_color.is_some()
    }
}

impl GlobalInputDispatcher {
    /// Create a new global input dispatcher
    pub fn new() -> Self {
        Self {
            event_provider: EventHandlingProvider::new(),
            focus_groups: Arc::new(RwLock::new(FocusGroupManager::default())),
            shortcuts: Arc::new(RwLock::new(ShortcutRegistry::default())),
            contexts: Arc::new(RwLock::new(InputContextManager::default())),
            dismiss_handlers: Arc::new(RwLock::new(HashMap::new())),
            action_handlers: Arc::new(RwLock::new(HashMap::new())),
            focus_indicator_config: Arc::new(RwLock::new(FocusIndicatorConfig::default())),
        }
    }

    /// Initialize the global input system
    pub fn initialize(&mut self, cx: &mut App) {
        // Initialize the underlying event provider
        self.event_provider.initialize(cx);

        // Register default focus groups
        self.register_default_focus_groups();

        // Register default shortcuts
        self.register_default_shortcuts();

        // Register default input contexts
        self.register_default_contexts();

        nucleotide_logging::info!("GlobalInputDispatcher initialized");
    }

    /// Register a focus group
    pub fn register_focus_group(&self, group: FocusGroup) {
        if let Ok(mut manager) = self.focus_groups.write() {
            let priority = group.priority;
            let id = group.id.clone();
            manager.groups.insert(id.clone(), group);
            manager.priorities.insert(id.clone(), priority);

            nucleotide_logging::debug!(
                group_id = id,
                priority = ?priority,
                "Registered focus group"
            );
        }
    }

    /// Register a global shortcut
    pub fn register_shortcut(&self, shortcut: ShortcutDefinition) {
        if let Ok(mut registry) = self.shortcuts.write() {
            let key = shortcut.key_combination.clone();
            let context = shortcut.context.clone();

            match &context {
                Some(ctx) => {
                    registry
                        .context_shortcuts
                        .entry(ctx.clone())
                        .or_insert_with(HashMap::new)
                        .insert(key.clone(), shortcut);
                }
                None => {
                    registry.global_shortcuts.insert(key.clone(), shortcut);
                }
            }

            nucleotide_logging::debug!(
                key_combination = key,
                context = ?context,
                "Registered shortcut"
            );
        }
    }

    /// Handle a key event through the global system
    pub fn handle_key_event(&self, event: &KeyDownEvent) -> EventResult {
        let key_string = self.key_event_to_string(event);

        nucleotide_logging::debug!(
            key = key_string,
            "Processing key event through global input system"
        );

        // 1. Check input contexts (highest priority)
        if let Some(result) = self.check_input_contexts(&key_string, event)
            && result != EventResult::NotHandled
        {
            return result;
        }

        // 2. Check context-specific shortcuts
        if let Some(result) = self.check_context_shortcuts(&key_string, event)
            && result != EventResult::NotHandled
        {
            return result;
        }

        // 3. Check global shortcuts
        if let Some(result) = self.check_global_shortcuts(&key_string, event)
            && result != EventResult::NotHandled
        {
            return result;
        }

        // 4. Handle focus group navigation
        if let Some(result) = self.handle_focus_navigation(&key_string, event)
            && result != EventResult::NotHandled
        {
            return result;
        }

        EventResult::NotHandled
    }

    /// Convert key event to string representation
    fn key_event_to_string(&self, event: &KeyDownEvent) -> String {
        let modifiers = &event.keystroke.modifiers;
        let key = &event.keystroke.key;

        let mut parts = Vec::new();

        if modifiers.control {
            parts.push("ctrl");
        }
        if modifiers.alt {
            parts.push("alt");
        }
        if modifiers.shift {
            parts.push("shift");
        }
        if modifiers.platform {
            parts.push("cmd");
        }

        parts.push(key);
        parts.join("-")
    }

    /// Check input contexts for key handling
    fn check_input_contexts(&self, key: &str, event: &KeyDownEvent) -> Option<EventResult> {
        if let Ok(contexts) = self.contexts.read() {
            // Process contexts in reverse order (last pushed = highest priority)
            for context in contexts.context_stack.iter().rev() {
                if let Some(handler) = context.custom_handlers.get(key) {
                    let result = handler(event);
                    if result != EventResult::NotHandled {
                        return Some(result);
                    }
                }
            }
        }
        None
    }

    /// Check context-specific shortcuts
    fn check_context_shortcuts(&self, key: &str, event: &KeyDownEvent) -> Option<EventResult> {
        if let Ok(registry) = self.shortcuts.read() {
            // Get current context (top of stack)
            if let Ok(contexts) = self.contexts.read()
                && let Some(current_context) = contexts.context_stack.last()
                && let Some(context_shortcuts) = registry.context_shortcuts.get(&current_context.id)
                && let Some(shortcut) = context_shortcuts.get(key)
            {
                return Some(self.execute_shortcut_action(&shortcut.action, event));
            }
        }
        None
    }

    /// Check global shortcuts
    fn check_global_shortcuts(&self, key: &str, event: &KeyDownEvent) -> Option<EventResult> {
        if let Ok(registry) = self.shortcuts.read()
            && let Some(shortcut) = registry.global_shortcuts.get(key)
        {
            return Some(self.execute_shortcut_action(&shortcut.action, event));
        }
        None
    }

    /// Handle focus group navigation
    fn handle_focus_navigation(&self, key: &str, _event: &KeyDownEvent) -> Option<EventResult> {
        match key {
            "tab" => {
                self.focus_next();
                Some(EventResult::Handled)
            }
            "shift-tab" => {
                self.focus_previous();
                Some(EventResult::Handled)
            }
            _ => None,
        }
    }

    /// Execute a shortcut action
    fn execute_shortcut_action(
        &self,
        action: &ShortcutAction,
        event: &KeyDownEvent,
    ) -> EventResult {
        match action {
            ShortcutAction::Focus(element_id) => {
                self.focus_element(element_id);
                // Also emit an action event so the workspace can handle the focus change
                self.emit_action_event(element_id);
                EventResult::Handled
            }
            ShortcutAction::Action(action_name) => {
                self.trigger_action(action_name);
                // Emit the action event for workspace handling
                self.emit_action_event(action_name);
                EventResult::Handled
            }
            ShortcutAction::Custom(handler) => handler(event),
            ShortcutAction::Navigate(direction) => {
                self.navigate_focus(*direction);
                EventResult::Handled
            }
            ShortcutAction::Dismiss(target) => {
                self.dismiss_target(*target);
                // Return Handled (not HandledAndStop) so normal processing can also handle the dismissal
                // This allows both global input management AND actual UI dismissal to occur
                EventResult::Handled
            }
        }
    }

    /// Emit an action event that can be handled by the workspace
    fn emit_action_event(&self, action_name: &str) {
        // For now, just log the action. In a full implementation, this would
        // emit events through a proper event system that the workspace subscribes to
        nucleotide_logging::debug!(
            action = action_name,
            "Global input system emitted action event"
        );
    }

    /// Focus next element in current group
    fn focus_next(&self) {
        if let Ok(mut manager) = self.focus_groups.write()
            && let Some(active_group_id) = manager.active_group.clone()
            && let Some(group) = manager.groups.get_mut(&active_group_id)
            && let Some(current_index) = group.active_element
        {
            let next_index = (current_index + 1) % group.elements.len();
            group.active_element = Some(next_index);
            nucleotide_logging::debug!(
                group = active_group_id,
                from_index = current_index,
                to_index = next_index,
                "Focus moved to next element"
            );
        }
    }

    /// Focus previous element in current group
    fn focus_previous(&self) {
        if let Ok(mut manager) = self.focus_groups.write()
            && let Some(active_group_id) = manager.active_group.clone()
            && let Some(group) = manager.groups.get_mut(&active_group_id)
            && let Some(current_index) = group.active_element
        {
            let prev_index = if current_index == 0 {
                group.elements.len() - 1
            } else {
                current_index - 1
            };
            group.active_element = Some(prev_index);
            nucleotide_logging::debug!(
                group = active_group_id,
                from_index = current_index,
                to_index = prev_index,
                "Focus moved to previous element"
            );
        }
    }

    /// Focus a specific element by ID
    fn focus_element(&self, element_id: &str) {
        nucleotide_logging::debug!(element_id = element_id, "Focusing element");

        if let Ok(mut manager) = self.focus_groups.write() {
            // Find the focus group containing this element
            let mut target_group_id: Option<String> = None;
            let mut target_element_index: Option<usize> = None;

            for (group_id, group) in &manager.groups {
                if group.id == element_id || group.elements.iter().any(|e| e.id == element_id) {
                    target_group_id = Some(group_id.clone());

                    // If element_id matches a specific element, set it as active
                    if let Some(element_index) =
                        group.elements.iter().position(|e| e.id == element_id)
                    {
                        target_element_index = Some(element_index);
                    } else {
                        // Focus the first element in the group
                        target_element_index = Some(0);
                    }
                    break;
                }
            }

            if let Some(group_id) = target_group_id {
                // Set this group as active
                manager.active_group = Some(group_id.clone());

                // Set the active element
                if let Some(group) = manager.groups.get_mut(&group_id) {
                    group.active_element = target_element_index;
                }

                nucleotide_logging::debug!(
                    element_id = element_id,
                    group_id = group_id,
                    "Focused element in group"
                );
                return;
            }
        }

        nucleotide_logging::warn!(element_id = element_id, "Element not found for focusing");
    }

    /// Trigger a named action
    fn trigger_action(&self, action_name: &str) {
        nucleotide_logging::debug!(action = action_name, "Triggering action");

        // Check if we have a registered action handler
        if let Some(handler) = self.get_action_handler(action_name) {
            handler();
        } else {
            nucleotide_logging::warn!(action = action_name, "No handler registered for action");
        }
    }

    /// Navigate focus in a specific direction
    fn navigate_focus(&self, direction: NavigationDirection) {
        nucleotide_logging::debug!(direction = ?direction, "Navigating focus");

        if let Ok(mut manager) = self.focus_groups.write()
            && let Some(active_group_id) = manager.active_group.clone()
            && let Some(group) = manager.groups.get_mut(&active_group_id)
        {
            match direction {
                NavigationDirection::Up | NavigationDirection::Previous => {
                    self.navigate_to_previous_element(group);
                }
                NavigationDirection::Down | NavigationDirection::Next => {
                    self.navigate_to_next_element(group);
                }
                NavigationDirection::First => {
                    if !group.elements.is_empty() {
                        group.active_element = Some(0);
                        nucleotide_logging::debug!(
                            group = active_group_id,
                            element_index = 0,
                            "Focused first element in group"
                        );
                    }
                }
                NavigationDirection::Last => {
                    if !group.elements.is_empty() {
                        let last_index = group.elements.len() - 1;
                        group.active_element = Some(last_index);
                        nucleotide_logging::debug!(
                            group = active_group_id,
                            element_index = last_index,
                            "Focused last element in group"
                        );
                    }
                }
                NavigationDirection::Left => {
                    self.navigate_to_left_panel(&mut manager);
                }
                NavigationDirection::Right => {
                    self.navigate_to_right_panel(&mut manager);
                }
            }
        }
    }

    /// Navigate to the previous enabled element within current group
    fn navigate_to_previous_element(&self, group: &mut FocusGroup) {
        if let Some(current_index) = group.active_element {
            let mut prev_index = if current_index == 0 {
                group.elements.len() - 1
            } else {
                current_index - 1
            };

            // Skip disabled elements
            let start_index = prev_index;
            loop {
                if group.elements.get(prev_index).is_some_and(|e| e.enabled) {
                    group.active_element = Some(prev_index);
                    nucleotide_logging::debug!(
                        group = group.id,
                        from_index = current_index,
                        to_index = prev_index,
                        "Focused previous enabled element"
                    );
                    break;
                }

                prev_index = if prev_index == 0 {
                    group.elements.len() - 1
                } else {
                    prev_index - 1
                };

                // Avoid infinite loop if all elements are disabled
                if prev_index == start_index {
                    break;
                }
            }
        }
    }

    /// Navigate to the next enabled element within current group
    fn navigate_to_next_element(&self, group: &mut FocusGroup) {
        if let Some(current_index) = group.active_element {
            let mut next_index = (current_index + 1) % group.elements.len();

            // Skip disabled elements
            let start_index = next_index;
            loop {
                if group.elements.get(next_index).is_some_and(|e| e.enabled) {
                    group.active_element = Some(next_index);
                    nucleotide_logging::debug!(
                        group = group.id,
                        from_index = current_index,
                        to_index = next_index,
                        "Focused next enabled element"
                    );
                    break;
                }

                next_index = (next_index + 1) % group.elements.len();

                // Avoid infinite loop if all elements are disabled
                if next_index == start_index {
                    break;
                }
            }
        }
    }

    /// Navigate to the left panel (higher priority focus groups)
    fn navigate_to_left_panel(&self, manager: &mut FocusGroupManager) {
        // Find the next highest priority focus group to the left
        let current_group_id = manager.active_group.clone();
        if let Some(current_group_id) = current_group_id {
            let current_priority = manager
                .priorities
                .get(&current_group_id)
                .copied()
                .unwrap_or(FocusPriority::Normal);

            // Look for groups with lower priority values (left side)
            let mut best_group: Option<(String, FocusPriority)> = None;

            for (group_id, &priority) in &manager.priorities {
                if *group_id != current_group_id
                    && priority < current_priority
                    && let Some(group) = manager.groups.get(group_id)
                    && group.enabled
                    && !group.elements.is_empty()
                {
                    match &best_group {
                        None => best_group = Some((group_id.clone(), priority)),
                        Some((_, best_priority)) => {
                            if priority > *best_priority {
                                best_group = Some((group_id.clone(), priority));
                            }
                        }
                    }
                }
            }

            if let Some((target_group_id, _)) = best_group {
                manager.active_group = Some(target_group_id.clone());
                if let Some(group) = manager.groups.get_mut(&target_group_id) {
                    group.active_element = Some(0);
                }
                nucleotide_logging::debug!(
                    from_group = current_group_id,
                    to_group = target_group_id,
                    "Navigated to left panel"
                );
            }
        }
    }

    /// Navigate to the right panel (lower priority focus groups)
    fn navigate_to_right_panel(&self, manager: &mut FocusGroupManager) {
        // Find the next lowest priority focus group to the right
        let current_group_id = manager.active_group.clone();
        if let Some(current_group_id) = current_group_id {
            let current_priority = manager
                .priorities
                .get(&current_group_id)
                .copied()
                .unwrap_or(FocusPriority::Normal);

            // Look for groups with higher priority values (right side)
            let mut best_group: Option<(String, FocusPriority)> = None;

            for (group_id, &priority) in &manager.priorities {
                if *group_id != current_group_id
                    && priority > current_priority
                    && let Some(group) = manager.groups.get(group_id)
                    && group.enabled
                    && !group.elements.is_empty()
                {
                    match &best_group {
                        None => best_group = Some((group_id.clone(), priority)),
                        Some((_, best_priority)) => {
                            if priority < *best_priority {
                                best_group = Some((group_id.clone(), priority));
                            }
                        }
                    }
                }
            }

            if let Some((target_group_id, _)) = best_group {
                manager.active_group = Some(target_group_id.clone());
                if let Some(group) = manager.groups.get_mut(&target_group_id) {
                    group.active_element = Some(0);
                }
                nucleotide_logging::debug!(
                    from_group = current_group_id,
                    to_group = target_group_id,
                    "Navigated to right panel"
                );
            }
        }
    }

    /// Dismiss a specific target
    fn dismiss_target(&self, target: DismissTarget) {
        nucleotide_logging::debug!(target = ?target, "Dismissing target");

        if let Ok(handlers) = self.dismiss_handlers.read() {
            // Try specific target handler first
            if let Some(handler) = handlers.get(&target) {
                handler();
                return;
            }

            // Fall back to Any handler if available
            if target != DismissTarget::Any
                && let Some(handler) = handlers.get(&DismissTarget::Any)
            {
                handler();
                return;
            }
        }

        nucleotide_logging::warn!(target = ?target, "No dismiss handler registered for target");
    }

    /// Register default focus groups
    fn register_default_focus_groups(&self) {
        let groups = vec![
            FocusGroup {
                id: "editor".to_string(),
                name: "Editor".to_string(),
                priority: FocusPriority::High,
                elements: vec![],
                active_element: None,
                enabled: true,
            },
            FocusGroup {
                id: "completion".to_string(),
                name: "Completion".to_string(),
                priority: FocusPriority::Critical,
                elements: vec![],
                active_element: None,
                enabled: true,
            },
            FocusGroup {
                id: "file_tree".to_string(),
                name: "File Tree".to_string(),
                priority: FocusPriority::Normal,
                elements: vec![],
                active_element: None,
                enabled: true,
            },
            FocusGroup {
                id: "overlays".to_string(),
                name: "Overlays".to_string(),
                priority: FocusPriority::High,
                elements: vec![],
                active_element: None,
                enabled: true,
            },
        ];

        for group in groups {
            self.register_focus_group(group);
        }
    }

    /// Register default shortcuts
    fn register_default_shortcuts(&self) {
        let shortcuts = vec![
            ShortcutDefinition {
                key_combination: "escape".to_string(),
                action: ShortcutAction::Dismiss(DismissTarget::Any),
                description: "Dismiss any overlay or popup".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-space".to_string(),
                action: ShortcutAction::Action("trigger_completion".to_string()),
                description: "Trigger completion".to_string(),
                context: Some("editor".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "tab".to_string(),
                action: ShortcutAction::Navigate(NavigationDirection::Next),
                description: "Focus next element".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "shift-tab".to_string(),
                action: ShortcutAction::Navigate(NavigationDirection::Previous),
                description: "Focus previous element".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
        ];

        for shortcut in shortcuts {
            self.register_shortcut(shortcut);
        }
    }

    /// Register default input contexts
    fn register_default_contexts(&self) {
        let contexts = vec![
            InputContextDefinition {
                id: "editor".to_string(),
                name: "Editor".to_string(),
                description: "Main editor context".to_string(),
                default_priority: EventPriority::Normal,
                key_bindings: HashMap::new(),
                parent_context: None,
            },
            InputContextDefinition {
                id: "completion".to_string(),
                name: "Completion".to_string(),
                description: "Completion popup context".to_string(),
                default_priority: EventPriority::High,
                key_bindings: HashMap::new(),
                parent_context: Some("editor".to_string()),
            },
        ];

        if let Ok(mut manager) = self.contexts.write() {
            for context_def in contexts {
                manager.contexts.insert(context_def.id.clone(), context_def);
            }
        }
    }

    /// Push an input context onto the stack
    pub fn push_context(&self, context_id: &str) {
        if let Ok(mut manager) = self.contexts.write()
            && let Some(definition) = manager.contexts.get(context_id)
        {
            let context = InputContext {
                id: context_id.to_string(),
                name: definition.name.clone(),
                priority: definition.default_priority,
                blocks_default: false,
                custom_handlers: HashMap::new(),
            };
            manager.context_stack.push(context);

            nucleotide_logging::debug!(
                context_id = context_id,
                stack_depth = manager.context_stack.len(),
                "Pushed input context"
            );
        }
    }

    /// Pop the top input context from the stack
    pub fn pop_context(&self) -> Option<String> {
        if let Ok(mut manager) = self.contexts.write()
            && let Some(context) = manager.context_stack.pop()
        {
            nucleotide_logging::debug!(
                context_id = context.id,
                stack_depth = manager.context_stack.len(),
                "Popped input context"
            );
            return Some(context.id);
        }
        None
    }

    /// Check if a specific context is currently active (top of stack)
    pub fn is_context_active(&self, context_id: &str) -> bool {
        if let Ok(contexts) = self.contexts.read() {
            contexts
                .context_stack
                .last()
                .map(|ctx| ctx.id == context_id)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Register a dismiss handler for a specific target
    pub fn register_dismiss_handler<F>(&self, target: DismissTarget, handler: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut handlers) = self.dismiss_handlers.write() {
            handlers.insert(target, Arc::new(handler));
            nucleotide_logging::debug!(target = ?target, "Registered dismiss handler");
        }
    }

    /// Get action handler for a named action
    fn get_action_handler(&self, action_name: &str) -> Option<Arc<dyn Fn() + Send + Sync>> {
        if let Ok(handlers) = self.action_handlers.read() {
            handlers.get(action_name).cloned()
        } else {
            None
        }
    }

    /// Register an action handler
    pub fn register_action_handler<F>(&self, action_name: String, handler: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut handlers) = self.action_handlers.write() {
            handlers.insert(action_name.clone(), Arc::new(handler));
            nucleotide_logging::debug!(action = action_name, "Registered action handler");
        }
    }

    // === Accessibility and Navigation Helpers ===

    /// Get the currently active focus group information for accessibility
    pub fn get_active_focus_group(&self) -> Option<FocusGroupInfo> {
        if let Ok(manager) = self.focus_groups.read()
            && let Some(group_id) = &manager.active_group
            && let Some(group) = manager.groups.get(group_id)
        {
            return Some(FocusGroupInfo {
                id: group.id.clone(),
                name: group.name.clone(),
                priority: group.priority,
                active_element_index: group.active_element,
                total_elements: group.elements.len(),
                enabled: group.enabled,
            });
        }
        None
    }

    /// Get information about the currently focused element for accessibility
    pub fn get_focused_element_info(&self) -> Option<FocusedElementInfo> {
        if let Ok(manager) = self.focus_groups.read()
            && let Some(group_id) = &manager.active_group
            && let Some(group) = manager.groups.get(group_id)
            && let Some(element_index) = group.active_element
            && let Some(element) = group.elements.get(element_index)
        {
            return Some(FocusedElementInfo {
                element_id: element.id.clone(),
                element_name: element.name.clone(),
                element_type: element.element_type,
                tab_index: element.tab_index,
                enabled: element.enabled,
                group_id: group.id.clone(),
                group_name: group.name.clone(),
                position_in_group: element_index + 1,
                total_in_group: group.elements.len(),
            });
        }
        None
    }

    /// Get all available navigation options from current focus position
    pub fn get_navigation_options(&self) -> NavigationOptions {
        let mut options = NavigationOptions {
            can_navigate_up: false,
            can_navigate_down: false,
            can_navigate_left: false,
            can_navigate_right: false,
            can_navigate_first: false,
            can_navigate_last: false,
            available_groups: Vec::new(),
        };

        if let Ok(manager) = self.focus_groups.read() {
            // Check for available focus groups
            for group in manager.groups.values() {
                if group.enabled && !group.elements.is_empty() {
                    options.available_groups.push(FocusGroupInfo {
                        id: group.id.clone(),
                        name: group.name.clone(),
                        priority: group.priority,
                        active_element_index: group.active_element,
                        total_elements: group.elements.len(),
                        enabled: group.enabled,
                    });
                }
            }

            if let Some(group_id) = &manager.active_group
                && let Some(group) = manager.groups.get(group_id)
            {
                // Check within-group navigation
                if let Some(current_index) = group.active_element {
                    options.can_navigate_up = group.elements.len() > 1;
                    options.can_navigate_down = group.elements.len() > 1;
                    options.can_navigate_first = current_index > 0;
                    options.can_navigate_last =
                        current_index < group.elements.len().saturating_sub(1);
                }

                // Check cross-panel navigation
                let current_priority = manager
                    .priorities
                    .get(group_id)
                    .copied()
                    .unwrap_or(FocusPriority::Normal);

                for (other_group_id, &priority) in &manager.priorities {
                    if other_group_id != group_id
                        && let Some(other_group) = manager.groups.get(other_group_id)
                        && other_group.enabled
                        && !other_group.elements.is_empty()
                    {
                        if priority < current_priority {
                            options.can_navigate_left = true;
                        }
                        if priority > current_priority {
                            options.can_navigate_right = true;
                        }
                    }
                }
            }
        }

        options
    }

    /// Announce focus change for screen readers (accessibility helper)
    pub fn announce_focus_change(&self, element_info: &FocusedElementInfo) {
        nucleotide_logging::info!(
            element_id = element_info.element_id,
            element_name = element_info.element_name,
            element_type = ?element_info.element_type,
            group_name = element_info.group_name,
            position = element_info.position_in_group,
            total = element_info.total_in_group,
            "Focus changed for accessibility"
        );

        // In a full implementation, this would integrate with platform accessibility APIs
        // For now, we log structured information that could be consumed by accessibility tools
    }

    /// Check if a specific shortcut is available in current context
    pub fn is_shortcut_available(&self, key_combination: &str) -> bool {
        if let Ok(registry) = self.shortcuts.read() {
            // Check global shortcuts first
            if let Some(shortcut) = registry.global_shortcuts.get(key_combination) {
                return shortcut.enabled;
            }

            // Check context-specific shortcuts
            if let Ok(contexts) = self.contexts.read()
                && let Some(current_context) = contexts.context_stack.last()
                && let Some(context_shortcuts) = registry.context_shortcuts.get(&current_context.id)
                && let Some(shortcut) = context_shortcuts.get(key_combination)
            {
                return shortcut.enabled;
            }
        }
        false
    }

    /// Get all available shortcuts in current context for help systems
    pub fn get_available_shortcuts(&self) -> Vec<ShortcutInfo> {
        let mut shortcuts = Vec::new();

        if let Ok(registry) = self.shortcuts.read() {
            // Add global shortcuts
            for (key_combo, shortcut) in &registry.global_shortcuts {
                if shortcut.enabled {
                    shortcuts.push(ShortcutInfo {
                        key_combination: key_combo.clone(),
                        description: shortcut.description.clone(),
                        context: None,
                        priority: shortcut.priority,
                    });
                }
            }

            // Add context-specific shortcuts
            if let Ok(contexts) = self.contexts.read()
                && let Some(current_context) = contexts.context_stack.last()
                && let Some(context_shortcuts) = registry.context_shortcuts.get(&current_context.id)
            {
                for (key_combo, shortcut) in context_shortcuts {
                    if shortcut.enabled {
                        shortcuts.push(ShortcutInfo {
                            key_combination: key_combo.clone(),
                            description: shortcut.description.clone(),
                            context: Some(current_context.name.clone()),
                            priority: shortcut.priority,
                        });
                    }
                }
            }
        }

        // Sort by priority and then by key combination
        shortcuts.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.key_combination.cmp(&b.key_combination))
        });

        shortcuts
    }

    // === Focus Indicator Management ===

    /// Configure focus indicators
    pub fn configure_focus_indicators(&self, config: FocusIndicatorConfig) {
        if let Ok(mut indicator_config) = self.focus_indicator_config.write() {
            *indicator_config = config;
            nucleotide_logging::debug!("Updated focus indicator configuration");
        }
    }

    /// Get current focus indicator configuration
    pub fn get_focus_indicator_config(&self) -> Option<FocusIndicatorConfig> {
        if let Ok(config) = self.focus_indicator_config.read() {
            Some(config.clone())
        } else {
            None
        }
    }

    /// Create focus indicator styles for a focused element
    pub fn create_focus_indicator_styles(&self, theme: &crate::Theme) -> FocusIndicatorStyles {
        let config = if let Ok(config) = self.focus_indicator_config.read() {
            config.clone()
        } else {
            FocusIndicatorConfig::default()
        };

        if !config.enabled {
            return FocusIndicatorStyles::none();
        }

        let tokens = &theme.tokens;

        match &config.style {
            FocusIndicatorStyle::Border { color, width } => {
                let focus_color = color.unwrap_or(if config.accessibility_high_contrast {
                    tokens.chrome.primary_active
                } else {
                    tokens.chrome.primary
                });
                let focus_width = width.unwrap_or(gpui::px(2.0));

                FocusIndicatorStyles {
                    border_color: Some(focus_color),
                    border_width: Some(focus_width),
                    background_color: None,
                    background_opacity: None,
                    outline_color: None,
                    outline_width: None,
                    outline_offset: None,
                    animation_duration: config.animation_duration,
                }
            }
            FocusIndicatorStyle::Background { color, opacity } => {
                let focus_color = color.unwrap_or(if config.accessibility_high_contrast {
                    tokens.chrome.primary_active.alpha(0.2)
                } else {
                    tokens.chrome.primary.alpha(0.1)
                });
                let focus_opacity = opacity.unwrap_or(0.1);

                FocusIndicatorStyles {
                    border_color: None,
                    border_width: None,
                    background_color: Some(focus_color),
                    background_opacity: Some(focus_opacity),
                    outline_color: None,
                    outline_width: None,
                    outline_offset: None,
                    animation_duration: config.animation_duration,
                }
            }
            FocusIndicatorStyle::Outline {
                color,
                width,
                offset,
            } => {
                let focus_color = color.unwrap_or(if config.accessibility_high_contrast {
                    tokens.chrome.primary_active
                } else {
                    tokens.chrome.primary
                });
                let focus_width = width.unwrap_or(gpui::px(2.0));
                let focus_offset = offset.unwrap_or(gpui::px(1.0));

                FocusIndicatorStyles {
                    border_color: None,
                    border_width: None,
                    background_color: None,
                    background_opacity: None,
                    outline_color: Some(focus_color),
                    outline_width: Some(focus_width),
                    outline_offset: Some(focus_offset),
                    animation_duration: config.animation_duration,
                }
            }
            FocusIndicatorStyle::Combined {
                border,
                background,
                outline,
            } => {
                let mut styles = FocusIndicatorStyles::none();
                styles.animation_duration = config.animation_duration;

                if let Some(border_style) = border
                    && let FocusIndicatorStyle::Border { color, width } = border_style.as_ref()
                {
                    styles.border_color = Some(color.unwrap_or(tokens.chrome.primary));
                    styles.border_width = Some(width.unwrap_or(gpui::px(1.0)));
                }

                if let Some(bg_style) = background
                    && let FocusIndicatorStyle::Background { color, opacity } = bg_style.as_ref()
                {
                    styles.background_color =
                        Some(color.unwrap_or(tokens.chrome.primary.alpha(0.1)));
                    styles.background_opacity = Some(opacity.unwrap_or(0.1));
                }

                if let Some(outline_style) = outline
                    && let FocusIndicatorStyle::Outline {
                        color,
                        width,
                        offset,
                    } = outline_style.as_ref()
                {
                    styles.outline_color = Some(color.unwrap_or(tokens.chrome.primary));
                    styles.outline_width = Some(width.unwrap_or(gpui::px(2.0)));
                    styles.outline_offset = Some(offset.unwrap_or(gpui::px(1.0)));
                }

                styles
            }
        }
    }
}

impl Default for GlobalInputDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_input_dispatcher_creation() {
        let dispatcher = GlobalInputDispatcher::new();

        // Basic initialization test
        assert!(dispatcher.focus_groups.read().is_ok());
        assert!(dispatcher.shortcuts.read().is_ok());
        assert!(dispatcher.contexts.read().is_ok());
    }

    #[test]
    fn test_focus_group_registration() {
        let dispatcher = GlobalInputDispatcher::new();

        let group = FocusGroup {
            id: "test-group".to_string(),
            name: "Test Group".to_string(),
            priority: FocusPriority::Normal,
            elements: vec![],
            active_element: None,
            enabled: true,
        };

        dispatcher.register_focus_group(group);

        let manager = dispatcher.focus_groups.read().unwrap();
        assert!(manager.groups.contains_key("test-group"));
        assert_eq!(
            manager.priorities.get("test-group"),
            Some(&FocusPriority::Normal)
        );
    }

    #[test]
    fn test_shortcut_registration() {
        let dispatcher = GlobalInputDispatcher::new();

        let shortcut = ShortcutDefinition {
            key_combination: "ctrl-t".to_string(),
            action: ShortcutAction::Action("test_action".to_string()),
            description: "Test shortcut".to_string(),
            context: None,
            priority: EventPriority::Normal,
            enabled: true,
        };

        dispatcher.register_shortcut(shortcut);

        let registry = dispatcher.shortcuts.read().unwrap();
        assert!(registry.global_shortcuts.contains_key("ctrl-t"));
    }

    #[test]
    fn test_context_stack() {
        let dispatcher = GlobalInputDispatcher::new();

        // Register contexts first
        dispatcher.register_default_contexts();

        dispatcher.push_context("editor");
        dispatcher.push_context("completion");

        {
            let manager = dispatcher.contexts.read().unwrap();
            assert_eq!(manager.context_stack.len(), 2);
            assert_eq!(manager.context_stack.last().unwrap().id, "completion");
        }

        let popped = dispatcher.pop_context();
        assert_eq!(popped, Some("completion".to_string()));

        {
            let manager = dispatcher.contexts.read().unwrap();
            assert_eq!(manager.context_stack.len(), 1);
            assert_eq!(manager.context_stack.last().unwrap().id, "editor");
        }
    }

    #[test]
    fn test_key_string_conversion() {
        let dispatcher = GlobalInputDispatcher::new();

        // This would require creating a mock KeyDownEvent, which is complex
        // In a real test, you'd create appropriate test events and verify the conversion
        // For now, we'll just test that the method exists and can be called

        // Note: Full testing would require GPUI test infrastructure
    }
}
