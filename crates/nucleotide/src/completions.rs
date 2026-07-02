// ABOUTME: Centralized command completion logic with deduplication for Nucleotide
// ABOUTME: Builds native prompt completions from Helix command metadata and editor state

use helix_core::fuzzy::fuzzy_match;
use helix_term::commands::{TYPABLE_COMMAND_LIST, TypableCommand};
use helix_view::{Editor, document::SCRATCH_BUFFER_NAME, editor::Config};
use nucleotide_ui::prompt_view::CompletionItem;
use once_cell::sync::Lazy;
use std::path::{MAIN_SEPARATOR, Path};

const RUNNABLE_COMMANDS: &[(&str, &str)] = &[
    ("run", "Show runnables for the focused Rust file"),
    ("runnables", "Show runnables for the focused Rust file"),
    ("run-nearest", "Run the nearest Rust runnable at the cursor"),
    ("run-file-tests", "Run tests for the focused Rust file"),
    ("run-last", "Run the last runnable again"),
    ("rerun", "Run the last runnable again"),
];

/// Static list of available settings keys derived from Helix's editor config.
pub static SETTINGS_KEYS: Lazy<Vec<String>> = Lazy::new(|| {
    let mut keys = Vec::new();
    collect_setting_keys(&serde_json::json!(Config::default()), &mut keys, None);
    keys.sort();
    keys.dedup();
    keys
});

#[derive(Clone, Debug)]
pub struct CommandCompletionCache {
    settings: Vec<String>,
    buffers: Vec<String>,
    languages: Vec<String>,
    configured_language_servers: Vec<String>,
    active_language_servers: Vec<String>,
    complete_filesystem_paths: bool,
}

impl CommandCompletionCache {
    pub fn from_editor(editor: &Editor) -> Self {
        let buffers = editor
            .documents
            .values()
            .map(|doc| {
                doc.relative_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| SCRATCH_BUFFER_NAME.to_string())
            })
            .collect();

        let text = "text".to_string();
        let mut languages: Vec<_> = editor
            .syn_loader
            .load()
            .language_configs()
            .map(|config| config.language_id.clone())
            .chain(std::iter::once(text))
            .collect();
        languages.sort();
        languages.dedup();

        let (configured_language_servers, active_language_servers) =
            if let Some(doc) = active_document(editor) {
                let configured = doc
                    .language_config()
                    .into_iter()
                    .flat_map(|config| &config.language_servers)
                    .map(|server| server.name.clone())
                    .collect();
                let active = doc
                    .language_servers()
                    .map(|server| server.name().to_string())
                    .collect();
                (configured, active)
            } else {
                (Vec::new(), Vec::new())
            };

        Self {
            settings: SETTINGS_KEYS.to_vec(),
            buffers,
            languages,
            configured_language_servers,
            active_language_servers,
            complete_filesystem_paths: true,
        }
    }

    pub fn with_filesystem_paths(mut self, enabled: bool) -> Self {
        self.complete_filesystem_paths = enabled;
        self
    }
}

impl Default for CommandCompletionCache {
    fn default() -> Self {
        Self {
            settings: SETTINGS_KEYS.to_vec(),
            buffers: Vec::new(),
            languages: Vec::new(),
            configured_language_servers: Vec::new(),
            active_language_servers: Vec::new(),
            complete_filesystem_paths: true,
        }
    }
}

/// Get command completions with deduplication of aliases.
pub fn get_command_completions(editor: &Editor, input: &str) -> Vec<CompletionItem> {
    let cache = CommandCompletionCache::from_editor(editor);
    get_command_completions_with_cache(input, Some(&cache))
}

/// Simplified version that doesn't require editor state.
pub fn get_command_completions_simple(input: &str) -> Vec<CompletionItem> {
    get_command_completions_with_cache(input, None)
}

