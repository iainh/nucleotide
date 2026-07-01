// ABOUTME: Backend-aware workspace file operation handler
// ABOUTME: Routes file tree mutations through WorkspaceBackend instead of host filesystem calls

use std::path::{Path, PathBuf};

use futures_executor::block_on;
use nucleotide_core::{EventAggregatorHandle, EventBus, EventHandler};
use nucleotide_events::v2::workspace::{
    DeleteMode, Event as WorkspaceEvent, FileOpIntent, PathCopyKind,
};
use nucleotide_logging::{error, info, warn};
use nucleotide_workspace::{FileKind, FileStat, WorkspaceBackendHandle, WorkspaceError};

pub struct WorkspaceFileOpHandler {
    bus: EventAggregatorHandle,
    backend: WorkspaceBackendHandle,
}

impl WorkspaceFileOpHandler {
    pub fn new(bus: EventAggregatorHandle, backend: WorkspaceBackendHandle) -> Self {
        Self { bus, backend }
    }

    fn create_child_path(
        &self,
        parent: &Path,
        name: &str,
        operation: &'static str,
    ) -> Result<PathBuf, WorkspaceError> {
        validate_child_name(name).map_err(|message| WorkspaceError::CommandFailed {
            operation,
            path: parent.to_path_buf(),
            message,
        })?;
        Ok(parent.join(name))
    }

    fn parent_for(path: &Path) -> PathBuf {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    fn dispatch_created(&self, stat: FileStat) {
        let parent_directory = Self::parent_for(&stat.path);
        self.bus.dispatch_workspace(WorkspaceEvent::FileCreated {
            path: stat.path,
            parent_directory,
        });
    }

    fn dispatch_deleted(&self, stat: FileStat) {
        self.bus.dispatch_workspace(WorkspaceEvent::FileDeleted {
            was_directory: stat.kind == FileKind::Directory,
            path: stat.path,
        });
    }

    fn handle_new_file(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = self.create_child_path(parent, name, "create file")?;
        let stat = block_on(self.backend.create_file(&path))?;
        self.dispatch_created(stat);
        Ok(())
    }

    fn handle_new_folder(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = self.create_child_path(parent, name, "create directory")?;
        let stat = block_on(self.backend.create_dir(&path))?;
        self.dispatch_created(stat);
        Ok(())
    }

    fn handle_rename(&self, path: &Path, new_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "rename path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let new_path = self.create_child_path(parent, new_name, "rename path")?;
        let stat = block_on(self.backend.rename_path(path, &new_path))?;
        self.bus.dispatch_workspace(WorkspaceEvent::FileRenamed {
            old_path: path.to_path_buf(),
            new_path: stat.path,
        });
        Ok(())
    }

    fn handle_delete(&self, path: &Path, mode: DeleteMode) -> Result<(), WorkspaceError> {
        if mode == DeleteMode::Trash {
            warn!(
                path = %path.display(),
                "Trash delete is not implemented by workspace backends; performing permanent delete"
            );
        }
        let stat = block_on(self.backend.delete_path(path))?;
        self.dispatch_deleted(stat);
        Ok(())
    }

    fn handle_duplicate(&self, path: &Path, target_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "copy path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let target_path = self.create_child_path(parent, target_name, "copy path")?;
        let stat = block_on(self.backend.copy_path(path, &target_path))?;
        self.dispatch_created(stat);
        Ok(())
    }

    fn handle_file_op(&self, intent: &FileOpIntent) -> Result<(), WorkspaceError> {
        match intent {
            FileOpIntent::NewFile { parent, name } => self.handle_new_file(parent, name),
            FileOpIntent::NewFolder { parent, name } => self.handle_new_folder(parent, name),
            FileOpIntent::Rename { path, new_name } => self.handle_rename(path, new_name),
            FileOpIntent::Delete { path, mode } => self.handle_delete(path, *mode),
            FileOpIntent::Duplicate { path, target_name } => {
                self.handle_duplicate(path, target_name)
            }
            FileOpIntent::CopyPath { path, kind } => {
                log_path_intent("CopyPath", path, *kind);
                Ok(())
            }
            FileOpIntent::RevealInOs { path } => {
                info!(path = %path.display(), "RevealInOs intent received");
                Ok(())
            }
        }
    }
}

impl EventHandler for WorkspaceFileOpHandler {
    fn handle_workspace(&mut self, event: &WorkspaceEvent) {
        let WorkspaceEvent::FileOpRequested { intent } = event else {
            return;
        };

        if let Err(err) = self.handle_file_op(intent) {
            error!(error = %err, intent = ?intent, "Failed to perform workspace file operation");
        }
    }
}

fn validate_child_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("invalid name".to_string());
    }
    if name.contains(std::path::MAIN_SEPARATOR) || name.contains('/') || name.contains('\\') {
        return Err("name must not contain path separators".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        const ILLEGAL: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
        if name.chars().any(|ch| ILLEGAL.contains(&ch)) {
            return Err("invalid characters".to_string());
        }
        let upper = name.to_ascii_uppercase();
        const RESERVED: [&str; 8] = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1", "COM2", "LPT2"];
        if RESERVED.iter().any(|reserved| *reserved == upper) {
            return Err("reserved name".to_string());
        }
    }

    Ok(())
}

fn log_path_intent(intent: &'static str, path: &Path, kind: PathCopyKind) {
    info!(path = %path.display(), kind = ?kind, intent, "Path intent received");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_child_name_rejects_path_traversal_and_separators() {
        assert!(validate_child_name("").is_err());
        assert!(validate_child_name(".").is_err());
        assert!(validate_child_name("..").is_err());
        assert!(validate_child_name("src/main.rs").is_err());
        assert!(validate_child_name(r"src\main.rs").is_err());
    }

    #[test]
    fn validate_child_name_accepts_plain_file_names() {
        assert!(validate_child_name("main.rs").is_ok());
        assert!(validate_child_name("README.md").is_ok());
    }
}
