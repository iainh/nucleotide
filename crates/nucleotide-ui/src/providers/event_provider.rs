// ABOUTME: Event handling provider component for centralized event management and distribution
// ABOUTME: Manages global event listeners, custom events, and event delegation patterns

use super::{Provider, ProviderContainer, use_provider};
use gpui::{AnyElement, App, ElementId, IntoElement, KeyDownEvent, SharedString};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Event handling provider for centralized event management
#[derive(Clone)]
pub struct EventHandlingProvider {
    /// Global event listeners
    pub global_listeners: Arc<RwLock<GlobalEventListeners>>,
    /// Event delegation configuration
    pub delegation_config: EventDelegationConfig,
    /// Custom event system
    pub custom_events: Arc<RwLock<CustomEventSystem>>,
    /// Event analytics and monitoring
    pub analytics_config: EventAnalyticsConfig,
    /// Event filtering and validation
    pub filter_config: EventFilterConfig,
}

/// Global event listeners registry
#[derive(Default)]
pub struct GlobalEventListeners {
    /// Keyboard event listeners
    pub keyboard_listeners: HashMap<String, Vec<KeyboardEventListener>>,
    /// Mouse event listeners
    pub mouse_listeners: HashMap<String, Vec<MouseEventListener>>,
    /// Focus event listeners
    pub focus_listeners: HashMap<String, Vec<FocusEventListener>>,
    /// Custom event listeners
    pub custom_listeners: HashMap<String, Vec<CustomEventListener>>,
    /// Event listener priorities
    pub listener_priorities: HashMap<String, EventPriority>,
}

/// Keyboard event listener
pub type KeyboardEventListener = Arc<dyn Fn(&KeyDownEvent) -> EventResult + Send + Sync>;

/// Mouse event listener
pub type MouseEventListener = Arc<dyn Fn(&MouseEventDetails) -> EventResult + Send + Sync>;

/// Focus event listener
pub type FocusEventListener = Arc<dyn Fn(&FocusEventDetails) -> EventResult + Send + Sync>;

/// Custom event listener
pub type CustomEventListener = Arc<dyn Fn(&CustomEventDetails) -> EventResult + Send + Sync>;

/// Event listener priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Event processing result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was handled, continue processing
    Handled,
    /// Event was handled, stop processing
    HandledAndStop,
    /// Event was not handled, continue processing
    NotHandled,
    /// Prevent default browser behavior
    PreventDefault,
}

/// Mouse event details
#[derive(Debug, Clone)]
pub struct MouseEventDetails {
    pub event_type: MouseEventType,
    pub position: gpui::Point<gpui::Pixels>,
    pub button: Option<gpui::MouseButton>,
    pub modifiers: gpui::Modifiers,
    pub target_id: Option<ElementId>,
}

/// Types of mouse events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    Down,
    Up,
    Move,
    Enter,
    Leave,
    Click,
    DoubleClick,
    Wheel,
}

/// Focus event details
#[derive(Debug, Clone)]
pub struct FocusEventDetails {
    pub event_type: FocusEventType,
    pub target_id: Option<ElementId>,
    pub related_target_id: Option<ElementId>,
    pub is_programmatic: bool,
}

/// Types of focus events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEventType {
    Focus,
    Blur,
    FocusIn,
    FocusOut,
}

/// Custom event details
#[derive(Debug, Clone)]
pub struct CustomEventDetails {
    pub event_type: SharedString,
    pub data: CustomEventData,
    pub target_id: Option<ElementId>,
    pub bubbles: bool,
    pub cancelable: bool,
}

/// Custom event data
#[derive(Debug, Clone)]
pub enum CustomEventData {
    None,
    String(SharedString),
    Number(f64),
    Boolean(bool),
    Object(HashMap<SharedString, CustomEventData>),
    Array(Vec<CustomEventData>),
}

/// Event delegation configuration
#[derive(Debug, Clone)]
pub struct EventDelegationConfig {
    /// Enable event delegation
    pub enable_delegation: bool,
    /// Delegation root selectors
    pub delegation_roots: Vec<SharedString>,
    /// Events to delegate
    pub delegated_events: Vec<SharedString>,
    /// Event bubbling configuration
    pub bubbling_config: EventBubblingConfig,
}

