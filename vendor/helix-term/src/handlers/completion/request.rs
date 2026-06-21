use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use std::collections::HashSet as FxHashSet;

use arc_swap::ArcSwap;
use futures_util::Future;
use helix_core::completion::CompletionProvider;
use helix_core::syntax::config::LanguageServerFeature;
use helix_event::{cancelable_future, TaskController, TaskHandle};
use helix_lsp::lsp;
use helix_lsp::lsp::{CompletionContext, CompletionTriggerKind};
use helix_lsp::util::pos_to_lsp_pos;
use helix_stdx::rope::RopeSliceExt;
use helix_view::document::{Mode, SavePoint};
use helix_view::handlers::completion::{CompletionEvent, ResponseContext};
use helix_view::{Document, DocumentId, Editor, ViewId};
use tokio::task::JoinSet;
use tokio::time::{timeout_at, Instant};

use crate::compositor::Compositor;
use crate::config::Config;
use crate::handlers::completion::item::CompletionResponse;
use crate::handlers::completion::path::path_completion;
use crate::handlers::completion::{
    handle_response, replace_completions, show_completion, CompletionItems,
};
use crate::job::{dispatch, dispatch_blocking};
use crate::ui;
use crate::ui::editor::InsertEvent;

use super::word;

/// GPUI Integration: Direct completion request bypassing async event system
/// This function provides a synchronous interface to trigger completions directly
/// for integration with GPUI-based editors that don't use Helix's async event loop
pub fn request_completions_direct(
    editor: &mut Editor,
    compositor: &mut Compositor,
    doc_id: DocumentId,
    view_id: ViewId,
    trigger_kind: TriggerKind,
) -> Result<(), anyhow::Error> {
    // Hook for GPUI direct invocation
    log::info!("🔫🎯 GPUI_DIRECT_INVOCATION: Bypassing async event system for trigger {:?}", trigger_kind);
    
    let (view, doc) = current_ref!(editor);
    
    // Validate that we have the correct document and view
    if view_id != view_id || doc.id() != doc_id {
        log::info!("🔫15 ERROR: At point=direct_validation, message=Document/view mismatch in direct invocation");
        return Err(anyhow::anyhow!("Document/view mismatch"));
    }
    
    let text = doc.text();
    let cursor = doc.selection(view_id).primary().cursor(text.slice(..));
    
    let trigger = Trigger {
        pos: cursor,
        view: view_id,
        doc: doc_id,
        kind: trigger_kind,
    };
    
    // Get a dummy task handle for direct invocation
    let mut task_controller = TaskController::new();
    let handle = task_controller.restart();
    
    // Call GPUI-compatible completion request that bypasses ui::EditorView dependencies
    request_completions_gpui_compatible(trigger, handle, editor, compositor);
    
    Ok(())
}

