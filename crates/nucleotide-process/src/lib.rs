use std::{ffi::OsStr, process::Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_window(&mut command);
    command
}

pub fn hide_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
}

#[cfg(feature = "tokio")]
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    hide_tokio_window(&mut command);
    command
}

#[cfg(feature = "tokio")]
pub fn hide_tokio_window(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}