/// Event bubbling configuration
#[derive(Debug, Clone)]
pub struct EventBubblingConfig {
    /// Enable event bubbling
    pub enable_bubbling: bool,
    /// Stop propagation on handled events
    pub stop_on_handled: bool,
    /// Maximum bubble depth
    pub max_bubble_depth: usize,
}

/// Custom event system
#[derive(Default)]
pub struct CustomEventSystem {
    /// Registered custom event types
    pub event_types: HashMap<SharedString, CustomEventDefinition>,
    /// Event queues for different priorities
    pub event_queues: HashMap<EventPriority, Vec<QueuedEvent>>,
    /// Event scheduling configuration
    pub scheduling_config: EventSchedulingConfig,
}

/// Custom event definition
#[derive(Debug, Clone)]
pub struct CustomEventDefinition {
    pub name: SharedString,
    pub description: Option<SharedString>,
    pub data_schema: Option<EventDataSchema>,
    pub default_priority: EventPriority,
    pub bubbles: bool,
    pub cancelable: bool,
}

/// Event data schema for validation
#[derive(Debug, Clone)]
pub struct EventDataSchema {
    pub required_fields: Vec<SharedString>,
    pub optional_fields: Vec<SharedString>,
    pub field_types: HashMap<SharedString, EventDataType>,
}

/// Event data types for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDataType {
    String,
    Number,
    Boolean,
    Object,
    Array,
    Any,
}

/// Queued event for processing
#[derive(Debug, Clone)]
pub struct QueuedEvent {
    pub event: CustomEventDetails,
    pub timestamp: std::time::Instant,
    pub priority: EventPriority,
    pub retry_count: usize,
    pub max_retries: usize,
}

/// Event scheduling configuration
#[derive(Debug, Clone)]
pub struct EventSchedulingConfig {
    /// Process events synchronously
    pub synchronous_processing: bool,
    /// Batch size for event processing
    pub batch_size: usize,
    /// Maximum queue size
    pub max_queue_size: usize,
    /// Event timeout
    pub event_timeout: std::time::Duration,
}

/// Event analytics and monitoring configuration
#[derive(Debug, Clone)]
pub struct EventAnalyticsConfig {
    /// Enable event analytics
    pub enable_analytics: bool,
    /// Track event performance
    pub track_performance: bool,
    /// Event sampling rate (0.0 to 1.0)
    pub sampling_rate: f32,
    /// Analytics buffer size
    pub buffer_size: usize,
    /// Analytics reporting interval
    pub reporting_interval: std::time::Duration,
}

/// Event filter configuration
#[derive(Debug, Clone)]
pub struct EventFilterConfig {
    /// Enable event filtering
    pub enable_filtering: bool,
    /// Rate limiting configuration
    pub rate_limiting: EventRateLimiting,
    /// Event validation rules
    pub validation_rules: Vec<EventValidationRule>,
    /// Blocked event types
    pub blocked_events: Vec<SharedString>,
}

/// Event rate limiting configuration
#[derive(Debug, Clone)]
pub struct EventRateLimiting {
    /// Enable rate limiting
    pub enabled: bool,
    /// Maximum events per second
    pub max_events_per_second: usize,
    /// Rate limiting window
    pub window_duration: std::time::Duration,
    /// Burst allowance
    pub burst_allowance: usize,
}

/// Event validation rule
#[derive(Debug, Clone)]
pub struct EventValidationRule {
    pub event_type: SharedString,
    pub validator: EventValidator,
    pub error_action: ValidationErrorAction,
}

/// Event validator types
#[derive(Debug, Clone)]
pub enum EventValidator {
    /// Validate required fields are present
    RequiredFields(Vec<SharedString>),
    /// Validate field types
    FieldTypes(HashMap<SharedString, EventDataType>),
    /// Custom validation function
    Custom(SharedString), // Function name for lookup
}

/// Actions to take on validation errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationErrorAction {
    /// Log the error and continue
    Log,
    /// Drop the event
    Drop,
    /// Throw an exception
    Throw,
}

