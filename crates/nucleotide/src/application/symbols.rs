//! LSP and syntax symbol collection, fallback scanning, and picker presentation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::{
    future::{BoxFuture, FutureExt},
    stream::FuturesOrdered,
};
use helix_core::{RopeSlice, Selection, syntax};
use helix_lsp::{OffsetEncoding, lsp};
use helix_view::{DocumentId, document::from_reader};
use nucleotide_logging::{debug, warn};
use nucleotide_workspace::WorkspaceIdentity;

use super::{Application, document_lsp_identifier, lsp_location_from_location};

type SymbolItemFuture = BoxFuture<'static, anyhow::Result<Vec<NativeSymbolItem>>>;

const WORKSPACE_SYNTAX_SYMBOL_FILE_LIMIT: usize = 10_000;

const WORKSPACE_SYNTAX_SYMBOL_ITEM_LIMIT: usize = 20_000;

const REMOTE_WORKSPACE_SYMBOL_FALLBACK_UNAVAILABLE: &str =
    "Workspace symbol fallback is unavailable for remote workspaces without LSP workspace symbols";

pub(super) fn should_use_workspace_syntax_symbol_fallback(identity: &WorkspaceIdentity) -> bool {
    matches!(identity, WorkspaceIdentity::Local)
}

#[derive(Debug)]
pub(super) struct NativeSymbolItem {
    pub(super) name: String,
    pub(super) kind: &'static str,
    pub(super) container_name: Option<String>,
    pub(super) path: Option<PathBuf>,
    pub(super) line: usize,
    pub(super) target: NativeSymbolTarget,
}

#[derive(Debug)]
pub(super) enum NativeSymbolTarget {
    Lsp(crate::types::LspLocation),
    Jump(crate::types::JumpLocation),
    SyntaxFile(crate::types::SyntaxFileLocation),
}

pub(super) fn native_symbol_item_from_lsp(
    name: String,
    kind: &'static str,
    container_name: Option<String>,
    location: crate::types::LspLocation,
) -> NativeSymbolItem {
    NativeSymbolItem {
        name,
        kind,
        container_name,
        path: Some(location.path.clone()),
        line: location.range.start.line as usize + 1,
        target: NativeSymbolTarget::Lsp(location),
    }
}

fn display_symbol_kind(kind: lsp::SymbolKind) -> &'static str {
    match kind {
        lsp::SymbolKind::FILE => "file",
        lsp::SymbolKind::MODULE => "module",
        lsp::SymbolKind::NAMESPACE => "namespace",
        lsp::SymbolKind::PACKAGE => "package",
        lsp::SymbolKind::CLASS => "class",
        lsp::SymbolKind::METHOD => "method",
        lsp::SymbolKind::PROPERTY => "property",
        lsp::SymbolKind::FIELD => "field",
        lsp::SymbolKind::CONSTRUCTOR => "constructor",
        lsp::SymbolKind::ENUM => "enum",
        lsp::SymbolKind::INTERFACE => "interface",
        lsp::SymbolKind::FUNCTION => "function",
        lsp::SymbolKind::VARIABLE => "variable",
        lsp::SymbolKind::CONSTANT => "constant",
        lsp::SymbolKind::STRING => "string",
        lsp::SymbolKind::NUMBER => "number",
        lsp::SymbolKind::BOOLEAN => "boolean",
        lsp::SymbolKind::ARRAY => "array",
        lsp::SymbolKind::OBJECT => "object",
        lsp::SymbolKind::KEY => "key",
        lsp::SymbolKind::NULL => "null",
        lsp::SymbolKind::ENUM_MEMBER => "enum member",
        lsp::SymbolKind::STRUCT => "struct",
        lsp::SymbolKind::EVENT => "event",
        lsp::SymbolKind::OPERATOR => "operator",
        lsp::SymbolKind::TYPE_PARAMETER => "type parameter",
        _ => "symbol",
    }
}

