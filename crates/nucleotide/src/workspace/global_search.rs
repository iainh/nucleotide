//! Workspace text search, open-buffer matching, and result pickers.

use std::path::{Path, PathBuf};

use gpui::Context;
use helix_core::{Rope, RopeSlice};
use helix_stdx::rope::RopeSliceExt;
use nucleotide_logging::debug;
#[cfg(test)]
use nucleotide_workspace::local_workspace_backend;
use nucleotide_workspace::{
    FileKind, TextSearchEvent, TextSearchQuery, TextSearchResult, WorkspaceBackend,
    WorkspaceIdentity,
};

use super::Workspace;
use crate::types::GlobalSearchLocation;

const GLOBAL_SEARCH_RESULT_LIMIT: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GlobalSearchMatch {
    pub(super) path: PathBuf,
    pub(super) line: usize,
    pub(super) line_text: String,
}

fn compile_global_search_regex(
    query: &str,
    smart_case: bool,
) -> Result<helix_stdx::rope::Regex, String> {
    let case_insensitive = smart_case && !query.chars().any(char::is_uppercase);
    helix_stdx::rope::RegexBuilder::new()
        .syntax(
            helix_stdx::rope::Config::new()
                .case_insensitive(case_insensitive)
                .multi_line(true),
        )
        .build(query)
        .map_err(|err| err.to_string())
}