impl Default for EventDelegationConfig {
    fn default() -> Self {
        Self {
            enable_delegation: true,
            delegation_roots: vec!["body".into(), "main".into()],
            delegated_events: vec![
                "click".into(),
                "keydown".into(),
                "focus".into(),
                "blur".into(),
            ],
            bubbling_config: EventBubblingConfig::default(),
        }
    }
}

impl Default for EventBubblingConfig {
    fn default() -> Self {
        Self {
            enable_bubbling: true,
            stop_on_handled: true,
            max_bubble_depth: 20,
        }
    }
}

impl Default for EventSchedulingConfig {
    fn default() -> Self {
        Self {
            synchronous_processing: false,
            batch_size: 10,
            max_queue_size: 1000,
            event_timeout: std::time::Duration::from_secs(5),
        }
    }
}

impl Default for EventAnalyticsConfig {
    fn default() -> Self {
        Self {
            enable_analytics: false,
            track_performance: false,
            sampling_rate: 0.1,
            buffer_size: 1000,
            reporting_interval: std::time::Duration::from_secs(60),
        }
    }
}

impl Default for EventFilterConfig {
    fn default() -> Self {
        Self {
            enable_filtering: true,
            rate_limiting: EventRateLimiting::default(),
            validation_rules: Vec::new(),
            blocked_events: Vec::new(),
        }
    }
}

impl Default for EventRateLimiting {
    fn default() -> Self {
        Self {
            enabled: true,
            max_events_per_second: 100,
            window_duration: std::time::Duration::from_secs(1),
            burst_allowance: 10,
        }
    }
}

impl EventHandlingProvider {
    /// Create a new event handling provider
    pub fn new() -> Self {
        Self {
            global_listeners: Arc::new(RwLock::new(GlobalEventListeners::default())),
            delegation_config: EventDelegationConfig::default(),
            custom_events: Arc::new(RwLock::new(CustomEventSystem::default())),
            analytics_config: EventAnalyticsConfig::default(),
            filter_config: EventFilterConfig::default(),
        }
    }

    /// Register a global keyboard event listener
    pub fn register_keyboard_listener(
        &self,
        event_type: impl Into<String>,
        listener: KeyboardEventListener,
        priority: EventPriority,
    ) {
        let event_type = event_type.into();

        if let Ok(mut listeners) = self.global_listeners.write() {
            listeners
                .keyboard_listeners
                .entry(event_type.clone())
                .or_insert_with(Vec::new)
                .push(listener);
            listeners
                .listener_priorities
                .insert(event_type.clone(), priority);

            nucleotide_logging::debug!(
                event_type = event_type,
                priority = ?priority,
                "Registered keyboard event listener"
            );
        }
    }

    /// Register a global mouse event listener
    pub fn register_mouse_listener(
        &self,
        event_type: impl Into<String>,
        listener: MouseEventListener,
        priority: EventPriority,
    ) {
        let event_type = event_type.into();

        if let Ok(mut listeners) = self.global_listeners.write() {
            listeners
                .mouse_listeners
                .entry(event_type.clone())
                .or_insert_with(Vec::new)
                .push(listener);
            listeners
                .listener_priorities
                .insert(event_type.clone(), priority);

            nucleotide_logging::debug!(
                event_type = event_type,
                priority = ?priority,
                "Registered mouse event listener"
            );
        }
    }

    /// Register a custom event type
    pub fn register_custom_event(&self, definition: CustomEventDefinition) {
        if let Ok(mut custom_events) = self.custom_events.write() {
            let event_name = definition.name.clone();
            custom_events
                .event_types
                .insert(event_name.clone(), definition);

            nucleotide_logging::debug!(
                event_name = %event_name,
                "Registered custom event type"
            );
        }
    }

