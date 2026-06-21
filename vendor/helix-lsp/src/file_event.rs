use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Weak,
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use tokio::sync::mpsc;

use crate::{lsp, Client, LanguageServerId};

enum Event {
    FileChanged {
        path: PathBuf,
        typ: lsp::FileChangeType,
    },
    Register {
        client_id: LanguageServerId,
        client: Weak<Client>,
        registration_id: String,
        options: lsp::DidChangeWatchedFilesRegistrationOptions,
    },
    Unregister {
        client_id: LanguageServerId,
        registration_id: String,
    },
    RemoveClient {
        client_id: LanguageServerId,
    },
}

#[derive(Default)]
struct ClientState {
    client: Weak<Client>,
    registered: HashMap<String, Vec<RegisteredWatcher>>,
}

struct RegisteredWatcher {
    globset: GlobSet,
    kind: lsp::WatchKind,
}

impl RegisteredWatcher {
    fn is_match(&self, path: &Path, typ: lsp::FileChangeType) -> bool {
        self.kind.contains(watch_kind_for_file_change(typ)) && self.globset.is_match(path)
    }
}

/// The Handler uses a dedicated tokio task to respond to file change events by
/// forwarding changes to LSPs that have registered for notifications with a
/// matching glob.
///
/// When an LSP registers for the DidChangeWatchedFiles notification, the
/// Handler is notified by sending the registration details in addition to a
/// weak reference to the LSP client. This is done so that the Handler can have
/// access to the client without preventing the client from being dropped if it
/// is closed and the Handler isn't properly notified.
#[derive(Clone, Debug)]
pub struct Handler {
    tx: mpsc::UnboundedSender<Event>,
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(Self::run(rx));
        Self { tx }
    }

    pub fn register(
        &self,
        client_id: LanguageServerId,
        client: Weak<Client>,
        registration_id: String,
        options: lsp::DidChangeWatchedFilesRegistrationOptions,
    ) {
        let _ = self.tx.send(Event::Register {
            client_id,
            client,
            registration_id,
            options,
        });
    }

    pub fn unregister(&self, client_id: LanguageServerId, registration_id: String) {
        let _ = self.tx.send(Event::Unregister {
            client_id,
            registration_id,
        });
    }

    pub fn file_changed(&self, path: PathBuf) {
        self.file_event(path, lsp::FileChangeType::CHANGED);
    }

    pub fn file_event(&self, path: PathBuf, typ: lsp::FileChangeType) {
        let _ = self.tx.send(Event::FileChanged { path, typ });
    }

    pub fn remove_client(&self, client_id: LanguageServerId) {
        let _ = self.tx.send(Event::RemoveClient { client_id });
    }

    async fn run(mut rx: mpsc::UnboundedReceiver<Event>) {
        let mut state: HashMap<LanguageServerId, ClientState> = HashMap::new();
        while let Some(event) = rx.recv().await {
            match event {
                Event::FileChanged { path, typ } => {
                    log::debug!("Received file event for {:?}", &path);

                    state.retain(|id, client_state| {
                        if !client_state
                            .registered
                            .values()
                            .flatten()
                            .any(|watcher| watcher.is_match(&path, typ))
                        {
                            return true;
                        }
                        let Some(client) = client_state.client.upgrade() else {
                            log::warn!("LSP client was dropped: {id}");
                            return false;
                        };
                        let Ok(uri) = lsp::Url::from_file_path(&path) else {
                            return true;
                        };
                        log::debug!(
                            "Sending didChangeWatchedFiles notification to client '{}'",
                            client.name()
                        );
                        client.did_change_watched_files(vec![lsp::FileEvent {
                            uri,
                            typ,
                        }]);
                        true
                    });
                }
                Event::Register {
                    client_id,
                    client,
                    registration_id,
                    options: ops,
                } => {
                    log::debug!(
                        "Registering didChangeWatchedFiles for client '{}' with id '{}'",
                        client_id,
                        registration_id
                    );

                    let entry = state.entry(client_id).or_default();
                    entry.client = client;

                    let watchers = registered_watchers_from_options(ops);
                    if watchers.is_empty() {
                        log::warn!(
                            "Ignoring didChangeWatchedFiles registration '{}' with no supported watchers",
                            registration_id
                        );
                        entry.registered.remove(&registration_id);
                    } else {
                        entry.registered.insert(registration_id, watchers);
                    }

                    if entry.registered.is_empty() {
                        state.remove(&client_id);
                    }
                }
                Event::Unregister {
                    client_id,
                    registration_id,
                } => {
                    log::debug!(
                        "Unregistering didChangeWatchedFiles with id '{}' for client '{}'",
                        registration_id,
                        client_id
                    );
                    if let Some(client_state) = state.get_mut(&client_id) {
                        client_state.registered.remove(&registration_id);
                        if client_state.registered.is_empty() {
                            state.remove(&client_id);
                        }
                    }
                }
                Event::RemoveClient { client_id } => {
                    log::debug!("Removing LSP client: {client_id}");
                    state.remove(&client_id);
                }
            }
        }
    }
}

