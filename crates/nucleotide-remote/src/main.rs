// ABOUTME: Nucleotide remote workspace service binary
// ABOUTME: Runs protocol traffic over stdio and persists diagnostics on the helper host

use anyhow::{Context, Result};
use nucleotide_logging::{
    LoggingConfig, default_remote_log_file_path, init_file_logging, init_logging_with_config,
};

fn setup_logging() -> Result<()> {
    let mut config = LoggingConfig::from_env()
        .context("failed to create remote logging config from environment")?;
    config.output.console = false;
    config.file.path = default_remote_log_file_path();

    if !config.output.file {
        config.output.console = true;
        return init_logging_with_config(config)
            .context("failed to initialize nucleotide-remote stderr logging");
    }

    match init_file_logging(config.clone()) {
        Ok(()) => Ok(()),
        Err(file_error) => {
            eprintln!(
                "nucleotide-remote could not initialize host file logging at {}: {file_error}",
                config.file.path.display()
            );
            config.output.file = false;
            config.output.console = true;
            init_logging_with_config(config)
                .context("failed to initialize nucleotide-remote stderr logging")?;
            tracing::warn!(
                error = %file_error,
                "Fell back to stderr because remote host file logging could not be initialized"
            );
            Ok(())
        }
    }
}

fn main() -> Result<()> {
    setup_logging()?;
    let result = nucleotide_remote::run_from_args(std::env::args().skip(1));
    if let Err(error) = &result {
        tracing::error!(
            error = %error,
            error_chain = %format_args!("{error:#}"),
            "nucleotide-remote command failed"
        );
    }
    result
}
