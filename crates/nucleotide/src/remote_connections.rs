// ABOUTME: Persistent saved and recent remote workspace targets
// ABOUTME: Keeps remote connection history separate from transport startup

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_RECENT_REMOTE_PROJECTS: usize = 12;
const STORE_FILE_NAME: &str = "remote_connections.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConnectionStore {
    #[serde(default)]
    pub saved: Vec<SavedRemoteConnection>,
    #[serde(default)]
    pub recent: Vec<RecentRemoteConnection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRemoteConnection {
    pub name: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_opened_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentRemoteConnection {
    pub target: String,
    pub last_opened_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConnectionCompletion {
    pub insert_text: String,
    pub display_text: String,
    pub description: String,
}

impl RemoteConnectionStore {
    pub fn load_default() -> Result<Self> {
        Self::load(&default_store_path())
    }

    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents)
                .with_context(|| format!("failed to parse {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(&default_store_path())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let contents =
            toml::to_string_pretty(self).context("failed to encode remote connections")?;
        std::fs::write(path, contents)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn save_named(&mut self, name: impl Into<String>, target: impl Into<String>) {
        let name = name.into();
        let target = target.into();
        self.saved.retain(|entry| entry.name != name);
        self.saved.insert(
            0,
            SavedRemoteConnection {
                name,
                target,
                last_opened_unix_secs: None,
            },
        );
    }

    pub fn remove_saved(&mut self, name: &str) -> bool {
        let before = self.saved.len();
        self.saved.retain(|entry| entry.name != name);
        self.saved.len() != before
    }

    pub fn saved_target(&self, name: &str) -> Option<&str> {
        self.saved
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.target.as_str())
    }

    pub fn record_successful_open(&mut self, target: impl Into<String>) {
        let target = target.into();
        let now = now_unix_secs();
        self.recent.retain(|entry| entry.target != target);
        self.recent.insert(
            0,
            RecentRemoteConnection {
                target: target.clone(),
                last_opened_unix_secs: now,
            },
        );
        self.recent.truncate(MAX_RECENT_REMOTE_PROJECTS);

        for saved in &mut self.saved {
            if saved.target == target {
                saved.last_opened_unix_secs = Some(now);
            }
        }
    }

    pub fn last_recent_target(&self) -> Option<&str> {
        self.recent.first().map(|entry| entry.target.as_str())
    }
}

pub fn default_store_path() -> PathBuf {
    helix_loader::config_dir().join(STORE_FILE_NAME)
}

pub fn target_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn valid_connection_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || first == '_' || first == '-')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

pub fn completions_for_input(
    input: &str,
    store: &RemoteConnectionStore,
) -> Vec<RemoteConnectionCompletion> {
    let input = input.trim_start();
    let mut completions = Vec::new();

    match input.split_once(char::is_whitespace) {
        Some(("forget", suffix)) => {
            push_saved_name_completions(&mut completions, "forget ", suffix.trim_start(), store);
        }
        Some(("open", suffix)) => {
            push_saved_name_completions(&mut completions, "open ", suffix.trim_start(), store);
            push_recent_target_completions(&mut completions, "open ", suffix.trim_start(), store);
        }
        Some(("save", _)) => {}
        Some(("reconnect", _)) | Some(("cancel", _)) => {}
        _ => {
            push_command_completion(
                &mut completions,
                input,
                "reconnect",
                "Reconnect to the most recent remote project",
            );
            push_command_completion(
                &mut completions,
                input,
                "cancel",
                "Cancel the active remote connection attempt",
            );
            push_command_completion(
                &mut completions,
                input,
                "save ",
                "Save a named remote project: save <name> <target>",
            );
            push_command_completion(
                &mut completions,
                input,
                "forget ",
                "Remove a saved remote project: forget <name>",
            );
            push_saved_name_completions(&mut completions, "", input, store);
            push_recent_target_completions(&mut completions, "", input, store);
        }
    }

    completions
}

fn push_command_completion(
    completions: &mut Vec<RemoteConnectionCompletion>,
    input: &str,
    command: &str,
    description: &str,
) {
    if command.starts_with(input) {
        completions.push(RemoteConnectionCompletion {
            insert_text: command.to_string(),
            display_text: command.to_string(),
            description: description.to_string(),
        });
    }
}

fn push_saved_name_completions(
    completions: &mut Vec<RemoteConnectionCompletion>,
    prefix: &str,
    suffix: &str,
    store: &RemoteConnectionStore,
) {
    for saved in &store.saved {
        if !saved.name.starts_with(suffix) {
            continue;
        }
        completions.push(RemoteConnectionCompletion {
            insert_text: format!("{prefix}{}", saved.name),
            display_text: saved.name.clone(),
            description: format!("saved: {}", saved.target),
        });
    }
}

fn push_recent_target_completions(
    completions: &mut Vec<RemoteConnectionCompletion>,
    prefix: &str,
    suffix: &str,
    store: &RemoteConnectionStore,
) {
    for recent in &store.recent {
        if !recent.target.starts_with(suffix) {
            continue;
        }
        completions.push(RemoteConnectionCompletion {
            insert_text: format!("{prefix}{}", recent.target),
            display_text: recent.target.clone(),
            description: "recent remote project".to_string(),
        });
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_connections_replace_existing_name() {
        let mut store = RemoteConnectionStore::default();
        store.save_named("dev", "ssh://old/home/me/project");
        store.save_named("dev", "ssh://new/home/me/project");

        assert_eq!(store.saved.len(), 1);
        assert_eq!(store.saved[0].target, "ssh://new/home/me/project");
    }

    #[test]
    fn recent_connections_are_mru_and_capped() {
        let mut store = RemoteConnectionStore::default();
        for index in 0..(MAX_RECENT_REMOTE_PROJECTS + 3) {
            store.record_successful_open(format!("ssh://host/{index}"));
        }
        store.record_successful_open("ssh://host/4");

        assert_eq!(store.recent.len(), MAX_RECENT_REMOTE_PROJECTS);
        assert_eq!(store.recent[0].target, "ssh://host/4");
        assert_eq!(
            store
                .recent
                .iter()
                .filter(|entry| entry.target == "ssh://host/4")
                .count(),
            1
        );
    }

    #[test]
    fn completions_include_saved_recent_and_management_commands() {
        let mut store = RemoteConnectionStore::default();
        store.save_named("devbox", "ssh://devbox/home/me/project");
        store.record_successful_open("ssh://recent/home/me/project");

        let completions = completions_for_input("", &store);
        let inserts = completions
            .iter()
            .map(|completion| completion.insert_text.as_str())
            .collect::<Vec<_>>();

        assert!(inserts.contains(&"devbox"));
        assert!(inserts.contains(&"ssh://recent/home/me/project"));
        assert!(inserts.contains(&"reconnect"));
        assert!(inserts.contains(&"cancel"));
    }

    #[test]
    fn connection_names_are_prompt_friendly() {
        assert!(valid_connection_name("dev-box.1"));
        assert!(!valid_connection_name(""));
        assert!(!valid_connection_name("dev box"));
        assert!(!valid_connection_name("ssh://host/path"));
    }
}
