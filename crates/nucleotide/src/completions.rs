// ABOUTME: Centralized command completion logic with deduplication for Nucleotide
// ABOUTME: Bridges between Helix's completion system and Nucleotide's UI layer

use helix_core::fuzzy::fuzzy_match;
use helix_term::commands::{TypableCommand, TYPABLE_COMMAND_LIST};
use helix_term::ui::completers;
use helix_view::Editor;
use nucleotide_ui::prompt_view::CompletionItem;
use once_cell::sync::Lazy;

/// Static list of available settings keys
pub static SETTINGS_KEYS: Lazy<Vec<String>> = Lazy::new(|| {
    // Hardcode some common settings to test if the completion logic works
    vec![
        "line-numbers".to_string(),
        "auto-pairs".to_string(),
        "auto-save".to_string(),
        "auto-format".to_string(),
        "auto-completion".to_string(),
        "auto-info".to_string(),
        "cursorline".to_string(),
        "cursorcolumn".to_string(),
        "color-modes".to_string(),
        "true-color".to_string(),
        "search.smart-case".to_string(),
        "search.wrap-around".to_string(),
        "lsp.display-messages".to_string(),
        "lsp.display-inlay-hints".to_string(),
        "rulers".to_string(),
        "whitespace.render".to_string(),
        "whitespace.characters".to_string(),
        "indent-guides.render".to_string(),
        "indent-guides.character".to_string(),
        "mouse".to_string(),
        "middle-click-paste".to_string(),
        "scroll-lines".to_string(),
        "shell".to_string(),
        "file-picker.hidden".to_string(),
        "file-picker.follow-symlinks".to_string(),
        "file-picker.deduplicate-links".to_string(),
        "file-picker.parents".to_string(),
        "file-picker.ignore".to_string(),
        "file-picker.git-ignore".to_string(),
        "file-picker.git-global".to_string(),
        "file-picker.git-exclude".to_string(),
        "file-picker.max-depth".to_string(),
    ]
});

/// Get command completions with deduplication of aliases
pub fn get_command_completions(editor: &Editor, input: &str) -> Vec<CompletionItem> {
    let parts: Vec<&str> = input.split_whitespace().collect();

    // Check if we're completing a command or arguments
    if parts.is_empty() || (parts.len() == 1 && !input.ends_with(' ')) {
        // Complete command names with deduplication
        complete_command_names(parts.first().copied().unwrap_or(""))
    } else {
        // Complete command arguments
        complete_command_arguments(editor, &parts, input)
    }
}

/// Simplified version that doesn't require editor (for use in closures)
/// Takes optional pre-cached data for completions that need context
pub fn get_command_completions_simple(input: &str) -> Vec<CompletionItem> {
    get_command_completions_with_cache(input, None)
}

/// Version that can use cached settings for better completions
pub fn get_command_completions_with_cache(
    input: &str,
    cached_settings: Option<Vec<String>>,
) -> Vec<CompletionItem> {
    let parts: Vec<&str> = input.split_whitespace().collect();

    // Check if we're completing a command or arguments
    if parts.is_empty() {
        complete_command_names("")
    } else if parts.len() == 1 && !input.ends_with(' ') {
        // Still typing the command name (e.g., "tog" for "toggle")
        complete_command_names(parts[0])
    } else {
        // We have a command and are completing arguments
        complete_command_arguments_with_cache(&parts, input, cached_settings)
    }
}

/// Complete command names, showing aliases but not as separate entries
fn complete_command_names(pattern: &str) -> Vec<CompletionItem> {
    // First, find all commands that match (either by name or alias)
    let mut matched_commands: Vec<(&TypableCommand, u16)> = Vec::new();

    for cmd in TYPABLE_COMMAND_LIST {
        // Check if the pattern matches the primary name
        if let Some((_, score)) = fuzzy_match(pattern, std::iter::once(cmd.name), false)
            .into_iter()
            .next()
        {
            matched_commands.push((cmd, score));
        } else {
            // Check if pattern matches any alias
            for alias in cmd.aliases {
                if let Some((_, score)) = fuzzy_match(pattern, std::iter::once(*alias), false)
                    .into_iter()
                    .next()
                {
                    matched_commands.push((cmd, score));
                    break; // Only need to match once per command
                }
            }
        }
    }

    // Sort by score (higher scores first)
    matched_commands.sort_by(|a, b| b.1.cmp(&a.1));

    // Convert to CompletionItems with formatted display
    matched_commands
        .into_iter()
        .map(|(cmd, _score)| {
            // Format the display text with aliases
            let display_text = if cmd.aliases.is_empty() {
                None // No need for separate display text if there are no aliases
            } else {
                Some(format!("{} ({})", cmd.name, cmd.aliases.join(", ")).into())
            };

            CompletionItem {
                text: cmd.name.to_string().into(), // Only the command name for insertion
                description: Some(cmd.doc.to_string().into()),
                display_text, // Optional display text with aliases
            }
        })
        .collect()
}

