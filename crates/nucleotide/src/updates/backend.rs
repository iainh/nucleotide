use std::{ffi::OsString, sync::mpsc::Sender, thread};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackAsset, sources::AutoSource};

use super::model::{AvailableUpdate, CheckOrigin, UpdateOperation};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub(crate) enum UpdateError {
    #[error("This build is not installed by Velopack: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Operation(String),
}

impl From<velopack::Error> for UpdateError {
    fn from(error: velopack::Error) -> Self {
        match error {
            velopack::Error::NotInstalled(reason) => Self::Unsupported(reason),
            other => Self::Operation(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckResult {
    NoUpdate,
    Available(AvailableUpdate),
}

pub(crate) trait UpdateBackend: Send + 'static {
    fn pending_restart(&mut self) -> Result<Option<AvailableUpdate>, UpdateError>;
    fn check(&mut self) -> Result<CheckResult, UpdateError>;
    fn download(&mut self, progress: Sender<i16>) -> Result<AvailableUpdate, UpdateError>;
    fn arm_apply_and_restart(&mut self, restart_args: &[OsString]) -> Result<(), UpdateError>;
}

struct VelopackBackend {
    manager: UpdateManager,
    available: Option<Box<UpdateInfo>>,
    downloaded: Option<VelopackAsset>,
}

impl VelopackBackend {
    fn new(source: &str) -> Result<Self, UpdateError> {
        let manager = UpdateManager::new(AutoSource::new(source), None, None)?;
        Ok(Self {
            manager,
            available: None,
            downloaded: None,
        })
    }
}

fn display_update(asset: &VelopackAsset) -> AvailableUpdate {
    AvailableUpdate {
        version: asset.Version.clone(),
        download_bytes: asset.Size,
        release_notes_markdown: asset.NotesMarkdown.clone(),
    }
}

impl UpdateBackend for VelopackBackend {
    fn pending_restart(&mut self) -> Result<Option<AvailableUpdate>, UpdateError> {
        let pending = self.manager.get_update_pending_restart();
        self.downloaded.clone_from(&pending);
        Ok(pending.as_ref().map(display_update))
    }

    fn check(&mut self) -> Result<CheckResult, UpdateError> {
        match self.manager.check_for_updates()? {
            UpdateCheck::UpdateAvailable(update) => {
                let display = display_update(&update.TargetFullRelease);
                self.available = Some(update);
                Ok(CheckResult::Available(display))
            }
            UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => {
                self.available = None;
                Ok(CheckResult::NoUpdate)
            }
        }
    }

    fn download(&mut self, progress: Sender<i16>) -> Result<AvailableUpdate, UpdateError> {
        let update = self.available.as_ref().ok_or_else(|| {
            UpdateError::Operation("No checked update is available to download".to_string())
        })?;
        self.manager.download_updates(update, Some(progress))?;

        let downloaded = self
            .manager
            .get_update_pending_restart()
            .unwrap_or_else(|| update.TargetFullRelease.clone());
        let display = display_update(&downloaded);
        self.downloaded = Some(downloaded);
        Ok(display)
    }

    fn arm_apply_and_restart(&mut self, restart_args: &[OsString]) -> Result<(), UpdateError> {
        let downloaded = self.downloaded.as_ref().ok_or_else(|| {
            UpdateError::Operation("No downloaded update is ready to apply".to_string())
        })?;
        self.manager
            .wait_exit_then_apply_updates(downloaded, false, true, restart_args)?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum WorkerCommand {
    Initialize {
        operation_id: u64,
    },
    Check {
        operation_id: u64,
        origin: CheckOrigin,
    },
    Download {
        operation_id: u64,
    },
    ArmApply {
        operation_id: u64,
        restart_args: Vec<OsString>,
    },
}

#[derive(Debug)]
pub(crate) enum WorkerEvent {
    Initialized {
        operation_id: u64,
        result: Result<Option<AvailableUpdate>, UpdateError>,
    },
    Checked {
        operation_id: u64,
        origin: CheckOrigin,
        result: Result<CheckResult, UpdateError>,
    },
    DownloadProgress {
        operation_id: u64,
        percent: u8,
    },
    Downloaded {
        operation_id: u64,
        result: Result<AvailableUpdate, UpdateError>,
    },
    ApplyArmed {
        operation_id: u64,
        result: Result<(), UpdateError>,
    },
}

pub(crate) fn start_worker(
    source: String,
) -> (
    UnboundedSender<WorkerCommand>,
    UnboundedReceiver<WorkerEvent>,
) {
    start_worker_with_factory(move || {
        VelopackBackend::new(&source).map(|backend| Box::new(backend) as Box<dyn UpdateBackend>)
    })
}

#[cfg(test)]
fn start_worker_with_backend(
    backend: Result<Box<dyn UpdateBackend>, UpdateError>,
) -> (
    UnboundedSender<WorkerCommand>,
    UnboundedReceiver<WorkerEvent>,
) {
    start_worker_with_factory(move || backend)
}

fn start_worker_with_factory(
    backend_factory: impl FnOnce() -> Result<Box<dyn UpdateBackend>, UpdateError> + Send + 'static,
) -> (
    UnboundedSender<WorkerCommand>,
    UnboundedReceiver<WorkerEvent>,
) {
    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let worker_event_tx = event_tx.clone();

    let spawn_result = thread::Builder::new()
        .name("nucleotide-update-worker".to_string())
        .spawn(move || {
            let event_tx = worker_event_tx;
            let mut backend = backend_factory();
            while let Some(command) = command_rx.blocking_recv() {
                match command {
                    WorkerCommand::Initialize { operation_id } => {
                        let result = match backend.as_mut() {
                            Ok(backend) => backend.pending_restart(),
                            Err(error) => Err(error.clone()),
                        };
                        let _ = event_tx.send(WorkerEvent::Initialized {
                            operation_id,
                            result,
                        });
                    }
                    WorkerCommand::Check {
                        operation_id,
                        origin,
                    } => {
                        let result = match backend.as_mut() {
                            Ok(backend) => backend.check(),
                            Err(error) => Err(error.clone()),
                        };
                        let _ = event_tx.send(WorkerEvent::Checked {
                            operation_id,
                            origin,
                            result,
                        });
                    }
                    WorkerCommand::Download { operation_id } => {
                        let (progress_tx, progress_rx) = std::sync::mpsc::channel::<i16>();
                        let progress_events = event_tx.clone();
                        let progress_forwarder = thread::Builder::new()
                            .name("nucleotide-update-progress".to_string())
                            .spawn(move || {
                                let mut last_percent = None;
                                while let Ok(percent) = progress_rx.recv() {
                                    let percent = percent.clamp(0, 100) as u8;
                                    if last_percent == Some(percent) {
                                        continue;
                                    }
                                    last_percent = Some(percent);
                                    if progress_events
                                        .send(WorkerEvent::DownloadProgress {
                                            operation_id,
                                            percent,
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            });

                        let result = match backend.as_mut() {
                            Ok(backend) => backend.download(progress_tx),
                            Err(error) => Err(error.clone()),
                        };
                        if let Ok(progress_forwarder) = progress_forwarder {
                            let _ = progress_forwarder.join();
                        }
                        let _ = event_tx.send(WorkerEvent::Downloaded {
                            operation_id,
                            result,
                        });
                    }
                    WorkerCommand::ArmApply {
                        operation_id,
                        restart_args,
                    } => {
                        let result = match backend.as_mut() {
                            Ok(backend) => backend.arm_apply_and_restart(&restart_args),
                            Err(error) => Err(error.clone()),
                        };
                        let _ = event_tx.send(WorkerEvent::ApplyArmed {
                            operation_id,
                            result,
                        });
                    }
                }
            }
        });

    if let Err(error) = spawn_result {
        let _ = event_tx.send(WorkerEvent::Initialized {
            operation_id: 0,
            result: Err(UpdateError::Operation(format!(
                "Failed to start the update worker: {error}"
            ))),
        });
    }

    (command_tx, event_rx)
}

pub(crate) fn operation_for_event(event: &WorkerEvent) -> UpdateOperation {
    match event {
        WorkerEvent::Initialized { .. } => UpdateOperation::Initialize,
        WorkerEvent::Checked { .. } => UpdateOperation::Check,
        WorkerEvent::DownloadProgress { .. } | WorkerEvent::Downloaded { .. } => {
            UpdateOperation::Download
        }
        WorkerEvent::ApplyArmed { .. } => UpdateOperation::Apply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend;

    impl UpdateBackend for FakeBackend {
        fn pending_restart(&mut self) -> Result<Option<AvailableUpdate>, UpdateError> {
            Ok(None)
        }

        fn check(&mut self) -> Result<CheckResult, UpdateError> {
            Ok(CheckResult::Available(AvailableUpdate {
                version: "2.0.0".to_string(),
                download_bytes: 2048,
                release_notes_markdown: "## Changes".to_string(),
            }))
        }

        fn download(&mut self, progress: Sender<i16>) -> Result<AvailableUpdate, UpdateError> {
            let _ = progress.send(25);
            let _ = progress.send(100);
            Ok(AvailableUpdate {
                version: "2.0.0".to_string(),
                download_bytes: 2048,
                release_notes_markdown: "## Changes".to_string(),
            })
        }

        fn arm_apply_and_restart(&mut self, _restart_args: &[OsString]) -> Result<(), UpdateError> {
            Ok(())
        }
    }

    #[test]
    fn display_update_copies_user_visible_asset_metadata() {
        let asset = VelopackAsset {
            Version: "2.0.0".to_string(),
            Size: 2048,
            NotesMarkdown: "## Changes".to_string(),
            ..VelopackAsset::default()
        };

        assert_eq!(
            display_update(&asset),
            AvailableUpdate {
                version: "2.0.0".to_string(),
                download_bytes: 2048,
                release_notes_markdown: "## Changes".to_string(),
            }
        );
    }

    #[test]
    fn not_installed_errors_are_normalized_as_unsupported() {
        let error = UpdateError::from(velopack::Error::NotInstalled("portable".to_string()));
        assert_eq!(error, UpdateError::Unsupported("portable".to_string()));
    }

    #[test]
    fn worker_serializes_check_download_and_apply_operations() {
        let (command_tx, mut event_rx) =
            start_worker_with_backend(Ok(Box::new(FakeBackend) as Box<dyn UpdateBackend>));

        command_tx
            .send(WorkerCommand::Check {
                operation_id: 1,
                origin: CheckOrigin::Manual,
            })
            .unwrap();
        assert!(matches!(
            event_rx.blocking_recv().unwrap(),
            WorkerEvent::Checked {
                operation_id: 1,
                origin: CheckOrigin::Manual,
                result: Ok(CheckResult::Available(_)),
            }
        ));

        command_tx
            .send(WorkerCommand::Download { operation_id: 2 })
            .unwrap();
        assert!(matches!(
            event_rx.blocking_recv().unwrap(),
            WorkerEvent::DownloadProgress {
                operation_id: 2,
                percent: 25,
            }
        ));
        assert!(matches!(
            event_rx.blocking_recv().unwrap(),
            WorkerEvent::DownloadProgress {
                operation_id: 2,
                percent: 100,
            }
        ));
        assert!(matches!(
            event_rx.blocking_recv().unwrap(),
            WorkerEvent::Downloaded {
                operation_id: 2,
                result: Ok(_),
            }
        ));

        command_tx
            .send(WorkerCommand::ArmApply {
                operation_id: 3,
                restart_args: Vec::new(),
            })
            .unwrap();
        assert!(matches!(
            event_rx.blocking_recv().unwrap(),
            WorkerEvent::ApplyArmed {
                operation_id: 3,
                result: Ok(()),
            }
        ));
    }
}