pub(super) fn syntax_symbol_kind_from_capture_name(capture_name: &str) -> Option<&'static str> {
    match capture_name.strip_prefix("definition.")? {
        "class" => Some("class"),
        "constant" => Some("constant"),
        "function" => Some("function"),
        "interface" => Some("interface"),
        "macro" => Some("macro"),
        "module" => Some("module"),
        "struct" => Some("struct"),
        "type" => Some("type"),
        _ => None,
    }
}

fn symbol_location_from_path(
    path: PathBuf,
    range: lsp::Range,
    offset_encoding: OffsetEncoding,
) -> crate::types::LspLocation {
    crate::types::LspLocation {
        path,
        range,
        offset_encoding,
    }
}

fn document_symbol_items_from_response(
    response: Option<lsp::DocumentSymbolResponse>,
    path: PathBuf,
    offset_encoding: OffsetEncoding,
) -> Vec<NativeSymbolItem> {
    match response {
        Some(lsp::DocumentSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                let location = lsp_location_from_location(symbol.location, offset_encoding)?;
                Some(native_symbol_item_from_lsp(
                    symbol.name,
                    display_symbol_kind(symbol.kind),
                    symbol.container_name,
                    location,
                ))
            })
            .collect(),
        Some(lsp::DocumentSymbolResponse::Nested(symbols)) => {
            let mut items = Vec::new();
            for symbol in symbols {
                push_document_symbol_item(&mut items, symbol, &path, offset_encoding);
            }
            items
        }
        None => Vec::new(),
    }
}

fn push_document_symbol_item(
    items: &mut Vec<NativeSymbolItem>,
    symbol: lsp::DocumentSymbol,
    path: &Path,
    offset_encoding: OffsetEncoding,
) {
    let location =
        symbol_location_from_path(path.to_path_buf(), symbol.selection_range, offset_encoding);
    items.push(NativeSymbolItem {
        name: symbol.name,
        kind: display_symbol_kind(symbol.kind),
        container_name: None,
        path: Some(location.path.clone()),
        line: location.range.start.line as usize + 1,
        target: NativeSymbolTarget::Lsp(location),
    });

    for child in symbol.children.into_iter().flatten() {
        push_document_symbol_item(items, child, path, offset_encoding);
    }
}

fn workspace_symbol_items_from_response(
    response: Option<lsp::WorkspaceSymbolResponse>,
    offset_encoding: OffsetEncoding,
) -> Vec<NativeSymbolItem> {
    match response {
        Some(lsp::WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                let location = lsp_location_from_location(symbol.location, offset_encoding)?;
                Some(native_symbol_item_from_lsp(
                    symbol.name,
                    display_symbol_kind(symbol.kind),
                    symbol.container_name,
                    location,
                ))
            })
            .collect(),
        Some(lsp::WorkspaceSymbolResponse::Nested(_)) | None => Vec::new(),
    }
}

fn syntax_symbol_items_from_document(
    doc_id: DocumentId,
    doc: &helix_view::Document,
    loader: &syntax::Loader,
) -> Vec<NativeSymbolItem> {
    let Some(syntax) = doc.syntax() else {
        return Vec::new();
    };

    let text = doc.text().slice(..);
    let path = doc.path().map(std::path::Path::to_path_buf);
    syntax_symbol_items_from_text(syntax, loader, text, path.clone(), |start, end, _line| {
        NativeSymbolTarget::Jump(crate::types::JumpLocation {
            doc_id,
            selection: Selection::single(start, end),
        })
    })
}

fn syntax_symbol_items_from_text(
    syntax: &syntax::Syntax,
    loader: &syntax::Loader,
    text: RopeSlice<'_>,
    path: Option<PathBuf>,
    target_for_range: impl Fn(usize, usize, usize) -> NativeSymbolTarget,
) -> Vec<NativeSymbolItem> {
    let mut tags_iter = syntax.tags(text, loader, ..);
    let mut items = Vec::new();
    while let Some(event) = tags_iter.next() {
        let syntax::QueryMatchIterEvent::Match(mat) = event else {
            continue;
        };
        let Some(query) = loader
            .tag_query(tags_iter.current_language())
            .map(|tag_query| &tag_query.query)
        else {
            continue;
        };
        let Some((matched, kind)) = mat.nodes.iter().find_map(|matched| {
            let kind = syntax_symbol_kind_from_capture_name(query.capture_name(matched.capture))?;
            Some((matched, kind))
        }) else {
            continue;
        };

        let range = matched.node.byte_range();
        let start = text.byte_to_char(range.start as usize);
        let end = text.byte_to_char(range.end as usize);
        let start_line = text.char_to_line(start);

        items.push(NativeSymbolItem {
            name: text.slice(start..end).to_string(),
            kind,
            container_name: None,
            path: path.clone(),
            line: start_line + 1,
            target: target_for_range(start, end, start_line + 1),
        });
    }

    items
}