/// Version that can use cached editor data for better completions.
pub fn get_command_completions_with_cache(
    input: &str,
    cache: Option<&CommandCompletionCache>,
) -> Vec<CompletionItem> {
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.is_empty() {
        complete_command_names("")
    } else if parts.len() == 1 && !input_ends_with_whitespace(input) {
        complete_command_names(parts[0])
    } else {
        let default_cache;
        let cache = if let Some(cache) = cache {
            cache
        } else {
            default_cache = CommandCompletionCache::default();
            &default_cache
        };
        complete_command_arguments_with_cache(&parts, input, cache)
    }
}

/// Complete command names, showing aliases but not as separate entries.
fn complete_command_names(pattern: &str) -> Vec<CompletionItem> {
    let mut matched_commands: Vec<(&TypableCommand, u16)> = Vec::new();

    for cmd in TYPABLE_COMMAND_LIST {
        if let Some((_, score)) = fuzzy_match(pattern, std::iter::once(cmd.name), false)
            .into_iter()
            .next()
        {
            matched_commands.push((cmd, score));
        } else {
            for alias in cmd.aliases {
                if let Some((_, score)) = fuzzy_match(pattern, std::iter::once(*alias), false)
                    .into_iter()
                    .next()
                {
                    matched_commands.push((cmd, score));
                    break;
                }
            }
        }
    }

    let mut items = matched_commands
        .into_iter()
        .map(|(cmd, score)| {
            let display_text = if cmd.aliases.is_empty() {
                None
            } else {
                Some(format!("{} ({})", cmd.name, cmd.aliases.join(", ")).into())
            };

            (
                CompletionItem {
                    text: cmd.name.to_string().into(),
                    description: Some(cmd.doc.to_string().into()),
                    display_text,
                },
                score,
            )
        })
        .collect::<Vec<_>>();

    for (name, description) in RUNNABLE_COMMANDS {
        if let Some((_, score)) = fuzzy_match(pattern, std::iter::once(*name), false)
            .into_iter()
            .next()
        {
            items.push((
                CompletionItem {
                    text: (*name).to_string().into(),
                    description: Some((*description).to_string().into()),
                    display_text: None,
                },
                score,
            ));
        }
    }

    items.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    items.into_iter().map(|(item, _)| item).collect()
}

/// Argument completion with cached editor data.
fn complete_command_arguments_with_cache(
    parts: &[&str],
    input: &str,
    cache: &CommandCompletionCache,
) -> Vec<CompletionItem> {
    let Some(context) = ArgumentContext::new(parts, input) else {
        return Vec::new();
    };

    let command = TYPABLE_COMMAND_LIST
        .iter()
        .find(|cmd| cmd.name == context.command || cmd.aliases.contains(&context.command));

    let Some(cmd) = command else {
        return Vec::new();
    };

    match cmd.name {
        "theme" if context.arg_index == 0 => {
            complete_themes(input, context.current_arg, context.command)
        }
        "toggle-option" | "set-option" | "get-option" if context.arg_index == 0 => {
            complete_settings(input, context.current_arg, cmd.name, &cache.settings)
        }
        "set-language" if context.arg_index == 0 => complete_list(
            input,
            context.current_arg,
            &cache.languages,
            false,
            |language| format!("Set language to {language}"),
        ),
        "buffer-close" | "buffer-close!" => {
            complete_list(input, context.current_arg, &cache.buffers, true, |buffer| {
                format!("Close buffer: {buffer}")
            })
        }
        "lsp-restart" => complete_list(
            input,
            context.current_arg,
            &cache.configured_language_servers,
            false,
            |server| format!("Restart language server: {server}"),
        ),
        "lsp-stop" => complete_list(
            input,
            context.current_arg,
            &cache.active_language_servers,
            false,
            |server| format!("Stop language server: {server}"),
        ),
        command if cache.complete_filesystem_paths && is_file_command(command) => {
            complete_filesystem_paths(
                input,
                context.current_arg,
                PathCompletionKind::FileOrDirectory,
                path_action_label(command, context.command),
            )
        }
        "change-current-directory" if cache.complete_filesystem_paths => complete_filesystem_paths(
            input,
            context.current_arg,
            PathCompletionKind::Directory,
            "Change directory to",
        ),
        _ if input_ends_with_whitespace(input) && parts.len() == 1 => {
            get_command_argument_hint(cmd).map_or_else(Vec::new, |hint| {
                vec![CompletionItem {
                    text: format!("{} {hint}", context.command).into(),
                    description: None,
                    display_text: Some(hint.into()),
                }]
            })
        }
        _ => Vec::new(),
    }
}

