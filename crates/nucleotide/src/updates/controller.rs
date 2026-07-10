use std::{
    env,
    ffi::OsString,
    time::{Duration, SystemTime},
};

use gpui::{Context, Entity, EventEmitter, Global};
use nucleotide_logging::{info, warn};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::UpdatesConfig;

use super::{
    backend::{
        CheckResult, UpdateError, WorkerCommand, WorkerEvent, operation_for_event, start_worker,
    },
    model::{AvailableUpdate, CheckOrigin, UpdateOperation, UpdateState},
};

const DEFAULT_UPDATE_SOURCE: &str = "https://github.com/iainh/nucleotide";
const UPDATE_SOURCE_ENV: &str = "NUCLEOTIDE_UPDATE_SOURCE";
const DISABLE_UPDATES_ENV: &str = "NUCLEOTIDE_DISABLE_AUTO_UPDATE";
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const STARTUP_CHECK_BASE_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct UpdateControllerHandle(pub Entity<UpdateController>);

impl Global for UpdateControllerHandle {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateControllerEvent {
    ApplyArmed,
}

pub struct UpdateController {
    state: UpdateState,
    config: UpdatesConfig,
    command_tx: Option<UnboundedSender<WorkerCommand>>,
    next_operation_id: u64,
    active_operation_id: Option<u64>,
    last_checked_at: Option<SystemTime>,
    last_available: Option<AvailableUpdate>,
    started: bool,
}

impl EventEmitter<UpdateControllerEvent> for UpdateController {}

impl UpdateController {
    pub fn new(config: UpdatesConfig, cx: &mut Context<Self>) -> Self {
        if !config.enabled || updates_disabled_from_environment() {
            info!(
                configured = config.enabled,
                env = DISABLE_UPDATES_ENV,
                "Application updates are disabled"
            );
            return Self {
                state: UpdateState::Disabled,
                config,
                command_tx: None,
                next_operation_id: 1,
                active_operation_id: None,
                last_checked_at: None,
                last_available: None,
                started: false,
            };
        }

        let source = update_source();
        if source != DEFAULT_UPDATE_SOURCE {
            info!(source = %source, env = UPDATE_SOURCE_ENV, "Using an overridden update source");
        }
        let (command_tx, mut event_rx) = start_worker(source);

        cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, |controller, cx| {
                    controller.handle_worker_event(event, cx);
                });
            }
        })
        .detach();

        Self {
            state: UpdateState::Idle {
                last_checked_at: None,
            },
            config,
            command_tx: Some(command_tx),
            next_operation_id: 1,
            active_operation_id: None,
            last_checked_at: None,
            last_available: None,
            started: false,
        }
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.started || self.command_tx.is_none() {
            return;
        }
        self.started = true;

        let operation_id = self.begin_operation();
        self.send_command(WorkerCommand::Initialize { operation_id }, cx);

        if !self.config.check_on_startup {
            return;
        }

        let jitter = Duration::from_secs(u64::from(std::process::id() % 20));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(STARTUP_CHECK_BASE_DELAY + jitter)
                .await;

            loop {
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, |controller, cx| {
                    controller.check(CheckOrigin::Automatic, cx);
                });

                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        self.check(CheckOrigin::Manual, cx);
    }

    pub fn check_if_stale(&mut self, cx: &mut Context<Self>) {
        let is_stale = self
            .last_checked_at
            .and_then(|checked| checked.elapsed().ok())
            .is_none_or(|elapsed| elapsed >= CHECK_INTERVAL);
        if is_stale {
            self.check(CheckOrigin::Automatic, cx);
        }
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state,
            UpdateState::Downloading { .. }
                | UpdateState::ReadyToRestart(_)
                | UpdateState::Applying(_)
        ) {
            return;
        }
        let Some(update) = self.last_available.clone() else {
            return;
        };

        let operation_id = self.begin_operation();
        self.state = UpdateState::Downloading { update, percent: 0 };
        self.send_command(WorkerCommand::Download { operation_id }, cx);
        cx.notify();
    }

    pub fn arm_apply_and_restart(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.last_available.clone() else {
            return;
        };
        if !matches!(
            self.state,
            UpdateState::ReadyToRestart(_)
                | UpdateState::Failed {
                    operation: UpdateOperation::Apply,
                    ..
                }
        ) {
            return;
        }

        let operation_id = self.begin_operation();
        self.state = UpdateState::Applying(update);
        self.send_command(
            WorkerCommand::ArmApply {
                operation_id,
                restart_args: sanitized_restart_args(env::args_os()),
            },
            cx,
        );
        cx.notify();
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let operation = match self.state {
            UpdateState::Failed { operation, .. } => operation,
            _ => return,
        };
        match operation {
            UpdateOperation::Initialize | UpdateOperation::Check => self.check_now(cx),
            UpdateOperation::Download => self.download(cx),
            UpdateOperation::Apply => self.arm_apply_and_restart(cx),
        }
    }

    fn check(&mut self, origin: CheckOrigin, cx: &mut Context<Self>) {
        if self.command_tx.is_none()
            || matches!(
                self.state,
                UpdateState::Checking { .. }
                    | UpdateState::Downloading { .. }
                    | UpdateState::ReadyToRestart(_)
                    | UpdateState::Applying(_)
                    | UpdateState::Disabled
                    | UpdateState::Unsupported { .. }
            )
        {
            return;
        }

        let operation_id = self.begin_operation();
        self.state = UpdateState::Checking { origin };
        self.send_command(
            WorkerCommand::Check {
                operation_id,
                origin,
            },
            cx,
        );
        cx.notify();
    }

    fn begin_operation(&mut self) -> u64 {
        let operation_id = self.next_operation_id;
        self.next_operation_id = self.next_operation_id.wrapping_add(1).max(1);
        self.active_operation_id = Some(operation_id);
        operation_id
    }

    fn send_command(&mut self, command: WorkerCommand, cx: &mut Context<Self>) {
        let operation = match &command {
            WorkerCommand::Initialize { .. } => UpdateOperation::Initialize,
            WorkerCommand::Check { .. } => UpdateOperation::Check,
            WorkerCommand::Download { .. } => UpdateOperation::Download,
            WorkerCommand::ArmApply { .. } => UpdateOperation::Apply,
        };
        let send_failed = self
            .command_tx
            .as_ref()
            .is_none_or(|command_tx| command_tx.send(command).is_err());
        if send_failed {
            self.active_operation_id = None;
            self.state = UpdateState::Failed {
                operation,
                message: "The update worker is not available".to_string(),
                retryable: false,
            };
            cx.notify();
        }
    }

    fn handle_worker_event(&mut self, event: WorkerEvent, cx: &mut Context<Self>) {
        let operation_id = worker_event_operation_id(&event);
        if self.active_operation_id != Some(operation_id) {
            info!(
                operation_id,
                active_operation_id = ?self.active_operation_id,
                operation = ?operation_for_event(&event),
                "Ignoring a stale update worker event"
            );
            return;
        }

        match event {
            WorkerEvent::Initialized { result, .. } => {
                self.active_operation_id = None;
                match result {
                    Ok(Some(update)) => {
                        self.last_available = Some(update.clone());
                        self.state = UpdateState::ReadyToRestart(update);
                    }
                    Ok(None) => {
                        self.state = UpdateState::Idle {
                            last_checked_at: self.last_checked_at,
                        };
                    }
                    Err(UpdateError::Unsupported(reason)) => {
                        info!(reason = %reason, "Updates are unavailable for this build");
                        self.command_tx = None;
                        self.state = UpdateState::Unsupported { reason };
                    }
                    Err(error) => self.fail(UpdateOperation::Initialize, error, true),
                }
            }
            WorkerEvent::Checked { origin, result, .. } => {
                self.active_operation_id = None;
                match result {
                    Ok(CheckResult::NoUpdate) => {
                        let checked_at = SystemTime::now();
                        self.last_checked_at = Some(checked_at);
                        self.state = match origin {
                            CheckOrigin::Manual => UpdateState::UpToDate { checked_at },
                            CheckOrigin::Automatic => UpdateState::Idle {
                                last_checked_at: Some(checked_at),
                            },
                        };
                    }
                    Ok(CheckResult::Available(update)) => {
                        self.last_checked_at = Some(SystemTime::now());
                        self.last_available = Some(update.clone());
                        self.state = UpdateState::Available(update);
                        if self.config.auto_download {
                            self.download(cx);
                        }
                    }
                    Err(UpdateError::Unsupported(reason)) => {
                        self.command_tx = None;
                        self.state = UpdateState::Unsupported { reason };
                    }
                    Err(error) if origin == CheckOrigin::Automatic => {
                        warn!(error = %error, "Automatic update check failed");
                        self.state = UpdateState::Idle {
                            last_checked_at: self.last_checked_at,
                        };
                    }
                    Err(error) => self.fail(UpdateOperation::Check, error, true),
                }
            }
            WorkerEvent::DownloadProgress { percent, .. } => {
                if let UpdateState::Downloading {
                    update,
                    percent: current,
                } = &mut self.state
                {
                    *current = (*current).max(percent);
                    let _ = update;
                }
            }
            WorkerEvent::Downloaded { result, .. } => {
                self.active_operation_id = None;
                match result {
                    Ok(update) => {
                        self.last_available = Some(update.clone());
                        self.state = UpdateState::ReadyToRestart(update);
                    }
                    Err(error) => self.fail(UpdateOperation::Download, error, true),
                }
            }
            WorkerEvent::ApplyArmed { result, .. } => {
                self.active_operation_id = None;
                match result {
                    Ok(()) => cx.emit(UpdateControllerEvent::ApplyArmed),
                    Err(error) => self.fail(UpdateOperation::Apply, error, true),
                }
            }
        }
        cx.notify();
    }

    fn fail(&mut self, operation: UpdateOperation, error: UpdateError, retryable: bool) {
        warn!(?operation, error = %error, "Application update operation failed");
        self.state = UpdateState::Failed {
            operation,
            message: user_message(operation, &error),
            retryable,
        };
    }
}