fn syntax_symbol_items_from_path(path: &Path, loader: &syntax::Loader) -> Vec<NativeSymbolItem> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            debug!(path = %path.display(), error = %err, "Skipping unreadable syntax symbol file");
            return Vec::new();
        }
    };
    let (rope, _encoding, _has_bom) = match from_reader(&mut file, None) {
        Ok(result) => result,
        Err(err) => {
            debug!(path = %path.display(), error = %err, "Skipping undecodable syntax symbol file");
            return Vec::new();
        }
    };
    let text = rope.slice(..);
    let Some(language) = loader
        .language_for_filename(path)
        .or_else(|| loader.language_for_shebang(text))
    else {
        return Vec::new();
    };
    let Ok(syntax) = syntax::Syntax::new(text, language, loader) else {
        return Vec::new();
    };
    let path = path.to_path_buf();
    syntax_symbol_items_from_text(
        &syntax,
        loader,
        text,
        Some(path.clone()),
        |start, end, _line| {
            NativeSymbolTarget::SyntaxFile(crate::types::SyntaxFileLocation {
                path: path.clone(),
                start,
                end,
            })
        },
    )
}

fn workspace_syntax_symbol_items_from_paths(
    search_root: PathBuf,
    file_picker_config: helix_view::editor::FilePickerConfig,
    loader: Arc<syntax::Loader>,
    open_paths: HashSet<PathBuf>,
) -> anyhow::Result<Vec<NativeSymbolItem>> {
    if !search_root.exists() {
        anyhow::bail!("Current working directory does not exist");
    }

    let absolute_root = search_root
        .canonicalize()
        .unwrap_or_else(|_| search_root.clone());
    let dedup_symlinks = file_picker_config.deduplicate_links;
    let mut walk_builder = ignore::WalkBuilder::new(&search_root);
    walk_builder
        .hidden(file_picker_config.hidden)
        .parents(file_picker_config.parents)
        .ignore(file_picker_config.ignore)
        .follow_links(file_picker_config.follow_symlinks)
        .git_ignore(file_picker_config.git_ignore)
        .git_global(file_picker_config.git_global)
        .git_exclude(file_picker_config.git_exclude)
        .max_depth(file_picker_config.max_depth)
        .filter_entry(move |entry| {
            filter_workspace_symbol_entry(entry, &absolute_root, dedup_symlinks)
        })
        .add_custom_ignore_filename(helix_loader::config_dir().join("ignore"))
        .add_custom_ignore_filename(".helix/ignore");

    let mut items = Vec::new();
    let mut files_seen = 0;
    for entry in walk_builder.build().filter_map(Result::ok) {
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if open_paths.contains(path) {
            continue;
        }

        files_seen += 1;
        if files_seen > WORKSPACE_SYNTAX_SYMBOL_FILE_LIMIT {
            warn!(
                limit = WORKSPACE_SYNTAX_SYMBOL_FILE_LIMIT,
                "Stopped workspace syntax symbol scan at file limit"
            );
            break;
        }

        items.extend(syntax_symbol_items_from_path(path, &loader));
        if items.len() >= WORKSPACE_SYNTAX_SYMBOL_ITEM_LIMIT {
            items.truncate(WORKSPACE_SYNTAX_SYMBOL_ITEM_LIMIT);
            warn!(
                limit = WORKSPACE_SYNTAX_SYMBOL_ITEM_LIMIT,
                "Stopped workspace syntax symbol scan at item limit"
            );
            break;
        }
    }

    Ok(items)
}

