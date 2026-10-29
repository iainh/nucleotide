//! Completion request preparation, provider collection, and item normalization.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use futures_util::{
    future::{BoxFuture, FutureExt},
    stream::FuturesOrdered,
};
use helix_core::syntax;
use helix_lsp::{LanguageServerId, OffsetEncoding, lsp};
use helix_stdx::path::{self as helix_path, get_path_suffix};
use nucleotide_logging::instrument;
use nucleotide_workspace::{
    DirectoryListing, FileKind, WorkspaceBackendHandle, WorkspaceIdentity,
    absolutize_workspace_path, classify_workspace_location, posix_path_string,
};
use slotmap::Key;

use super::{Application, document_lsp_identifier, remote_lsp_file_url_path};

type CompletionServerResult = anyhow::Result<(
    LanguageServerId,
    OffsetEncoding,
    Option<lsp::CompletionResponse>,
)>;

pub(super) type CompletionServerFuture = BoxFuture<'static, CompletionServerResult>;

type LocalCompletionFuture = BoxFuture<'static, Vec<nucleotide_events::completion::CompletionItem>>;

type CompletionResolveFuture =
    BoxFuture<'static, anyhow::Result<nucleotide_events::completion::CompletionItem>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspCompletionTrigger {
    Manual,
    Automatic,
    Character(char),
    Incomplete,
}

pub struct PendingCompletionRequest {
    pub(super) prefix: String,
    pub(super) retained_items: Vec<nucleotide_events::completion::CompletionItem>,
    pub(super) local_items: Vec<nucleotide_events::completion::CompletionItem>,
    pub(super) local_futures: FuturesOrdered<LocalCompletionFuture>,
    pub(super) lsp_error: Option<anyhow::Error>,
    pub(super) lsp_futures: FuturesOrdered<CompletionServerFuture>,
}

impl PendingCompletionRequest {
    pub async fn collect(
        self,
    ) -> anyhow::Result<(
        Vec<nucleotide_events::completion::CompletionItem>,
        String,
        bool,
        Vec<u64>,
    )> {
        let PendingCompletionRequest {
            prefix,
            retained_items,
            mut local_items,
            mut local_futures,
            mut lsp_error,
            mut lsp_futures,
        } = self;

        let lsp_collect = async move {
            let mut lsp_items = Vec::new();
            let mut is_incomplete = false;
            let mut incomplete_server_ids = Vec::new();

            while let Some(response) = futures_util::StreamExt::next(&mut lsp_futures).await {
                match response {
                    Ok((server_id, offset_encoding, Some(lsp_response))) => {
                        let server_is_incomplete =
                            lsp_completion_response_is_incomplete(&lsp_response);
                        is_incomplete |= server_is_incomplete;
                        if server_is_incomplete {
                            incomplete_server_ids.push(server_id.data().as_ffi());
                        }
                        let mut server_items = lsp_completion_items_from_response_for_server(
                            lsp_response,
                            offset_encoding,
                            Some(server_id),
                        );
                        nucleotide_logging::info!(
                            server_id = ?server_id,
                            item_count = server_items.len(),
                            is_incomplete = server_is_incomplete,
                            "Received LSP completion items from language server"
                        );
                        lsp_items.append(&mut server_items);
                    }
                    Ok((server_id, _, None)) => {
                        nucleotide_logging::info!(
                            server_id = ?server_id,
                            "LSP server returned no completions"
                        );
                    }
                    Err(err) => {
                        nucleotide_logging::warn!(
                            error = %err,
                            "LSP completion request failed"
                        );
                        if lsp_error.is_none() {
                            lsp_error = Some(err);
                        }
                    }
                }
            }

            (lsp_items, lsp_error, is_incomplete, incomplete_server_ids)
        };

        let local_collect = async move {
            while let Some(mut items) = futures_util::StreamExt::next(&mut local_futures).await {
                local_items.append(&mut items);
            }
            local_items
        };

        let ((mut lsp_items, lsp_error, is_incomplete, incomplete_server_ids), mut local_items) =
            futures_util::future::join(lsp_collect, local_collect).await;

        let mut items = retained_items;
        items.append(&mut lsp_items);

        nucleotide_logging::debug!(
            lsp_item_count = items.len(),
            local_item_count = local_items.len(),
            incomplete_server_count = incomplete_server_ids.len(),
            prefix = %prefix,
            is_incomplete = is_incomplete,
            "Merging LSP and local completion items"
        );
        suppress_shadowed_buffer_word_completion_items(&items, &mut local_items);
        items.extend(local_items);
        dedupe_completion_items(&mut items);

        if items.is_empty()
            && let Some(err) = lsp_error
        {
            return Err(err);
        }

        Ok((items, prefix, is_incomplete, incomplete_server_ids))
    }
}

