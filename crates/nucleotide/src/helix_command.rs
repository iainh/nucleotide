// ABOUTME: Adapter for executing Helix typable commands from native UI code
// ABOUTME: Keeps Helix terminal prompt event details out of workspace/UI modules

pub(crate) fn execute_command_line(
    editor: &mut helix_view::Editor,
    jobs: &mut helix_term::job::Jobs,
    command: &str,
) {
    let mut comp_ctx = helix_term::compositor::Context {
        editor,
        scroll: None,
        jobs,
    };

    let (cmd_name, args, _) = helix_core::command_line::split(command);
    if cmd_name.is_empty() {
        return;
    }

    if cmd_name.parse::<usize>().is_ok() && args.trim().is_empty() {
        execute_line_number_command(&mut comp_ctx, cmd_name);
        return;
    }

    execute_named_command(&mut comp_ctx, cmd_name, args);
}

fn execute_line_number_command(comp_ctx: &mut helix_term::compositor::Context, line: &str) {
    let Some(cmd) = helix_term::commands::TYPABLE_COMMAND_MAP.get("goto") else {
        return;
    };

    let parsed_args =
        helix_core::command_line::Args::parse(line, cmd.signature, true, |token| Ok(token.content));

    match parsed_args {
        Ok(parsed_args) => {
            if let Err(err) =
                (cmd.fun)(comp_ctx, parsed_args, helix_term::ui::PromptEvent::Validate)
            {
                comp_ctx.editor.set_error(err.to_string());
            }
        }
        Err(_) => {
            comp_ctx
                .editor
                .set_error("Failed to parse arguments".to_string());
        }
    }
}

fn execute_named_command(
    comp_ctx: &mut helix_term::compositor::Context,
    cmd_name: &str,
    args: &str,
) {
    let resolved_cmd_name = if helix_term::commands::TYPABLE_COMMAND_MAP.contains_key(cmd_name) {
        cmd_name
    } else {
        helix_term::commands::TYPABLE_COMMAND_LIST
            .iter()
            .find(|cmd| cmd.aliases.contains(&cmd_name))
            .map(|cmd| cmd.name)
            .unwrap_or(cmd_name)
    };

    let Some(cmd) = helix_term::commands::TYPABLE_COMMAND_MAP.get(resolved_cmd_name) else {
        comp_ctx
            .editor
            .set_error(format!("no such command: '{cmd_name}'"));
        return;
    };

    let parsed_args = helix_core::command_line::Args::parse(args, cmd.signature, true, |token| {
        helix_view::expansion::expand(comp_ctx.editor, token).map_err(std::convert::Into::into)
    });

    match parsed_args {
        Ok(parsed_args) => {
            if let Err(err) =
                (cmd.fun)(comp_ctx, parsed_args, helix_term::ui::PromptEvent::Validate)
            {
                comp_ctx.editor.set_error(format!("'{cmd_name}': {err}"));
            }
        }
        Err(err) => {
            comp_ctx.editor.set_error(format!("'{cmd_name}': {err}"));
        }
    }
}
