use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use helix_core::chars::char_is_word;
use helix_core::completion::CompletionProvider;
use helix_core::syntax::config::LanguageServerFeature;
use helix_event::{register_hook, TaskHandle};
use helix_lsp::lsp;
use helix_stdx::rope::RopeSliceExt;
use helix_view::document::Mode;
use helix_view::handlers::completion::{CompletionEvent, ResponseContext};
use helix_view::Editor;
use tokio::task::JoinSet;

use crate::commands;
use crate::compositor::Compositor;
use crate::events::{OnModeSwitch, PostCommand, PostInsertChar};
use crate::handlers::completion::request::{request_incomplete_completion_list, Trigger};
use crate::job::dispatch;
use crate::keymap::MappableCommand;
use crate::ui::lsp::signature_help::SignatureHelp;
use crate::ui::{self, Popup};

use super::Handlers;

// GPUI completion hook system
#[derive(Clone)]
pub struct GpuiCompletionResults {
    pub doc_id: helix_view::DocumentId,
    pub view_id: helix_view::ViewId,
    pub trigger_pos: usize,
    pub trigger_kind: request::TriggerKind,
    pub items: Vec<CompletionItem>,
    pub context: HashMap<CompletionProvider, ResponseContext>,
    pub text_prefix: String,
}

static GPUI_COMPLETION_HOOK: Mutex<Option<Arc<dyn Fn(GpuiCompletionResults) + Send + Sync>>> = Mutex::new(None);

pub fn set_gpui_completion_hook(hook: impl Fn(GpuiCompletionResults) + Send + Sync + 'static) {
    *GPUI_COMPLETION_HOOK.lock().unwrap() = Some(Arc::new(hook));
}

pub fn get_gpui_completion_hook() -> Option<Arc<dyn Fn(GpuiCompletionResults) + Send + Sync>> {
    GPUI_COMPLETION_HOOK.lock().unwrap().clone()
}

/// Extract the text prefix being completed from the document at the cursor position
pub fn extract_completion_prefix(editor: &Editor, doc_id: helix_view::DocumentId, view_id: helix_view::ViewId, pos: usize) -> String {
    use helix_core::chars::char_is_word;
    
    // Get the document and view
    let Some(doc) = editor.documents.get(&doc_id) else {
        log::info!("🔫🎯 PREFIX_EXTRACT: Document not found, returning empty prefix");
        return String::new();
    };
    
    let text = doc.text().slice(..);
    
    // Find word boundary backwards from cursor position
    let mut start_pos = pos;
    let mut chars = text.chars_at(pos);
    chars.reverse();
    
    // Move backwards while we're in a word
    for ch in chars {
        if char_is_word(ch) {
            if start_pos > 0 {
                start_pos -= ch.len_utf8();
            }
        } else {
            break;
        }
    }
    
    // Extract the prefix text
    let prefix = text.slice(start_pos..pos).to_string();
    log::info!("🔫🎯 PREFIX_EXTRACT: Extracted prefix='{}' from pos {} (start={})", prefix, pos, start_pos);
    prefix
}

pub use item::{CompletionItem, CompletionItems, CompletionResponse, LspCompletionItem};
pub use request::{CompletionHandler, TriggerKind, request_completions_direct};
pub use resolve::ResolveHandler;

// The hook functions and types are already defined above and will be exported automatically

mod item;
mod path;
mod request;
mod resolve;
mod word;

async fn handle_response(
    requests: &mut JoinSet<CompletionResponse>,
    is_incomplete: bool,
) -> Option<CompletionResponse> {
    log::info!("🔫🔍 HANDLE_RESPONSE: Starting response handling, is_incomplete={}", is_incomplete);
    
    loop {
        log::info!("🔫🔍 HANDLE_RESPONSE: Waiting for join_next()...");
        
        let join_result = requests.join_next().await;
        let Some(result) = join_result else {
            log::info!("🔫🔍 HANDLE_RESPONSE: join_next() returned None - no more tasks");
            return None;
        };
        
        let response = match result {
            Ok(response) => {
                log::info!("🔫🔍 HANDLE_RESPONSE: Got response with {} items", response.items.len());
                response
            }
            Err(e) => {
                log::info!("🔫🔍 HANDLE_RESPONSE: Task failed: {:?}", e);
                continue;
            }
        };
        
        if !is_incomplete && !response.context.is_incomplete && response.items.is_empty() {
            log::info!("🔫🔍 HANDLE_RESPONSE: Skipping empty response (incomplete={}, items={})", 
                      response.context.is_incomplete, response.items.len());
            continue;
        }
        
        log::info!("🔫🔍 HANDLE_RESPONSE: Returning response with {} items", response.items.len());
        return Some(response);
    }
}