/// Complete theme names.
fn complete_themes(input: &str, current_arg: &str, command: &str) -> Vec<CompletionItem> {
    let mut names =
        helix_view::theme::Loader::read_names(&helix_loader::config_dir().join("themes"));

    for rt_dir in helix_loader::runtime_dirs() {
        let rt_names = helix_view::theme::Loader::read_names(&rt_dir.join("themes"));
        names.extend(rt_names);
    }

    names.push("default".into());
    names.push("base16_default".into());
    names.sort();
    names.dedup();

    fuzzy_match(current_arg, names, false)
        .into_iter()
        .map(|(name, _score)| CompletionItem {
            text: replace_current_arg(input, current_arg, &name).into(),
            description: Some(format!("Switch to {name} theme").into()),
            display_text: Some(format!("{command} {name}").into()),
        })
        .collect()
}

fn complete_settings(
    input: &str,
    current_arg: &str,
    command: &str,
    settings: &[String],
) -> Vec<CompletionItem> {
    let action = match command {
        "toggle-option" => "Toggle",
        "set-option" => "Set",
        "get-option" => "Get",
        _ => "Use",
    };

    complete_list(input, current_arg, settings, false, |setting| {
        format!("{action} the {setting} setting")
    })
}

fn complete_list(
    input: &str,
    current_arg: &str,
    candidates: &[String],
    path: bool,
    description: impl Fn(&str) -> String,
) -> Vec<CompletionItem> {
    fuzzy_match(current_arg, candidates.iter().map(String::as_str), path)
        .into_iter()
        .map(|(candidate, _score)| CompletionItem {
            text: replace_current_arg(input, current_arg, candidate).into(),
            description: Some(description(candidate).into()),
            display_text: None,
        })
        .collect()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PathCompletionKind {
    FileOrDirectory,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathCandidate {
    path: String,
    is_dir: bool,
}

impl AsRef<str> for PathCandidate {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum PathCandidateMatch {
    Reject,
    AcceptIncomplete,
    Accept,
}

fn complete_filesystem_paths(
    input: &str,
    current_arg: &str,
    kind: PathCompletionKind,
    action_label: &str,
) -> Vec<CompletionItem> {
    let is_tilde = current_arg == "~";
    let path = helix_stdx::path::expand_tilde(Path::new(current_arg));

    let (dir, file_name) = if current_arg.ends_with(MAIN_SEPARATOR) {
        (path, None)
    } else {
        let is_period = (current_arg.ends_with(&format!("{MAIN_SEPARATOR}."))
            && current_arg.len() > 2)
            || current_arg == ".";
        let file_name = if is_period {
            Some(".".to_string())
        } else {
            path.file_name()
                .and_then(|file| file.to_str().map(str::to_string))
        };

        let path = if is_period {
            path
        } else {
            match path.parent() {
                Some(path) if !path.as_os_str().is_empty() => std::borrow::Cow::Borrowed(path),
                _ => std::borrow::Cow::Owned(helix_stdx::env::current_working_dir()),
            }
        };

        (path, file_name)
    };

    let candidates = ignore::WalkBuilder::new(&dir)
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .max_depth(Some(1))
        .build()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let candidate_match = match kind {
                PathCompletionKind::FileOrDirectory => {
                    if entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_dir())
                    {
                        PathCandidateMatch::AcceptIncomplete
                    } else {
                        PathCandidateMatch::Accept
                    }
                }
                PathCompletionKind::Directory => {
                    if entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_dir())
                    {
                        PathCandidateMatch::Accept
                    } else {
                        PathCandidateMatch::Reject
                    }
                }
            };

            if candidate_match == PathCandidateMatch::Reject {
                return None;
            }

            let is_dir = entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir());
            let mut path = if is_tilde {
                entry.path().to_path_buf()
            } else {
                entry
                    .path()
                    .strip_prefix(&dir)
                    .unwrap_or(entry.path())
                    .to_path_buf()
            };

            if candidate_match == PathCandidateMatch::AcceptIncomplete {
                path.push("");
            }

            let path = path.into_os_string().into_string().ok()?;
            if path.is_empty() {
                return None;
            }

            Some(PathCandidate { path, is_dir })
        });

    let arg_prefix = argument_path_prefix(current_arg, file_name.as_deref());
    let matches = if let Some(file_name) = file_name {
        fuzzy_match(&file_name, candidates, true)
            .into_iter()
            .map(|(candidate, _score)| candidate)
            .collect()
    } else {
        let mut candidates: Vec<_> = candidates.collect();
        candidates
            .sort_by(|left, right| (!left.is_dir, &left.path).cmp(&(!right.is_dir, &right.path)));
        candidates
    };

    matches
        .into_iter()
        .map(|candidate| {
            let completed_arg = if is_tilde {
                candidate.path
            } else {
                format!("{arg_prefix}{}", candidate.path)
            };
            CompletionItem {
                text: replace_current_arg(input, current_arg, &completed_arg).into(),
                description: Some(format!("{action_label} {completed_arg}").into()),
                display_text: None,
            }
        })
        .collect()
}