fn registered_watchers_from_options(
    options: lsp::DidChangeWatchedFilesRegistrationOptions,
) -> Vec<RegisteredWatcher> {
    options
        .watchers
        .into_iter()
        .filter_map(|watcher| {
            let lsp::GlobPattern::String(pattern) = watcher.glob_pattern else {
                log::warn!(
                    "Ignoring didChangeWatchedFiles watcher with unsupported relative pattern"
                );
                return None;
            };

            let globset = match build_globset(&pattern) {
                Ok(globset) => globset,
                Err(err) => {
                    log::warn!(
                        "Ignoring invalid didChangeWatchedFiles glob pattern '{pattern}': {err}"
                    );
                    return None;
                }
            };

            Some(RegisteredWatcher {
                globset,
                kind: watcher.kind.unwrap_or_else(all_watch_kinds),
            })
        })
        .collect()
}

fn build_globset(pattern: &str) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    builder.add(GlobBuilder::new(pattern).build()?);
    builder.build()
}

fn all_watch_kinds() -> lsp::WatchKind {
    lsp::WatchKind::Create | lsp::WatchKind::Change | lsp::WatchKind::Delete
}

fn watch_kind_for_file_change(typ: lsp::FileChangeType) -> lsp::WatchKind {
    if typ == lsp::FileChangeType::CREATED {
        lsp::WatchKind::Create
    } else if typ == lsp::FileChangeType::DELETED {
        lsp::WatchKind::Delete
    } else {
        lsp::WatchKind::Change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(
        glob_pattern: impl Into<lsp::GlobPattern>,
        kind: Option<lsp::WatchKind>,
    ) -> lsp::DidChangeWatchedFilesRegistrationOptions {
        lsp::DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![lsp::FileSystemWatcher {
                glob_pattern: glob_pattern.into(),
                kind,
            }],
        }
    }

    #[test]
    fn watched_file_registration_defaults_to_all_event_kinds() {
        let watchers = registered_watchers_from_options(options("**/*.rs".to_string(), None));

        assert_eq!(watchers.len(), 1);
        assert!(watchers[0].is_match(Path::new("src/main.rs"), lsp::FileChangeType::CREATED));
        assert!(watchers[0].is_match(Path::new("src/main.rs"), lsp::FileChangeType::CHANGED));
        assert!(watchers[0].is_match(Path::new("src/main.rs"), lsp::FileChangeType::DELETED));
    }

    #[test]
    fn watched_file_registration_honours_event_kind_mask() {
        let watchers = registered_watchers_from_options(options(
            "**/*.rs".to_string(),
            Some(lsp::WatchKind::Create | lsp::WatchKind::Delete),
        ));

        assert_eq!(watchers.len(), 1);
        assert!(watchers[0].is_match(Path::new("src/lib.rs"), lsp::FileChangeType::CREATED));
        assert!(!watchers[0].is_match(Path::new("src/lib.rs"), lsp::FileChangeType::CHANGED));
        assert!(watchers[0].is_match(Path::new("src/lib.rs"), lsp::FileChangeType::DELETED));
    }

    #[test]
    fn watched_file_registration_rejects_relative_patterns_when_not_advertised() {
        let watchers = registered_watchers_from_options(options(
            lsp::GlobPattern::Relative(lsp::RelativePattern {
                base_uri: lsp::OneOf::Right(lsp::Url::parse("file:///tmp").unwrap()),
                pattern: "**/*.rs".to_string(),
            }),
            None,
        ));

        assert!(watchers.is_empty());
    }
}