/// GPUI-compatible version of request_completions that bypasses ui::EditorView dependencies
/// This version handles completion requests without relying on terminal UI components
fn request_completions_gpui_compatible(
    mut trigger: Trigger,
    handle: TaskHandle,
    editor: &mut Editor,
    compositor: &mut Compositor,
) {
    // Hook 06: Result processing start  
    log::info!("🔫06 RESULT_PROCESSING_START: request_completions_gpui_compatible called for trigger {:?}", trigger.kind);
    
    // Skip ui::EditorView checks - GPUI doesn't have terminal UI components
    log::info!("🔫🎯 GPUI_BYPASS: Skipping ui::EditorView checks for GPUI compatibility");
    
    if editor.mode != Mode::Insert {
        log::info!("🔫17 EARLY_RETURN: Not in insert mode - mode={:?}", editor.mode);
        return;
    }

    // Validation and cursor position update
    let cursor = {
        let (view, doc) = current_ref!(editor);
        let text = doc.text();
        let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
        if trigger.view != view.id || trigger.doc != doc.id() || cursor < trigger.pos {
            log::info!("🔫17 EARLY_RETURN: Trigger validation failed - cursor moved or document changed");
            return;
        }
        cursor
    };
    
    // Update trigger position and get document for LSP requests
    trigger.pos = cursor;
    let doc = doc_mut!(editor, &trigger.doc);
    
    // Get view from editor.tree using view_id to avoid borrow conflict with doc_mut
    let view = editor.tree.get(trigger.view);
    let savepoint = doc.savepoint(view);
    
    // Create the trigger text slice
    let text = doc.text();
    let trigger_text = text.slice(trigger.pos.saturating_sub(256)..trigger.pos);
    
    let mut seen_language_servers: FxHashSet<_> = FxHashSet::default();
    let language_servers: Vec<_> = doc
        .language_servers_with_feature(LanguageServerFeature::Completion)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .collect();
    let mut requests = JoinSet::new();
    
    // Hook 07: Pre-dispatch preparation
    log::info!("🔫07 PRE_DISPATCH: Preparing LSP requests to {} language servers", language_servers.len());
    
    for (priority, ls) in language_servers.iter().enumerate() {
        // Hook 03: LSP request preparation
        log::info!("🔫03 LSP_REQUEST_PREP: Preparing LSP completion request to server={}, doc={:?}", 
                   ls.name(), doc.path());
        
        let context = if trigger.kind == TriggerKind::Manual {
            lsp::CompletionContext {
                trigger_kind: lsp::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            }
        } else {
            let trigger_char =
                ls.capabilities()
                    .completion_provider
                    .as_ref()
                    .and_then(|provider| {
                        provider
                            .trigger_characters
                            .as_deref()?
                            .iter()
                            .find(|&trigger| trigger_text.ends_with(trigger))
                    });

            if trigger_char.is_some() {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: trigger_char.cloned(),
                }
            } else {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::INVOKED,
                    trigger_character: None,
                }
            }
        };
        
        // Hook 04: LSP request sent
        log::info!("🔫04 LSP_REQUEST_SENT: Spawning completion request to server={}, trigger_kind={:?}", 
                   ls.name(), context.trigger_kind);
        
        requests.spawn(request_completions_from_language_server(
            ls,
            doc,
            view.id,
            context,
            -(priority as i8),
            savepoint.clone(),
        ));
    }
    
    // Add path and word completions  
    if let Some(path_completion_request) = path_completion(
        doc.selection(view.id).clone(),
        doc,
        handle.clone(),
        savepoint.clone(),
    ) {
        requests.spawn_blocking(path_completion_request);
    }
    if let Some(word_completion_request) =
        word::completion(editor, trigger, handle.clone(), savepoint)
    {
        requests.spawn_blocking(word_completion_request);
    }

    // GPUI Integration: Skip ui::EditorView InsertEvent handling - not needed for GPUI
    log::info!("🔫🎯 GPUI_BYPASS: Skipping ui::EditorView InsertEvent for GPUI compatibility");
    log::info!("🔫🎯 JOINSET_STATUS: JoinSet has {} pending tasks before spawning async closure", requests.len());
    
    // Extract text prefix before async closure (while we have access to editor)
    let text_prefix = crate::handlers::completion::extract_completion_prefix(
        editor, 
        trigger.doc, 
        trigger.view, 
        trigger.pos
    );
    
    let handle_ = handle.clone();
    let request_completions = async move {
        log::info!("🔫🎯 ASYNC_START: Starting async completion processing with {} tasks in JoinSet", requests.len());
        let mut context = HashMap::new();
        let Some(mut response) = handle_response(&mut requests, false).await else {
            log::info!("🔫17 EARLY_RETURN: No completion response received");
            return;
        };
        
        // Hook 05: LSP response received
        log::info!("🔫05 LSP_RESPONSE_RECEIVED: Got {} completion items from provider={:?}", 
                   response.items.len(), response.provider);

        let mut items: Vec<_> = Vec::new();
        response.take_items(&mut items);
        log::info!("🔫🎯 ITEMS_EXTRACTED: Extracted {} items from response", items.len());
        context.insert(response.provider, response.context);

        // Process additional responses
        while let Some(mut response) = handle_response(&mut requests, false).await {
            log::info!("🔫05 LSP_RESPONSE_RECEIVED: Got {} completion items from provider={:?}", 
                       response.items.len(), response.provider);
            response.take_items(&mut items);
            log::info!("🔫🎯 ITEMS_ADDED: Added items, total now {} items", items.len());
            context.insert(response.provider, response.context);
        }
        
        log::info!("🔫🎯 FINAL_ITEMS_COUNT: Final items count before hook dispatch: {}", items.len());

        if items.is_empty() {
            log::info!("🔫17 EARLY_RETURN: No completion items received from any provider");
            return;
        }

        // GPUI Integration: Instead of calling show_completion with ui::EditorView,
        // we'll dispatch the completion results back to GPUI via custom hook
        log::info!("🔫🎯 GPUI_COMPLETION_READY: {} items ready for GPUI integration", items.len());
        
        // Convert completion items to a GPUI-friendly format
        // We'll send these results back via a completion hook that GPUI can register
        
        let gpui_completion_results = crate::handlers::completion::GpuiCompletionResults {
            doc_id: trigger.doc,
            view_id: trigger.view,
            trigger_pos: trigger.pos,
            trigger_kind: trigger.kind,
            items: items.clone(),
            context,
            text_prefix,
        };
        
        log::info!("🔫🎯 GPUI_HOOK_DISPATCH: Calling GPUI completion hook with {} items", 
                   gpui_completion_results.items.len());
        
        // Call the GPUI completion hook that Nucleotide can register
        // This allows GPUI to receive the completion results without modifying Helix's event system
        if let Some(hook) = crate::handlers::completion::get_gpui_completion_hook() {
            hook(gpui_completion_results);
            log::info!("🔫16 SUCCESS: GPUI completion hook called successfully");
        } else {
            log::info!("🔫16 NO_HOOK: No GPUI completion hook registered - results not forwarded");
        }
        
        log::info!("🔫16 SUCCESS: Completion processing completed successfully with {} items", items.len());
    };
    
    // Debug: Check if handle is valid before spawning
    log::info!("🔫🎯 HANDLE_STATUS: Handle is_canceled={}, spawning async closure", handle.is_canceled());
    
    // GPUI Integration: Skip cancelable_future wrapper for GPUI-compatible completions
    // The TaskController goes out of scope immediately, canceling the handle
    // GPUI manages its own lifecycle, so we don't need Helix's cancellation logic
    log::info!("🔫🎯 GPUI_SPAWN: Spawning async closure without cancelable_future wrapper");
    tokio::spawn(request_completions);
    log::info!("🔫🎯 SPAWN_RESULT: Async task spawned successfully");
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TriggerKind {
    Auto,
    TriggerChar,
    Manual,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Trigger {
    pub(super) pos: usize,
    pub(super) view: ViewId,
    pub(super) doc: DocumentId,
    pub(super) kind: TriggerKind,
}

#[derive(Debug)]
pub struct CompletionHandler {
    /// The currently active trigger which will cause a completion request after the timeout.
    trigger: Option<Trigger>,
    in_flight: Option<Trigger>,
    task_controller: TaskController,
    config: Arc<ArcSwap<Config>>,
}

impl CompletionHandler {
    pub fn new(config: Arc<ArcSwap<Config>>) -> CompletionHandler {
        Self {
            config,
            task_controller: TaskController::new(),
            trigger: None,
            in_flight: None,
        }
    }
}

impl helix_event::AsyncHook for CompletionHandler {
    type Event = CompletionEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        _old_timeout: Option<Instant>,
    ) -> Option<Instant> {
        // Hook 02: Handler processing
        let event_type = match &event {
            CompletionEvent::AutoTrigger { .. } => "AutoTrigger",
            CompletionEvent::ManualTrigger { .. } => "ManualTrigger", 
            CompletionEvent::TriggerChar { .. } => "TriggerChar",
            CompletionEvent::DeleteText { .. } => "DeleteText",
            CompletionEvent::Cancel => "Cancel",
        };
        log::info!("🔫02 HANDLER_PROCESSING: Completion handler processing event={}", event_type);
        
        if self.in_flight.is_some() && !self.task_controller.is_running() {
            self.in_flight = None;
        }
        match event {
            CompletionEvent::AutoTrigger {
                cursor: trigger_pos,
                doc,
                view,
            } => {
                // Technically it shouldn't be possible to switch views/documents in insert mode
                // but people may create weird keymaps/use the mouse so let's be extra careful.
                if self
                    .trigger
                    .or(self.in_flight)
                    .map_or(true, |trigger| trigger.doc != doc || trigger.view != view)
                {
                    self.trigger = Some(Trigger {
                        pos: trigger_pos,
                        view,
                        doc,
                        kind: TriggerKind::Auto,
                    });
                }
            }
            CompletionEvent::TriggerChar { cursor, doc, view } => {
                // immediately request completions and drop all auto completion requests
                self.task_controller.cancel();
                self.trigger = Some(Trigger {
                    pos: cursor,
                    view,
                    doc,
                    kind: TriggerKind::TriggerChar,
                });
            }
            CompletionEvent::ManualTrigger { cursor, doc, view } => {
                // immediately request completions and drop all auto completion requests
                self.trigger = Some(Trigger {
                    pos: cursor,
                    view,
                    doc,
                    kind: TriggerKind::Manual,
                });
                // stop debouncing immediately and request the completion
                self.finish_debounce();
                return None;
            }
            CompletionEvent::Cancel => {
                self.trigger = None;
                self.task_controller.cancel();
            }
            CompletionEvent::DeleteText { cursor } => {
                // if we deleted the original trigger, abort the completion
                if matches!(self.trigger.or(self.in_flight), Some(Trigger{ pos, .. }) if cursor < pos)
                {
                    self.trigger = None;
                    self.task_controller.cancel();
                }
            }
        }
        self.trigger.map(|trigger| {
            // if the current request was closed forget about it
            // otherwise immediately restart the completion request
            let timeout = if trigger.kind == TriggerKind::Auto {
                self.config.load().editor.completion_timeout
            } else {
                // we want almost instant completions for trigger chars
                // and restarting completion requests. The small timeout here mainly
                // serves to better handle cases where the completion handler
                // may fall behind (so multiple events in the channel) and macros
                Duration::from_millis(5)
            };
            Instant::now() + timeout
        })
    }

    fn finish_debounce(&mut self) {
        let trigger = self.trigger.take().expect("debounce always has a trigger");
        self.in_flight = Some(trigger);
        let handle = self.task_controller.restart();
        dispatch_blocking(move |editor, compositor| {
            request_completions(trigger, handle, editor, compositor)
        });
    }
}