fn argument_path_prefix(current_arg: &str, file_name: Option<&str>) -> String {
    if current_arg.ends_with(MAIN_SEPARATOR) {
        return current_arg.to_string();
    }

    file_name
        .and_then(|file_name| current_arg.strip_suffix(file_name))
        .unwrap_or("")
        .to_string()
}

fn replace_current_arg(input: &str, current_arg: &str, replacement: &str) -> String {
    if current_arg.is_empty() {
        format!("{input}{replacement}")
    } else if let Some(prefix) = input.strip_suffix(current_arg) {
        format!("{prefix}{replacement}")
    } else {
        format!("{input}{replacement}")
    }
}

fn collect_setting_keys(value: &serde_json::Value, keys: &mut Vec<String>, scope: Option<&str>) {
    let Some(map) = value.as_object() else {
        return;
    };

    for (key, value) in map {
        let key = match scope {
            Some(scope) => format!("{scope}.{key}"),
            None => key.clone(),
        };
        collect_setting_keys(value, keys, Some(&key));
        if !value.is_object() {
            keys.push(key);
        }
    }
}

fn active_document(editor: &Editor) -> Option<&helix_view::Document> {
    editor
        .tree
        .try_get(editor.tree.focus)
        .and_then(|view| editor.document(view.doc))
}

#[derive(Clone, Copy, Debug)]
struct ArgumentContext<'a> {
    command: &'a str,
    current_arg: &'a str,
    arg_index: usize,
}

impl<'a> ArgumentContext<'a> {
    fn new(parts: &'a [&'a str], input: &'a str) -> Option<Self> {
        let command = parts.first().copied()?;
        let trailing = input_ends_with_whitespace(input);
        let current_arg = if trailing {
            ""
        } else {
            parts.last().copied().unwrap_or("")
        };
        let arg_index = if trailing {
            parts.len().saturating_sub(1)
        } else {
            parts.len().saturating_sub(2)
        };

        Some(Self {
            command,
            current_arg,
            arg_index,
        })
    }
}

fn input_ends_with_whitespace(input: &str) -> bool {
    input.chars().last().is_some_and(char::is_whitespace)
}

fn is_file_command(command: &str) -> bool {
    matches!(
        command,
        "open"
            | "write"
            | "write!"
            | "write-buffer-close"
            | "write-buffer-close!"
            | "write-quit"
            | "write-quit!"
            | "vsplit"
            | "hsplit"
    )
}

