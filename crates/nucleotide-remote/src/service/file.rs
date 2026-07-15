// ABOUTME: Streaming file reads and atomic write staging for the remote service
// ABOUTME: Enforces cancellation, byte limits, and expected-modification checks

use super::*;

pub(crate) fn v5_stream_file_chunks<R, F>(
    mut reader: R,
    mut remaining: u64,
    path: &Path,
    cancellation: &WorkspaceCancellationToken,
    mut emit: F,
) -> std::result::Result<(), RemoteError>
where
    R: Read,
    F: FnMut(Vec<u8>) -> std::result::Result<(), RemoteError>,
{
    let mut buffer = vec![0_u8; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize];
    while remaining > 0 {
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
        let read_limit = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..read_limit]).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "read file",
                path: path.to_path_buf(),
                source,
            })
        })?;
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
        if read == 0 {
            break;
        }
        remaining = remaining.saturating_sub(read as u64);
        emit(buffer[..read].to_vec())?;
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
    }
    Ok(())
}

pub(crate) fn v5_streamed_file_read_limit(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(V5_MAX_STREAMED_FILE_READ_BYTES)
        .min(V5_MAX_STREAMED_FILE_READ_BYTES)
}

#[derive(Debug)]
pub(crate) struct V5StreamingWrite {
    original_path: PathBuf,
    target_path: PathBuf,
    expected_modified: Option<SystemTime>,
    existing_permissions: Option<std::fs::Permissions>,
    temp: tempfile::NamedTempFile,
}

impl V5StreamingWrite {
    pub(crate) fn create(
        path: PathBuf,
        create_parent_dirs: bool,
        expected_modified: Option<SystemTime>,
    ) -> std::result::Result<Self, WorkspaceError> {
        if let Some(parent) = path.parent()
            && create_parent_dirs
        {
            std::fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
                operation: "create parent directories",
                path: parent.to_path_buf(),
                source,
            })?;
        }

        v5_validate_write_expected_modified(&path, expected_modified)?;
        let target_path = v5_write_target_for_path(&path)?;
        let existing_permissions = match std::fs::metadata(&target_path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
            Ok(_) => {
                return Err(WorkspaceError::NotFile { path });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(WorkspaceError::Io {
                    operation: "stat write target",
                    path: target_path.clone(),
                    source,
                });
            }
        };
        let parent = target_path.parent().ok_or_else(|| WorkspaceError::Io {
            operation: "resolve write parent",
            path: target_path.clone(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"),
        })?;
        let temp = tempfile::Builder::new()
            .prefix(".nucleotide-write-")
            .tempfile_in(parent)
            .map_err(|source| WorkspaceError::Io {
                operation: "create temporary file",
                path: parent.to_path_buf(),
                source,
            })?;

        Ok(Self {
            original_path: path,
            target_path,
            expected_modified,
            existing_permissions,
            temp,
        })
    }

    pub(crate) fn write_chunk(&mut self, bytes: &[u8]) -> std::result::Result<(), WorkspaceError> {
        self.temp
            .write_all(bytes)
            .map_err(|source| WorkspaceError::Io {
                operation: "write temporary file",
                path: self.target_path.clone(),
                source,
            })
    }

    pub(crate) fn finish(
        mut self,
        cancellation: Option<&WorkspaceCancellationToken>,
    ) -> std::result::Result<WriteResult, WorkspaceError> {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_write_error(&self.original_path));
        }
        v5_validate_write_expected_modified(&self.original_path, self.expected_modified)?;
        self.temp.flush().map_err(|source| WorkspaceError::Io {
            operation: "write temporary file",
            path: self.target_path.clone(),
            source,
        })?;
        if let Some(permissions) = self.existing_permissions {
            self.temp
                .as_file()
                .set_permissions(permissions)
                .map_err(|source| WorkspaceError::Io {
                    operation: "set temporary file permissions",
                    path: self.target_path.clone(),
                    source,
                })?;
        }
        self.temp
            .as_file()
            .sync_all()
            .map_err(|source| WorkspaceError::Io {
                operation: "sync temporary file",
                path: self.target_path.clone(),
                source,
            })?;
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_write_error(&self.original_path));
        }

        let temp_path = self.temp.into_temp_path();
        std::fs::rename(&temp_path, &self.target_path).map_err(|source| WorkspaceError::Io {
            operation: "replace file",
            path: self.target_path.clone(),
            source,
        })?;

        let metadata =
            std::fs::metadata(&self.target_path).map_err(|source| WorkspaceError::Io {
                operation: "stat written file",
                path: self.target_path,
                source,
            })?;

        Ok(WriteResult {
            path: self.original_path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

pub(crate) fn v5_cancelled_write_error(path: &Path) -> WorkspaceError {
    WorkspaceError::Cancelled {
        operation: "write file",
        path: path.to_path_buf(),
    }
}

pub(crate) fn v5_validate_write_expected_modified(
    path: &Path,
    expected_modified: Option<SystemTime>,
) -> std::result::Result<(), WorkspaceError> {
    let Some(expected_modified) = expected_modified else {
        return Ok(());
    };
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| WorkspaceError::Io {
            operation: "stat file before write",
            path: path.to_path_buf(),
            source,
        })?;
    if modified != expected_modified {
        return Err(WorkspaceError::Modified {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub(crate) fn v5_write_target_for_path(
    path: &Path,
) -> std::result::Result<PathBuf, WorkspaceError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "stat write target",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if !metadata.file_type().is_symlink() {
        return Ok(path.to_path_buf());
    }

    let target = std::fs::read_link(path).map_err(|source| WorkspaceError::Io {
        operation: "read write symlink",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or_else(|| Path::new(".")).join(target)
    })
}