fn filter_workspace_symbol_entry(
    entry: &ignore::DirEntry,
    root: &Path,
    dedup_symlinks: bool,
) -> bool {
    if matches!(
        entry.file_name().to_str(),
        Some(".git" | ".pijul" | ".jj" | ".hg" | ".svn")
    ) {
        return false;
    }

    if dedup_symlinks && entry.path_is_symlink() {
        return entry
            .path()
            .canonicalize()
            .ok()
            .is_some_and(|path| !path.starts_with(root));
    }

    true
}

pub(super) fn lsp_symbol_picker(
    title: &str,
    symbols: Vec<NativeSymbolItem>,
    project_directory: Option<&Path>,
) -> crate::picker::Picker {
    use crate::picker_view::PickerItem;

    let items = symbols
        .into_iter()
        .map(|symbol| {
            let path_label = symbol
                .path
                .as_deref()
                .map(|path| {
                    project_directory
                        .and_then(|root| path.strip_prefix(root).ok())
                        .unwrap_or(path)
                        .display()
                        .to_string()
                })
                .unwrap_or_else(|| "[scratch]".to_string());
            let label = format!("{} {}", symbol.kind, symbol.name);
            let sublabel = symbol
                .container_name
                .filter(|container| !container.is_empty())
                .map(|container| format!("{container} - {path_label}:{}", symbol.line))
                .unwrap_or_else(|| format!("{path_label}:{}", symbol.line));
            let data: Arc<dyn std::any::Any + Send + Sync> = match symbol.target {
                NativeSymbolTarget::Lsp(location) => Arc::new(location),
                NativeSymbolTarget::Jump(location) => Arc::new(location),
                NativeSymbolTarget::SyntaxFile(location) => Arc::new(location),
            };

            PickerItem {
                label: label.into(),
                sublabel: Some(sublabel.into()),
                data,
                file_path: symbol.path,
                vcs_status: None,
                columns: None,
            }
        })
        .collect();

    crate::picker::Picker::native(title.to_string(), items, |_index| {
        // Symbol selection is handled by the overlay via typed item data.
    })
}

impl Application {
    pub fn trigger_lsp_symbol_picker(
        &mut self,
        workspace: bool,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let feature = if workspace {
            syntax::config::LanguageServerFeature::WorkspaceSymbols
        } else {
            syntax::config::LanguageServerFeature::DocumentSymbols
        };
        let mut futures: FuturesOrdered<SymbolItemFuture> = FuturesOrdered::new();
        let mut syntax_fallback_symbols = None;

        {
            let Some(view) = self.editor.tree.try_get(self.editor.tree.focus) else {
                self.editor.set_error("No active view for symbol picker");
                return;
            };
            let Some(doc) = self.editor.document(view.doc) else {
                self.editor
                    .set_error("No active document for symbol picker");
                return;
            };
            if !workspace && doc.syntax().is_some() {
                let loader = self.editor.syn_loader.load();
                syntax_fallback_symbols =
                    Some(syntax_symbol_items_from_document(view.doc, doc, &loader));
            }
            let doc_path = doc.path().map(std::path::Path::to_path_buf);
            let mut seen_language_servers = HashSet::new();

            for language_server in doc
                .language_servers_with_feature(feature)
                .filter(|server| seen_language_servers.insert(server.id()))
            {
                let offset_encoding = language_server.offset_encoding();
                if workspace {
                    if let Some(future) = language_server.workspace_symbols(String::new()) {
                        futures.push_back(
                            async move {
                                let response = future.await?;
                                Ok(workspace_symbol_items_from_response(
                                    response,
                                    offset_encoding,
                                ))
                            }
                            .boxed(),
                        );
                    }
                } else if let Some(path) = doc_path.clone()
                    && let Some(identifier) = document_lsp_identifier(doc)
                    && let Some(future) = language_server.document_symbols(identifier)
                {
                    futures.push_back(
                        async move {
                            let response = future.await?;
                            Ok(document_symbol_items_from_response(
                                response,
                                path,
                                offset_encoding,
                            ))
                        }
                        .boxed(),
                    );
                }
            }
        }

        if futures.is_empty() {
            if workspace {
                self.trigger_workspace_syntax_symbol_picker(cx);
            } else if let Some(symbols) = syntax_fallback_symbols {
                self.finish_native_symbol_picker("Document Symbols", symbols, cx);
            } else {
                self.editor.set_error(
                    "No language server supporting document symbols or syntax info available",
                );
            }
            return;
        }

        let title = if workspace {
            "Workspace Symbols"
        } else {
            "Document Symbols"
        }
        .to_string();

        cx.spawn(async move |core, cx| {
            let mut symbols = Vec::new();
            while let Some(response) = futures_util::StreamExt::next(&mut futures).await {
                match response {
                    Ok(mut response_symbols) => symbols.append(&mut response_symbols),
                    Err(err) => warn!(error = %err, "LSP symbol request failed"),
                }
            }

            if let Some(core) = core.upgrade() {
                core.update(cx, move |core, cx| {
                    core.finish_native_symbol_picker(&title, symbols, cx);
                });
            }
        })
        .detach();
    }

