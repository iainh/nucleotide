// ABOUTME: Backend-aware workspace file operation handler
// ABOUTME: Routes file tree mutations through WorkspaceBackend instead of host filesystem calls

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use nucleotide_core::{EventAggregatorHandle, EventBus, EventHandler};
use nucleotide_events::v2::workspace::{
    DeleteMode, Event as WorkspaceEvent, FileOpIntent, PathCopyKind,
};
use nucleotide_logging::{error, info, warn};
use nucleotide_workspace::{FileKind, FileStat, WorkspaceBackendHandle, WorkspaceError};

pub struct WorkspaceFileOpHandler {
    bus: EventAggregatorHandle,
    backend: WorkspaceBackendHandle,
    runtime: tokio::runtime::Handle,
}

impl WorkspaceFileOpHandler {
    pub fn new(
        bus: EventAggregatorHandle,
        backend: WorkspaceBackendHandle,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            bus,
            backend,
            runtime,
        }
    }

    fn create_child_path(
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

    fn created_event(stat: FileStat) -> WorkspaceEvent {
        let parent_directory = Self::parent_for(&stat.path);
        WorkspaceEvent::FileCreated {
            path: stat.path,
            parent_directory,
        }
    }

    fn deleted_event(stat: FileStat) -> WorkspaceEvent {
        WorkspaceEvent::FileDeleted {
            was_directory: stat.kind == FileKind::Directory,
            path: stat.path,
        }
    }

    fn spawn_file_op<F>(&self, intent: FileOpIntent, future: F)
    where
        F: Future<Output = Result<WorkspaceEvent, WorkspaceError>> + Send + 'static,
    {
        let bus = self.bus.clone();
        self.runtime.spawn(async move {
            match future.await {
                Ok(event) => {
                    bus.dispatch_workspace(event);
                    bus.process_events();
                }
                Err(err) => {
                    error!(error = %err, intent = ?intent, "Failed to perform workspace file operation");
                }
            }
        });
    }

    fn handle_new_file(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = Self::create_child_path(parent, name, "create file")?;
        let backend = self.backend.clone();
        self.spawn_file_op(
            FileOpIntent::NewFile {
                parent: parent.to_path_buf(),
                name: name.to_string(),
            },
            async move {
                let stat = backend.create_file(&path).await?;
                Ok(Self::created_event(stat))
            },
        );
        Ok(())
    }

    fn handle_new_folder(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = Self::create_child_path(parent, name, "create directory")?;
        let backend = self.backend.clone();
        self.spawn_file_op(
            FileOpIntent::NewFolder {
                parent: parent.to_path_buf(),
                name: name.to_string(),
            },
            async move {
                let stat = backend.create_dir(&path).await?;
                Ok(Self::created_event(stat))
            },
        );
        Ok(())
    }

    fn handle_rename(&self, path: &Path, new_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "rename path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let new_path = Self::create_child_path(parent, new_name, "rename path")?;
        let old_path = path.to_path_buf();
        let backend = self.backend.clone();
        self.spawn_file_op(
            FileOpIntent::Rename {
                path: old_path.clone(),
                new_name: new_name.to_string(),
            },
            async move {
                let stat = backend.rename_path(&old_path, &new_path).await?;
                Ok(WorkspaceEvent::FileRenamed {
                    old_path,
                    new_path: stat.path,
                })
            },
        );
        Ok(())
    }

    fn handle_delete(&self, path: &Path, mode: DeleteMode) -> Result<(), WorkspaceError> {
        if mode == DeleteMode::Trash {
            warn!(
                path = %path.display(),
                "Trash delete is not implemented by workspace backends; performing permanent delete"
            );
        }
        let path = path.to_path_buf();
        let backend = self.backend.clone();
        self.spawn_file_op(
            FileOpIntent::Delete {
                path: path.clone(),
                mode,
            },
            async move {
                let stat = backend.delete_path(&path).await?;
                Ok(Self::deleted_event(stat))
            },
        );
        Ok(())
    }

    fn handle_duplicate(&self, path: &Path, target_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "copy path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let target_path = Self::create_child_path(parent, target_name, "copy path")?;
        let source_path = path.to_path_buf();
        let backend = self.backend.clone();
        self.spawn_file_op(
            FileOpIntent::Duplicate {
                path: source_path.clone(),
                target_name: target_name.to_string(),
            },
            async move {
                let stat = backend.copy_path(&source_path, &target_path).await?;
                Ok(Self::created_event(stat))
            },
        );
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
    use nucleotide_core::EventAggregator;
    use std::sync::{Arc, Mutex};

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

    struct CapturedWorkspaceEvents {
        events: Arc<Mutex<Vec<WorkspaceEvent>>>,
    }

    impl EventHandler for CapturedWorkspaceEvents {
        fn handle_workspace(&mut self, event: &WorkspaceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[tokio::test]
    async fn file_op_handler_dispatches_created_event_after_backend_op() {
        let temp_dir = tempfile::tempdir().unwrap();
        let created_path = temp_dir.path().join("new.rs");
        let events = Arc::new(Mutex::new(Vec::new()));
        let bus = EventAggregatorHandle::new(EventAggregator::new());

        bus.register_handler(CapturedWorkspaceEvents {
            events: events.clone(),
        });
        bus.register_handler(WorkspaceFileOpHandler::new(
            bus.clone(),
            nucleotide_workspace::local_workspace_backend(),
            tokio::runtime::Handle::current(),
        ));

        bus.dispatch_workspace(WorkspaceEvent::FileOpRequested {
            intent: FileOpIntent::NewFile {
                parent: temp_dir.path().to_path_buf(),
                name: "new.rs".to_string(),
            },
        });
        bus.process_events();

        for _ in 0..50 {
            if created_path.exists()
                && events.lock().unwrap().iter().any(|event| {
                    matches!(
                        event,
                        WorkspaceEvent::FileCreated { path, .. } if path == &created_path
                    )
                })
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for async file creation event");
    }
}
