// ABOUTME: Remote workspace service dispatch and protocol v5 server implementation
// ABOUTME: Owns server scheduling, watches, streaming operations, and wire conversions

use super::*;

mod convert;
mod dispatch;
mod file;
mod process;
mod process_session;
mod runtime;
mod search;
mod watch;

pub(crate) use convert::*;
pub use dispatch::*;
pub(crate) use file::*;
pub(crate) use process::*;
pub(crate) use process_session::*;
pub use runtime::*;
pub(crate) use search::*;
pub(crate) use watch::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionInputDisposition {
    Consumed,
    Discarded,
}
