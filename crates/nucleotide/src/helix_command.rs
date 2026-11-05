// ABOUTME: Adapter for executing Helix typable commands from native UI code
// ABOUTME: Keeps terminal command execution details out of workspace/UI modules

use helix_term::commands::{MappableCommand, TYPABLE_COMMAND_LIST, TYPABLE_COMMAND_MAP};

/// A native command prompt's temporary theme, with rollback owned by Helix.
#[derive(Default)]
pub(crate) struct ThemePreview {
    name: Option<String>,
}

impl ThemePreview {
    pub(crate) fn update(
        &mut self,
        editor: &mut helix_view::Editor,
        candidate: Option<&str>,
    ) -> bool {
        use helix_core::command_line::{TokenKind, Tokenizer, split};

        let name = candidate.and_then(|input| {
            let (command, args, _) = split(input.strip_prefix(':').unwrap_or(input));
            if command != "theme" {
                return None;
            }
            let mut tokens = Tokenizer::new(args, true);
            let token = tokens.next()?.ok()?;
            // Never evaluate expansions (especially shell commands) for a preview.
            let literal = match token.kind {
                TokenKind::Unquoted | TokenKind::Quoted(_) => true,
                TokenKind::Expand => !token.content.contains('%'),
                _ => false,
            };
            (literal && tokens.next().is_none()).then_some(token.content)
        });

        if let Some(name) = name {
            if self.name.as_deref() == Some(name.as_ref()) && editor.theme.name() == name {
                return true;
            }
            if let Ok(theme) = editor.theme_loader.load(&name)
                && editor.set_theme_preview(theme).is_ok()
            {
                self.name = Some(name.into_owned());
                return true;
            }
        }

        if self.name.take().is_some()
            && let Err(error) = editor.unset_theme_preview()
        {
            nucleotide_logging::warn!(%error, "Failed to restore theme preview");
        }
        false
    }
}

pub(crate) fn execute_command_line(
    editor: &mut helix_view::Editor,
    jobs: &mut helix_term::job::Jobs,
    command: &str,
) {
    match command_lookup(command) {
        CommandLookup::Empty => {}
        CommandLookup::Unknown(cmd_name) => {
            editor.set_error(format!("no such command: '{cmd_name}'"));
        }
        CommandLookup::Found(command) => {
            let mut cx = helix_term::commands::Context {
                register: None,
                count: None,
                editor,
                callback: Vec::new(),
                on_next_key_callback: None,
                jobs,
            };
            command.execute(&mut cx);
        }
    }
}

#[derive(Debug)]
pub(crate) enum CommandLookup {
    Empty,
    Found(MappableCommand),
    Unknown(String),
}

pub(crate) fn command_lookup(command: &str) -> CommandLookup {
    let (cmd_name, args, _) = helix_core::command_line::split(command);
    if cmd_name.is_empty() {
        return CommandLookup::Empty;
    }

    if cmd_name.parse::<usize>().is_ok() && args.trim().is_empty() {
        return TYPABLE_COMMAND_MAP.get("goto").map_or_else(
            || CommandLookup::Unknown("goto".to_string()),
            |cmd| CommandLookup::Found(typable_command(cmd.name, cmd_name, cmd.doc)),
        );
    }

    let command = TYPABLE_COMMAND_MAP.get(cmd_name).copied().or_else(|| {
        TYPABLE_COMMAND_LIST
            .iter()
            .find(|cmd| cmd.aliases.contains(&cmd_name))
    });

    if let Some(cmd) = command {
        return CommandLookup::Found(typable_command(cmd.name, args, cmd.doc));
    }

    if args.trim().is_empty()
        && let Some(cmd) = MappableCommand::STATIC_COMMAND_LIST
            .iter()
            .find(|cmd| cmd.name() == cmd_name)
            .cloned()
    {
        return CommandLookup::Found(cmd);
    }

    CommandLookup::Unknown(cmd_name.to_string())
}

