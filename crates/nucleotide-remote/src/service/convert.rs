// ABOUTME: Workspace path, response, error, and timestamp conversion helpers
// ABOUTME: Keeps protocol DTO mapping separate from service scheduling and I/O

use super::*;

pub(crate) fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

pub(crate) fn path_is_within_workspace(path: &Path, workspace_root: &Path) -> bool {
    let path = normalize_path_lexically(path);
    let workspace_root = normalize_path_lexically(workspace_root);
    path == workspace_root || path.starts_with(workspace_root)
}

pub(crate) fn path_outside_workspace_error(path: &Path, workspace_root: &Path) -> RemoteError {
    RemoteError {
        code: "path_outside_workspace".to_string(),
        message: format!(
            "path {} is outside workspace root {}",
            path.display(),
            workspace_root.display()
        ),
        diagnostic: None,
    }
}

pub(crate) fn file_stat_response(stat: FileStat) -> FileStatResponse {
    FileStatResponse {
        path: stat.path,
        kind: remote_file_kind(stat.kind),
        size: stat.size,
        version: stat.version.map(FileVersion::into_bytes),
        modified_unix_millis: stat.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: stat.modified.and_then(system_time_unix_nanos),
        readonly: stat.readonly,
    }
}

pub(crate) fn directory_listing_response_with_cancellation(
    listing: DirectoryListing,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<DirectoryListingResponse> {
    let path = listing.path;
    cancellation.check_cancelled("prepare directory listing response", &path)?;
    let mut entries = Vec::with_capacity(listing.entries.len());
    for entry in listing.entries {
        cancellation.check_cancelled("prepare directory listing response", &path)?;
        entries.push(DirectoryEntryResponse {
            name: entry.name,
            path: entry.path,
            stat: file_stat_response(entry.stat),
            symlink_target: entry.symlink_target,
            target_exists: entry.target_exists,
            ignored: entry.ignored,
        });
    }
    let mut response = DirectoryListingResponse {
        path,
        generation: None,
        fingerprint: None,
        complete: true,
        not_modified: false,
        delta: None,
        entries,
    };
    let fingerprint =
        directory_listing_response_fingerprint_with_cancellation(&response, cancellation)?;
    response.generation = Some(fingerprint);
    response.fingerprint = Some(fingerprint);
    cancellation.check_cancelled("prepare directory listing response", &response.path)?;
    Ok(response)
}

pub(crate) fn annotate_directory_listing_response_metadata(
    response: &mut DirectoryListingResponse,
) {
    let fingerprint = directory_listing_response_fingerprint(response);
    response.generation = Some(fingerprint);
    response.fingerprint = Some(fingerprint);
    response.complete = true;
}

pub(crate) fn directory_listing_not_modified_response(
    mut response: DirectoryListingResponse,
) -> DirectoryListingResponse {
    annotate_directory_listing_response_metadata(&mut response);
    response.entries.clear();
    response.not_modified = true;
    response.delta = None;
    response
}

pub(crate) fn directory_listing_response_for_known_state(
    response: DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> DirectoryListingResponse {
    let generation = response.generation;
    let fingerprint = response.fingerprint;
    if known_generation.is_some() && known_generation == generation {
        return directory_listing_not_modified_response(response);
    }
    if known_fingerprint.is_some() && known_fingerprint == fingerprint {
        return directory_listing_not_modified_response(response);
    }
    response
}

pub(crate) fn directory_listing_delta_response_for_known_state(
    mut response: DirectoryListingResponse,
    previous: &DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> DirectoryListingResponse {
    if !directory_listing_state_matches(previous, known_generation, known_fingerprint) {
        return response;
    }
    let delta = directory_listing_delta_response(previous, &response);
    let delta_entry_count = delta.added.len() + delta.updated.len() + delta.removed.len();
    if delta_entry_count == 0 || delta_entry_count > response.entries.len() {
        return response;
    }
    response.entries.clear();
    response.delta = Some(delta);
    response
}

pub(crate) fn directory_listing_state_matches(
    response: &DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> bool {
    (known_generation.is_some() && response.generation == known_generation)
        || (known_fingerprint.is_some() && response.fingerprint == known_fingerprint)
}

pub(crate) fn directory_listing_delta_response(
    previous: &DirectoryListingResponse,
    current: &DirectoryListingResponse,
) -> DirectoryListingDeltaResponse {
    let previous_entries = previous
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let current_entries = current
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();

    let mut added = Vec::new();
    let mut updated = Vec::new();
    for entry in &current.entries {
        match previous_entries.get(&entry.path) {
            Some(previous_entry) if *previous_entry == entry => {}
            Some(_) => updated.push(entry.clone()),
            None => added.push(entry.clone()),
        }
    }

    let removed = previous
        .entries
        .iter()
        .filter(|entry| !current_entries.contains_key(&entry.path))
        .map(|entry| entry.path.clone())
        .collect();

    DirectoryListingDeltaResponse {
        base_generation: previous.generation,
        base_fingerprint: previous.fingerprint,
        added,
        updated,
        removed,
    }
}

pub(crate) fn apply_directory_listing_delta(
    cache_key: &Path,
    base: DirectoryListingResponse,
    mut response: DirectoryListingResponse,
    delta: DirectoryListingDeltaResponse,
) -> std::result::Result<DirectoryListingResponse, RemoteClientError> {
    if !directory_listing_state_matches(&base, delta.base_generation, delta.base_fingerprint) {
        return Err(RemoteClientError::Protocol(format!(
            "v5 directory listing delta for {} did not match the cached base",
            cache_key.display()
        )));
    }

    let mut entries = base
        .entries
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for path in delta.removed {
        entries.remove(&path);
    }
    for entry in delta.added.into_iter().chain(delta.updated) {
        entries.insert(entry.path.clone(), entry);
    }

    response.entries = entries.into_values().collect();
    sort_directory_entry_responses(&mut response.entries);
    response.not_modified = false;
    response.delta = None;
    Ok(response)
}

pub(crate) fn sort_directory_entry_responses(entries: &mut [DirectoryEntryResponse]) {
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
}

pub(crate) fn directory_listing_response_fingerprint(response: &DirectoryListingResponse) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "directory-listing-v5".hash(&mut hasher);
    response.path.hash(&mut hasher);
    response.complete.hash(&mut hasher);
    for entry in &response.entries {
        entry.name.hash(&mut hasher);
        entry.path.hash(&mut hasher);
        remote_file_kind_discriminant(&entry.stat.kind).hash(&mut hasher);
        entry.stat.path.hash(&mut hasher);
        entry.stat.size.hash(&mut hasher);
        entry.stat.version.hash(&mut hasher);
        entry.stat.modified_unix_millis.hash(&mut hasher);
        entry.stat.modified_unix_nanos.hash(&mut hasher);
        entry.stat.readonly.hash(&mut hasher);
        entry.symlink_target.hash(&mut hasher);
        entry.target_exists.hash(&mut hasher);
        entry.ignored.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn directory_listing_response_fingerprint_with_cancellation(
    response: &DirectoryListingResponse,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "directory-listing-v5".hash(&mut hasher);
    response.path.hash(&mut hasher);
    response.complete.hash(&mut hasher);
    for entry in &response.entries {
        cancellation.check_cancelled("fingerprint directory listing", &response.path)?;
        entry.name.hash(&mut hasher);
        entry.path.hash(&mut hasher);
        remote_file_kind_discriminant(&entry.stat.kind).hash(&mut hasher);
        entry.stat.path.hash(&mut hasher);
        entry.stat.size.hash(&mut hasher);
        entry.stat.version.hash(&mut hasher);
        entry.stat.modified_unix_millis.hash(&mut hasher);
        entry.stat.modified_unix_nanos.hash(&mut hasher);
        entry.stat.readonly.hash(&mut hasher);
        entry.symlink_target.hash(&mut hasher);
        entry.target_exists.hash(&mut hasher);
        entry.ignored.hash(&mut hasher);
    }
    cancellation.check_cancelled("fingerprint directory listing", &response.path)?;
    Ok(hasher.finish())
}

pub(crate) fn remote_file_kind_discriminant(kind: &RemoteFileKind) -> u8 {
    match kind {
        RemoteFileKind::File => 0,
        RemoteFileKind::Directory => 1,
        RemoteFileKind::Symlink => 2,
        RemoteFileKind::Other => 3,
    }
}

pub(crate) fn file_read_response(read: &FileRead) -> FileReadResponse {
    FileReadResponse {
        path: read.path.clone(),
        size: read.size,
        version: read
            .version
            .as_ref()
            .map(|version| version.as_bytes().to_vec()),
        modified_unix_millis: read.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: read.modified.and_then(system_time_unix_nanos),
        readonly: read.readonly,
        truncated: read.truncated,
    }
}

pub(crate) fn write_result_response(result: WriteResult) -> WriteResultResponse {
    WriteResultResponse {
        path: result.path,
        size: result.size,
        version: result.version.map(FileVersion::into_bytes),
        modified_unix_millis: result.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: result.modified.and_then(system_time_unix_nanos),
    }
}

pub(crate) fn file_search_response(result: FileSearchResult) -> FileSearchResponse {
    FileSearchResponse {
        root: result.root,
        files: result.files,
        truncated: result.truncated,
    }
}

pub(crate) fn text_search_response(result: TextSearchResult) -> TextSearchResponse {
    TextSearchResponse {
        root: result.root,
        matches: result
            .matches
            .into_iter()
            .map(|match_| TextSearchMatchResponse {
                relative_path: match_.relative_path,
                line_number: match_.line_number,
                line_text: match_.line_text,
                start: match_.start,
                end: match_.end,
            })
            .collect(),
        truncated: result.truncated,
    }
}

pub(crate) fn project_environment_response(
    snapshot: ProjectEnvironmentSnapshot,
) -> ProjectEnvironmentResponse {
    ProjectEnvironmentResponse {
        root: snapshot.root,
        variables: snapshot.variables,
        origin: remote_project_environment_origin(snapshot.origin),
        diagnostics: snapshot.diagnostics,
    }
}

pub(crate) fn git_head_response(result: GitHeadResult) -> GitHeadResponse {
    GitHeadResponse {
        root: result.root,
        head: result.head,
        display_ref: result.display_ref,
    }
}

pub(crate) fn git_status_response(result: GitStatusResult) -> GitStatusResponse {
    GitStatusResponse {
        root: result.root,
        entries: result
            .entries
            .into_iter()
            .map(|entry| GitStatusEntryResponse {
                relative_path: entry.relative_path,
                original_relative_path: entry.original_relative_path,
                index_status: remote_git_status_kind(entry.index_status),
                working_tree_status: remote_git_status_kind(entry.working_tree_status),
            })
            .collect(),
        truncated: result.truncated,
    }
}

pub(crate) fn process_output_response(output: &ProcessOutput) -> ProcessOutputResponse {
    ProcessOutputResponse {
        status_code: output.status_code,
        success: output.success,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        stdout_len: output.stdout.len(),
        stderr_len: output.stderr.len(),
        timed_out: output.timed_out,
    }
}

pub(crate) fn build_service_ignore_matcher(root_path: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(root_path);

    if let Ok(gitignore_path) = root_path.join(".gitignore").canonicalize()
        && gitignore_path.exists()
    {
        let _ = builder.add(&gitignore_path);
    }

    if let Some(git_config_dir) = dirs::config_dir() {
        let global_gitignore = git_config_dir.join("git").join("ignore");
        if global_gitignore.exists() {
            let _ = builder.add(&global_gitignore);
        }
    }

    let git_exclude = root_path.join(".git").join("info").join("exclude");
    if git_exclude.exists() {
        let _ = builder.add(&git_exclude);
    }

    let ignore_file = root_path.join(".ignore");
    if ignore_file.exists() {
        let _ = builder.add(&ignore_file);
    }

    let helix_ignore = root_path.join(".helix").join("ignore");
    if helix_ignore.exists() {
        let _ = builder.add(&helix_ignore);
    }

    builder.build().ok()
}

pub(crate) fn service_path_is_ignored(
    root_path: &Path,
    matcher: Option<&Gitignore>,
    path: &Path,
    kind: FileKind,
) -> bool {
    for component in path.components() {
        if let Component::Normal(name) = component
            && let Some(name_str) = name.to_str()
            && matches!(name_str, ".git" | ".svn" | ".hg" | ".bzr")
        {
            return true;
        }
    }

    if let Some(matcher) = matcher
        && let Ok(relative_path) = path.strip_prefix(root_path)
    {
        let matched = matcher.matched(relative_path, kind == FileKind::Directory);
        return matched.is_ignore();
    }

    false
}

pub(crate) fn annotate_directory_listing_ignored_with_cancellation(
    mut listing: DirectoryListing,
    root_path: &Path,
    matcher: Option<&Gitignore>,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<DirectoryListing> {
    cancellation.check_cancelled("annotate directory listing", &listing.path)?;
    for entry in &mut listing.entries {
        cancellation.check_cancelled("annotate directory listing", &listing.path)?;
        entry.ignored = Some(service_path_is_ignored(
            root_path,
            matcher,
            &entry.path,
            entry.stat.kind,
        ));
    }
    cancellation.check_cancelled("annotate directory listing", &listing.path)?;
    Ok(listing)
}

pub(crate) fn file_stat_from_response(stat: FileStatResponse) -> FileStat {
    FileStat {
        path: stat.path,
        kind: file_kind_from_response(stat.kind),
        size: stat.size,
        version: stat.version.map(FileVersion::from_bytes),
        modified: system_time_from_unix_millis_and_nanos(
            stat.modified_unix_millis,
            stat.modified_unix_nanos,
        ),
        readonly: stat.readonly,
    }
}

pub(crate) fn directory_listing_from_response(
    listing: DirectoryListingResponse,
) -> DirectoryListing {
    DirectoryListing {
        path: listing.path,
        entries: listing
            .entries
            .into_iter()
            .map(|entry| nucleotide_workspace::DirectoryEntry {
                name: entry.name,
                path: entry.path,
                stat: file_stat_from_response(entry.stat),
                symlink_target: entry.symlink_target,
                target_exists: entry.target_exists,
                ignored: entry.ignored,
            })
            .collect(),
    }
}

pub(crate) fn validate_file_read_body(
    read: &FileReadResponse,
    body_len: usize,
) -> std::result::Result<(), RemoteClientError> {
    let body_len_u64 = u64::try_from(body_len).unwrap_or(u64::MAX);
    if body_len_u64 > read.size {
        return Err(RemoteClientError::Protocol(format!(
            "malformed read_file body: body has {} bytes but file size is {}",
            body_len, read.size
        )));
    }
    if !read.truncated && body_len_u64 != read.size {
        return Err(RemoteClientError::Protocol(format!(
            "malformed read_file body: response is not truncated but body has {} bytes and file size is {}",
            body_len, read.size
        )));
    }
    Ok(())
}

pub(crate) fn write_result_from_response(result: WriteResultResponse) -> WriteResult {
    WriteResult {
        path: result.path,
        size: result.size,
        version: result.version.map(FileVersion::from_bytes),
        modified: system_time_from_unix_millis_and_nanos(
            result.modified_unix_millis,
            result.modified_unix_nanos,
        ),
    }
}

pub(crate) fn text_search_match_from_response(match_: TextSearchMatchResponse) -> TextSearchMatch {
    TextSearchMatch {
        relative_path: match_.relative_path,
        line_number: match_.line_number,
        line_text: match_.line_text,
        start: match_.start,
        end: match_.end,
    }
}

pub(crate) fn project_environment_from_response(
    snapshot: ProjectEnvironmentResponse,
) -> ProjectEnvironmentSnapshot {
    ProjectEnvironmentSnapshot {
        root: snapshot.root,
        variables: snapshot.variables,
        origin: project_environment_origin_from_response(snapshot.origin),
        diagnostics: snapshot.diagnostics,
    }
}

pub(crate) fn git_head_from_response(result: GitHeadResponse) -> GitHeadResult {
    GitHeadResult {
        root: result.root,
        head: result.head,
        display_ref: result.display_ref,
    }
}

pub(crate) fn git_status_from_response(result: GitStatusResponse) -> GitStatusResult {
    GitStatusResult {
        root: result.root,
        entries: result
            .entries
            .into_iter()
            .map(|entry| GitStatusEntry {
                relative_path: entry.relative_path,
                original_relative_path: entry.original_relative_path,
                index_status: git_status_kind_from_response(entry.index_status),
                working_tree_status: git_status_kind_from_response(entry.working_tree_status),
            })
            .collect(),
        truncated: result.truncated,
    }
}

pub(crate) fn process_output_from_response(
    response: ProcessOutputResponse,
    mut body: Vec<u8>,
) -> std::result::Result<ProcessOutput, RemoteClientError> {
    let expected_body_len = response
        .stdout_len
        .checked_add(response.stderr_len)
        .ok_or_else(|| {
            RemoteClientError::Protocol(
                "malformed run_process body: stdout and stderr lengths overflow".to_string(),
            )
        })?;
    if expected_body_len != body.len() {
        return Err(RemoteClientError::Protocol(format!(
            "malformed run_process body: header declares {expected_body_len} bytes but body has {} bytes",
            body.len()
        )));
    }

    let stdout_len = response.stdout_len;
    let stderr_start = stdout_len;
    let stderr_end = stderr_start + response.stderr_len;
    let stderr = body[stderr_start..stderr_end].to_vec();
    body.truncate(stdout_len);

    Ok(ProcessOutput {
        status_code: response.status_code,
        success: response.success,
        stdout: body,
        stderr,
        stdout_truncated: response.stdout_truncated,
        stderr_truncated: response.stderr_truncated,
        timed_out: response.timed_out,
    })
}

pub(crate) fn file_kind_from_response(kind: RemoteFileKind) -> FileKind {
    match kind {
        RemoteFileKind::File => FileKind::File,
        RemoteFileKind::Directory => FileKind::Directory,
        RemoteFileKind::Symlink => FileKind::Symlink,
        RemoteFileKind::Other => FileKind::Other,
    }
}

pub(crate) fn remote_file_kind(kind: FileKind) -> RemoteFileKind {
    match kind {
        FileKind::File => RemoteFileKind::File,
        FileKind::Directory => RemoteFileKind::Directory,
        FileKind::Symlink => RemoteFileKind::Symlink,
        FileKind::Other => RemoteFileKind::Other,
    }
}

pub(crate) fn remote_project_environment_origin(
    origin: ProjectEnvironmentOrigin,
) -> RemoteProjectEnvironmentOrigin {
    match origin {
        ProjectEnvironmentOrigin::NativeFlake => RemoteProjectEnvironmentOrigin::NativeFlake,
        ProjectEnvironmentOrigin::DirectoryShell => RemoteProjectEnvironmentOrigin::DirectoryShell,
        ProjectEnvironmentOrigin::ProcessBaseline => {
            RemoteProjectEnvironmentOrigin::ProcessBaseline
        }
        ProjectEnvironmentOrigin::Cli => RemoteProjectEnvironmentOrigin::Cli,
        ProjectEnvironmentOrigin::Unknown => RemoteProjectEnvironmentOrigin::Unknown,
    }
}

pub(crate) fn project_environment_origin_from_response(
    origin: RemoteProjectEnvironmentOrigin,
) -> ProjectEnvironmentOrigin {
    match origin {
        RemoteProjectEnvironmentOrigin::NativeFlake => ProjectEnvironmentOrigin::NativeFlake,
        RemoteProjectEnvironmentOrigin::DirectoryShell => ProjectEnvironmentOrigin::DirectoryShell,
        RemoteProjectEnvironmentOrigin::ProcessBaseline => {
            ProjectEnvironmentOrigin::ProcessBaseline
        }
        RemoteProjectEnvironmentOrigin::Cli => ProjectEnvironmentOrigin::Cli,
        RemoteProjectEnvironmentOrigin::Unknown => ProjectEnvironmentOrigin::Unknown,
    }
}

pub(crate) fn project_environment_origin_from_cached(
    origin: EnvironmentOrigin,
) -> ProjectEnvironmentOrigin {
    match origin {
        EnvironmentOrigin::Cli => ProjectEnvironmentOrigin::Cli,
        EnvironmentOrigin::NativeFlake => ProjectEnvironmentOrigin::NativeFlake,
        EnvironmentOrigin::DirectoryShell => ProjectEnvironmentOrigin::DirectoryShell,
        EnvironmentOrigin::Process => ProjectEnvironmentOrigin::ProcessBaseline,
    }
}

pub(crate) fn remote_git_status_kind(kind: GitStatusKind) -> RemoteGitStatusKind {
    match kind {
        GitStatusKind::Unmodified => RemoteGitStatusKind::Unmodified,
        GitStatusKind::Modified => RemoteGitStatusKind::Modified,
        GitStatusKind::Added => RemoteGitStatusKind::Added,
        GitStatusKind::Deleted => RemoteGitStatusKind::Deleted,
        GitStatusKind::Renamed => RemoteGitStatusKind::Renamed,
        GitStatusKind::Copied => RemoteGitStatusKind::Copied,
        GitStatusKind::TypeChanged => RemoteGitStatusKind::TypeChanged,
        GitStatusKind::Untracked => RemoteGitStatusKind::Untracked,
        GitStatusKind::Conflicted => RemoteGitStatusKind::Conflicted,
        GitStatusKind::Unknown => RemoteGitStatusKind::Unknown,
    }
}

pub(crate) fn git_status_kind_from_response(kind: RemoteGitStatusKind) -> GitStatusKind {
    match kind {
        RemoteGitStatusKind::Unmodified => GitStatusKind::Unmodified,
        RemoteGitStatusKind::Modified => GitStatusKind::Modified,
        RemoteGitStatusKind::Added => GitStatusKind::Added,
        RemoteGitStatusKind::Deleted => GitStatusKind::Deleted,
        RemoteGitStatusKind::Renamed => GitStatusKind::Renamed,
        RemoteGitStatusKind::Copied => GitStatusKind::Copied,
        RemoteGitStatusKind::TypeChanged => GitStatusKind::TypeChanged,
        RemoteGitStatusKind::Untracked => GitStatusKind::Untracked,
        RemoteGitStatusKind::Conflicted => GitStatusKind::Conflicted,
        RemoteGitStatusKind::Unknown => GitStatusKind::Unknown,
    }
}

pub(crate) fn remote_error_from_workspace(error: WorkspaceError) -> RemoteError {
    let code = match &error {
        WorkspaceError::Io { .. } => "io",
        WorkspaceError::Modified { .. } => "modified",
        WorkspaceError::NotFile { .. } => "not_file",
        WorkspaceError::InvalidSearchPattern(_) => "invalid_search_pattern",
        WorkspaceError::CommandFailed { .. } => "command_failed",
        WorkspaceError::Remote { .. } => "remote",
        WorkspaceError::Cancelled { .. } => protocol_v5::RESET_CANCELLED,
    };

    RemoteError {
        code: code.to_string(),
        message: error.to_string(),
        diagnostic: Some(format!("{error:?}")),
    }
}

pub(crate) fn remote_error_from_environment(error: ShellEnvironmentError) -> RemoteError {
    RemoteError {
        code: "project_environment".to_string(),
        message: error.to_string(),
        diagnostic: Some(format!("{error:?}")),
    }
}

pub(crate) fn client_error_to_workspace(
    operation: &'static str,
    path: &Path,
    error: RemoteClientError,
) -> WorkspaceError {
    match error {
        RemoteClientError::Remote(error) => remote_error_to_workspace(operation, path, error),
        RemoteClientError::Io(source) => WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        },
        other => WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: other.to_string(),
            diagnostic: Some(format!("{other:?}")),
        },
    }
}

pub(crate) fn remote_error_to_workspace(
    operation: &'static str,
    path: &Path,
    error: RemoteError,
) -> WorkspaceError {
    match error.code.as_str() {
        "modified" => WorkspaceError::Modified {
            path: path.to_path_buf(),
        },
        "not_file" => WorkspaceError::NotFile {
            path: path.to_path_buf(),
        },
        protocol_v5::RESET_CANCELLED => WorkspaceError::Cancelled {
            operation,
            path: path.to_path_buf(),
        },
        _ => WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: error.message,
            diagnostic: error.diagnostic,
        },
    }
}

pub(crate) fn unexpected_response_error(
    operation: &'static str,
    path: &Path,
    response: RemoteResponse,
) -> WorkspaceError {
    WorkspaceError::Remote {
        operation,
        path: path.to_path_buf(),
        message: format!("unexpected response: {response:?}"),
        diagnostic: None,
    }
}

pub(crate) fn system_time_unix_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

pub(crate) fn system_time_unix_nanos(time: SystemTime) -> Option<u32> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.subsec_nanos())
}

pub(crate) fn system_time_from_unix_millis(millis: i64) -> Option<SystemTime> {
    u64::try_from(millis)
        .ok()
        .map(|millis| UNIX_EPOCH + Duration::from_millis(millis))
}

pub(crate) fn system_time_from_unix_millis_and_nanos(
    millis: Option<i64>,
    nanos: Option<u32>,
) -> Option<SystemTime> {
    if let (Some(millis), Some(nanos)) = (millis, nanos)
        && nanos < 1_000_000_000
    {
        let seconds = u64::try_from(millis.div_euclid(1_000)).ok()?;
        return Some(UNIX_EPOCH + Duration::new(seconds, nanos));
    }

    millis.and_then(system_time_from_unix_millis)
}