    /// Emit a custom event
    pub fn emit_custom_event(&self, event: CustomEventDetails) -> bool {
        // Validate the event if filtering is enabled
        if self.filter_config.enable_filtering && !self.validate_event(&event) {
            return false;
        }

        // Check rate limiting
        if self.filter_config.rate_limiting.enabled && !self.check_rate_limit() {
            nucleotide_logging::warn!(
                event_type = %event.event_type,
                "Event dropped due to rate limiting"
            );
            return false;
        }

        if let Ok(mut custom_events) = self.custom_events.write() {
            // Determine priority
            let priority = custom_events
                .event_types
                .get(&event.event_type)
                .map(|def| def.default_priority)
                .unwrap_or(EventPriority::Normal);

            // Create queued event
            let queued_event = QueuedEvent {
                event,
                timestamp: std::time::Instant::now(),
                priority,
                retry_count: 0,
                max_retries: 3,
            };

            // Add to appropriate queue
            custom_events
                .event_queues
                .entry(priority)
                .or_insert_with(Vec::new)
                .push(queued_event);

            // Trim queue if necessary
            let max_size = custom_events.scheduling_config.max_queue_size;
            if let Some(queue) = custom_events.event_queues.get_mut(&priority)
                && queue.len() > max_size
            {
                queue.drain(0..queue.len() - max_size);
            }

            true
        } else {
            false
        }
    }

    /// Process queued custom events
    pub fn process_custom_events(&self) -> usize {
        let mut processed_count = 0;

        if let Ok(mut custom_events) = self.custom_events.write() {
            let batch_size = custom_events.scheduling_config.batch_size;

            // Process events by priority (highest first)
            for priority in [
                EventPriority::Critical,
                EventPriority::High,
                EventPriority::Normal,
                EventPriority::Low,
            ] {
                if let Some(queue) = custom_events.event_queues.get_mut(&priority) {
                    let process_count = queue.len().min(batch_size);

                    for _ in 0..process_count {
                        if let Some(queued_event) = queue.pop() {
                            if self.dispatch_custom_event(&queued_event.event) {
                                processed_count += 1;
                            } else if queued_event.retry_count < queued_event.max_retries {
                                // Re-queue for retry
                                let mut retry_event = queued_event;
                                retry_event.retry_count += 1;
                                queue.push(retry_event);
                            }
                        }
                    }

                    if processed_count >= batch_size {
                        break;
                    }
                }
            }
        }

        processed_count
    }

    /// Dispatch a custom event to listeners
    fn dispatch_custom_event(&self, event: &CustomEventDetails) -> bool {
        if let Ok(listeners) = self.global_listeners.read()
            && let Some(event_listeners) = listeners.custom_listeners.get(event.event_type.as_ref())
        {
            for listener in event_listeners {
                match listener(event) {
                    EventResult::HandledAndStop => return true,
                    EventResult::PreventDefault => return true,
                    _ => continue,
                }
            }
        }

        true
    }

