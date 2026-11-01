// ABOUTME: Typed host operations shared by native keymaps and typable commands
// ABOUTME: Validates Helix arguments and completes writes before yielding host ownership

use helix_core::command_line::{Args, ExpansionKind, TokenKind};
use helix_term::{commands::MappableCommand, compositor, ui::PromptEvent};
use helix_view::{Editor, expansion};
use std::path::{Path, PathBuf};

use super::NativeFileOpenAction;

/// Resolve an already expanded/quoted command argument as Helix's open command
/// does: preserve existing literal names, parse optional one-based line/column,
/// then expand tilde. Helix's equivalent args::parse_file is crate-private.
pub fn parse_open_target(argument: &str) -> (PathBuf, helix_core::Position) {
    let positioned = if Path::new(argument).exists() {
        None
    } else {
        let trimmed = argument.trim_end_matches(':');
        (|| {
            let mut parts = trimmed.rsplitn(3, ':');
            let col = parts.next()?.parse::<usize>().ok()?;
            let row = parts.next()?.parse::<usize>().ok()?;
            Some((
                PathBuf::from(parts.next()?),
                helix_core::Position::new(row.saturating_sub(1), col.saturating_sub(1)),
            ))
        })()
        .or_else(|| {
            let (path, row) = trimmed.rsplit_once(':')?;
            Some((
                PathBuf::from(path),
                helix_core::Position::new(row.parse::<usize>().ok()?.saturating_sub(1), 0),
            ))
        })
    };
    let (path, position) =
        positioned.unwrap_or_else(|| (PathBuf::from(argument), helix_core::Position::default()));
    (helix_stdx::path::expand_tilde(path).into_owned(), position)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceDirection {
    Left,
    Right,
    Up,
    Down,
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCommand {
    /// Drain incidental editor facts and project authoritative host focus before
    /// continuing. This is a reconciliation fence, not a user pane operation.
    Reconcile,
    Focus(WorkspaceDirection),
    Swap(WorkspaceDirection),
    /// Toggle only the focused pane's immediate parent, not all descendants.
    Transpose,
    Split {
        action: NativeFileOpenAction,
        new_document: bool,
    },
    Open {
        paths: Vec<String>,
        action: NativeFileOpenAction,
    },
    ClosePane {
        force: bool,
        quit: bool,
    },
    KeepOnly,
    QuitWorkspace {
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCommandRequest {
    pub command: WorkspaceCommand,
    /// Helix's parsed prefix count, not a second key interpreter.
    /// Structural commands run once, as in Helix; the suspended editing tail
    /// still receives the original count. Macro repetition is handled upstream.
    pub count: usize,
}

/// The host calls this before closing the last pane or quitting the workspace.
/// Closing a document is deliberately NOT a pane-close operation.
pub fn validate_workspace_quit(editor: &Editor, force: bool) -> Result<(), String> {
    if force {
        return Ok(());
    }
    let modified: Vec<_> = editor
        .documents()
        .filter(|doc| doc.is_modified())
        .map(|doc| doc.display_name().to_string())
        .collect();
    if modified.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} unsaved buffer{} remaining: {:?}",
            modified.len(),
            if modified.len() == 1 { "" } else { "s" },
            modified
        ))
    }
}

pub(super) fn is_workspace_command(command: &MappableCommand) -> bool {
    match command {
        MappableCommand::Static { name, .. } => static_command(name).is_some(),
        MappableCommand::Typable { name, .. } => canonical_typable(name).is_some_and(|name| {
            matches!(
                name,
                "open"
                    | "new"
                    | "vsplit"
                    | "hsplit"
                    | "vsplit-new"
                    | "hsplit-new"
                    | "quit"
                    | "quit!"
                    | "quit-all"
                    | "quit-all!"
                    | "cquit"
                    | "cquit!"
                    | "write-quit"
                    | "write-quit!"
                    | "write-quit-all"
                    | "write-quit-all!"
                    | "exit"
                    | "exit!"
            )
        }),
        _ => false,
    }
}

pub(super) fn requires_document(command: &MappableCommand) -> bool {
    matches!(command, MappableCommand::Typable { name, .. }
        if canonical_typable(name).is_some_and(|name| matches!(name, "write-quit" | "write-quit!" | "exit" | "exit!")))
}

fn canonical_typable(name: &str) -> Option<&'static str> {
    helix_term::commands::TYPABLE_COMMAND_MAP
        .get(name)
        .map(|cmd| cmd.name)
}

fn static_command(name: &str) -> Option<WorkspaceCommand> {
    use WorkspaceCommand::*;
    use WorkspaceDirection::*;
    Some(match name {
        "jump_view_left" => Focus(Left),
        "jump_view_right" => Focus(Right),
        "jump_view_up" => Focus(Up),
        "jump_view_down" => Focus(Down),
        "rotate_view" => Focus(Next),
        "rotate_view_reverse" => Focus(Previous),
        "swap_view_left" => Swap(Left),
        "swap_view_right" => Swap(Right),
        "swap_view_up" => Swap(Up),
        "swap_view_down" => Swap(Down),
        "transpose_view" => Transpose,
        "hsplit" | "hsplit_new" => Split {
            action: NativeFileOpenAction::HorizontalSplit,
            new_document: name == "hsplit_new",
        },
        "vsplit" | "vsplit_new" => Split {
            action: NativeFileOpenAction::VerticalSplit,
            new_document: name == "vsplit_new",
        },
        "wclose" => ClosePane {
            force: false,
            quit: false,
        },
        "wonly" => KeepOnly,
        _ => return None,
    })
}