fn worker_event_operation_id(event: &WorkerEvent) -> u64 {
    match event {
        WorkerEvent::Initialized { operation_id, .. }
        | WorkerEvent::Checked { operation_id, .. }
        | WorkerEvent::DownloadProgress { operation_id, .. }
        | WorkerEvent::Downloaded { operation_id, .. }
        | WorkerEvent::ApplyArmed { operation_id, .. } => *operation_id,
    }
}

fn user_message(operation: UpdateOperation, error: &UpdateError) -> String {
    match error {
        UpdateError::Unsupported(_) => "Updates are unavailable for this build".to_string(),
        UpdateError::Operation(_) => match operation {
            UpdateOperation::Initialize | UpdateOperation::Check => {
                "Nucleotide could not check for updates. Check your connection and try again."
                    .to_string()
            }
            UpdateOperation::Download => {
                "Nucleotide could not download or verify the update. Try again.".to_string()
            }
            UpdateOperation::Apply => {
                "Nucleotide could not prepare the update for restart. Try again.".to_string()
            }
        },
    }
}

fn updates_disabled_from_environment() -> bool {
    env::var(DISABLE_UPDATES_ENV)
        .ok()
        .is_some_and(|value| parse_boolean_override(&value))
}

fn parse_boolean_override(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn update_source() -> String {
    env::var(UPDATE_SOURCE_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPDATE_SOURCE.to_owned())
}

fn sanitized_restart_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    let _program = args.next();
    let mut sanitized = Vec::new();
    let mut skip_next = false;

    for argument in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        let text = argument.to_string_lossy();
        if text.starts_with("--veloapp-") {
            skip_next = true;
            continue;
        }
        if matches!(
            text.as_ref(),
            "--health" | "--fetch-grammars" | "--build-grammars"
        ) {
            continue;
        }
        if text == "--dock-action" {
            skip_next = true;
            continue;
        }
        sanitized.push(argument);
    }

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolean_environment_overrides_are_case_insensitive() {
        for enabled in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(parse_boolean_override(enabled));
        }
        for disabled in ["0", "false", "off", ""] {
            assert!(!parse_boolean_override(disabled));
        }
    }

    #[test]
    fn restart_arguments_drop_velopack_and_one_shot_modes() {
        let args = [
            "nucl",
            "--dock-action",
            "2",
            "--veloapp-updated",
            "1.2.3",
            "project",
            "src/main.rs",
        ];
        let sanitized = sanitized_restart_args(args.map(OsString::from));

        assert_eq!(
            sanitized,
            vec![OsString::from("project"), OsString::from("src/main.rs")]
        );
    }
}
