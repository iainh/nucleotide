// ABOUTME: Language-agnostic contracts for non-standard LSP protocol extensions
// ABOUTME: Keeps raw JSON-RPC transport separate from server-specific protocol adapters

use helix_lsp::lsp;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::jdtls::JdtlsExtension;

/// A typed non-standard client-to-server request.
pub trait CustomRequest {
    type Params: Serialize;
    type Result: DeserializeOwned;

    const METHOD: &'static str;
}

/// A typed non-standard client-to-server notification.
pub trait CustomNotification {
    type Params: Serialize;

    const METHOD: &'static str;
}

/// A typed server command executed through the standard
/// `workspace/executeCommand` request.
pub trait CustomCommand {
    type Result: DeserializeOwned;

    const COMMAND: &'static str;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtensionMessageSeverity {
    Hint,
    Info,
    Warning,
    Error,
}

/// Presentation-neutral server-extension messages. Adapters decode their wire
/// payloads here; application and UI crates decide how to present or act on them.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerExtensionNotification {
    Status {
        status_type: String,
        message: String,
    },
    Actionable {
        severity: ExtensionMessageSeverity,
        message: String,
        data: Option<Value>,
        commands: Vec<lsp::Command>,
    },
    Event {
        event_type: String,
        data: Option<Value>,
    },
    Progress {
        token: String,
        title: String,
        message: Option<String>,
        percentage: Option<u32>,
        complete: bool,
    },
    Command {
        command: String,
        arguments: Vec<Value>,
    },
}

/// Adapter boundary for one language server's non-standard protocol.
pub trait LanguageServerExtension: Sync {
    fn matches_server(&self, server_name: &str) -> bool;

    /// Initialization options to merge over the user's configured options.
    /// Adapters must only advertise client features implemented end-to-end.
    fn initialization_options(&self) -> Option<Value>;

    /// Decode a server-to-client custom notification. Unknown methods return
    /// `Ok(None)` so standard LSP dispatch can continue.
    fn decode_notification(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Option<ServerExtensionNotification>, String>;
}

static JDTLS_EXTENSION: JdtlsExtension = JdtlsExtension;
static BUILTIN_EXTENSIONS: [&dyn LanguageServerExtension; 1] = [&JDTLS_EXTENSION];

pub fn initialization_options_for_server(server_name: &str) -> Option<Value> {
    BUILTIN_EXTENSIONS
        .iter()
        .find(|extension| extension.matches_server(server_name))
        .and_then(|extension| extension.initialization_options())
}

pub fn decode_server_notification(
    server_name: &str,
    method: &str,
    params: Value,
) -> Result<Option<ServerExtensionNotification>, String> {
    let Some(extension) = BUILTIN_EXTENSIONS
        .iter()
        .find(|extension| extension.matches_server(server_name))
    else {
        return Ok(None);
    };

    extension.decode_notification(method, params)
}