/// Prepare, but never apply, a structural command. All argument parsing,
/// expansion, write flags and flushing use Helix's own public contracts.
pub(super) fn prepare(
    command: &MappableCommand,
    cx: &mut helix_term::commands::Context<'_>,
    has_document: bool,
) -> Result<WorkspaceCommandRequest, String> {
    let count = cx.count();
    let operation = match command {
        MappableCommand::Static { name, .. } => static_command(name).unwrap(),
        MappableCommand::Typable { name, args, .. } => {
            let cmd = helix_term::commands::TYPABLE_COMMAND_MAP
                .get(name.as_str())
                .unwrap();
            let parsed = Args::parse(args, cmd.signature, true, |token| {
                let needs_document = match token.kind {
                    TokenKind::Unquoted
                    | TokenKind::Quoted(_)
                    | TokenKind::Expansion(ExpansionKind::Unicode) => false,
                    TokenKind::Expand => token.content.contains('%'),
                    _ => true,
                };
                if !has_document && needs_document {
                    return Err("Command expansion requires a focused document".into());
                }
                expansion::expand(cx.editor, token).map_err(|err| err.into())
            })
            .map_err(|err| format!("'{}': {err}", cmd.name))?;
            let force = cmd.name.ends_with('!');
            match cmd.name {
                "open" | "vsplit" | "hsplit" => {
                    let action = match cmd.name {
                        "vsplit" => NativeFileOpenAction::VerticalSplit,
                        "hsplit" => NativeFileOpenAction::HorizontalSplit,
                        _ => NativeFileOpenAction::Replace,
                    };
                    if parsed.is_empty() {
                        WorkspaceCommand::Split {
                            action,
                            new_document: false,
                        }
                    } else {
                        WorkspaceCommand::Open {
                            paths: parsed.into_iter().map(|path| path.into_owned()).collect(),
                            action,
                        }
                    }
                }
                "new" | "vsplit-new" | "hsplit-new" => WorkspaceCommand::Split {
                    action: match cmd.name {
                        "vsplit-new" => NativeFileOpenAction::VerticalSplit,
                        "hsplit-new" => NativeFileOpenAction::HorizontalSplit,
                        _ => NativeFileOpenAction::Replace,
                    },
                    new_document: true,
                },
                name => {
                    let mut context = compositor::Context {
                        editor: cx.editor,
                        scroll: None,
                        jobs: cx.jobs,
                    };
                    if matches!(name, "cquit" | "cquit!") {
                        context.editor.exit_code = parsed
                            .first()
                            .and_then(|code| code.parse().ok())
                            .unwrap_or(1);
                    }
                    let write = match name {
                        "write-quit" | "write-quit!" => {
                            Some(if force { "write!" } else { "write" })
                        }
                        "write-quit-all" | "write-quit-all!" => {
                            Some(if force { "write-all!" } else { "write-all" })
                        }
                        "exit" | "exit!" if helix_view::doc!(context.editor).is_modified() => {
                            Some(if force { "write!" } else { "write" })
                        }
                        _ => None,
                    };
                    if let Some(write) = write {
                        context.editor.status_msg = None;
                        let writer = helix_term::commands::TYPABLE_COMMAND_MAP
                            .get(write)
                            .unwrap();
                        let result = (writer.fun)(&mut context, parsed, PromptEvent::Validate);
                        // Helix's forced write-all-and-quit deliberately ignores
                        // per-buffer write failures (but still flushes pending writes).
                        if name != "write-quit-all!" {
                            result.map_err(|err| format!("'{}': {err}", cmd.name))?;
                        }
                    }
                    context
                        .block_try_flush_writes()
                        .map_err(|err| format!("'{}': {err}", cmd.name))?;
                    // Formatting/code-action followups report a failed save via
                    // Editor::set_error rather than returning a failed Job.
                    // A completed flush alone therefore does not validate :wq.
                    if write.is_some()
                        && name != "write-quit-all!"
                        && let Some((error, helix_view::editor::Severity::Error)) =
                            &context.editor.status_msg
                    {
                        return Err(format!("'{}': {error}", cmd.name));
                    }
                    if matches!(
                        name,
                        "quit-all"
                            | "quit-all!"
                            | "write-quit-all"
                            | "write-quit-all!"
                            | "cquit"
                            | "cquit!"
                    ) {
                        WorkspaceCommand::QuitWorkspace { force }
                    } else {
                        WorkspaceCommand::ClosePane { force, quit: true }
                    }
                }
            }
        }
        _ => unreachable!("prepare is only called for classified host commands"),
    };
    Ok(WorkspaceCommandRequest {
        command: operation,
        count,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_open_target;
    use helix_core::Position;
    use std::path::PathBuf;

    #[test]
    fn open_targets_preserve_helix_positions_literal_names_and_tilde_order() {
        for (argument, path, position) in [
            (
                "folder/file.rs:17:4::",
                "folder/file.rs",
                Position::new(16, 3),
            ),
            (
                "name:with:colon.rs:8",
                "name:with:colon.rs",
                Position::new(7, 0),
            ),
            ("file.rs:0:0", "file.rs", Position::default()),
            ("file.rs:nope", "file.rs:nope", Position::default()),
            ("file.rs:17:nope", "file.rs:17:nope", Position::default()),
        ] {
            assert_eq!(parse_open_target(argument), (PathBuf::from(path), position));
        }
        #[cfg(not(windows))]
        {
            let temp = tempfile::tempdir().unwrap();
            let literal = temp.path().join("literal:17:4");
            std::fs::write(&literal, "real colon name").unwrap();
            assert_eq!(
                parse_open_target(literal.to_str().unwrap()),
                (literal, Position::default())
            );
        }
        if let Ok(home) = helix_stdx::path::home_dir() {
            assert_eq!(
                parse_open_target("~/not-yet-created.rs:12:3"),
                (home.join("not-yet-created.rs"), Position::new(11, 2))
            );
        }
    }
}