    /// Validate an event against configured rules
    fn validate_event(&self, event: &CustomEventDetails) -> bool {
        // Check if event type is blocked
        if self
            .filter_config
            .blocked_events
            .contains(&event.event_type)
        {
            return false;
        }

        // Apply validation rules
        for rule in &self.filter_config.validation_rules {
            if rule.event_type == event.event_type && !self.apply_validation_rule(rule, event) {
                match rule.error_action {
                    ValidationErrorAction::Log => {
                        nucleotide_logging::warn!(
                            event_type = %event.event_type,
                            rule = ?rule.validator,
                            "Event validation failed"
                        );
                    }
                    ValidationErrorAction::Drop => return false,
                    ValidationErrorAction::Throw => {
                        // In a real implementation, you might want to panic or return an error
                        nucleotide_logging::error!(
                            event_type = %event.event_type,
                            "Event validation failed with throw action"
                        );
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Apply a specific validation rule
    fn apply_validation_rule(
        &self,
        rule: &EventValidationRule,
        event: &CustomEventDetails,
    ) -> bool {
        match &rule.validator {
            EventValidator::RequiredFields(fields) => {
                if let CustomEventData::Object(ref data) = event.data {
                    fields.iter().all(|field| data.contains_key(field))
                } else {
                    fields.is_empty()
                }
            }
            EventValidator::FieldTypes(type_map) => {
                if let CustomEventData::Object(ref data) = event.data {
                    type_map.iter().all(|(field, expected_type)| {
                        data.get(field)
                            .is_none_or(|value| self.check_data_type(value, *expected_type))
                    })
                } else {
                    true
                }
            }
            EventValidator::Custom(_function_name) => {
                // In a real implementation, you would look up and call the custom function
                true
            }
        }
    }

    /// Check if event data matches expected type
    fn check_data_type(&self, data: &CustomEventData, expected: EventDataType) -> bool {
        matches!(
            (data, expected),
            (CustomEventData::String(_), EventDataType::String)
                | (CustomEventData::Number(_), EventDataType::Number)
                | (CustomEventData::Boolean(_), EventDataType::Boolean)
                | (CustomEventData::Object(_), EventDataType::Object)
                | (CustomEventData::Array(_), EventDataType::Array)
                | (_, EventDataType::Any)
        )
    }

    /// Check rate limiting for an event type
    fn check_rate_limit(&self) -> bool {
        // Simplified rate limiting check
        // In a real implementation, you would track event counts per time window
        true
    }

    /// Get event statistics
    pub fn get_event_statistics(&self) -> EventStatistics {
        let mut stats = EventStatistics::default();

        if let Ok(listeners) = self.global_listeners.read() {
            stats.keyboard_listener_count = listeners.keyboard_listeners.len();
            stats.mouse_listener_count = listeners.mouse_listeners.len();
            stats.focus_listener_count = listeners.focus_listeners.len();
            stats.custom_listener_count = listeners.custom_listeners.len();
        }

        if let Ok(custom_events) = self.custom_events.read() {
            stats.custom_event_types = custom_events.event_types.len();
            stats.queued_events = custom_events
                .event_queues
                .values()
                .map(|queue| queue.len())
                .sum();
        }

        stats
    }

    /// Clear all event listeners and queues
    pub fn clear_all_events(&self) {
        if let Ok(mut listeners) = self.global_listeners.write() {
            listeners.keyboard_listeners.clear();
            listeners.mouse_listeners.clear();
            listeners.focus_listeners.clear();
            listeners.custom_listeners.clear();
            listeners.listener_priorities.clear();
        }

        if let Ok(mut custom_events) = self.custom_events.write() {
            custom_events.event_queues.clear();
        }

        nucleotide_logging::debug!("Cleared all event listeners and queues");
    }
}

/// Event statistics
#[derive(Default)]
pub struct EventStatistics {
    pub keyboard_listener_count: usize,
    pub mouse_listener_count: usize,
    pub focus_listener_count: usize,
    pub custom_listener_count: usize,
    pub custom_event_types: usize,
    pub queued_events: usize,
}

impl Default for EventHandlingProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for EventHandlingProvider {
    fn type_name(&self) -> &'static str {
        "EventHandlingProvider"
    }

    fn initialize(&mut self, _cx: &mut App) {
        nucleotide_logging::info!(
            delegation_enabled = self.delegation_config.enable_delegation,
            analytics_enabled = self.analytics_config.enable_analytics,
            filtering_enabled = self.filter_config.enable_filtering,
            "EventHandlingProvider initialized"
        );
    }

    fn cleanup(&mut self, _cx: &mut App) {
        self.clear_all_events();
        nucleotide_logging::debug!("EventHandlingProvider cleaned up");
    }
}

/// Create an event handling provider component
pub fn event_provider(provider: EventHandlingProvider) -> EventProviderComponent {
    EventProviderComponent::new(provider)
}

/// Event handling provider component wrapper
pub struct EventProviderComponent {
    provider: EventHandlingProvider,
    children: Vec<AnyElement>,
}

impl EventProviderComponent {
    pub fn new(provider: EventHandlingProvider) -> Self {
        Self {
            provider,
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }
}

impl IntoElement for EventProviderComponent {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        ProviderContainer::new("event-provider", self.provider)
            .children(self.children)
            .into_any_element()
    }
}

/// Hook to use the event handling provider
pub fn use_event_provider() -> Option<EventHandlingProvider> {
    use_provider::<EventHandlingProvider>()
}

/// Hook to emit a custom event
pub fn use_emit_event() -> impl Fn(CustomEventDetails) -> bool {
    let provider = use_provider::<EventHandlingProvider>();
    move |event| {
        provider
            .as_ref()
            .map(|p| p.emit_custom_event(event.clone()))
            .unwrap_or(false)
    }
}

/// Hook to register an event listener
pub fn use_event_listener<F>(event_type: impl Into<String>, listener: F, priority: EventPriority)
where
    F: Fn(&CustomEventDetails) -> EventResult + Send + Sync + 'static,
{
    if let Some(provider) = use_provider::<EventHandlingProvider>() {
        let listener = Arc::new(listener);
        if let Ok(mut listeners) = provider.global_listeners.write() {
            let event_type = event_type.into();
            listeners
                .custom_listeners
                .entry(event_type.clone())
                .or_insert_with(Vec::new)
                .push(listener);
            listeners.listener_priorities.insert(event_type, priority);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_provider_creation() {
        let provider = EventHandlingProvider::new();

        assert!(provider.delegation_config.enable_delegation);
        assert!(!provider.analytics_config.enable_analytics);
        assert!(provider.filter_config.enable_filtering);
    }

    #[test]
    fn test_custom_event_registration() {
        let provider = EventHandlingProvider::new();

        let event_def = CustomEventDefinition {
            name: "test-event".into(),
            description: Some("A test event".into()),
            data_schema: None,
            default_priority: EventPriority::High,
            bubbles: true,
            cancelable: true,
        };

        provider.register_custom_event(event_def.clone());

        {
            let custom_events = provider.custom_events.read().unwrap();
            let registered = custom_events.event_types.get("test-event");
            assert!(registered.is_some());
            assert_eq!(registered.unwrap().default_priority, EventPriority::High);
        }
    }

    #[test]
    fn test_custom_event_emission() {
        let provider = EventHandlingProvider::new();

        // Register event type first
        let event_def = CustomEventDefinition {
            name: "test-emit".into(),
            description: None,
            data_schema: None,
            default_priority: EventPriority::Normal,
            bubbles: false,
            cancelable: false,
        };
        provider.register_custom_event(event_def);

        let event = CustomEventDetails {
            event_type: "test-emit".into(),
            data: CustomEventData::String("test data".into()),
            target_id: None,
            bubbles: false,
            cancelable: false,
        };

        let emitted = provider.emit_custom_event(event);
        assert!(emitted);

        // Check that event was queued
        {
            let custom_events = provider.custom_events.read().unwrap();
            let queue = custom_events.event_queues.get(&EventPriority::Normal);
            assert!(queue.is_some());
            assert_eq!(queue.unwrap().len(), 1);
        }
    }

    #[test]
    fn test_event_processing() {
        let provider = EventHandlingProvider::new();

        // Register an event type
        let event_def = CustomEventDefinition {
            name: "process-test".into(),
            description: None,
            data_schema: None,
            default_priority: EventPriority::Normal,
            bubbles: false,
            cancelable: false,
        };
        provider.register_custom_event(event_def);

        // Emit some events
        for i in 0..5 {
            let event = CustomEventDetails {
                event_type: "process-test".into(),
                data: CustomEventData::Number(i as f64),
                target_id: None,
                bubbles: false,
                cancelable: false,
            };
            provider.emit_custom_event(event);
        }

        let processed = provider.process_custom_events();
        assert!(processed > 0);
    }

    #[test]
    fn test_event_validation() {
        let mut provider = EventHandlingProvider::new();

        // Add a validation rule
        let rule = EventValidationRule {
            event_type: "validated-event".into(),
            validator: EventValidator::RequiredFields(vec!["required_field".into()]),
            error_action: ValidationErrorAction::Drop,
        };
        provider.filter_config.validation_rules.push(rule);

        // Test valid event
        let valid_event = CustomEventDetails {
            event_type: "validated-event".into(),
            data: CustomEventData::Object({
                let mut data = HashMap::new();
                data.insert(
                    "required_field".into(),
                    CustomEventData::String("value".into()),
                );
                data
            }),
            target_id: None,
            bubbles: false,
            cancelable: false,
        };

        assert!(provider.validate_event(&valid_event));

        // Test invalid event
        let invalid_event = CustomEventDetails {
            event_type: "validated-event".into(),
            data: CustomEventData::Object(HashMap::new()),
            target_id: None,
            bubbles: false,
            cancelable: false,
        };

        assert!(!provider.validate_event(&invalid_event));
    }

    #[test]
    fn test_blocked_events() {
        let mut provider = EventHandlingProvider::new();
        provider
            .filter_config
            .blocked_events
            .push("blocked-event".into());

        let blocked_event = CustomEventDetails {
            event_type: "blocked-event".into(),
            data: CustomEventData::None,
            target_id: None,
            bubbles: false,
            cancelable: false,
        };

        assert!(!provider.validate_event(&blocked_event));

        let allowed_event = CustomEventDetails {
            event_type: "allowed-event".into(),
            data: CustomEventData::None,
            target_id: None,
            bubbles: false,
            cancelable: false,
        };

        assert!(provider.validate_event(&allowed_event));
    }

    #[test]
    fn test_event_statistics() {
        let provider = EventHandlingProvider::new();

        // Register some listeners
        let listener = Arc::new(|_: &KeyDownEvent| EventResult::Handled);
        provider.register_keyboard_listener("keydown", listener, EventPriority::Normal);

        let mouse_listener = Arc::new(|_: &MouseEventDetails| EventResult::Handled);
        provider.register_mouse_listener("click", mouse_listener, EventPriority::Normal);

        // Register custom event
        let event_def = CustomEventDefinition {
            name: "stats-test".into(),
            description: None,
            data_schema: None,
            default_priority: EventPriority::Normal,
            bubbles: false,
            cancelable: false,
        };
        provider.register_custom_event(event_def);

        let stats = provider.get_event_statistics();
        assert_eq!(stats.keyboard_listener_count, 1);
        assert_eq!(stats.mouse_listener_count, 1);
        assert_eq!(stats.custom_event_types, 1);
    }

    #[test]
    fn test_event_priority_ordering() {
        let provider = EventHandlingProvider::new();

        // Register events with different priorities
        for priority in [
            EventPriority::Low,
            EventPriority::Critical,
            EventPriority::Normal,
            EventPriority::High,
        ] {
            let event_def = CustomEventDefinition {
                name: format!("priority-{:?}", priority).into(),
                description: None,
                data_schema: None,
                default_priority: priority,
                bubbles: false,
                cancelable: false,
            };
            provider.register_custom_event(event_def);

            let event = CustomEventDetails {
                event_type: format!("priority-{:?}", priority).into(),
                data: CustomEventData::None,
                target_id: None,
                bubbles: false,
                cancelable: false,
            };
            provider.emit_custom_event(event);
        }

        {
            let custom_events = provider.custom_events.read().unwrap();
            // Check that events are in separate priority queues
            assert!(custom_events
                .event_queues
                .contains_key(&EventPriority::Critical));
            assert!(custom_events
                .event_queues
                .contains_key(&EventPriority::High));
            assert!(custom_events
                .event_queues
                .contains_key(&EventPriority::Normal));
            assert!(custom_events
                .event_queues
                .contains_key(&EventPriority::Low));
        }
    }

    #[test]
    fn test_data_type_validation() {
        let provider = EventHandlingProvider::new();

        // Test different data types
        assert!(provider.check_data_type(
            &CustomEventData::String("test".into()),
            EventDataType::String
        ));
        assert!(provider.check_data_type(&CustomEventData::Number(42.0), EventDataType::Number));
        assert!(provider.check_data_type(&CustomEventData::Boolean(true), EventDataType::Boolean));
        assert!(provider.check_data_type(
            &CustomEventData::Object(HashMap::new()),
            EventDataType::Object
        ));
        assert!(
            provider.check_data_type(&CustomEventData::Array(Vec::new()), EventDataType::Array)
        );

        // Test Any type
        assert!(
            provider.check_data_type(&CustomEventData::String("test".into()), EventDataType::Any)
        );
        assert!(provider.check_data_type(&CustomEventData::Number(42.0), EventDataType::Any));

        // Test mismatched types
        assert!(!provider.check_data_type(
            &CustomEventData::String("test".into()),
            EventDataType::Number
        ));
        assert!(!provider.check_data_type(&CustomEventData::Number(42.0), EventDataType::String));
    }
}
