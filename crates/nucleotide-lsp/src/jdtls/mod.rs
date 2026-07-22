// ABOUTME: Eclipse JDT LS protocol adapter and conservative client capability profile
// ABOUTME: Normalizes JDT LS custom notifications without coupling them to GPUI

pub mod protocol;

use serde_json::{Value, json};

use crate::extensions::{
    ExtensionMessageSeverity, LanguageServerExtension, ServerExtensionNotification,
};
use protocol::{ActionableNotification, EventNotification, ProgressReport, StatusReport};

pub struct JdtlsExtension;

impl LanguageServerExtension for JdtlsExtension {
    fn matches_server(&self, server_name: &str) -> bool {
        matches!(
            server_name.to_ascii_lowercase().as_str(),
            "jdtls" | "eclipse-jdtls" | "eclipse.jdt.ls"
        )
    }

    fn initialization_options(&self) -> Option<Value> {
        // These explicit false values prevent JDT LS from selecting protocol
        // paths that require UI callbacks or snippet-bearing workspace edits.
        // Individual capabilities can be enabled as their Nucleotide consumer
        // is completed; the transport and typed protocol contracts already exist.
        Some(json!({
            "extendedClientCapabilities": {
                "snippetEditSupport": false,
                "progressReportProvider": false,
                "classFileContentsSupport": false,
                "actionableNotificationSupported": false,
                "actionableRuntimeNotificationSupport": false,
                "executeClientCommandSupport": false,
                "shouldLanguageServerExitOnShutdown": false
            }
        }))
    }

    fn decode_notification(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Option<ServerExtensionNotification>, String> {
        let notification = match method {
            protocol::notification::STATUS => {
                let params: StatusReport = decode(method, params)?;
                ServerExtensionNotification::Status {
                    status_type: params.status_type,
                    message: params.message,
                }
            }
            protocol::notification::ACTIONABLE => {
                let params: ActionableNotification = decode(method, params)?;
                ServerExtensionNotification::Actionable {
                    severity: actionable_severity(&params.severity),
                    message: params.message,
                    data: params.data,
                    commands: params.commands,
                }
            }
            protocol::notification::EVENT => {
                let params: EventNotification = decode(method, params)?;
                ServerExtensionNotification::Event {
                    event_type: event_type_name(&params.event_type),
                    data: params.data,
                }
            }
            protocol::notification::PROGRESS => {
                let params: ProgressReport = decode(method, params)?;
                let percentage = progress_percentage(params.work_done, params.total_work);
                ServerExtensionNotification::Progress {
                    token: params.id,
                    title: params.task,
                    message: params.sub_task.or(params.status),
                    percentage,
                    complete: params.complete,
                }
            }
            protocol::notification::WORKSPACE_NOTIFY => {
                let params: helix_lsp::lsp::ExecuteCommandParams = decode(method, params)?;
                ServerExtensionNotification::Command {
                    command: params.command,
                    arguments: params.arguments,
                }
            }
            _ => return Ok(None),
        };

        Ok(Some(notification))
    }
}

fn decode<T: serde::de::DeserializeOwned>(method: &str, params: Value) -> Result<T, String> {
    serde_json::from_value(params)
        .map_err(|error| format!("invalid {method} notification payload: {error}"))
}

fn actionable_severity(value: &Value) -> ExtensionMessageSeverity {
    match value {
        Value::Number(number) => match number.as_u64() {
            Some(1) => ExtensionMessageSeverity::Error,
            Some(2) => ExtensionMessageSeverity::Warning,
            _ => ExtensionMessageSeverity::Info,
        },
        Value::String(value) if value.eq_ignore_ascii_case("error") => {
            ExtensionMessageSeverity::Error
        }
        Value::String(value) if value.eq_ignore_ascii_case("warning") => {
            ExtensionMessageSeverity::Warning
        }
        Value::String(value) if value.eq_ignore_ascii_case("hint") => {
            ExtensionMessageSeverity::Hint
        }
        _ => ExtensionMessageSeverity::Info,
    }
}

fn event_type_name(value: &Value) -> String {
    fn numeric_event_type(value: i64) -> String {
        match value {
            100 => "ClasspathUpdated".to_string(),
            200 => "ProjectsImported".to_string(),
            210 => "ProjectsDeleted".to_string(),
            300 => "IncompatibleGradleJdkIssue".to_string(),
            400 => "UpgradeGradleWrapper".to_string(),
            500 => "SourceInvalidated".to_string(),
            600 => "PreviewFeaturesNotAllowed".to_string(),
            _ => value.to_string(),
        }
    }

    match value {
        Value::String(value) => value
            .parse()
            .map(numeric_event_type)
            .unwrap_or_else(|_| value.clone()),
        Value::Number(number) => number
            .as_i64()
            .map(numeric_event_type)
            .unwrap_or_else(|| number.to_string()),
        value => value.to_string(),
    }
}

fn progress_percentage(work_done: Option<u64>, total_work: Option<u64>) -> Option<u32> {
    let (work_done, total_work) = (work_done?, total_work?);
    if total_work == 0 {
        return None;
    }

    Some(((work_done.saturating_mul(100) / total_work).min(100)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::LanguageServerExtension;

    #[test]
    fn matches_only_jdt_ls_names() {
        let extension = JdtlsExtension;
        assert!(extension.matches_server("jdtls"));
        assert!(extension.matches_server("ECLIPSE.JDT.LS"));
        assert!(!extension.matches_server("java-language-server"));
    }

    #[test]
    fn decodes_numeric_event_types_forward_compatibly() {
        let extension = JdtlsExtension;
        let event = extension
            .decode_notification(
                protocol::notification::EVENT,
                json!({ "eventType": 500, "data": { "uri": "jdt://class" } }),
            )
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ServerExtensionNotification::Event { event_type, .. }
                if event_type == "SourceInvalidated"
        ));
    }

    #[test]
    fn decodes_numeric_actionable_severity() {
        let extension = JdtlsExtension;
        let event = extension
            .decode_notification(
                protocol::notification::ACTIONABLE,
                json!({
                    "severity": 1,
                    "message": "Project configuration failed",
                    "commands": []
                }),
            )
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ServerExtensionNotification::Actionable {
                severity: ExtensionMessageSeverity::Error,
                ..
            }
        ));
    }

    #[test]
    fn preserves_unknown_numeric_event_types() {
        let extension = JdtlsExtension;
        let event = extension
            .decode_notification(
                protocol::notification::EVENT,
                json!({ "eventType": 999, "data": null }),
            )
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ServerExtensionNotification::Event { event_type, .. }
                if event_type == "999"
        ));
    }

    #[test]
    fn computes_legacy_progress_percentage() {
        let extension = JdtlsExtension;
        let event = extension
            .decode_notification(
                protocol::notification::PROGRESS,
                json!({
                    "id": "import",
                    "task": "Importing projects",
                    "totalWork": 8,
                    "workDone": 3,
                    "complete": false
                }),
            )
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ServerExtensionNotification::Progress {
                percentage: Some(37),
                ..
            }
        ));
    }
}
