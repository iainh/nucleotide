use std::time::{Duration, Instant};
use std::{io, ptr};

use anyhow::{Context as _, Result};
use helix_term::args::Args;
use nucleotide_logging::{error, info, warn};
use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_MORE_DATA, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::{ExternalOpenFile, ExternalOpenRequest};

const MUTEX_NAME: &str = "Local\\org.spiralpoint.nucleotide.SingleInstance";
const PIPE_NAME: &str = r"\\.\pipe\org.spiralpoint.nucleotide.open";
const IPC_BUFFER_SIZE: u32 = 64 * 1024;

pub enum ClaimResult {
    Primary(WindowsSingleInstanceGuard),
    Forwarded,
}

pub struct WindowsSingleInstanceGuard {
    mutex: HANDLE,
}

impl Drop for WindowsSingleInstanceGuard {
    fn drop(&mut self) {
        if !self.mutex.is_null() {
            unsafe {
                CloseHandle(self.mutex);
            }
        }
    }
}

struct HandleGuard(HANDLE);

impl HandleGuard {
    fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn claim_or_forward(args: &Args, dock_action: Option<usize>) -> Result<ClaimResult> {
    let mutex_name = wide_nul(MUTEX_NAME);
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        return Err(io::Error::last_os_error()).context("failed to create single-instance mutex");
    }

    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running {
        unsafe {
            CloseHandle(mutex);
        }

        let request = request_from_args(args, dock_action);
        forward_request(&request)
            .context("failed to forward launch request to running Nucleotide")?;
        info!("Forwarded launch request to running Nucleotide instance");
        return Ok(ClaimResult::Forwarded);
    }

    info!("Claimed primary Windows Nucleotide instance");
    Ok(ClaimResult::Primary(WindowsSingleInstanceGuard { mutex }))
}

pub fn start_listener(sender: UnboundedSender<ExternalOpenRequest>) {
    start_listener_on_pipe(sender, PIPE_NAME.to_string());
}

fn start_listener_on_pipe(sender: UnboundedSender<ExternalOpenRequest>, pipe_name: String) {
    if let Err(error) = std::thread::Builder::new()
        .name("WindowsSingleInstance".to_string())
        .spawn(move || listen_for_requests(sender, &pipe_name))
    {
        error!(error = %error, "Failed to spawn Windows single-instance listener");
    }
}

fn listen_for_requests(sender: UnboundedSender<ExternalOpenRequest>, pipe_name: &str) {
    loop {
        let pipe_name = wide_nul(pipe_name);
        let pipe = unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                IPC_BUFFER_SIZE,
                IPC_BUFFER_SIZE,
                0,
                ptr::null(),
            )
        };

        if pipe == INVALID_HANDLE_VALUE {
            error!(
                error = %io::Error::last_os_error(),
                "Failed to create Windows single-instance pipe"
            );
            break;
        }

        let pipe = HandleGuard::new(pipe);
        let connected = unsafe { ConnectNamedPipe(pipe.get(), ptr::null_mut()) != 0 }
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

        if connected {
            match read_request(pipe.get()) {
                Ok(request) => {
                    if sender.send(request).is_err() {
                        unsafe {
                            DisconnectNamedPipe(pipe.get());
                        }
                        break;
                    }
                }
                Err(err) => {
                    warn!(error = %err, "Failed to read Windows single-instance request");
                }
            }
        }

        unsafe {
            DisconnectNamedPipe(pipe.get());
        }
    }
}