async fn replace_completions(
    handle: TaskHandle,
    mut requests: JoinSet<CompletionResponse>,
    is_incomplete: bool,
) {
    // Hook 18: Async processing
    log::info!("🔫18 ASYNC_PROCESSING: replace_completions async stage started");
    
    while let Some(mut response) = handle_response(&mut requests, is_incomplete).await {
        // Hook 05: LSP response received
        log::info!("🔫05 LSP_RESPONSE_RECEIVED: Got {} completion items from provider={:?}", 
                   response.items.len(), response.provider);
        
        let handle = handle.clone();
        dispatch(move |editor, compositor| {
            // Hook 19: Job system callback
            log::info!("🔫19 JOB_SYSTEM: replace_completions dispatch callback");
            
            // CRITICAL: This is another failure point - ui::EditorView doesn't exist in GPUI
            let editor_view_result = compositor.find::<ui::EditorView>();
            
            if editor_view_result.is_none() {
                log::info!("🔫15 ERROR: At point=replace_completions_find, message=ui::EditorView not found in GPUI compositor");
                return;
            }
            
            let editor_view = editor_view_result.unwrap();
            
            let Some(completion) = &mut editor_view.completion else {
                log::info!("🔫17 EARLY_RETURN: No completion UI available");
                return;
            };
            
            if handle.is_canceled() {
                log::info!("🔫17 EARLY_RETURN: Handle canceled - dropping outdated completion response");
                return;
            }

            // Hook 14: UI update attempt
            log::info!("🔫14 UI_UPDATE_ATTEMPT: Attempting to update UI with {} items", response.items.len());
            
            completion.replace_provider_completions(&mut response, is_incomplete);
            if completion.is_empty() {
                editor_view.clear_completion(editor);
                // clearing completions might mean we want to immediately re-request them (usually
                // this occurs if typing a trigger char)
                trigger_auto_completion(editor, false);
            } else {
                // Hook 16: Success
                log::info!("🔫16 SUCCESS: Completion UI successfully updated with {} items", response.items.len());
                
                editor
                    .handlers
                    .completions
                    .active_completions
                    .insert(response.provider, response.context);
            }
        })
        .await;
    }
}

fn show_completion(
    editor: &mut Editor,
    compositor: &mut Compositor,
    mut items: Vec<CompletionItem>,
    context: HashMap<CompletionProvider, ResponseContext>,
    trigger: Trigger,
) {
    // Hook 08: show_completion called
    log::info!("🔫08 DISPATCH_CALLED: show_completion function called with {} items", items.len());
    
    let (view, doc) = current_ref!(editor);
    
    // Hook 10: Mode check
    log::info!("🔫10 MODE_CHECK: Editor mode={:?}, is_insert={}", editor.mode, editor.mode == Mode::Insert);
    
    // check if the completion request is stale.
    //
    // Completions are completed asynchronously and therefore the user could
    //switch document/view or leave insert mode. In all of thoise cases the
    // completion should be discarded
    if editor.mode != Mode::Insert || view.id != trigger.view || doc.id() != trigger.doc {
        // Hook 17: Early return
        log::info!("🔫17 EARLY_RETURN: Stale completion request - mode={:?}, view_match={}, doc_match={}", 
                   editor.mode, view.id == trigger.view, doc.id() == trigger.doc);
        return;
    }

    let size = compositor.size();
    
    // Hook 12: Compositor search
    log::info!("🔫12 COMPOSITOR_SEARCH: Looking for ui::EditorView in compositor");
    
    // CRITICAL: This is where GPUI integration fails - ui::EditorView doesn't exist in GPUI
    let ui_result = compositor.find::<ui::EditorView>();
    
    // Hook 13: EditorView result 
    if ui_result.is_none() {
        log::info!("🔫13 EDITORVIEW_RESULT: found=false, completion_exists=None");
        log::info!("🔫15 ERROR: At point=compositor_find, message=ui::EditorView not found in GPUI compositor");
        return;
    }
    
    let ui = ui_result.unwrap();
    log::info!("🔫13 EDITORVIEW_RESULT: found=true, completion_exists={}", ui.completion.is_some());
    
    if ui.completion.is_some() {
        log::info!("🔫17 EARLY_RETURN: Completion already exists");
        return;
    }
    word::retain_valid_completions(trigger, doc, view.id, &mut items);
    editor.handlers.completions.active_completions = context.clone();

    // Forward to GPUI completion system if hook is registered
    if let Some(hook) = get_gpui_completion_hook() {
        // Extract the text prefix being completed
        let text_prefix = extract_completion_prefix(editor, trigger.doc, trigger.view, trigger.pos);
        
        let gpui_results = GpuiCompletionResults {
            doc_id: trigger.doc,
            view_id: trigger.view,
            trigger_pos: trigger.pos,
            trigger_kind: trigger.kind,
            items: items.clone(),
            context: context.clone(),
            text_prefix,
        };
        hook(gpui_results);
    }

    let completion_area = ui.set_completion(editor, items, trigger.pos, size);
    let signature_help_area = compositor
        .find_id::<Popup<SignatureHelp>>(SignatureHelp::ID)
        .map(|signature_help| signature_help.area(size, editor));
    // Delete the signature help popup if they intersect.
    if matches!((completion_area, signature_help_area),(Some(a), Some(b)) if a.intersects(b)) {
        compositor.remove(SignatureHelp::ID);
    }
}

