// ABOUTME: Backend-aware workspace file operation handler
// ABOUTME: Routes file tree mutations through WorkspaceBackend instead of host filesystem calls

use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::{process::Command, task::JoinHandle};

use nucleotide_events::workspace::{
    DeleteMode, Event as WorkspaceEvent, FileOpIntent, PathCopyKind,
};
use nucleotide_logging::{error, info, warn};
use nucleotide_workspace::{WorkspaceBackendHandle, WorkspaceError, WorkspaceIdentity};

#[derive(Clone)]
pub struct WorkspaceFileOpHandler {
    backend: WorkspaceBackendHandle,
    runtime: tokio::runtime::Handle,
}

impl WorkspaceFileOpHandler {
    pub fn new(backend: WorkspaceBackendHandle, runtime: tokio::runtime::Handle) -> Self {
        Self { backend, runtime }
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

    async fn handle_new_file(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = Self::create_child_path(parent, name, "create file")?;
        self.backend.create_file(&path).await?;
        Ok(())
    }

    async fn handle_new_folder(&self, parent: &Path, name: &str) -> Result<(), WorkspaceError> {
        let path = Self::create_child_path(parent, name, "create directory")?;
        self.backend.create_dir(&path).await?;
        Ok(())
    }

    async fn handle_rename(&self, path: &Path, new_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "rename path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let new_path = Self::create_child_path(parent, new_name, "rename path")?;
        self.backend.rename_path(path, &new_path).await?;
        Ok(())
    }

    async fn handle_delete(&self, path: &Path, mode: DeleteMode) -> Result<(), WorkspaceError> {
        if mode == DeleteMode::Trash {
            return Err(WorkspaceError::CommandFailed {
                operation: "move path to trash",
                path: path.to_path_buf(),
                message: "trash delete is not supported by workspace backends".to_string(),
            });
        }
        self.backend.delete_path(path).await?;
        Ok(())
    }

    async fn handle_duplicate(&self, path: &Path, target_name: &str) -> Result<(), WorkspaceError> {
        let parent = path.parent().ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "copy path",
            path: path.to_path_buf(),
            message: "path has no parent".to_string(),
        })?;
        let target_path = Self::create_child_path(parent, target_name, "copy path")?;
        self.backend.copy_path(path, &target_path).await?;
        Ok(())
    }

    async fn handle_file_op(&self, intent: &FileOpIntent) -> Result<(), WorkspaceError> {
        match intent {
            FileOpIntent::NewFile { parent, name } => self.handle_new_file(parent, name).await,
            FileOpIntent::NewFolder { parent, name } => self.handle_new_folder(parent, name).await,
            FileOpIntent::Rename { path, new_name } => self.handle_rename(path, new_name).await,
            FileOpIntent::Delete { path, mode } => self.handle_delete(path, *mode).await,
            FileOpIntent::Duplicate { path, target_name } => {
                self.handle_duplicate(path, target_name).await
            }
            FileOpIntent::CopyPath { path, kind } => {
                log_path_intent("CopyPath", path, *kind);
                Ok(())
            }
            FileOpIntent::RevealInOs { path } => self.handle_reveal_in_os(path).await,
        }
    }

    async fn handle_reveal_in_os(&self, path: &Path) -> Result<(), WorkspaceError> {
        if !should_reveal_in_os(&self.backend.identity()) {
            warn!(
                path = %path.display(),
                backend = ?self.backend.identity(),
                "RevealInOs is unavailable for remote workspace paths"
            );
            return Ok(());
        }

        reveal_path_in_os(path)
            .await
            .map_err(|source| WorkspaceError::Io {
                operation: "reveal path in OS",
                path: path.to_path_buf(),
                source,
            })
    }

    /// Start an operation and return its actual result, including validation and backend errors.
    /// Dropping the handle leaves the operation running; unobserved errors are still logged.
    pub fn dispatch(
        &self,
        event: &WorkspaceEvent,
    ) -> Option<JoinHandle<Result<(), WorkspaceError>>> {
        let WorkspaceEvent::FileOpRequested { intent } = event else {
            return None;
        };

        let handler = self.clone();
        let intent = intent.clone();
        Some(self.runtime.spawn(async move {
            let result = handler.handle_file_op(&intent).await;
            if let Err(err) = &result {
                error!(error = %err, intent = ?intent, "Failed to perform workspace file operation");
            }
            result
        }))
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

fn should_reveal_in_os(identity: &WorkspaceIdentity) -> bool {
    matches!(identity, WorkspaceIdentity::Local)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RevealCommand {
    program: &'static str,
    args: Vec<String>,
}

fn reveal_command_for_path(path: &Path) -> Option<RevealCommand> {
    #[cfg(target_os = "macos")]
    {
        return Some(RevealCommand {
            program: "open",
            args: vec!["-R".to_string(), path.display().to_string()],
        });
    }

    #[cfg(target_os = "windows")]
    {
        return Some(RevealCommand {
            program: "explorer",
            args: vec![format!("/select,{}", path.display())],
        });
    }

    #[cfg(target_os = "linux")]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        return Some(RevealCommand {
            program: "xdg-open",
            args: vec![target.display().to_string()],
        });
    }

    #[allow(unreachable_code)]
    None
}