    fn trigger_workspace_syntax_symbol_picker(&mut self, cx: &mut gpui::Context<crate::Core>) {
        let Some(view) = self.editor.tree.try_get(self.editor.tree.focus) else {
            self.editor
                .set_error("No active view for workspace symbol picker");
            return;
        };
        let Some(active_doc) = self.editor.document(view.doc) else {
            self.editor
                .set_error("No active document for workspace symbol picker");
            return;
        };

        let search_root = active_doc
            .path()
            .map(|path| helix_loader::find_workspace_in(path).0)
            .or_else(|| self.project_directory.clone())
            .unwrap_or_else(|| helix_loader::find_workspace().0);
        let file_picker_config = self.editor.config().file_picker.clone();
        let loader = self.editor.syn_loader.load_full();
        let open_paths = self
            .editor
            .documents()
            .filter_map(|doc| doc.path().map(std::path::Path::to_path_buf))
            .collect::<HashSet<_>>();
        let mut open_symbols = Vec::new();
        for doc in self.editor.documents() {
            open_symbols.extend(syntax_symbol_items_from_document(doc.id(), doc, &loader));
        }

        if !should_use_workspace_syntax_symbol_fallback(&self.workspace_backend.identity()) {
            if open_symbols.is_empty() {
                self.editor
                    .set_error(REMOTE_WORKSPACE_SYMBOL_FALLBACK_UNAVAILABLE);
            } else {
                self.finish_native_symbol_picker("Workspace Symbols", open_symbols, cx);
            }
            return;
        }

        self.editor.set_status("Indexing workspace symbols...");
        cx.spawn(async move |core, cx| {
            let workspace_symbols = cx
                .background_executor()
                .spawn(async move {
                    workspace_syntax_symbol_items_from_paths(
                        search_root,
                        file_picker_config,
                        loader,
                        open_paths,
                    )
                })
                .await;

            if let Some(core) = core.upgrade() {
                core.update(cx, move |core, cx| {
                    let mut symbols = open_symbols;
                    match workspace_symbols {
                        Ok(mut workspace_symbols) => {
                            symbols.append(&mut workspace_symbols);
                            core.finish_native_symbol_picker("Workspace Symbols", symbols, cx);
                        }
                        Err(err) if symbols.is_empty() => {
                            core.editor.set_error(err.to_string());
                            cx.emit(crate::Update::Redraw);
                        }
                        Err(err) => {
                            warn!(error = %err, "Workspace syntax symbol scan failed");
                            core.finish_native_symbol_picker("Workspace Symbols", symbols, cx);
                        }
                    }
                });
            }
        })
        .detach();
    }

    fn finish_native_symbol_picker(
        &mut self,
        title: &str,
        symbols: Vec<NativeSymbolItem>,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        if symbols.is_empty() {
            self.editor.set_status("No symbols found");
        } else {
            let picker = lsp_symbol_picker(title, symbols, self.project_directory.as_deref());
            cx.emit(crate::Update::Picker(picker));
        }
        cx.emit(crate::Update::Redraw);
    }
}
