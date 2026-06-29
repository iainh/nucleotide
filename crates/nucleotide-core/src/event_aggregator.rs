// ABOUTME: Event aggregator that collects events from different crates and dispatches them
// ABOUTME: Implements the event bus pattern to break circular dependencies

use nucleotide_events::{
    EventBus, EventHandler,
    integration::Event as IntegrationEvent,
    v2::{
        diagnostics::Event as DiagnosticsEvent, document::Event as DocumentEvent,
        editor::Event as EditorEvent, lsp::Event as LspEvent, run::Event as RunEvent,
        terminal::Event as TerminalEvent, ui::Event as UiEvent, vcs::Event as VcsEvent,
        workspace::Event as WorkspaceEvent,
    },
};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};

/// V2 App-level event wrapper for the event aggregator
#[derive(Debug, Clone)]
pub enum AppEvent {
    Document(DocumentEvent),
    Editor(EditorEvent),
    Ui(UiEvent),
    Workspace(WorkspaceEvent),
    Lsp(LspEvent),
    Run(Box<RunEvent>),
    Terminal(TerminalEvent),
    Vcs(VcsEvent),
    Diagnostics(DiagnosticsEvent),
    Integration(IntegrationEvent),
    Core(crate::core_event::CoreEvent),
}

fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, name: &'static str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            nucleotide_logging::error!(lock = name, "Event aggregator lock poisoned; recovering");
            poisoned.into_inner()
        }
    }
}

fn app_event_kind(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::Document(_) => "document",
        AppEvent::Editor(_) => "editor",
        AppEvent::Ui(_) => "ui",
        AppEvent::Workspace(_) => "workspace",
        AppEvent::Lsp(_) => "lsp",
        AppEvent::Run(_) => "run",
        AppEvent::Terminal(_) => "terminal",
        AppEvent::Vcs(_) => "vcs",
        AppEvent::Diagnostics(_) => "diagnostics",
        AppEvent::Integration(_) => "integration",
        AppEvent::Core(_) => "core",
    }
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn dispatch_event_to_handler(handler: &mut (dyn EventHandler + Send), event: &AppEvent) {
    match event {
        AppEvent::Document(e) => handler.handle_document(e),
        AppEvent::Editor(e) => handler.handle_editor(e),
        AppEvent::Ui(e) => handler.handle_ui(e),
        AppEvent::Workspace(e) => handler.handle_workspace(e),
        AppEvent::Lsp(e) => handler.handle_lsp(e),
        AppEvent::Run(e) => handler.handle_run(e),
        AppEvent::Terminal(e) => handler.handle_terminal(e),
        AppEvent::Vcs(e) => handler.handle_vcs(e),
        AppEvent::Integration(e) => handler.handle_integration(e),
        AppEvent::Diagnostics(e) => handler.handle_diagnostics(e),
        AppEvent::Core(_) => {
            // Core events are consumed by the workspace Update path.
        }
    }
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
        let mut handlers = lock_or_recover(&self.handlers, "handlers");
        handlers.push(Box::new(handler));
    }

    /// Process all queued events
    pub fn process_events(&self) {
        let events: Vec<AppEvent> = {
            let mut queue = lock_or_recover(&self.event_queue, "event_queue");
            std::mem::take(&mut *queue)
        };

        if events.is_empty() {
            return;
        }

        let mut handlers = lock_or_recover(&self.handlers, "handlers");

        for event in events {
            for handler in handlers.iter_mut() {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    dispatch_event_to_handler(handler.as_mut(), &event);
                }));
                if let Err(payload) = result {
                    let panic_message = panic_payload_message(payload.as_ref());
                    nucleotide_logging::error!(
                        event_kind = app_event_kind(&event),
                        panic = %panic_message,
                        "Event handler panicked while processing app event"
                    );
                }
            }
        }
    }

    /// Queue an event for processing
    pub fn queue_event(&self, event: AppEvent) {
        let mut queue = lock_or_recover(&self.event_queue, "event_queue");
        queue.push(event);
    }

    /// Get the number of queued events
    pub fn queued_count(&self) -> usize {
        lock_or_recover(&self.event_queue, "event_queue").len()
    }

    /// Check whether there are events waiting to be processed
    pub fn has_queued_events(&self) -> bool {
        !lock_or_recover(&self.event_queue, "event_queue").is_empty()
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

    fn dispatch_run(&self, event: RunEvent) {
        self.queue_event(AppEvent::Run(Box::new(event)));
    }

    fn dispatch_terminal(&self, event: TerminalEvent) {
        self.queue_event(AppEvent::Terminal(event));
    }

    fn dispatch_vcs(&self, event: VcsEvent) {
        self.queue_event(AppEvent::Vcs(event));
    }

    fn dispatch_diagnostics(&self, event: DiagnosticsEvent) {
        nucleotide_logging::trace!(event = ?event, "DIAG: Aggregator queue diagnostics event");
        self.queue_event(AppEvent::Diagnostics(event));
    }

    fn dispatch_integration(&self, event: IntegrationEvent) {
        self.queue_event(AppEvent::Integration(event));
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

    /// Check whether there are events waiting to be processed
    pub fn has_queued_events(&self) -> bool {
        self.inner.has_queued_events()
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

    fn dispatch_run(&self, event: RunEvent) {
        self.inner.dispatch_run(event);
    }

    fn dispatch_terminal(&self, event: TerminalEvent) {
        self.inner.dispatch_terminal(event);
    }

    fn dispatch_vcs(&self, event: VcsEvent) {
        self.inner.dispatch_vcs(event);
    }

    fn dispatch_diagnostics(&self, event: DiagnosticsEvent) {
        nucleotide_logging::trace!(event = ?event, "DIAG: AggregatorHandle dispatch diagnostics event");
        self.inner.dispatch_diagnostics(event);
    }

    fn dispatch_integration(&self, event: IntegrationEvent) {
        self.inner.dispatch_integration(event);
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

    struct PanicHandler;

    impl EventHandler for PanicHandler {
        fn handle_document(&mut self, _event: &DocumentEvent) {
            panic!("test event handler panic");
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

    #[test]
    fn panicking_handler_does_not_stop_later_handlers() {
        let aggregator = EventAggregator::new();
        let events = Arc::new(Mutex::new(Vec::new()));

        aggregator.register_handler(PanicHandler);
        aggregator.register_handler(TestHandler {
            document_events: events.clone(),
        });

        aggregator.dispatch_document(DocumentEvent::ContentChanged {
            doc_id: helix_view::DocumentId::default(),
            revision: 1,
            change_summary: nucleotide_events::v2::document::ChangeType::Insert,
        });

        aggregator.process_events();

        let collected = events.lock().unwrap();
        assert_eq!(collected.len(), 1);
    }
}
