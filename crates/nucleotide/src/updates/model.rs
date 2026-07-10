use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub download_bytes: u64,
    pub release_notes_markdown: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOrigin {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOperation {
    Initialize,
    Check,
    Download,
    Apply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Disabled,
    Unsupported {
        reason: String,
    },
    Idle {
        last_checked_at: Option<SystemTime>,
    },
    Checking {
        origin: CheckOrigin,
    },
    UpToDate {
        checked_at: SystemTime,
    },
    Available(AvailableUpdate),
    Downloading {
        update: AvailableUpdate,
        percent: u8,
    },
    ReadyToRestart(AvailableUpdate),
    Applying(AvailableUpdate),
    Failed {
        operation: UpdateOperation,
        message: String,
        retryable: bool,
    },
}

impl UpdateState {
    pub fn has_titlebar_indicator(&self) -> bool {
        matches!(
            self,
            Self::Checking {
                origin: CheckOrigin::Manual
            } | Self::Available(_)
                | Self::Downloading { .. }
                | Self::ReadyToRestart(_)
                | Self::Applying(_)
                | Self::Failed { .. }
        )
    }

    pub fn available_update(&self) -> Option<&AvailableUpdate> {
        match self {
            Self::Available(update)
            | Self::ReadyToRestart(update)
            | Self::Applying(update)
            | Self::Downloading { update, .. } => Some(update),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update() -> AvailableUpdate {
        AvailableUpdate {
            version: "1.2.3".to_string(),
            download_bytes: 42,
            release_notes_markdown: String::new(),
        }
    }

    #[test]
    fn indicator_is_only_present_for_user_visible_update_states() {
        assert!(!UpdateState::Disabled.has_titlebar_indicator());
        assert!(
            !UpdateState::Checking {
                origin: CheckOrigin::Automatic
            }
            .has_titlebar_indicator()
        );
        assert!(
            UpdateState::Checking {
                origin: CheckOrigin::Manual
            }
            .has_titlebar_indicator()
        );
        assert!(UpdateState::Available(update()).has_titlebar_indicator());
        assert!(UpdateState::ReadyToRestart(update()).has_titlebar_indicator());
    }

    #[test]
    fn available_update_is_preserved_while_downloading_and_applying() {
        let expected = update();
        assert_eq!(
            UpdateState::Downloading {
                update: expected.clone(),
                percent: 50,
            }
            .available_update(),
            Some(&expected)
        );
        assert_eq!(
            UpdateState::Applying(expected.clone()).available_update(),
            Some(&expected)
        );
    }
}