fn request_completions(
    mut trigger: Trigger,
    handle: TaskHandle,
    editor: &mut Editor,
    compositor: &mut Compositor,
) {
    // Hook 06: Result processing start  
    log::info!("🔫06 RESULT_PROCESSING_START: request_completions called for trigger {:?}", trigger.kind);
    
    let (view, doc) = current_ref!(editor);

    // Hook 12: Compositor search (first check)
    log::info!("🔫12 COMPOSITOR_SEARCH: request_completions checking for ui::EditorView");
    
    // CRITICAL: This is another failure point - request_completions also fails on ui::EditorView
    let ui_check = compositor.find::<ui::EditorView>();
    if ui_check.is_none() {
        log::info!("🔫15 ERROR: At point=request_completions_find, message=ui::EditorView not found in GPUI compositor");
        return;
    }
    
    let ui = ui_check.unwrap();
    if ui.completion.is_some() || editor.mode != Mode::Insert {
        log::info!("🔫17 EARLY_RETURN: Completion exists or not in insert mode - completion_exists={}, mode={:?}", 
                   ui.completion.is_some(), editor.mode);
        return;
    }

    let text = doc.text();
    let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
    if trigger.view != view.id || trigger.doc != doc.id() || cursor < trigger.pos {
        return;
    }
    // This looks odd... Why are we not using the trigger position from the `trigger` here? Won't
    // that mean that the trigger char doesn't get send to the language server if we type fast
    // enough? Yes that is true but it's not actually a problem. The language server will resolve
    // the completion to the identifier anyway (in fact sending the later position is necessary to
    // get the right results from language servers that provide incomplete completion list). We
    // rely on the trigger offset and primary cursor matching for multi-cursor completions so this
    // is definitely necessary from our side too.
    trigger.pos = cursor;
    let doc = doc_mut!(editor, &doc.id());
    let savepoint = doc.savepoint(view);
    let text = doc.text();
    let trigger_text = text.slice(..cursor);

    let mut seen_language_servers = HashSet::new();
    let language_servers: Vec<_> = doc
        .language_servers_with_feature(LanguageServerFeature::Completion)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .collect();
    let mut requests = JoinSet::new();
    
    // Hook 07: Pre-dispatch preparation
    log::info!("🔫07 PRE_DISPATCH: Preparing LSP requests to {} language servers", language_servers.len());
    
    for (priority, ls) in language_servers.iter().enumerate() {
        // Hook 03: LSP request preparation
        log::info!("🔫03 LSP_REQUEST_PREP: Preparing LSP completion request to server={}, doc={:?}", 
                   ls.name(), doc.path());
        
        let context = if trigger.kind == TriggerKind::Manual {
            lsp::CompletionContext {
                trigger_kind: lsp::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            }
        } else {
            let trigger_char =
                ls.capabilities()
                    .completion_provider
                    .as_ref()
                    .and_then(|provider| {
                        provider
                            .trigger_characters
                            .as_deref()?
                            .iter()
                            .find(|&trigger| trigger_text.ends_with(trigger))
                    });

            if trigger_char.is_some() {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: trigger_char.cloned(),
                }
            } else {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::INVOKED,
                    trigger_character: None,
                }
            }
        };
        
        // Hook 04: LSP request sent
        log::info!("🔫04 LSP_REQUEST_SENT: Spawning completion request to server={}, trigger_kind={:?}", 
                   ls.name(), context.trigger_kind);
        
        requests.spawn(request_completions_from_language_server(
            ls,
            doc,
            view.id,
            context,
            -(priority as i8),
            savepoint.clone(),
        ));
    }
    if let Some(path_completion_request) = path_completion(
        doc.selection(view.id).clone(),
        doc,
        handle.clone(),
        savepoint.clone(),
    ) {
        requests.spawn_blocking(path_completion_request);
    }
    if let Some(word_completion_request) =
        word::completion(editor, trigger, handle.clone(), savepoint)
    {
        requests.spawn_blocking(word_completion_request);
    }

    // CRITICAL: Another failure point - ui::EditorView lookup
    log::info!("🔫12 COMPOSITOR_SEARCH: request_completions final ui::EditorView lookup");
    
    let ui_final = compositor.find::<ui::EditorView>();
    if ui_final.is_none() {
        log::info!("🔫15 ERROR: At point=request_completions_final_find, message=ui::EditorView not found for InsertEvent");
        return;
    }
    
    let ui = ui_final.unwrap();
    ui.last_insert.1.push(InsertEvent::RequestCompletion);
    let handle_ = handle.clone();
    let request_completions = async move {
        let mut context = HashMap::new();
        let Some(mut response) = handle_response(&mut requests, false).await else {
            return;
        };

        let mut items: Vec<_> = Vec::new();
        response.take_items(&mut items);
        context.insert(response.provider, response.context);
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            let Some(mut response) = timeout_at(deadline, handle_response(&mut requests, false))
                .await
                .ok()
                .flatten()
            else {
                break;
            };
            response.take_items(&mut items);
            context.insert(response.provider, response.context);
        }
        dispatch(move |editor, compositor| {
            show_completion(editor, compositor, items, context, trigger)
        })
        .await;
        if !requests.is_empty() {
            replace_completions(handle_, requests, false).await;
        }
    };
    tokio::spawn(cancelable_future(request_completions, handle));
}

