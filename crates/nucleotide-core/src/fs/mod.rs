pub mod operations;

use crate::{EventAggregatorHandle, EventBus};
use nucleotide_events::v2::workspace::Event as WorkspaceEvent;

/// Filesystem operation handler that listens for workspace file operation intents
/// and executes them, dispatching result events.
pub struct FsOpHandler {
    bus: EventAggregatorHandle,
}

impl FsOpHandler {
    pub fn new(bus: EventAggregatorHandle) -> Self {
        Self { bus }
    }
}

impl crate::EventHandler for FsOpHandler {
    fn handle_workspace(&mut self, event: &WorkspaceEvent) {
        use nucleotide_events::v2::workspace::FileOpIntent;
        use operations::*;

        if let WorkspaceEvent::FileOpRequested { intent } = event {
            match intent {
                FileOpIntent::NewFile { parent, name } => match create_file(parent, name) {
                    Ok(path) => self.bus.dispatch_workspace(WorkspaceEvent::FileCreated {
                        path,
                        parent_directory: parent.clone(),
                    }),
                    Err(e) => {
                        tracing::error!(error=%e, parent=%parent.display(), name=%name, "Failed to create file")
                    }
                },
                FileOpIntent::NewFolder { parent, name } => match create_dir(parent, name) {
                    Ok(path) => self.bus.dispatch_workspace(WorkspaceEvent::FileCreated {
                        path,
                        parent_directory: parent.clone(),
                    }),
                    Err(e) => {
                        tracing::error!(error=%e, parent=%parent.display(), name=%name, "Failed to create folder")
                    }
                },
                FileOpIntent::Rename { path, new_name } => match rename_path(path, new_name) {
                    Ok(new_path) => self.bus.dispatch_workspace(WorkspaceEvent::FileRenamed {
                        old_path: path.clone(),
                        new_path,
                    }),
                    Err(e) => {
                        tracing::error!(error=%e, path=%path.display(), new_name=%new_name, "Failed to rename path")
                    }
                },
                FileOpIntent::Delete { path, mode } => {
                    let is_dir = path.is_dir();
                    let res = match mode {
                        nucleotide_events::v2::workspace::DeleteMode::Permanent => {
                            delete_path(path)
                        }
                        nucleotide_events::v2::workspace::DeleteMode::Trash => {
                            // If trash feature is enabled, try moving to trash; otherwise fall back
                            #[cfg(feature = "trash")]
                            {
                                trash::delete(path)
                                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
                            }
                            #[cfg(not(feature = "trash"))]
                            {
                                tracing::warn!(path=%path.display(), "Trash feature not enabled; performing permanent delete");
                                delete_path(path)
                            }
                        }
                    };

                    match res {
                        Ok(()) => self.bus.dispatch_workspace(WorkspaceEvent::FileDeleted {
                            path: path.clone(),
                            was_directory: is_dir,
                        }),
                        Err(e) => {
                            tracing::error!(error=%e, path=%path.display(), mode=?mode, "Failed to delete path")
                        }
                    }
                }
                FileOpIntent::Duplicate { path, target_name } => {
                    match duplicate_path(path, target_name) {
                        Ok(new_path) => {
                            // Emit as FileCreated of the new item
                            let parent = new_path
                                .parent()
                                .unwrap_or_else(|| std::path::Path::new("."))
                                .to_path_buf();
                            self.bus.dispatch_workspace(WorkspaceEvent::FileCreated {
                                path: new_path,
                                parent_directory: parent,
                            })
                        }
                        Err(e) => {
                            tracing::error!(error=%e, path=%path.display(), target=%target_name, "Failed to duplicate")
                        }
                    }
                }
                FileOpIntent::CopyPath { path, kind } => {
                    // Leave clipboard handling to UI for now; just log
                    tracing::info!(path=%path.display(), kind=?kind, "CopyPath intent received");
                }
                FileOpIntent::RevealInOs { path } => {
                    // Platform-specific reveal left to UI layer
                    tracing::info!(path=%path.display(), "RevealInOs intent received");
                }
            }
        }
    }
}