pub(super) fn lsp_completion_insert_text(item: &lsp::CompletionItem) -> String {
    item.text_edit
        .as_ref()
        .map(|edit| match edit {
            lsp::CompletionTextEdit::Edit(edit) => edit.new_text.clone(),
            lsp::CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.clone(),
        })
        .or_else(|| item.insert_text.clone())
        .unwrap_or_else(|| item.label.clone())
}

pub(super) fn lsp_completion_insert_text_format(
    item: &lsp::CompletionItem,
) -> nucleotide_events::completion::InsertTextFormat {
    if matches!(
        item.insert_text_format,
        Some(lsp::InsertTextFormat::SNIPPET)
    ) || matches!(item.kind, Some(lsp::CompletionItemKind::SNIPPET))
    {
        nucleotide_events::completion::InsertTextFormat::Snippet
    } else {
        nucleotide_events::completion::InsertTextFormat::PlainText
    }
}

pub(super) fn completion_context_for_trigger(
    trigger: LspCompletionTrigger,
    trigger_text: &str,
    advertised_trigger_characters: Option<&[String]>,
) -> lsp::CompletionContext {
    match trigger {
        LspCompletionTrigger::Manual => lsp::CompletionContext {
            trigger_kind: lsp::CompletionTriggerKind::INVOKED,
            trigger_character: None,
        },
        LspCompletionTrigger::Incomplete => lsp::CompletionContext {
            trigger_kind: lsp::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS,
            trigger_character: None,
        },
        LspCompletionTrigger::Automatic | LspCompletionTrigger::Character(_) => {
            let trigger_character =
                advertised_completion_trigger(trigger, trigger_text, advertised_trigger_characters);

            if let Some(trigger_character) = trigger_character {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: Some(trigger_character),
                }
            } else {
                lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::INVOKED,
                    trigger_character: None,
                }
            }
        }
    }
}

fn advertised_completion_trigger(
    trigger: LspCompletionTrigger,
    trigger_text: &str,
    advertised_trigger_characters: Option<&[String]>,
) -> Option<String> {
    advertised_trigger_characters?
        .iter()
        .filter(|candidate| {
            !candidate.is_empty()
                && trigger_text.ends_with(candidate.as_str())
                && match trigger {
                    LspCompletionTrigger::Character(ch) => candidate.ends_with(ch),
                    LspCompletionTrigger::Automatic => true,
                    LspCompletionTrigger::Manual | LspCompletionTrigger::Incomplete => false,
                }
        })
        .max_by_key(|candidate| candidate.len())
        .cloned()
}

pub(super) fn lsp_completion_response_is_incomplete(response: &lsp::CompletionResponse) -> bool {
    match response {
        lsp::CompletionResponse::Array(_) => false,
        lsp::CompletionResponse::List(list) => list.is_incomplete,
    }
}

pub(super) fn lsp_completion_resolve_supported(provider: Option<&lsp::CompletionOptions>) -> bool {
    provider
        .and_then(|provider| provider.resolve_provider)
        .unwrap_or(false)
}

#[cfg(test)]
pub(super) fn lsp_completion_items_from_response(
    response: lsp::CompletionResponse,
    offset_encoding: OffsetEncoding,
) -> Vec<nucleotide_events::completion::CompletionItem> {
    lsp_completion_items_from_response_for_server(response, offset_encoding, None)
}

pub(super) fn lsp_completion_items_from_response_for_server(
    response: lsp::CompletionResponse,
    offset_encoding: OffsetEncoding,
    server_id: Option<LanguageServerId>,
) -> Vec<nucleotide_events::completion::CompletionItem> {
    match response {
        lsp::CompletionResponse::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(source_index, item)| {
                lsp_completion_item(item, offset_encoding, source_index, server_id)
            })
            .collect(),
        lsp::CompletionResponse::List(list) => list
            .items
            .into_iter()
            .enumerate()
            .map(|(source_index, item)| {
                lsp_completion_item(item, offset_encoding, source_index, server_id)
            })
            .collect(),
    }
}

