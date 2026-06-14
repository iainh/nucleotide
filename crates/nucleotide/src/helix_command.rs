// ABOUTME: Adapter for executing Helix typable commands from native UI code
// ABOUTME: Keeps terminal command execution details out of workspace/UI modules

use helix_term::commands::{MappableCommand, TYPABLE_COMMAND_LIST, TYPABLE_COMMAND_MAP};

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
enum CommandLookup {
    Empty,
    Found(MappableCommand),
    Unknown(String),
}

fn command_lookup(command: &str) -> CommandLookup {
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

    command.map_or_else(
        || CommandLookup::Unknown(cmd_name.to_string()),
        |cmd| CommandLookup::Found(typable_command(cmd.name, args, cmd.doc)),
    )
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

    fn unwrap_typable(lookup: CommandLookup) -> (String, String) {
        match lookup {
            CommandLookup::Found(MappableCommand::Typable { name, args, .. }) => (name, args),
            other => panic!("expected typable command, got {other:?}"),
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
    fn unknown_command_reports_original_name() {
        match command_lookup("not-a-command") {
            CommandLookup::Unknown(name) => assert_eq!(name, "not-a-command"),
            other => panic!("expected unknown command, got {other:?}"),
        }
    }
}
