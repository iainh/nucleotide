// ABOUTME: Event aggregator that collects events from different crates and dispatches them
// ABOUTME: Implements the event bus pattern to break circular dependencies

use nucleotide_events::{
    EventBus, EventHandler,
    v2::{
        document::Event as DocumentEvent, editor::Event as EditorEvent, lsp::Event as LspEvent,
        ui::Event as UiEvent, vcs::Event as VcsEvent, workspace::Event as WorkspaceEvent,
    },
};
use std::sync::{Arc, Mutex};

/// V2 App-level event wrapper for the event aggregator
#[derive(Debug, Clone)]
pub enum AppEvent {
    Document(DocumentEvent),
    Editor(EditorEvent),
    Ui(UiEvent),
    Workspace(WorkspaceEvent),
    Lsp(LspEvent),
    Vcs(VcsEvent),
    Core(crate::core_event::CoreEvent),
}

/// Event aggregator that collects and dispatches events
pub struct EventAggregator {
    handlers: Arc<Mutex<Vec<Box<dyn EventHandler + Send>>>>,
    event_queue: Arc<Mutex<Vec<AppEvent>>>,
}

impl EventAggregator {
    /// Create a new event aggregator
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(Mutex::new(Vec::new())),
            event_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register an event handler
    pub fn register_handler<H>(&self, handler: H)
    where
        H: EventHandler + Send + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        handlers.push(Box::new(handler));
    }

    /// Process all queued events
    pub fn process_events(&self) {
        let events: Vec<AppEvent> = {
            let mut queue = self.event_queue.lock().unwrap();
            std::mem::take(&mut *queue)
        };

        let mut handlers = self.handlers.lock().unwrap();

        for event in events {
            for handler in handlers.iter_mut() {
                match &event {
                    AppEvent::Document(e) => handler.handle_document(e),
                    AppEvent::Editor(e) => handler.handle_editor(e),
                    AppEvent::Ui(e) => handler.handle_ui(e),
                    AppEvent::Workspace(e) => handler.handle_workspace(e),
                    AppEvent::Lsp(e) => handler.handle_lsp(e),
                    AppEvent::Vcs(e) => handler.handle_vcs(e),
                    AppEvent::Core(_) => {
                        // Core events are handled through the legacy Update system
                        // Future enhancement: Migrate core events to proper V2 domain events
                    }
                }
            }
        }
    }

    /// Queue an event for processing
    pub fn queue_event(&self, event: AppEvent) {
        let mut queue = self.event_queue.lock().unwrap();
        queue.push(event);
    }

    /// Get the number of queued events
    pub fn queued_count(&self) -> usize {
        self.event_queue.lock().unwrap().len()
    }
}

impl EventBus for EventAggregator {
    fn dispatch_document(&self, event: DocumentEvent) {
        self.queue_event(AppEvent::Document(event));
    }

    fn dispatch_editor(&self, event: EditorEvent) {
        self.queue_event(AppEvent::Editor(event));
    }

    fn dispatch_ui(&self, event: UiEvent) {
        self.queue_event(AppEvent::Ui(event));
    }

    fn dispatch_workspace(&self, event: WorkspaceEvent) {
        self.queue_event(AppEvent::Workspace(event));
    }

    fn dispatch_lsp(&self, event: LspEvent) {
        self.queue_event(AppEvent::Lsp(event));
    }

    fn dispatch_vcs(&self, event: VcsEvent) {
        self.queue_event(AppEvent::Vcs(event));
    }
}

impl Default for EventAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// A handle to the event aggregator that can be cloned and shared
#[derive(Clone)]
pub struct EventAggregatorHandle {
    inner: Arc<EventAggregator>,
}

impl EventAggregatorHandle {
    /// Create a new handle from an event aggregator
    pub fn new(aggregator: EventAggregator) -> Self {
        Self {
            inner: Arc::new(aggregator),
        }
    }

    /// Register an event handler
    pub fn register_handler<H>(&self, handler: H)
    where
        H: EventHandler + Send + 'static,
    {
        self.inner.register_handler(handler);
    }

    /// Process all queued events
    pub fn process_events(&self) {
        self.inner.process_events();
    }

    /// Get the event bus interface
    pub fn as_event_bus(&self) -> &dyn EventBus {
        &*self.inner
    }
}

impl EventBus for EventAggregatorHandle {
    fn dispatch_document(&self, event: DocumentEvent) {
        self.inner.dispatch_document(event);
    }

    fn dispatch_editor(&self, event: EditorEvent) {
        self.inner.dispatch_editor(event);
    }

    fn dispatch_ui(&self, event: UiEvent) {
        self.inner.dispatch_ui(event);
    }

    fn dispatch_workspace(&self, event: WorkspaceEvent) {
        self.inner.dispatch_workspace(event);
    }

    fn dispatch_lsp(&self, event: LspEvent) {
        self.inner.dispatch_lsp(event);
    }

    fn dispatch_vcs(&self, event: VcsEvent) {
        self.inner.dispatch_vcs(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHandler {
        document_events: Arc<Mutex<Vec<DocumentEvent>>>,
    }

    impl EventHandler for TestHandler {
        fn handle_document(&mut self, event: &DocumentEvent) {
            let mut events = self.document_events.lock().unwrap();
            events.push(event.clone());
        }
    }

    #[test]
    fn test_event_aggregation() {
        let aggregator = EventAggregator::new();
        let events = Arc::new(Mutex::new(Vec::new()));

        aggregator.register_handler(TestHandler {
            document_events: events.clone(),
        });

        aggregator.dispatch_document(DocumentEvent::ContentChanged {
            doc_id: helix_view::DocumentId::default(),
            revision: 1,
            change_summary: nucleotide_events::v2::document::ChangeType::Insert,
        });
        assert_eq!(aggregator.queued_count(), 1);

        aggregator.process_events();
        assert_eq!(aggregator.queued_count(), 0);

        let collected = events.lock().unwrap();
        assert_eq!(collected.len(), 1);
    }
}