fn lsp_completion_item(
    item: lsp::CompletionItem,
    offset_encoding: OffsetEncoding,
    source_index: usize,
    server_id: Option<LanguageServerId>,
) -> nucleotide_events::completion::CompletionItem {
    use nucleotide_events::completion::{CompletionItem, CompletionItemKind};

    let raw_lsp_item = serde_json::to_value(&item).ok();
    let kind = match item.kind {
        Some(lsp::CompletionItemKind::TEXT) => CompletionItemKind::Text,
        Some(lsp::CompletionItemKind::METHOD) => CompletionItemKind::Method,
        Some(lsp::CompletionItemKind::FUNCTION) => CompletionItemKind::Function,
        Some(lsp::CompletionItemKind::CONSTRUCTOR) => CompletionItemKind::Constructor,
        Some(lsp::CompletionItemKind::FIELD) => CompletionItemKind::Field,
        Some(lsp::CompletionItemKind::VARIABLE) => CompletionItemKind::Variable,
        Some(lsp::CompletionItemKind::CLASS) => CompletionItemKind::Class,
        Some(lsp::CompletionItemKind::INTERFACE) => CompletionItemKind::Interface,
        Some(lsp::CompletionItemKind::MODULE) => CompletionItemKind::Module,
        Some(lsp::CompletionItemKind::PROPERTY) => CompletionItemKind::Property,
        Some(lsp::CompletionItemKind::UNIT) => CompletionItemKind::Unit,
        Some(lsp::CompletionItemKind::VALUE) => CompletionItemKind::Value,
        Some(lsp::CompletionItemKind::ENUM) => CompletionItemKind::Enum,
        Some(lsp::CompletionItemKind::KEYWORD) => CompletionItemKind::Keyword,
        Some(lsp::CompletionItemKind::SNIPPET) => CompletionItemKind::Snippet,
        Some(lsp::CompletionItemKind::COLOR) => CompletionItemKind::Color,
        Some(lsp::CompletionItemKind::FILE) => CompletionItemKind::File,
        Some(lsp::CompletionItemKind::REFERENCE) => CompletionItemKind::Reference,
        Some(lsp::CompletionItemKind::FOLDER) => CompletionItemKind::Folder,
        Some(lsp::CompletionItemKind::ENUM_MEMBER) => CompletionItemKind::EnumMember,
        Some(lsp::CompletionItemKind::CONSTANT) => CompletionItemKind::Constant,
        Some(lsp::CompletionItemKind::STRUCT) => CompletionItemKind::Struct,
        Some(lsp::CompletionItemKind::EVENT) => CompletionItemKind::Event,
        Some(lsp::CompletionItemKind::OPERATOR) => CompletionItemKind::Operator,
        Some(lsp::CompletionItemKind::TYPE_PARAMETER) => CompletionItemKind::TypeParameter,
        Some(_) | None => CompletionItemKind::Text,
    };

    let insert_text = lsp_completion_insert_text(&item);
    let insert_text_format = lsp_completion_insert_text_format(&item);
    let edit = lsp_completion_edit(&item, offset_encoding);
    let tags = lsp_completion_tags(&item);

    let signature_info = item
        .label_details
        .as_ref()
        .and_then(|details| details.detail.clone())
        .or_else(|| {
            item.detail.as_ref().and_then(|detail| {
                if detail.contains('(') && detail.contains(')') {
                    Some(detail.clone())
                } else {
                    None
                }
            })
        });

    let type_info = item
        .label_details
        .as_ref()
        .and_then(|details| details.description.clone())
        .or_else(|| {
            item.detail.as_ref().and_then(|detail| {
                if let Some(arrow_pos) = detail.find(" -> ") {
                    Some(detail[(arrow_pos + 4)..].trim().to_string())
                } else if detail.contains(':') && !detail.contains('(') {
                    detail.split(':').nth(1).map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
        });

    CompletionItem::new(item.label.clone(), kind)
        .with_insert_text(insert_text)
        .with_insert_text_format(insert_text_format)
        .with_optional_edit(edit)
        .with_sort_text(item.sort_text)
        .with_filter_text(item.filter_text)
        .with_preselect(item.preselect.unwrap_or(false))
        .with_commit_characters(item.commit_characters.unwrap_or_default())
        .with_tags(tags)
        .with_data(item.data)
        .with_source_index(source_index)
        .with_server_id(server_id.map(|server_id| server_id.data().as_ffi()))
        .with_raw_lsp_item(raw_lsp_item)
        .with_detail(item.detail.unwrap_or_default())
        .with_signature_info(signature_info.unwrap_or_default())
        .with_type_info(type_info.unwrap_or_default())
        .with_documentation(
            item.documentation
                .as_ref()
                .map(|doc| match doc {
                    lsp::Documentation::String(s) => s.clone(),
                    lsp::Documentation::MarkupContent(markup) => markup.value.clone(),
                })
                .unwrap_or_default(),
        )
}

fn lsp_completion_tags(
    item: &lsp::CompletionItem,
) -> Vec<nucleotide_events::completion::CompletionItemTag> {
    let mut tags = Vec::new();
    if item.deprecated == Some(true) {
        tags.push(nucleotide_events::completion::CompletionItemTag::Deprecated);
    }
    if let Some(item_tags) = item.tags.as_deref() {
        for tag in item_tags {
            if matches!(*tag, lsp::CompletionItemTag::DEPRECATED)
                && !tags.contains(&nucleotide_events::completion::CompletionItemTag::Deprecated)
            {
                tags.push(nucleotide_events::completion::CompletionItemTag::Deprecated);
            }
        }
    }
    tags
}

fn lsp_completion_edit(
    item: &lsp::CompletionItem,
    offset_encoding: OffsetEncoding,
) -> Option<nucleotide_events::completion::CompletionEdit> {
    let text_edit = item.text_edit.as_ref().map(|edit| match edit {
        lsp::CompletionTextEdit::Edit(edit) => event_completion_text_edit(edit),
        lsp::CompletionTextEdit::InsertAndReplace(edit) => {
            event_completion_text_edit(&lsp::TextEdit::new(edit.insert, edit.new_text.clone()))
        }
    });
    let additional_text_edits: Vec<_> = item
        .additional_text_edits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(event_completion_text_edit)
        .collect();

    if text_edit.is_none() && additional_text_edits.is_empty() {
        None
    } else {
        Some(nucleotide_events::completion::CompletionEdit {
            offset_encoding: event_completion_offset_encoding(offset_encoding),
            text_edit,
            additional_text_edits,
        })
    }
}

fn event_completion_text_edit(
    edit: &lsp::TextEdit,
) -> nucleotide_events::completion::CompletionTextEdit {
    nucleotide_events::completion::CompletionTextEdit {
        range: event_completion_range(edit.range),
        new_text: edit.new_text.clone(),
    }
}

fn event_completion_range(range: lsp::Range) -> nucleotide_events::completion::CompletionRange {
    nucleotide_events::completion::CompletionRange {
        start: event_completion_position(range.start),
        end: event_completion_position(range.end),
    }
}

fn event_completion_position(
    position: lsp::Position,
) -> nucleotide_events::completion::CompletionPosition {
    nucleotide_events::completion::CompletionPosition {
        line: position.line,
        character: position.character,
    }
}

fn event_completion_offset_encoding(
    offset_encoding: OffsetEncoding,
) -> nucleotide_events::completion::CompletionOffsetEncoding {
    match offset_encoding {
        OffsetEncoding::Utf8 => nucleotide_events::completion::CompletionOffsetEncoding::Utf8,
        OffsetEncoding::Utf16 => nucleotide_events::completion::CompletionOffsetEncoding::Utf16,
        OffsetEncoding::Utf32 => nucleotide_events::completion::CompletionOffsetEncoding::Utf32,
    }
}

const MIN_BUFFER_WORD_COMPLETION_PREFIX_CHARS: usize = 2;

const MAX_LOCAL_COMPLETION_ITEMS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalPathCompletionContext {
    pub(super) dir_path: PathBuf,
    pub(super) typed_file_name: Option<String>,
}

pub(super) fn buffer_word_completion_items(
    chars: impl IntoIterator<Item = char>,
    prefix: &str,
) -> Vec<nucleotide_events::completion::CompletionItem> {
    let prefix_len = prefix.chars().count();
    if prefix_len < MIN_BUFFER_WORD_COMPLETION_PREFIX_CHARS {
        return Vec::new();
    }

    let mut words = BTreeSet::new();
    let mut current_word = String::new();
    for ch in chars {
        if helix_core::chars::char_is_word(ch) {
            current_word.push(ch);
        } else {
            maybe_insert_buffer_word(&mut words, &current_word, prefix, prefix_len);
            current_word.clear();
        }
    }
    maybe_insert_buffer_word(&mut words, &current_word, prefix, prefix_len);

    words
        .into_iter()
        .take(MAX_LOCAL_COMPLETION_ITEMS)
        .map(|word| {
            nucleotide_events::completion::CompletionItem::new(
                word.clone(),
                nucleotide_events::completion::CompletionItemKind::Text,
            )
            .with_insert_text(word)
            .with_detail(LOCAL_BUFFER_COMPLETION_DETAIL.to_string())
        })
        .collect()
}

fn maybe_insert_buffer_word(
    words: &mut BTreeSet<String>,
    word: &str,
    prefix: &str,
    prefix_len: usize,
) {
    if word.chars().count() > prefix_len && word.starts_with(prefix) {
        words.insert(word.to_string());
    }
}

pub(super) fn local_path_completion_context(
    matched_path: &str,
    doc_path: Option<&Path>,
    workspace_identity: WorkspaceIdentity,
) -> Option<LocalPathCompletionContext> {
    let is_local_workspace = matches!(workspace_identity, WorkspaceIdentity::Local);
    if !is_local_workspace && matched_path.starts_with('~') {
        return None;
    }

    let path = if matched_path.starts_with("file://") {
        let url = lsp::Url::parse(matched_path).ok()?;
        if is_local_workspace {
            url.to_file_path().ok()?
        } else {
            remote_lsp_file_url_path(&url).or_else(|| url.to_file_path().ok())?
        }
    } else {
        PathBuf::from(matched_path)
    };

    let path = if is_local_workspace {
        helix_path::expand(&path).into_owned()
    } else {
        path
    };
    let parent_dir = doc_path.and_then(Path::parent);
    let path_is_remote = classify_workspace_location(&path).is_remote();
    let path = match parent_dir {
        Some(parent_dir) if path.is_relative() && !path_is_remote => {
            absolutize_workspace_path(parent_dir, path.as_path())
        }
        _ => path,
    };

    if path_suffix_ends_with_separator(matched_path) {
        Some(LocalPathCompletionContext {
            dir_path: path,
            typed_file_name: None,
        })
    } else {
        path.parent().map(|parent_path| LocalPathCompletionContext {
            dir_path: parent_path.to_path_buf(),
            typed_file_name: path
                .file_name()
                .and_then(|file_name| file_name.to_str().map(String::from)),
        })
    }
}

fn path_suffix_ends_with_separator(path: &str) -> bool {
    matches!(path.as_bytes().last(), Some(b'/'))
        || (cfg!(windows) && matches!(path.as_bytes().last(), Some(b'\\')))
}

pub(super) async fn path_completion_items(
    workspace_backend: WorkspaceBackendHandle,
    dir_path: PathBuf,
    typed_file_name: Option<String>,
) -> Vec<nucleotide_events::completion::CompletionItem> {
    let workspace_identity = workspace_backend.identity();
    let listing = match workspace_backend.list_dir(&dir_path).await {
        Ok(listing) => listing,
        Err(err) => {
            nucleotide_logging::debug!(
                directory = %dir_path.display(),
                error = %err,
                "Path completion directory listing failed"
            );
            return Vec::new();
        }
    };

    path_completion_items_from_listing(&listing, typed_file_name.as_deref(), workspace_identity)
}

pub(super) fn path_completion_items_from_listing(
    listing: &DirectoryListing,
    typed_file_name: Option<&str>,
    workspace_identity: WorkspaceIdentity,
) -> Vec<nucleotide_events::completion::CompletionItem> {
    let mut entries: Vec<_> = listing
        .entries
        .iter()
        .filter(|entry| {
            typed_file_name
                .map(|typed| typed.is_empty() || entry.name.starts_with(typed))
                .unwrap_or(true)
        })
        .collect();

    entries.sort_by(|left, right| {
        let left_is_dir = left.stat.kind == FileKind::Directory;
        let right_is_dir = right.stat.kind == FileKind::Directory;
        right_is_dir
            .cmp(&left_is_dir)
            .then_with(|| left.name.cmp(&right.name))
    });

    entries
        .into_iter()
        .take(MAX_LOCAL_COMPLETION_ITEMS)
        .map(|entry| {
            let is_dir = entry.stat.kind == FileKind::Directory;
            let kind = if is_dir {
                nucleotide_events::completion::CompletionItemKind::Folder
            } else {
                nucleotide_events::completion::CompletionItemKind::File
            };
            let kind_name = if is_dir { "folder" } else { "file" };
            let documentation =
                local_path_documentation(&entry.path, kind_name, workspace_identity.clone());

            nucleotide_events::completion::CompletionItem::new(entry.name.clone(), kind)
                .with_insert_text(entry.name.clone())
                .with_detail(kind_name.to_string())
                .with_documentation(documentation)
        })
        .collect()
}

fn local_path_documentation(
    full_path: &Path,
    kind: &str,
    workspace_identity: WorkspaceIdentity,
) -> String {
    let full_path = if matches!(workspace_identity, WorkspaceIdentity::Local) {
        helix_path::fold_home_dir(helix_path::canonicalize(full_path)).into_owned()
    } else {
        full_path.to_path_buf()
    };
    let full_path = if matches!(workspace_identity, WorkspaceIdentity::Local) {
        full_path.display().to_string()
    } else {
        posix_path_string(&full_path)
    };
    format!("type: `{kind}`\nfull path: `{full_path}`")
}

pub(super) fn dedupe_completion_items(
    items: &mut Vec<nucleotide_events::completion::CompletionItem>,
) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert((item.label.clone(), item.insert_text.clone())));
}

const LOCAL_BUFFER_COMPLETION_DETAIL: &str = "buffer";

pub(super) fn suppress_shadowed_buffer_word_completion_items(
    existing_items: &[nucleotide_events::completion::CompletionItem],
    local_items: &mut Vec<nucleotide_events::completion::CompletionItem>,
) {
    let shadowed_symbols: HashSet<String> = existing_items
        .iter()
        .filter(|item| !is_local_buffer_word_completion(item))
        .filter(|item| {
            !matches!(
                item.kind,
                nucleotide_events::completion::CompletionItemKind::File
                    | nucleotide_events::completion::CompletionItemKind::Folder
            )
        })
        .flat_map(completion_symbol_keys)
        .collect();

    local_items.retain(|item| {
        !is_local_buffer_word_completion(item)
            || completion_symbol_keys(item).all(|key| !shadowed_symbols.contains(&key))
    });
}

fn is_local_buffer_word_completion(item: &nucleotide_events::completion::CompletionItem) -> bool {
    item.kind == nucleotide_events::completion::CompletionItemKind::Text
        && item.detail.as_deref() == Some(LOCAL_BUFFER_COMPLETION_DETAIL)
}

fn completion_symbol_keys(
    item: &nucleotide_events::completion::CompletionItem,
) -> impl Iterator<Item = String> + '_ {
    [
        Some(item.label.as_str()),
        Some(item.insert_text.as_str()),
        item.filter_text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(completion_symbol_key)
}

fn completion_symbol_key(text: &str) -> Option<String> {
    let key: String = text
        .chars()
        .skip_while(|ch| !helix_core::chars::char_is_word(*ch))
        .take_while(|ch| helix_core::chars::char_is_word(*ch))
        .collect();

    (!key.is_empty()).then_some(key)
}

impl Application {
    /// Prepare LSP completions with prefix extraction for filtering.
    ///
    /// This snapshots the editor state needed to create LSP request futures but does not await
    /// those futures. Callers should spawn `PendingCompletionRequest::collect` and re-enter the UI
    /// when the results are ready.
    #[instrument(skip(self))]
    pub fn prepare_lsp_completions_with_prefix(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
    ) -> anyhow::Result<PendingCompletionRequest> {
        self.prepare_lsp_completions_with_prefix_for_servers(
            cursor,
            doc_id,
            view_id,
            trigger,
            None,
            Vec::new(),
        )
    }

    pub fn prepare_lsp_completions_with_prefix_for_servers(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        server_filter: Option<Vec<u64>>,
        retained_items: Vec<nucleotide_events::completion::CompletionItem>,
    ) -> anyhow::Result<PendingCompletionRequest> {
        nucleotide_logging::info!(
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            trigger = ?trigger,
            filtered_server_count = server_filter.as_ref().map_or(0, Vec::len),
            retained_item_count = retained_items.len(),
            "Preparing LSP completion request with prefix extraction"
        );

        // Extract completion prefix for filtering
        let prefix = self.extract_completion_prefix(doc_id, cursor);
        nucleotide_logging::info!(
            prefix = %prefix,
            cursor = cursor,
            doc_id = ?doc_id,
            "Extracted completion prefix for filtering"
        );

        let (local_items, local_futures) =
            self.collect_local_completion_items(cursor, doc_id, &prefix);
        let (lsp_futures, lsp_error) = match self.prepare_lsp_completion_futures(
            cursor,
            doc_id,
            view_id,
            trigger,
            server_filter.as_deref(),
        ) {
            Ok(futures) => (futures, None),
            Err(err) => {
                nucleotide_logging::warn!(
                    error = %err,
                    "LSP completion request preparation failed; falling back to local completion providers"
                );
                (FuturesOrdered::new(), Some(err))
            }
        };

        Ok(PendingCompletionRequest {
            prefix,
            retained_items,
            local_items,
            local_futures,
            lsp_error,
            lsp_futures,
        })
    }

    /// Prepare LSP completion futures for event-driven completion.
    #[instrument(skip(self))]
    fn prepare_lsp_completion_futures(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        server_filter: Option<&[u64]>,
    ) -> anyhow::Result<FuturesOrdered<CompletionServerFuture>> {
        nucleotide_logging::info!(
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            trigger = ?trigger,
            filtered_server_count = server_filter.map_or(0, |server_filter| server_filter.len()),
            "Preparing LSP completion futures for event-driven system"
        );

        // Try to get the document
        let doc = match self.editor.documents.get(&doc_id) {
            Some(doc) => doc,
            None => {
                nucleotide_logging::error!(doc_id = ?doc_id, "Document not found for completion request");
                return Err(anyhow::anyhow!("Document not found"));
            }
        };

        // Get view to check if it exists
        let view = match self.editor.tree.try_get(view_id) {
            Some(v) => v,
            None => {
                nucleotide_logging::error!(view_id = ?view_id, "View not found for completion request");
                return Err(anyhow::anyhow!("View not found"));
            }
        };
        if view.doc != doc_id {
            nucleotide_logging::error!(
                doc_id = ?doc_id,
                view_id = ?view_id,
                view_doc = ?view.doc,
                "View document mismatch"
            );
            return Err(anyhow::anyhow!("View document mismatch"));
        }

        let language_servers: Vec<_> = doc
            .language_servers_with_feature(syntax::config::LanguageServerFeature::Completion)
            .filter(|language_server| {
                server_filter.is_none_or(|server_filter| {
                    server_filter.contains(&language_server.id().data().as_ffi())
                })
            })
            .collect();
        if language_servers.is_empty() {
            nucleotide_logging::warn!(
                doc_id = ?doc_id,
                path = ?doc.path(),
                "No completion-capable language servers available"
            );
            return Err(anyhow::anyhow!(
                "No completion-capable language servers available"
            ));
        }

        // Helix selections/cursors are character positions; LSP conversion handles
        // the server's negotiated wire encoding below.
        let text = doc.text();
        let cursor_pos = cursor.min(text.len_chars());

        let trigger_text = text.slice(..cursor_pos).to_string();

        let Some(doc_id_lsp) = document_lsp_identifier(doc) else {
            nucleotide_logging::warn!(
                doc_id = ?doc_id,
                "Skipping LSP completion for document without file URL"
            );
            return Err(anyhow::anyhow!(
                "Cannot request LSP completions for an untitled document"
            ));
        };
        let server_count = language_servers.len();
        let mut lsp_futures = FuturesOrdered::new();

        for language_server in language_servers {
            let offset_encoding = language_server.offset_encoding();
            let position = helix_lsp::util::pos_to_lsp_pos(text, cursor_pos, offset_encoding);
            let completion_context = completion_context_for_trigger(
                trigger,
                &trigger_text,
                language_server
                    .capabilities()
                    .completion_provider
                    .as_ref()
                    .and_then(|provider| provider.trigger_characters.as_deref()),
            );

            nucleotide_logging::debug!(
                cursor_chars = cursor_pos,
                line = position.line,
                character = position.character,
                offset_encoding = ?offset_encoding,
                server_id = ?language_server.id(),
                server_count = server_count,
                trigger_kind = ?completion_context.trigger_kind,
                trigger_character = ?completion_context.trigger_character,
                "Requesting completions from language server"
            );

            nucleotide_logging::info!(
                line = position.line,
                character = position.character,
                offset_encoding = ?offset_encoding,
                server_id = ?language_server.id(),
                "Preparing LSP completion request"
            );

            crate::lsp_traffic_logger::log_outgoing(
                language_server.id(),
                language_server.name(),
                "textDocument/completion",
                &serde_json::json!({
                    "textDocument": doc_id_lsp.clone(),
                    "position": {"line": position.line, "character": position.character},
                    "context": {
                        "triggerKind": completion_context.trigger_kind,
                        "triggerCharacter": completion_context.trigger_character
                    }
                }),
            );

            let Some(completion_future) = language_server.completion(
                doc_id_lsp.clone(),
                position,
                None,
                completion_context.clone(),
            ) else {
                nucleotide_logging::warn!(
                    server_id = ?language_server.id(),
                    "Language server does not support completions"
                );
                continue;
            };

            let server_id = language_server.id();
            lsp_futures.push_back(
                async move {
                    completion_future
                        .await
                        .map(|response| (server_id, offset_encoding, response))
                        .map_err(Into::into)
                }
                .boxed(),
            );
        }

        Ok(lsp_futures)
    }

    pub fn prepare_lsp_completion_resolve(
        &mut self,
        server_id: LanguageServerId,
        completion_item: lsp::CompletionItem,
        source_index: usize,
    ) -> anyhow::Result<Option<CompletionResolveFuture>> {
        let Some(language_server) = self.editor.language_server_by_id(server_id) else {
            nucleotide_logging::warn!(
                server_id = ?server_id,
                "Cannot resolve completion item because the language server is unavailable"
            );
            return Err(anyhow::anyhow!("Language server not found"));
        };

        if !lsp_completion_resolve_supported(
            language_server.capabilities().completion_provider.as_ref(),
        ) {
            nucleotide_logging::debug!(
                server_id = ?server_id,
                "Skipping completion resolve because the language server does not advertise it"
            );
            return Ok(None);
        }

        crate::lsp_traffic_logger::log_outgoing(
            language_server.id(),
            language_server.name(),
            "completionItem/resolve",
            &serde_json::to_value(&completion_item).unwrap_or(serde_json::Value::Null),
        );

        let offset_encoding = language_server.offset_encoding();
        let resolve_future = language_server.resolve_completion_item(&completion_item);

        Ok(Some(
            async move {
                let resolved_item = resolve_future.await?;
                Ok(lsp_completion_item(
                    resolved_item,
                    offset_encoding,
                    source_index,
                    Some(server_id),
                ))
            }
            .boxed(),
        ))
    }

    /// Request LSP completions synchronously with prefix extraction for filtering.
    ///
    /// Tests and legacy call sites should prefer `prepare_lsp_completions_with_prefix`, which
    /// avoids blocking the UI thread while LSP servers respond.
    #[cfg(test)]
    #[instrument(skip(self))]
    pub async fn request_lsp_completions_with_prefix_for_test(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> anyhow::Result<(Vec<nucleotide_events::completion::CompletionItem>, String)> {
        let (items, prefix, _, _) = self
            .prepare_lsp_completions_with_prefix(
                cursor,
                doc_id,
                view_id,
                LspCompletionTrigger::Manual,
            )?
            .collect()
            .await?;
        Ok((items, prefix))
    }

    fn collect_local_completion_items(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        prefix: &str,
    ) -> (
        Vec<nucleotide_events::completion::CompletionItem>,
        FuturesOrdered<LocalCompletionFuture>,
    ) {
        let items = self.collect_buffer_word_completion_items(doc_id, prefix);
        let mut futures = FuturesOrdered::new();
        if let Some(path_completion_future) = self.collect_path_completion_items(cursor, doc_id) {
            futures.push_back(path_completion_future);
        }
        (items, futures)
    }

    fn collect_buffer_word_completion_items(
        &self,
        doc_id: helix_view::DocumentId,
        prefix: &str,
    ) -> Vec<nucleotide_events::completion::CompletionItem> {
        let Some(doc) = self.editor.documents.get(&doc_id) else {
            return Vec::new();
        };

        buffer_word_completion_items(doc.text().slice(..).chars(), prefix)
    }

    fn collect_path_completion_items(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
    ) -> Option<LocalCompletionFuture> {
        let doc = self.editor.documents.get(&doc_id)?;

        let text = doc.text();
        let cursor_pos = cursor.min(text.len_chars());
        let current_line = text.char_to_line(cursor_pos);
        let start = text
            .line_to_char(current_line)
            .max(cursor_pos.saturating_sub(1000));
        let line_until_cursor = text.slice(start..cursor_pos);
        let matched_path = get_path_suffix(line_until_cursor, false)?;

        let matched_path = String::from(matched_path);
        let workspace_backend = self.workspace_backend.clone();
        let context =
            local_path_completion_context(&matched_path, doc.path(), workspace_backend.identity())?;

        Some(
            async move {
                path_completion_items(workspace_backend, context.dir_path, context.typed_file_name)
                    .await
            }
            .boxed(),
        )
    }

    /// Extract the current word prefix at the cursor position for completion filtering
    /// This implements the same logic as Helix for consistency
    pub(super) fn extract_completion_prefix(
        &self,
        doc_id: helix_view::DocumentId,
        cursor: usize,
    ) -> String {
        if let Some(doc) = self.editor.documents.get(&doc_id) {
            let text = doc.text();
            let cursor_pos = cursor.min(text.len_chars());
            let text_len = text.len_chars();

            nucleotide_logging::debug!(
                doc_id = ?doc_id,
                cursor = cursor,
                cursor_pos = cursor_pos,
                text_len = text_len,
                "Starting prefix extraction"
            );

            // Walk backwards from cursor while characters are word characters
            let offset = text
                .chars_at(cursor_pos)
                .reversed()
                .take_while(|ch| helix_core::chars::char_is_word(*ch))
                .count();

            let start_offset = cursor_pos.saturating_sub(offset);
            let fragment = text.slice(start_offset..cursor_pos);
            let prefix = String::from(fragment);

            // Log the context around the cursor for debugging
            let context_start = cursor_pos.saturating_sub(20);
            let context_end = std::cmp::min(cursor_pos + 10, text_len);
            let context = text.slice(context_start..context_end);

            nucleotide_logging::info!(
                doc_id = ?doc_id,
                cursor_chars = cursor_pos,
                start_offset = start_offset,
                offset = offset,
                prefix = %prefix,
                context = %String::from(context),
                context_range = ?(context_start, context_end),
                "Extracted completion prefix"
            );

            prefix
        } else {
            nucleotide_logging::warn!(
                doc_id = ?doc_id,
                cursor = cursor,
                "Document not found for prefix extraction, returning empty prefix"
            );
            String::new()
        }
    }
}