pub fn trigger_auto_completion(editor: &Editor, trigger_char_only: bool) {
    let config = editor.config.load();
    if !config.auto_completion {
        return;
    }
    let (view, doc): (&helix_view::View, &helix_view::Document) = current_ref!(editor);
    let mut text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    text = doc.text().slice(..cursor);

    let is_trigger_char = doc
        .language_servers_with_feature(LanguageServerFeature::Completion)
        .any(|ls| {
            matches!(&ls.capabilities().completion_provider, Some(lsp::CompletionOptions {
                        trigger_characters: Some(triggers),
                        ..
                    }) if triggers.iter().any(|trigger| text.ends_with(trigger)))
        });

    let cursor_char = text
        .get_bytes_at(text.len_bytes())
        .and_then(|t| t.reversed().next());

    #[cfg(windows)]
    let is_path_completion_trigger = matches!(cursor_char, Some(b'/' | b'\\'));
    #[cfg(not(windows))]
    let is_path_completion_trigger = matches!(cursor_char, Some(b'/'));

    let handler = &editor.handlers.completions;
    if is_trigger_char || (is_path_completion_trigger && doc.path_completion_enabled()) {
        handler.event(CompletionEvent::TriggerChar {
            cursor,
            doc: doc.id(),
            view: view.id,
        });
        return;
    }

    let is_auto_trigger = !trigger_char_only
        && doc
            .text()
            .chars_at(cursor)
            .reversed()
            .take(config.completion_trigger_len as usize)
            .all(char_is_word);

    if is_auto_trigger {
        handler.event(CompletionEvent::AutoTrigger {
            cursor,
            doc: doc.id(),
            view: view.id,
        });
    }
}

fn update_completion_filter(cx: &mut commands::Context, c: Option<char>) {
    cx.callback.push(Box::new(move |compositor, cx| {
        let editor_view = compositor.find::<ui::EditorView>().unwrap();
        if let Some(completion) = &mut editor_view.completion {
            completion.update_filter(c);
            if completion.is_empty() || c.is_some_and(|c| !char_is_word(c)) {
                editor_view.clear_completion(cx.editor);
                // clearing completions might mean we want to immediately rerequest them (usually
                // this occurs if typing a trigger char)
                if c.is_some() {
                    trigger_auto_completion(cx.editor, false);
                }
            } else {
                let handle = cx.editor.handlers.completions.request_controller.restart();
                request_incomplete_completion_list(cx.editor, handle)
            }
        }
    }))
}

fn clear_completions(cx: &mut commands::Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let editor_view = compositor.find::<ui::EditorView>().unwrap();
        editor_view.clear_completion(cx.editor);
    }))
}

fn completion_post_command_hook(
    PostCommand { command, cx }: &mut PostCommand<'_, '_>,
) -> anyhow::Result<()> {
    if cx.editor.mode == Mode::Insert {
        if cx.editor.last_completion.is_some() {
            match command {
                MappableCommand::Static {
                    name: "delete_word_forward" | "delete_char_forward" | "completion",
                    ..
                } => (),
                MappableCommand::Static {
                    name: "delete_char_backward",
                    ..
                } => update_completion_filter(cx, None),
                _ => clear_completions(cx),
            }
        } else {
            let event = match command {
                MappableCommand::Static {
                    name: "delete_char_backward" | "delete_word_forward" | "delete_char_forward",
                    ..
                } => {
                    let (view, doc) = current!(cx.editor);
                    let primary_cursor = doc
                        .selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..));
                    CompletionEvent::DeleteText {
                        cursor: primary_cursor,
                    }
                }
                // hacks: some commands are handeled elsewhere and we don't want to
                // cancel in that case
                MappableCommand::Static {
                    name: "completion" | "insert_mode" | "append_mode",
                    ..
                } => return Ok(()),
                _ => CompletionEvent::Cancel,
            };
            cx.editor.handlers.completions.event(event);
        }
    }
    Ok(())
}

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut PostCommand<'_, '_>| completion_post_command_hook(event));

    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        if event.old_mode == Mode::Insert {
            event
                .cx
                .editor
                .handlers
                .completions
                .event(CompletionEvent::Cancel);
            clear_completions(event.cx);
        } else if event.new_mode == Mode::Insert {
            trigger_auto_completion(event.cx.editor, false)
        }
        Ok(())
    });

    register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
        if event.cx.editor.last_completion.is_some() {
            update_completion_filter(event.cx, Some(event.c))
        } else {
            trigger_auto_completion(event.cx.editor, false);
        }
        Ok(())
    });
}