fn path_action_label(command: &str, typed_command: &str) -> &'static str {
    match command {
        "open" | "vsplit" | "hsplit" => "Open",
        "write" | "write!" => "Write to",
        "write-buffer-close" | "write-buffer-close!" => "Write and close",
        "write-quit" | "write-quit!" => "Write and quit",
        _ if matches!(typed_command, "cd") => "Change directory to",
        _ => "Use",
    }
}

/// Get a hint about what arguments a command expects.
fn get_command_argument_hint(cmd: &TypableCommand) -> Option<String> {
    match cmd.name {
        "open" | "vsplit" | "hsplit" => Some("<file>".to_string()),
        "write"
        | "write!"
        | "write-buffer-close"
        | "write-buffer-close!"
        | "write-quit"
        | "write-quit!" => Some("[<file>]".to_string()),
        "buffer-close" | "buffer-close!" => Some("[<buffer>]".to_string()),
        "toggle-option" => Some("<setting>".to_string()),
        "set-option" => Some("<setting> <value>".to_string()),
        "get-option" => Some("<setting>".to_string()),
        "set-language" => Some("<language>".to_string()),
        "change-current-directory" => Some("<directory>".to_string()),
        "lsp-restart" | "lsp-stop" => Some("<language-server>".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_keys_are_derived_from_helix_config() {
        assert!(SETTINGS_KEYS.iter().any(|key| key == "search.smart-case"));
        assert!(SETTINGS_KEYS.iter().any(|key| key == "scroll-lines"));
        assert!(SETTINGS_KEYS.len() > 20);
    }

    #[test]
    fn cached_setting_completion_preserves_typed_alias() {
        let cache = CommandCompletionCache {
            settings: vec!["line-number".to_string(), "mouse".to_string()],
            ..CommandCompletionCache::default()
        };

        let items = get_command_completions_with_cache("toggle line", Some(&cache));

        assert!(
            items
                .iter()
                .any(|item| item.text.as_ref() == "toggle line-number")
        );
    }

    #[test]
    fn cached_context_completion_preserves_typed_aliases() {
        let cache = CommandCompletionCache {
            languages: vec!["rust".to_string()],
            buffers: vec!["src/main.rs".to_string()],
            configured_language_servers: vec!["rust-analyzer".to_string()],
            active_language_servers: vec!["rust-analyzer".to_string()],
            ..CommandCompletionCache::default()
        };

        let language_items = get_command_completions_with_cache("lang ru", Some(&cache));
        assert!(
            language_items
                .iter()
                .any(|item| item.text.as_ref() == "lang rust")
        );

        let buffer_items = get_command_completions_with_cache("bc src", Some(&cache));
        assert!(
            buffer_items
                .iter()
                .any(|item| item.text.as_ref() == "bc src/main.rs")
        );

        let lsp_items = get_command_completions_with_cache("lsp-restart rust", Some(&cache));
        assert!(
            lsp_items
                .iter()
                .any(|item| item.text.as_ref() == "lsp-restart rust-analyzer")
        );
    }

    #[test]
    fn filesystem_completion_replaces_the_full_prompt_argument() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("alpha.txt");
        std::fs::write(&file_path, "").unwrap();

        let partial_path = temp_dir.path().join("alp");
        let input = format!("open {}", partial_path.display());
        let expected = format!("open {}", file_path.display());

        let items = get_command_completions_with_cache(&input, None);

        assert!(items.iter().any(|item| item.text.as_ref() == expected));
    }

    #[test]
    fn filesystem_completion_can_be_disabled_for_remote_workspaces() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("alpha.txt");
        std::fs::write(&file_path, "").unwrap();

        let partial_path = temp_dir.path().join("alp");
        let input = format!("open {}", partial_path.display());
        let expected = format!("open {}", file_path.display());
        let cache = CommandCompletionCache::default().with_filesystem_paths(false);

        let items = get_command_completions_with_cache(&input, Some(&cache));

        assert!(!items.iter().any(|item| item.text.as_ref() == expected));
    }
}