fn request_completions_from_language_server(
    ls: &helix_lsp::Client,
    doc: &Document,
    view: ViewId,
    context: lsp::CompletionContext,
    priority: i8,
    savepoint: Arc<SavePoint>,
) -> impl Future<Output = CompletionResponse> {
    let provider = ls.id();
    let offset_encoding = ls.offset_encoding();
    let text = doc.text();
    let cursor = doc.selection(view).primary().cursor(text.slice(..));
    let pos = pos_to_lsp_pos(text, cursor, offset_encoding);
    let doc_id = doc.identifier();

    // it's important that this is before the async block (and that this is not an async function)
    // to ensure the request is dispatched right away before any new edit notifications
    let completion_response = ls.completion(doc_id, pos, None, context).unwrap();
    async move {
        let response: Option<lsp::CompletionResponse> = completion_response
            .await
            .inspect_err(|err| log::error!("completion request failed: {err}"))
            .ok()
            .flatten();
        let (mut items, is_incomplete) = match response {
            Some(lsp::CompletionResponse::Array(items)) => (items, false),
            Some(lsp::CompletionResponse::List(lsp::CompletionList {
                is_incomplete,
                items,
            })) => (items, is_incomplete),
            None => (Vec::new(), false),
        };
        items.sort_by(|item1, item2| {
            let sort_text1 = item1.sort_text.as_deref().unwrap_or(&item1.label);
            let sort_text2 = item2.sort_text.as_deref().unwrap_or(&item2.label);
            sort_text1.cmp(sort_text2)
        });
        CompletionResponse {
            items: CompletionItems::Lsp(items),
            context: ResponseContext {
                is_incomplete,
                priority,
                savepoint,
            },
            provider: CompletionProvider::Lsp(provider),
        }
    }
}