fn typable_command(name: &str, args: &str, doc: &str) -> MappableCommand {
    MappableCommand::Typable {
        name: name.to_string(),
        args: args.to_string(),
        doc: if args.is_empty() {
            doc.to_string()
        } else {
            format!(":{name} {args:?}")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::{ArcSwap, access::Map};
    use helix_core::syntax;
    use helix_term::job::Jobs;
    use helix_view::{
        Editor,
        editor::{Action, Config},
        graphics::Rect,
        handlers::{
            Handlers, completion::CompletionHandler, word_index::Handler as WordIndexHandler,
        },
        theme,
    };
    use std::sync::Arc;

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_links_tx, _) = tokio::sync::mpsc::channel(1);
        let (pull_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
        let (pull_all_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
        let (code_action_hint_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            document_links: doc_links_tx,
            word_index: WordIndexHandler::spawn(),
            pull_diagnostics: pull_diagnostics_tx,
            pull_all_documents_diagnostics: pull_all_diagnostics_tx,
            code_action_hint: code_action_hint_tx,
        }
    }

    fn test_editor() -> Editor {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
            helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        );
        editor.new_file(Action::VerticalSplit);
        editor
    }

    fn unwrap_typable(lookup: CommandLookup) -> (String, String) {
        match lookup {
            CommandLookup::Found(MappableCommand::Typable { name, args, .. }) => (name, args),
            other => panic!("expected typable command, got {other:?}"),
        }
    }

    fn unwrap_static(lookup: CommandLookup) -> String {
        match lookup {
            CommandLookup::Found(MappableCommand::Static { name, .. }) => name.to_string(),
            other => panic!("expected static command, got {other:?}"),
        }
    }

    #[test]
    fn numeric_command_resolves_to_goto() {
        let (name, args) = unwrap_typable(command_lookup("42"));

        assert_eq!(name, "goto");
        assert_eq!(args, "42");
    }

    #[test]
    fn command_alias_resolves_to_canonical_name() {
        let (name, args) = unwrap_typable(command_lookup("w test.txt"));

        assert_eq!(name, "write");
        assert_eq!(args, "test.txt");
    }

    #[test]
    fn command_lookup_preserves_arguments() {
        let (name, args) = unwrap_typable(command_lookup("open src/main.rs"));

        assert_eq!(name, "open");
        assert_eq!(args, "src/main.rs");
    }

    #[test]
    fn static_command_resolves_by_name() {
        let name = unwrap_static(command_lookup("swap_view_left"));

        assert_eq!(name, "swap_view_left");
    }

    #[test]
    fn static_command_rejects_arguments() {
        match command_lookup("swap_view_left ignored") {
            CommandLookup::Unknown(name) => assert_eq!(name, "swap_view_left"),
            other => panic!("expected unknown command, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_command_line_runs_split_and_static_swap_commands() {
        let mut editor = test_editor();
        let mut jobs = Jobs::new();

        execute_command_line(&mut editor, &mut jobs, "vsplit");
        assert_eq!(editor.tree.views().count(), 2);

        let focused_view_id = editor.tree.focus;
        let focused_area_before_swap = editor.tree.get(focused_view_id).area;
        assert!(focused_area_before_swap.x > 0);

        execute_command_line(&mut editor, &mut jobs, "swap_view_left");

        assert_eq!(editor.tree.views().count(), 2);
        assert_eq!(editor.tree.focus, focused_view_id);
        assert_eq!(editor.tree.get(focused_view_id).area.x, 0);
    }

    #[tokio::test]
    async fn theme_preview_restores_original_after_cycling_and_preserves_commits() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("themes")).unwrap();
        std::fs::write(
            root.path().join("themes/preview.toml"),
            "inherits = 'default'\n'ui.background' = { bg = '#17394b' }\n",
        )
        .unwrap();
        let mut editor = test_editor();
        editor.theme_loader = Arc::new(theme::Loader::new(&[root.path().to_path_buf()]));
        editor
            .set_theme(editor.theme_loader.load("default").unwrap())
            .unwrap();
        let mut preview = ThemePreview::default();

        assert!(preview.update(&mut editor, Some("theme base16_default")));
        assert_eq!(editor.theme.name(), "base16_default");
        assert!(preview.update(&mut editor, Some("theme 'preview'")));
        assert_eq!(editor.theme.name(), "preview");
        assert!(!preview.update(&mut editor, None));
        assert_eq!(editor.theme.name(), "default");

        assert!(preview.update(&mut editor, Some("theme base16_default")));
        preview.update(&mut editor, None);
        execute_command_line(&mut editor, &mut Jobs::new(), "theme base16_default");
        preview.update(&mut editor, None);
        assert_eq!(editor.theme.name(), "base16_default");

        // A config/appearance-driven commit supersedes the temporary snapshot.
        assert!(preview.update(&mut editor, Some("theme preview")));
        editor
            .set_theme(editor.theme_loader.load("default").unwrap())
            .unwrap();
        preview.update(&mut editor, None);
        assert_eq!(editor.theme.name(), "default");
    }

    #[tokio::test]
    async fn invalid_or_non_theme_candidates_restore_without_executing_commands() {
        let mut editor = test_editor();
        editor
            .set_theme(editor.theme_loader.load("default").unwrap())
            .unwrap();
        let mut preview = ThemePreview::default();
        let views = editor.tree.views().count();
        for candidate in [
            "theme",
            "theme no-such-preview-theme",
            "theme default extra",
            "theme 'unterminated",
            "theme %sh{echo default}",
            "vsplit",
        ] {
            assert!(preview.update(&mut editor, Some("theme base16_default")));
            assert!(!preview.update(&mut editor, Some(candidate)), "{candidate}");
            assert_eq!(editor.theme.name(), "default", "{candidate}");
            assert_eq!(editor.tree.views().count(), views);
            assert!(!editor.is_err());
        }
    }

    #[test]
    fn unknown_command_reports_original_name() {
        match command_lookup("not-a-command") {
            CommandLookup::Unknown(name) => assert_eq!(name, "not-a-command"),
            other => panic!("expected unknown command, got {other:?}"),
        }
    }
}