async fn reveal_path_in_os(path: &Path) -> io::Result<()> {
    let command = reveal_command_for_path(path)
        .ok_or_else(|| io::Error::other("Reveal in OS is unsupported on this platform"))?;
    let status = Command::new(command.program)
        .args(command.args)
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "reveal command exited with {status}"
        )))
    }
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

    #[test]
    fn reveal_in_os_is_local_only() {
        assert!(should_reveal_in_os(&WorkspaceIdentity::Local));
        assert!(!should_reveal_in_os(&WorkspaceIdentity::Remote(
            nucleotide_workspace::RemoteWorkspaceIdentity {
                kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
                name: "example.test".to_string(),
            },
        )));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reveal_command_uses_finder_reveal_on_macos() {
        let path = Path::new("/tmp/nucleotide-test/file.rs");
        let command = reveal_command_for_path(path).unwrap();

        assert_eq!(command.program, "open");
        assert_eq!(
            command.args,
            vec!["-R".to_string(), path.display().to_string()]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reveal_command_opens_parent_directory_on_linux() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("file.rs");
        std::fs::write(&path, "").unwrap();
        let command = reveal_command_for_path(&path).unwrap();

        assert_eq!(command.program, "xdg-open");
        assert_eq!(command.args, vec![temp_dir.path().display().to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reveal_command_uses_explorer_select_on_windows() {
        let path = Path::new(r"C:\nucleotide-test\file.rs");
        let command = reveal_command_for_path(path).unwrap();

        assert_eq!(command.program, "explorer");
        assert_eq!(command.args, vec![format!("/select,{}", path.display())]);
    }

    #[tokio::test]
    async fn trash_delete_is_rejected_by_backend_handler() {
        let handler = WorkspaceFileOpHandler::new(
            nucleotide_workspace::local_workspace_backend(),
            tokio::runtime::Handle::current(),
        );

        let result = handler
            .dispatch(&WorkspaceEvent::FileOpRequested {
                intent: FileOpIntent::Delete {
                    path: PathBuf::from("/tmp/delete-me"),
                    mode: DeleteMode::Trash,
                },
            })
            .unwrap()
            .await
            .unwrap();

        assert!(matches!(
            result,
            Err(WorkspaceError::CommandFailed {
                operation: "move path to trash",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn file_op_handler_completes_backend_mutations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let created_path = temp_dir.path().join("new.rs");
        let handler = WorkspaceFileOpHandler::new(
            nucleotide_workspace::local_workspace_backend(),
            tokio::runtime::Handle::current(),
        );

        handler
            .dispatch(&WorkspaceEvent::FileOpRequested {
                intent: FileOpIntent::NewFile {
                    parent: temp_dir.path().to_path_buf(),
                    name: "new.rs".to_string(),
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(created_path.is_file());
        std::fs::write(&created_path, "original contents").unwrap();

        for intent in [
            FileOpIntent::Duplicate {
                path: created_path.clone(),
                target_name: "copy.rs".into(),
            },
            FileOpIntent::Rename {
                path: created_path.clone(),
                new_name: "renamed.rs".into(),
            },
            FileOpIntent::Delete {
                path: temp_dir.path().join("renamed.rs"),
                mode: DeleteMode::Permanent,
            },
            FileOpIntent::NewFolder {
                parent: temp_dir.path().to_path_buf(),
                name: "folder".into(),
            },
        ] {
            handler
                .dispatch(&WorkspaceEvent::FileOpRequested { intent })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
        }

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("copy.rs")).unwrap(),
            "original contents"
        );
        assert!(!created_path.exists());
        assert!(!temp_dir.path().join("renamed.rs").exists());
        assert!(temp_dir.path().join("folder").is_dir());
    }

    #[tokio::test]
    async fn file_op_completion_reports_validation_and_backend_errors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let handler = WorkspaceFileOpHandler::new(
            nucleotide_workspace::local_workspace_backend(),
            tokio::runtime::Handle::current(),
        );

        let error = handler
            .dispatch(&WorkspaceEvent::FileOpRequested {
                intent: FileOpIntent::NewFile {
                    parent: temp_dir.path().to_path_buf(),
                    name: "../escape.rs".into(),
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::CommandFailed { .. }));

        // The destination already exists, but a failed copy must still report failure.
        // Polling its metadata would incorrectly report this operation as successful.
        let destination = temp_dir.path().join("existing.rs");
        std::fs::write(&destination, "unchanged").unwrap();
        let error = handler
            .dispatch(&WorkspaceEvent::FileOpRequested {
                intent: FileOpIntent::Duplicate {
                    path: temp_dir.path().join("missing.rs"),
                    target_name: "existing.rs".into(),
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::Io { source, .. }
            if source.kind() == io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "unchanged");
    }
}