fn push_global_search_matches(
    matches: &mut Vec<GlobalSearchMatch>,
    path: &Path,
    text: RopeSlice<'_>,
    regex: &helix_stdx::rope::Regex,
    limit: usize,
) -> bool {
    for line in 0..text.len_lines() {
        let line_slice = text.line(line);
        if regex.is_match(line_slice.regex_input()) {
            matches.push(GlobalSearchMatch {
                path: path.to_path_buf(),
                line,
                line_text: line_slice.to_string().trim_end().to_string(),
            });

            if matches.len() >= limit {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
pub(super) fn global_search_matches(
    root: &Path,
    query: &str,
    smart_case: bool,
    file_picker_config: &helix_view::editor::FilePickerConfig,
    open_documents: &[(PathBuf, Rope)],
    limit: usize,
) -> Result<Vec<GlobalSearchMatch>, String> {
    let backend = local_workspace_backend();
    futures_executor::block_on(global_search_matches_with_backend_async(
        backend.as_ref(),
        root,
        query,
        smart_case,
        file_picker_config,
        open_documents,
        limit,
    ))
}

async fn global_search_matches_with_backend_async(
    backend: &(impl WorkspaceBackend + ?Sized),
    root: &Path,
    query: &str,
    smart_case: bool,
    file_picker_config: &helix_view::editor::FilePickerConfig,
    open_documents: &[(PathBuf, Rope)],
    limit: usize,
) -> Result<Vec<GlobalSearchMatch>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let regex = compile_global_search_regex(query, smart_case)
        .map_err(|err| format!("Failed to compile regex: {err}"))?;
    let mut matches = Vec::new();

    let root_stat = backend
        .stat(root)
        .await
        .map_err(|_| "Current working directory does not exist".to_string())?;
    if root_stat.kind == FileKind::File {
        return Err("Current working directory is not a directory".to_string());
    }

    let excluded_relative_paths =
        global_search_open_document_matches(&mut matches, root, open_documents, &regex, limit);

    let disk_limit = limit.saturating_sub(matches.len());
    if disk_limit == 0 {
        return Ok(matches);
    }

    let disk_matches = global_search_disk_matches_with_backend_async(
        backend,
        root.to_path_buf(),
        query.to_string(),
        smart_case,
        file_picker_config.clone(),
        excluded_relative_paths,
        disk_limit,
    )
    .await?;

    matches.extend(disk_matches);

    Ok(matches)
}

pub(super) async fn global_search_disk_matches_with_backend_async(
    backend: &(impl WorkspaceBackend + ?Sized),
    root: PathBuf,
    query: String,
    smart_case: bool,
    file_picker_config: helix_view::editor::FilePickerConfig,
    excluded_relative_paths: Vec<PathBuf>,
    limit: usize,
) -> Result<Vec<GlobalSearchMatch>, String> {
    let root_stat = backend
        .stat(&root)
        .await
        .map_err(|_| "Current working directory does not exist".to_string())?;
    if root_stat.kind == FileKind::File {
        return Err("Current working directory is not a directory".to_string());
    }

    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut disk_stream = backend
        .text_search_stream(global_search_text_query(
            &root,
            &query,
            smart_case,
            &file_picker_config,
            excluded_relative_paths,
            limit,
            backend.identity(),
        ))
        .await
        .map_err(|err| err.to_string())?;

    let mut matches = Vec::new();
    while let Some(event) = disk_stream.next().await {
        match event.map_err(|err| err.to_string())? {
            TextSearchEvent::Batch(batch) => append_global_search_text_matches(
                &mut matches,
                &root,
                TextSearchResult {
                    root: root.clone(),
                    matches: batch,
                    truncated: false,
                },
                limit,
            ),
            TextSearchEvent::Complete { .. } => break,
        }
    }
    Ok(matches)
}

fn global_search_open_document_matches(
    matches: &mut Vec<GlobalSearchMatch>,
    root: &Path,
    open_documents: &[(PathBuf, Rope)],
    regex: &helix_stdx::rope::Regex,
    limit: usize,
) -> Vec<PathBuf> {
    let mut excluded_relative_paths = Vec::new();
    for (path, doc_text) in open_documents {
        let Ok(relative_path) = path.strip_prefix(root) else {
            continue;
        };
        if relative_path.as_os_str().is_empty() {
            continue;
        }

        excluded_relative_paths.push(relative_path.to_path_buf());

        if push_global_search_matches(matches, path, doc_text.slice(..), regex, limit) {
            break;
        }
    }
    excluded_relative_paths
}

fn global_search_text_query(
    root: &Path,
    query: &str,
    smart_case: bool,
    file_picker_config: &helix_view::editor::FilePickerConfig,
    excluded_relative_paths: Vec<PathBuf>,
    limit: usize,
    workspace_identity: WorkspaceIdentity,
) -> TextSearchQuery {
    TextSearchQuery {
        root: root.to_path_buf(),
        pattern: query.to_string(),
        limit,
        smart_case,
        hidden: !file_picker_config.hidden,
        parents: file_picker_config.parents,
        ignore: file_picker_config.ignore,
        git_ignore: file_picker_config.git_ignore,
        git_global: file_picker_config.git_global,
        git_exclude: file_picker_config.git_exclude,
        follow_links: file_picker_config.follow_symlinks,
        max_depth: file_picker_config.max_depth,
        excluded_relative_paths,
        custom_ignore_filenames: global_search_custom_ignore_filenames(workspace_identity),
        ..TextSearchQuery::default()
    }
}

fn append_global_search_text_matches(
    matches: &mut Vec<GlobalSearchMatch>,
    root: &Path,
    disk_matches: TextSearchResult,
    limit: usize,
) {
    for disk_match in disk_matches.matches {
        let path = root.join(&disk_match.relative_path);
        matches.push(GlobalSearchMatch {
            path,
            line: disk_match.line_number.saturating_sub(1),
            line_text: disk_match.line_text,
        });
        if matches.len() >= limit {
            break;
        }
    }
}

pub(super) fn global_search_custom_ignore_filenames(identity: WorkspaceIdentity) -> Vec<PathBuf> {
    match identity {
        WorkspaceIdentity::Local => vec![helix_loader::config_dir().join("ignore")],
        WorkspaceIdentity::Remote(_) => Vec::new(),
    }
}

fn global_search_picker(root: &Path, matches: Vec<GlobalSearchMatch>) -> crate::picker::Picker {
    use crate::picker_view::PickerItem;
    use std::sync::Arc;

    let items = matches
        .into_iter()
        .map(|search_match| {
            let path = search_match.path;
            let label_path = path.strip_prefix(root).unwrap_or(&path);
            let label = format!("{}:{}", label_path.display(), search_match.line + 1);
            let data = GlobalSearchLocation {
                path: path.clone(),
                line: search_match.line,
            };

            PickerItem::with_sublabel_and_path(label, search_match.line_text, path, Arc::new(data))
        })
        .collect();

    crate::picker::Picker::native("Global Search", items, |_index| {
        // Selection is handled by OverlayView via GlobalSearchLocation payloads.
    })
}

impl Workspace {
    pub(super) fn handle_global_search_submitted(&mut self, query: &str, cx: &mut Context<Self>) {
        debug!(query = query, "Global search submitted");

        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_all(cx);
        });

        if query.is_empty() {
            return;
        }

        let (search_root, smart_case, file_picker_config, open_documents, workspace_backend) = {
            let core = self.core.read(cx);
            let search_root = core
                .project_directory
                .clone()
                .unwrap_or_else(helix_stdx::env::current_working_dir);
            let config = core.editor.config();
            let smart_case = config.search.smart_case;
            let file_picker_config = config.file_picker.clone();
            let open_documents = core
                .editor
                .documents
                .values()
                .filter_map(|doc| {
                    doc.path()
                        .map(std::path::Path::to_path_buf)
                        .map(|path| (path, doc.text().to_owned()))
                })
                .collect::<Vec<_>>();

            (
                search_root,
                smart_case,
                file_picker_config,
                open_documents,
                core.workspace_backend.clone(),
            )
        };

        self.core.update(cx, |core, _cx| {
            core.editor.registers.last_search_register = '/';
            let _ = core.editor.registers.push('/', query.to_string());
        });

        self.core.update(cx, |core, _cx| {
            core.editor
                .set_status(format!("Searching workspace for: {query}"));
        });

        let runtime_handle = self.handle.clone();
        let root_for_task = search_root.clone();
        let query_for_task = query.to_string();
        let query_for_result = query.to_string();
        cx.spawn(async move |this, cx| {
            // Both local backends and open-document regex scans do synchronous work.
            // Poll the entire search off the GPUI foreground executor.
            let result = match runtime_handle
                .spawn(async move {
                    global_search_matches_with_backend_async(
                        workspace_backend.as_ref(),
                        &root_for_task,
                        &query_for_task,
                        smart_case,
                        &file_picker_config,
                        &open_documents,
                        GLOBAL_SEARCH_RESULT_LIMIT,
                    )
                    .await
                })
                .await
            {
                Ok(result) => result,
                Err(err) => Err(err.to_string()),
            };
            _ = this.update(cx, |workspace, cx| {
                workspace.finish_global_search_submitted(
                    &search_root,
                    &query_for_result,
                    result,
                    cx,
                );
            });
        })
        .detach();
    }

    fn finish_global_search_submitted(
        &mut self,
        search_root: &Path,
        query: &str,
        result: Result<Vec<GlobalSearchMatch>, String>,
        cx: &mut Context<Self>,
    ) {
        let matches = match result {
            Ok(matches) => matches,
            Err(err) => {
                self.core.update(cx, |core, _cx| {
                    core.editor.set_error(err);
                });
                return;
            }
        };

        if matches.is_empty() {
            self.core.update(cx, |core, _cx| {
                core.editor.set_error(format!("Pattern not found: {query}"));
            });
            return;
        }

        let match_count = matches.len();
        let picker = global_search_picker(search_root, matches);
        self.core.update(cx, |core, cx| {
            if match_count >= GLOBAL_SEARCH_RESULT_LIMIT {
                core.editor.set_status(format!(
                    "Showing first {GLOBAL_SEARCH_RESULT_LIMIT} global search matches"
                ));
            } else {
                core.editor
                    .set_status(format!("{match_count} global search matches"));
            }
            cx.emit(crate::Update::Picker(picker));
        });
    }

    /// Start global search across files
    pub fn start_global_search(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting global search");

        let prompt = crate::prompt::Prompt::native("global-search:", "", |_| {}).with_cancel(|| {});
        self.core.update(cx, |_core, cx| {
            cx.emit(crate::Update::Prompt(prompt));
        });
    }
}