/// Complete command arguments using Helix's completers
fn complete_command_arguments(editor: &Editor, parts: &[&str], input: &str) -> Vec<CompletionItem> {
    let command_name = parts[0];

    // Find the command (checking both name and aliases)
    let command = TYPABLE_COMMAND_LIST
        .iter()
        .find(|cmd| cmd.name == command_name || cmd.aliases.contains(&command_name));

    if let Some(cmd) = command {
        // Get the current argument being typed (or empty string if at a space)
        let current_arg = if input.ends_with(' ') {
            ""
        } else {
            parts.last().copied().unwrap_or("")
        };

        // Special handling for specific commands with custom completion logic
        match cmd.name {
            "theme" => complete_themes(current_arg),
            "toggle" | "set" | "get" => {
                // Use the setting completer and format with command prefix
                let command = cmd.name;
                completers::setting(editor, current_arg)
                    .into_iter()
                    .map(|(_, text)| CompletionItem {
                        text: format!("{} {}", command, text.content).into(),
                        description: Some(
                            format!(
                                "{} the {} setting",
                                if command == "toggle" {
                                    "Toggle"
                                } else if command == "set" {
                                    "Set"
                                } else {
                                    "Get"
                                },
                                text.content
                            )
                            .into(),
                        ),
                        display_text: None,
                    })
                    .collect()
            }
            "language" => {
                // Use the language completer and format with command prefix
                completers::language(editor, current_arg)
                    .into_iter()
                    .map(|(_, text)| CompletionItem {
                        text: format!("language {}", text.content).into(),
                        description: Some(format!("Set language to {}", text.content).into()),
                        display_text: None,
                    })
                    .collect()
            }
            "buffer-close" | "buffer-close!" => {
                // Use the buffer completer and format with command prefix
                let command = cmd.name;
                completers::buffer(editor, current_arg)
                    .into_iter()
                    .map(|(_, span)| {
                        // Extract content from the Span - span.content is a Cow<str>
                        CompletionItem {
                            text: format!("{} {}", command, span.content).into(),
                            description: Some(format!("Close buffer: {}", span.content).into()),
                            display_text: None,
                        }
                    })
                    .collect()
            }
            "lsp-restart" | "lsp-stop" => {
                // Use the language server completer and format with command prefix
                let command = cmd.name;
                completers::configured_language_servers(editor, current_arg)
                    .into_iter()
                    .map(|(_, text)| CompletionItem {
                        text: format!("{} {}", command, text.content).into(),
                        description: Some(
                            format!(
                                "{} language server: {}",
                                if command == "lsp-restart" {
                                    "Restart"
                                } else {
                                    "Stop"
                                },
                                text.content
                            )
                            .into(),
                        ),
                        display_text: None,
                    })
                    .collect()
            }
            "open" | "edit" | "e" | "o" | "write" | "w" | "cd" => {
                // Use the filename completer for file-based commands
                // For file paths, we don't include the command prefix to keep them readable
                let command = cmd.name;
                completers::filename(editor, current_arg)
                    .into_iter()
                    .map(|(_, text)| CompletionItem {
                        text: text.content.to_string().into(),
                        description: Some(
                            format!(
                                "{} {}",
                                match command {
                                    "open" | "edit" | "e" | "o" => "Open",
                                    "write" | "w" => "Write to",
                                    "cd" => "Change directory to",
                                    _ => "Use",
                                },
                                text.content
                            )
                            .into(),
                        ),
                        display_text: None,
                    })
                    .collect()
            }
            _ => {
                // For other commands, return empty
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

/// Complete theme names
fn complete_themes(prefix: &str) -> Vec<CompletionItem> {
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

    fuzzy_match(prefix, names, false)
        .into_iter()
        .map(|(name, _score)| CompletionItem {
            text: format!("theme {}", name).into(),
            description: Some(format!("Switch to {} theme", name).into()),
            display_text: None,
        })
        .collect()
}

/// Argument completion with optional cached data
fn complete_command_arguments_with_cache(
    parts: &[&str],
    input: &str,
    cached_settings: Option<Vec<String>>,
) -> Vec<CompletionItem> {
    let command_name = parts[0];

    // Find the command (checking both name and aliases)
    let command = TYPABLE_COMMAND_LIST
        .iter()
        .find(|cmd| cmd.name == command_name || cmd.aliases.contains(&command_name));

    if let Some(cmd) = command {
        // Get the current argument being typed
        // When typing "toggle line", parts = ["toggle", "line"]
        // We want "line" as the current_arg
        let current_arg = if parts.len() > 1 {
            if input.ends_with(' ') {
                // "toggle " -> empty string for new argument
                ""
            } else {
                // "toggle line" -> "line"
                parts[1]
            }
        } else {
            ""
        };

        // Handle completions based on command type
        // Note: "toggle" is an alias for "toggle-option", "set" for "set-option", etc.
        match cmd.name {
            "theme" => complete_themes(current_arg),
            "toggle-option" | "set-option" | "get-option" => {
                // Use cached settings if available
                if let Some(settings) = cached_settings {
                    use helix_core::fuzzy::fuzzy_match;
                    let matches =
                        fuzzy_match(current_arg, settings.iter().map(|s| s.as_str()), false);

                    // Use the command that was actually typed (which might be an alias)
                    let typed_command = command_name;

                    matches
                        .into_iter()
                        .map(|(setting, _score)| CompletionItem {
                            text: format!("{} {}", typed_command, setting).into(),
                            description: Some(
                                format!(
                                    "{} the {} setting",
                                    if cmd.name == "toggle-option" {
                                        "Toggle"
                                    } else if cmd.name == "set-option" {
                                        "Set"
                                    } else {
                                        "Get"
                                    },
                                    setting
                                )
                                .into(),
                            ),
                            display_text: None,
                        })
                        .collect()
                } else {
                    // Fallback hint when no cached settings available
                    if input.ends_with(' ') && parts.len() == 1 {
                        vec![CompletionItem {
                            text: "<setting>".to_string().into(),
                            description: Some(
                                format!(
                                    "{} a configuration setting",
                                    if cmd.name == "toggle" {
                                        "Toggle"
                                    } else if cmd.name == "set" {
                                        "Set"
                                    } else {
                                        "Get"
                                    }
                                )
                                .into(),
                            ),
                            display_text: None,
                        }]
                    } else {
                        Vec::new()
                    }
                }
            }
            _ => {
                // For other commands, we can't provide completions without editor context
                // But we can at least show helpful information
                if input.ends_with(' ') && parts.len() == 1 {
                    // Show a hint about what arguments the command expects
                    if let Some(hint) = get_command_argument_hint(cmd) {
                        vec![CompletionItem {
                            text: hint.into(),
                            description: None,
                            display_text: None,
                        }]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
        }
    } else {
        Vec::new()
    }
}

/// Get a hint about what arguments a command expects
fn get_command_argument_hint(cmd: &TypableCommand) -> Option<String> {
    match cmd.name {
        "open" | "edit" | "e" | "o" => Some("<file>".to_string()),
        "write" | "w" => Some("[<file>]".to_string()),
        "buffer-close" | "bc" => Some("[<buffer>]".to_string()),
        "toggle" => Some("<setting>".to_string()),
        "set" => Some("<setting> <value>".to_string()),
        "get" => Some("<setting>".to_string()),
        "language" => Some("<language>".to_string()),
        "cd" => Some("<directory>".to_string()),
        "lsp-restart" | "lsp-stop" => Some("<language-server>".to_string()),
        _ => None,
    }
}
