use anyhow::{Context, Result, bail};
use std::path::PathBuf;

const HELP: &str = "\
USAGE:
    nucl-grammar {fetch|build}

FLAGS:
    -c, --config <file>            Specifies a file to use for configuration
    --log <file>                   Specifies a file to use for logging
    -h, --help                     Prints help information
";

enum GrammarCommand {
    Fetch,
    Build,
}

struct GrammarArgs {
    command: Option<GrammarCommand>,
    config_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    display_help: bool,
}

fn parse_args() -> Result<GrammarArgs> {
    let mut args = GrammarArgs {
        command: None,
        config_file: None,
        log_file: None,
        display_help: false,
    };
    let mut argv = std::env::args().skip(1);

    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "-h" | "--help" => args.display_help = true,
            "fetch" => set_command(&mut args, GrammarCommand::Fetch)?,
            "build" => set_command(&mut args, GrammarCommand::Build)?,
            "-c" | "--config" => match argv.next() {
                Some(path) => args.config_file = Some(path.into()),
                None => bail!("{arg} must specify a path to read"),
            },
            "--log" => match argv.next() {
                Some(path) => args.log_file = Some(path.into()),
                None => bail!("{arg} must specify a path to write"),
            },
            _ => bail!("unknown argument `{arg}`"),
        }
    }

    Ok(args)
}

fn set_command(args: &mut GrammarArgs, command: GrammarCommand) -> Result<()> {
    if args.command.is_some() {
        bail!("only one grammar command can be specified");
    }

    args.command = Some(command);
    Ok(())
}

fn main() -> Result<()> {
    let args = parse_args().context("could not parse arguments")?;

    if args.display_help {
        print!("{HELP}");
        return Ok(());
    }

    helix_loader::initialize_config_file(args.config_file);
    helix_loader::initialize_log_file(args.log_file);

    match args.command {
        Some(GrammarCommand::Fetch) => helix_loader::grammar::fetch_grammars(false),
        Some(GrammarCommand::Build) => helix_loader::grammar::build_grammars(None, false),
        None => bail!("nucl-grammar requires `fetch` or `build`"),
    }
}