pub fn request_incomplete_completion_list(editor: &mut Editor, handle: TaskHandle) {
    let handler = &mut editor.handlers.completions;
    let mut requests = JoinSet::new();
    let mut savepoint = None;
    for (&provider, context) in &handler.active_completions {
        if !context.is_incomplete {
            continue;
        }
        let CompletionProvider::Lsp(ls_id) = provider else {
            log::error!("non-lsp incomplete completion lists");
            continue;
        };
        let Some(ls) = editor.language_servers.get_by_id(ls_id) else {
            continue;
        };
        let (view, doc) = current!(editor);
        let savepoint = savepoint.get_or_insert_with(|| doc.savepoint(view)).clone();
        let request = request_completions_from_language_server(
            ls,
            doc,
            view.id,
            CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS,
                trigger_character: None,
            },
            context.priority,
            savepoint,
        );
        requests.spawn(request);
    }
    if !requests.is_empty() {
        // GPUI Integration: Check if GPUI completion hook is registered
        // If so, skip the regular completion flow to avoid race condition with GPUI-compatible processing
        if crate::handlers::completion::get_gpui_completion_hook().is_some() {
            log::info!("🔫🎯 REGULAR_FLOW_SKIPPED: Skipping regular completion flow - GPUI hook registered");
        } else {
            tokio::spawn(replace_completions(handle, requests, true));
        }
    }
}
