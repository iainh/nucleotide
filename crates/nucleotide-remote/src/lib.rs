// ABOUTME: Framed stdio protocol and service loop for Nucleotide remote workspaces
// ABOUTME: Keeps WSL, SSH, and local service transports on one request model

use anyhow::{Context, Result, bail};
use futures::executor::block_on;
use nucleotide_workspace::{
    DirectoryListing, FileKind, FileRead, FileSearchQuery, FileSearchResult, FileStat,
    LocalWorkspaceBackend, ReadOptions, WorkspaceBackend, WorkspaceError, WriteOptions,
    WriteResult,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PROTOCOL_VERSION: u32 = 1;
pub const FRAME_VERSION: u16 = 1;
pub const FRAME_MAGIC: [u8; 4] = *b"NUCL";
pub const FRAME_HEADER_LEN: usize = 36;
pub const MAX_FRAME_HEADER_LEN: u32 = 1024 * 1024;
pub const MAX_FRAME_BODY_LEN: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameKind {
    Request = 1,
    Response = 2,
    Error = 3,
    Data = 4,
    Progress = 5,
    Cancel = 6,
    Shutdown = 7,
}

impl TryFrom<u16> for FrameKind {
    type Error = io::Error;

    fn try_from(value: u16) -> std::result::Result<Self, <Self as TryFrom<u16>>::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Error),
            4 => Ok(Self::Data),
            5 => Ok(Self::Progress),
            6 => Ok(Self::Cancel),
            7 => Ok(Self::Shutdown),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame kind: {value}"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub flags: u16,
    pub request_id: u64,
    pub stream_id: u32,
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn from_json_header<T: Serialize>(
        kind: FrameKind,
        request_id: u64,
        stream_id: u32,
        header: &T,
        body: Vec<u8>,
    ) -> serde_json::Result<Self> {
        Ok(Self {
            kind,
            flags: 0,
            request_id,
            stream_id,
            header: serde_json::to_vec(header)?,
            body,
        })
    }

    pub fn decode_json_header<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.header)
    }
}

pub fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    if frame.header.len() > MAX_FRAME_HEADER_LEN as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame header is too large",
        ));
    }
    if frame.body.len() as u64 > MAX_FRAME_BODY_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame body is too large",
        ));
    }

    let mut header = [0_u8; FRAME_HEADER_LEN];
    header[0..4].copy_from_slice(&FRAME_MAGIC);
    header[4..6].copy_from_slice(&FRAME_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(frame.kind as u16).to_le_bytes());
    header[8..10].copy_from_slice(&frame.flags.to_le_bytes());
    header[10..12].copy_from_slice(&0_u16.to_le_bytes());
    header[12..20].copy_from_slice(&frame.request_id.to_le_bytes());
    header[20..24].copy_from_slice(&frame.stream_id.to_le_bytes());
    header[24..28].copy_from_slice(&(frame.header.len() as u32).to_le_bytes());
    header[28..36].copy_from_slice(&(frame.body.len() as u64).to_le_bytes());

    writer.write_all(&header)?;
    writer.write_all(&frame.header)?;
    writer.write_all(&frame.body)?;
    writer.flush()
}

pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut fixed = [0_u8; FRAME_HEADER_LEN];
    match reader.read(&mut fixed[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut fixed[1..])?,
        _ => unreachable!("read buffer length is one byte"),
    }

    if fixed[0..4] != FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame magic",
        ));
    }

    let version = u16::from_le_bytes([fixed[4], fixed[5]]);
    if version != FRAME_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported frame version: {version}"),
        ));
    }

    let kind = FrameKind::try_from(u16::from_le_bytes([fixed[6], fixed[7]]))?;
    let flags = u16::from_le_bytes([fixed[8], fixed[9]]);
    let request_id = u64::from_le_bytes([
        fixed[12], fixed[13], fixed[14], fixed[15], fixed[16], fixed[17], fixed[18], fixed[19],
    ]);
    let stream_id = u32::from_le_bytes([fixed[20], fixed[21], fixed[22], fixed[23]]);
    let header_len = u32::from_le_bytes([fixed[24], fixed[25], fixed[26], fixed[27]]);
    let body_len = u64::from_le_bytes([
        fixed[28], fixed[29], fixed[30], fixed[31], fixed[32], fixed[33], fixed[34], fixed[35],
    ]);

    if header_len > MAX_FRAME_HEADER_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame header exceeds maximum length",
        ));
    }
    if body_len > MAX_FRAME_BODY_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame body exceeds maximum length",
        ));
    }

    let mut header = vec![0_u8; header_len as usize];
    reader.read_exact(&mut header)?;
    let mut body = vec![0_u8; body_len as usize];
    reader.read_exact(&mut body)?;

    Ok(Some(Frame {
        kind,
        flags,
        request_id,
        stream_id,
        header,
        body,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request: RemoteRequest,
}

impl RequestEnvelope {
    pub fn new(request: RemoteRequest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum RemoteRequest {
    Hello,
    Stat {
        path: PathBuf,
    },
    ListDir {
        path: PathBuf,
    },
    ReadFile {
        path: PathBuf,
        max_bytes: Option<u64>,
    },
    WriteFile {
        path: PathBuf,
        create_parent_dirs: bool,
        expected_modified_unix_millis: Option<i64>,
    },
    FileSearch(FileSearchRequest),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub protocol_version: u32,
    pub response: RemoteResponse,
}

impl ResponseEnvelope {
    pub fn new(response: RemoteResponse) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            response,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "snake_case")]
pub enum RemoteResponse {
    Hello(HelloResponse),
    Stat(FileStatResponse),
    ListDir(DirectoryListingResponse),
    ReadFile(FileReadResponse),
    WriteFile(WriteResultResponse),
    FileSearch(FileSearchResponse),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub protocol_version: u32,
    pub error: RemoteError,
}

impl ErrorEnvelope {
    pub fn new(error: RemoteError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub helper_version: String,
    pub os: String,
    pub arch: String,
    pub workspace_root: PathBuf,
    pub capabilities: Vec<String>,
}

impl HelloResponse {
    fn current(workspace_root: PathBuf) -> Self {
        Self {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            workspace_root,
            capabilities: vec![
                "stat".to_string(),
                "list_dir".to_string(),
                "read_file".to_string(),
                "write_file".to_string(),
                "file_search".to_string(),
                "binary_body_frames".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteFileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatResponse {
    pub path: PathBuf,
    pub kind: RemoteFileKind,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntryResponse {
    pub name: String,
    pub path: PathBuf,
    pub stat: FileStatResponse,
    pub symlink_target: Option<PathBuf>,
    pub target_exists: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListingResponse {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntryResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    pub readonly: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResultResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchRequest {
    pub root: PathBuf,
    pub pattern: Option<String>,
    pub limit: usize,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
}

impl Default for FileSearchRequest {
    fn default() -> Self {
        let query = FileSearchQuery::default();
        Self {
            root: query.root,
            pattern: query.pattern,
            limit: query.limit,
            hidden: query.hidden,
            parents: query.parents,
            ignore: query.ignore,
            git_ignore: query.git_ignore,
            git_global: query.git_global,
            git_exclude: query.git_exclude,
            follow_links: query.follow_links,
            max_depth: query.max_depth,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchResponse {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

pub struct WorkspaceService<B> {
    backend: B,
    workspace_root: PathBuf,
}

impl<B> WorkspaceService<B>
where
    B: WorkspaceBackend,
{
    pub fn new(backend: B, workspace_root: PathBuf) -> Self {
        Self {
            backend,
            workspace_root,
        }
    }

    pub fn serve<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> Result<()> {
        while let Some(frame) = read_frame(reader).context("failed to read protocol frame")? {
            match frame.kind {
                FrameKind::Request => {
                    let should_continue = self.handle_request(frame, writer)?;
                    if !should_continue {
                        break;
                    }
                }
                FrameKind::Cancel => {
                    self.write_error(
                        writer,
                        frame.request_id,
                        "unsupported_cancel",
                        "cancellation is not available for this operation yet",
                        None,
                    )?;
                }
                FrameKind::Shutdown => break,
                other => {
                    self.write_error(
                        writer,
                        frame.request_id,
                        "unexpected_frame",
                        format!("unexpected frame kind from client: {other:?}"),
                        None,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn handle_request<W: Write>(&self, frame: Frame, writer: &mut W) -> Result<bool> {
        let request = match frame.decode_json_header::<RequestEnvelope>() {
            Ok(request) => request,
            Err(error) => {
                self.write_error(
                    writer,
                    frame.request_id,
                    "invalid_request",
                    "request header is not valid JSON",
                    Some(error.to_string()),
                )?;
                return Ok(true);
            }
        };

        if request.protocol_version != PROTOCOL_VERSION {
            self.write_error(
                writer,
                frame.request_id,
                "protocol_mismatch",
                format!(
                    "unsupported protocol version {}; expected {}",
                    request.protocol_version, PROTOCOL_VERSION
                ),
                None,
            )?;
            return Ok(true);
        }

        match self.execute(request.request, frame.body) {
            Ok(ServiceOutcome::Continue(response, body)) => {
                self.write_response(writer, frame.request_id, response, body)?;
                Ok(true)
            }
            Ok(ServiceOutcome::Shutdown) => {
                self.write_response(
                    writer,
                    frame.request_id,
                    RemoteResponse::Shutdown,
                    Vec::new(),
                )?;
                Ok(false)
            }
            Err(error) => {
                self.write_error(
                    writer,
                    frame.request_id,
                    &error.code,
                    error.message,
                    error.diagnostic,
                )?;
                Ok(true)
            }
        }
    }

    fn execute(
        &self,
        request: RemoteRequest,
        request_body: Vec<u8>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        match request {
            RemoteRequest::Hello => Ok(ServiceOutcome::Continue(
                RemoteResponse::Hello(HelloResponse::current(self.workspace_root.clone())),
                Vec::new(),
            )),
            RemoteRequest::Stat { path } => {
                let path = self.resolve_path(&path);
                let stat =
                    block_on(self.backend.stat(&path)).map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::Stat(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ListDir { path } => {
                let path = self.resolve_path(&path);
                let listing =
                    block_on(self.backend.list_dir(&path)).map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::ListDir(directory_listing_response(listing)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ReadFile { path, max_bytes } => {
                let path = self.resolve_path(&path);
                let max_bytes = Some(
                    max_bytes
                        .unwrap_or(MAX_FRAME_BODY_LEN)
                        .min(MAX_FRAME_BODY_LEN),
                );
                let read = block_on(self.backend.read_file(&path, ReadOptions { max_bytes }))
                    .map_err(remote_error_from_workspace)?;
                let response = file_read_response(&read);
                let body = read.bytes;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::ReadFile(response),
                    body,
                ))
            }
            RemoteRequest::WriteFile {
                path,
                create_parent_dirs,
                expected_modified_unix_millis,
            } => {
                let path = self.resolve_path(&path);
                let expected_modified =
                    expected_modified_unix_millis.and_then(system_time_from_unix_millis);
                let result = block_on(self.backend.write_file(
                    &path,
                    &request_body,
                    WriteOptions {
                        create_parent_dirs,
                        expected_modified,
                    },
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::WriteFile(write_result_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::FileSearch(request) => {
                let query = FileSearchQuery {
                    root: self.resolve_search_root(&request.root),
                    pattern: request.pattern,
                    limit: request.limit,
                    hidden: request.hidden,
                    parents: request.parents,
                    ignore: request.ignore,
                    git_ignore: request.git_ignore,
                    git_global: request.git_global,
                    git_exclude: request.git_exclude,
                    follow_links: request.follow_links,
                    max_depth: request.max_depth,
                };
                let result = block_on(self.backend.file_search(query))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::FileSearch(file_search_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::Shutdown => Ok(ServiceOutcome::Shutdown),
        }
    }

    fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        }
    }

    fn resolve_search_root(&self, root: &Path) -> PathBuf {
        if root.as_os_str().is_empty() {
            self.workspace_root.clone()
        } else {
            self.resolve_path(root)
        }
    }

    fn write_response<W: Write>(
        &self,
        writer: &mut W,
        request_id: u64,
        response: RemoteResponse,
        body: Vec<u8>,
    ) -> Result<()> {
        let envelope = ResponseEnvelope::new(response);
        let frame = Frame::from_json_header(FrameKind::Response, request_id, 0, &envelope, body)
            .context("failed to encode response frame")?;
        write_frame(writer, &frame).context("failed to write response frame")
    }

    fn write_error<W: Write>(
        &self,
        writer: &mut W,
        request_id: u64,
        code: impl Into<String>,
        message: impl Into<String>,
        diagnostic: Option<String>,
    ) -> Result<()> {
        let envelope = ErrorEnvelope::new(RemoteError {
            code: code.into(),
            message: message.into(),
            diagnostic,
        });
        let frame = Frame::from_json_header(FrameKind::Error, request_id, 0, &envelope, Vec::new())
            .context("failed to encode error frame")?;
        write_frame(writer, &frame).context("failed to write error frame")
    }
}

pub fn serve_local_workspace<R: Read, W: Write>(
    workspace_root: PathBuf,
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    WorkspaceService::new(LocalWorkspaceBackend, workspace_root).serve(reader, writer)
}

enum ServiceOutcome {
    Continue(RemoteResponse, Vec<u8>),
    Shutdown,
}

fn file_stat_response(stat: FileStat) -> FileStatResponse {
    FileStatResponse {
        path: stat.path,
        kind: remote_file_kind(stat.kind),
        size: stat.size,
        modified_unix_millis: stat.modified.and_then(system_time_unix_millis),
        readonly: stat.readonly,
    }
}

fn directory_listing_response(listing: DirectoryListing) -> DirectoryListingResponse {
    DirectoryListingResponse {
        path: listing.path,
        entries: listing
            .entries
            .into_iter()
            .map(|entry| DirectoryEntryResponse {
                name: entry.name,
                path: entry.path,
                stat: file_stat_response(entry.stat),
                symlink_target: entry.symlink_target,
                target_exists: entry.target_exists,
            })
            .collect(),
    }
}

fn file_read_response(read: &FileRead) -> FileReadResponse {
    FileReadResponse {
        path: read.path.clone(),
        size: read.size,
        modified_unix_millis: read.modified.and_then(system_time_unix_millis),
        readonly: read.readonly,
        truncated: read.truncated,
    }
}

fn write_result_response(result: WriteResult) -> WriteResultResponse {
    WriteResultResponse {
        path: result.path,
        size: result.size,
        modified_unix_millis: result.modified.and_then(system_time_unix_millis),
    }
}

fn file_search_response(result: FileSearchResult) -> FileSearchResponse {
    FileSearchResponse {
        root: result.root,
        files: result.files,
        truncated: result.truncated,
    }
}

fn remote_file_kind(kind: FileKind) -> RemoteFileKind {
    match kind {
        FileKind::File => RemoteFileKind::File,
        FileKind::Directory => RemoteFileKind::Directory,
        FileKind::Symlink => RemoteFileKind::Symlink,
        FileKind::Other => RemoteFileKind::Other,
    }
}

fn remote_error_from_workspace(error: WorkspaceError) -> RemoteError {
    let code = match &error {
        WorkspaceError::Io { .. } => "io",
        WorkspaceError::Modified { .. } => "modified",
        WorkspaceError::NotFile { .. } => "not_file",
        WorkspaceError::InvalidSearchPattern(_) => "invalid_search_pattern",
    };

    RemoteError {
        code: code.to_string(),
        message: error.to_string(),
        diagnostic: Some(format!("{error:?}")),
    }
}

fn system_time_unix_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn system_time_from_unix_millis(millis: i64) -> Option<SystemTime> {
    u64::try_from(millis)
        .ok()
        .map(|millis| UNIX_EPOCH + Duration::from_millis(millis))
}

pub fn run_from_args<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "serve" => {
            let workspace_root = parse_workspace_root(args)?;
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_local_workspace(workspace_root, &mut stdin.lock(), &mut stdout.lock())
        }
        "--help" | "-h" | "help" => {
            print_help(&mut std::io::stdout()).context("failed to write help")
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }
}

fn parse_workspace_root<I>(args: I) -> Result<PathBuf>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            other => bail!("unknown serve argument: {other}"),
        }
    }

    workspace_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .context("failed to resolve workspace root")
}

fn print_help<W: Write>(writer: &mut W) -> io::Result<()> {
    writeln!(writer, "nucleotide-remote serve [--workspace <path>]")?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Protocol traffic uses framed messages on stdin/stdout."
    )?;
    writeln!(writer, "Logs and diagnostics must be written to stderr.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn request_frame(id: u64, request: RemoteRequest, body: Vec<u8>) -> Frame {
        Frame::from_json_header(
            FrameKind::Request,
            id,
            0,
            &RequestEnvelope::new(request),
            body,
        )
        .unwrap()
    }

    fn single_request_output(root: &Path, request: RemoteRequest, body: Vec<u8>) -> Vec<u8> {
        let mut input = Vec::new();
        write_frame(&mut input, &request_frame(7, request, body)).unwrap();
        write_frame(
            &mut input,
            &request_frame(8, RemoteRequest::Shutdown, Vec::new()),
        )
        .unwrap();

        let service = WorkspaceService::new(LocalWorkspaceBackend, root.to_path_buf());
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();
        service.serve(&mut reader, &mut output).unwrap();
        output
    }

    fn read_first_output_frame(output: Vec<u8>) -> Frame {
        read_frame(&mut Cursor::new(output)).unwrap().unwrap()
    }

    #[test]
    fn frame_round_trip_preserves_header_and_body() {
        let envelope = RequestEnvelope::new(RemoteRequest::ReadFile {
            path: PathBuf::from("src/main.rs"),
            max_bytes: Some(10),
        });
        let frame = Frame::from_json_header(FrameKind::Request, 42, 3, &envelope, b"body".to_vec())
            .unwrap();

        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let decoded = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();

        assert_eq!(decoded.kind, FrameKind::Request);
        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.stream_id, 3);
        assert_eq!(decoded.body, b"body");
        assert_eq!(
            decoded.decode_json_header::<RequestEnvelope>().unwrap(),
            envelope
        );
    }

    #[test]
    fn frame_reader_returns_none_on_clean_eof() {
        assert!(read_frame(&mut Cursor::new(Vec::new())).unwrap().is_none());
    }

    #[test]
    fn service_hello_returns_workspace_root_and_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::Hello,
            Vec::new(),
        ));

        assert_eq!(frame.kind, FrameKind::Response);
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::Hello(hello) = response.response else {
            panic!("expected hello response");
        };
        assert_eq!(hello.workspace_root, temp.path());
        assert!(
            hello
                .capabilities
                .contains(&"binary_body_frames".to_string())
        );
    }

    #[test]
    fn service_read_file_returns_metadata_and_raw_body() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "abcdef").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::ReadFile {
                path: PathBuf::from("main.rs"),
                max_bytes: Some(3),
            },
            Vec::new(),
        ));

        assert_eq!(frame.body, b"abc");
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::ReadFile(read) = response.response else {
            panic!("expected read response");
        };
        assert_eq!(read.size, 6);
        assert!(read.truncated);
    }

    #[test]
    fn service_write_file_accepts_raw_body() {
        let temp = tempfile::tempdir().unwrap();
        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::WriteFile {
                path: PathBuf::from("src/main.rs"),
                create_parent_dirs: true,
                expected_modified_unix_millis: None,
            },
            b"fn main() {}\n".to_vec(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::WriteFile(write) = response.response else {
            panic!("expected write response");
        };
        assert_eq!(write.size, 13);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn service_file_search_uses_workspace_root_for_empty_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("main.rs"), "").unwrap();
        std::fs::write(temp.path().join("README.md"), "").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::FileSearch(FileSearchRequest {
                pattern: Some(r"\.rs$".to_string()),
                limit: 10,
                ..FileSearchRequest::default()
            }),
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::FileSearch(search) = response.response else {
            panic!("expected file search response");
        };
        assert_eq!(search.files, vec![PathBuf::from("src/main.rs")]);
        assert!(!search.truncated);
    }

    #[test]
    fn service_reports_protocol_mismatch_as_error_frame() {
        let temp = tempfile::tempdir().unwrap();
        let request = RequestEnvelope {
            protocol_version: PROTOCOL_VERSION + 1,
            request: RemoteRequest::Hello,
        };
        let frame =
            Frame::from_json_header(FrameKind::Request, 9, 0, &request, Vec::new()).unwrap();
        let mut input = Vec::new();
        write_frame(&mut input, &frame).unwrap();

        let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf());
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();
        service.serve(&mut reader, &mut output).unwrap();

        let frame = read_first_output_frame(output);
        assert_eq!(frame.kind, FrameKind::Error);
        let error = frame.decode_json_header::<ErrorEnvelope>().unwrap();
        assert_eq!(error.error.code, "protocol_mismatch");
    }
}