fn read_request(pipe: HANDLE) -> Result<ExternalOpenRequest> {
    let mut bytes = Vec::new();

    loop {
        let mut buffer = vec![0u8; IPC_BUFFER_SIZE as usize];
        let mut bytes_read = 0u32;
        let ok = unsafe {
            ReadFile(
                pipe,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };

        bytes.extend_from_slice(&buffer[..bytes_read as usize]);

        if ok != 0 {
            break;
        }

        let error = unsafe { GetLastError() };
        if error != ERROR_MORE_DATA {
            return Err(io::Error::last_os_error()).context("failed to read from named pipe");
        }
    }

    serde_json::from_slice(&bytes).context("failed to decode single-instance request")
}

fn forward_request(request: &ExternalOpenRequest) -> Result<()> {
    forward_request_to_pipe(request, PIPE_NAME)
}

fn forward_request_to_pipe(request: &ExternalOpenRequest, pipe_name: &str) -> Result<()> {
    let payload = serde_json::to_vec(request).context("failed to encode launch request")?;
    let pipe_name = wide_nul(pipe_name);
    let pipe = HandleGuard::new(open_pipe(pipe_name.as_ptr())?);
    let mut bytes_written = 0u32;

    let ok = unsafe {
        WriteFile(
            pipe.get(),
            payload.as_ptr(),
            payload.len() as u32,
            &mut bytes_written,
            ptr::null_mut(),
        )
    };

    if ok == 0 {
        return Err(io::Error::last_os_error()).context("failed to write launch request");
    }

    if bytes_written as usize != payload.len() {
        anyhow::bail!(
            "wrote incomplete launch request: {bytes_written} of {} bytes",
            payload.len()
        );
    }

    Ok(())
}

fn open_pipe(pipe_name: windows_sys::core::PCWSTR) -> Result<HANDLE> {
    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

    const WAIT_FOR_PIPE_MS: u32 = 5_000;
    const RETRY_DELAY: Duration = Duration::from_millis(25);

    let deadline = Instant::now() + Duration::from_millis(WAIT_FOR_PIPE_MS as u64);

    loop {
        let pipe = unsafe {
            CreateFileW(
                pipe_name,
                GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };

        if pipe != INVALID_HANDLE_VALUE {
            return Ok(pipe);
        }

        let error = io::Error::last_os_error();
        if Instant::now() >= deadline {
            return Err(error).context("timed out waiting for Nucleotide pipe");
        }

        std::thread::sleep(RETRY_DELAY);
    }
}

fn request_from_args(args: &Args, dock_action: Option<usize>) -> ExternalOpenRequest {
    let files = args
        .files
        .iter()
        .flat_map(|(path, positions)| {
            let positions = if positions.is_empty() {
                vec![helix_core::Position::default()]
            } else {
                positions.clone()
            };

            positions
                .into_iter()
                .map(|position| ExternalOpenFile::new(path.clone(), position))
        })
        .collect::<Vec<_>>();

    ExternalOpenRequest {
        files,
        working_directory: args.working_directory.clone(),
        dock_action,
    }
}

fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use helix_core::Position;

    #[test]
    fn request_from_args_preserves_paths_working_dir_and_dock_action() {
        let mut args = Args::default();
        let project = PathBuf::from(r"C:\Users\Example\project");
        let file = project.join("src").join("main.rs");
        args.working_directory = Some(project.clone());
        args.files.insert(file.clone(), vec![Position::new(4, 2)]);

        let request = request_from_args(&args, Some(1));

        assert_eq!(
            request.files,
            vec![ExternalOpenFile::new(file, Position::new(4, 2))]
        );
        assert_eq!(request.working_directory, Some(project));
        assert_eq!(request.dock_action, Some(1));
    }

    #[test]
    fn request_from_args_preserves_multiple_file_positions() {
        let mut args = Args::default();
        let file = PathBuf::from(r"C:\Users\Example\project\src\main.rs");
        args.files
            .insert(file.clone(), vec![Position::new(2, 0), Position::new(5, 3)]);

        let request = request_from_args(&args, None);

        assert_eq!(
            request.files,
            vec![
                ExternalOpenFile::new(file.clone(), Position::new(2, 0)),
                ExternalOpenFile::new(file, Position::new(5, 3)),
            ]
        );
    }

    #[test]
    fn request_json_round_trips_windows_paths() {
        let request = ExternalOpenRequest {
            files: vec![ExternalOpenFile::new(
                PathBuf::from(r"C:\Users\Example\project\src\main.rs"),
                Position::new(10, 4),
            )],
            working_directory: Some(PathBuf::from(r"C:\Users\Example\project")),
            dock_action: None,
        };

        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: ExternalOpenRequest = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn forwards_request_through_named_pipe() {
        let pipe_name = format!(
            r"\\.\pipe\org.spiralpoint.nucleotide.test.{}",
            std::process::id()
        );
        let request = ExternalOpenRequest {
            files: vec![ExternalOpenFile::new(
                PathBuf::from(r"C:\Users\Example\project"),
                Position::default(),
            )],
            working_directory: None,
            dock_action: Some(0),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        start_listener_on_pipe(tx, pipe_name.clone());
        forward_request_to_pipe(&request, &pipe_name).unwrap();

        assert_eq!(rx.blocking_recv(), Some(request));
    }

    #[test]
    fn wide_nul_is_nul_terminated() {
        let value = wide_nul(PIPE_NAME);

        assert_eq!(value.last().copied(), Some(0));
        assert_eq!(value.iter().filter(|&&ch| ch == 0).count(), 1);
    }
}
