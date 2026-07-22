// ABOUTME: Wire contracts and centralized method/command catalog for Eclipse JDT LS
// ABOUTME: Stable contracts are typed; evolving multi-step refactors retain JSON values

use helix_lsp::lsp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::extensions::{CustomCommand, CustomNotification, CustomRequest};

macro_rules! json_request {
    ($name:ident, $method:literal) => {
        pub enum $name {}
        impl CustomRequest for $name {
            type Params = Value;
            type Result = Value;
            const METHOD: &'static str = $method;
        }
    };
}

macro_rules! json_command {
    ($name:ident, $command:literal) => {
        pub enum $name {}
        impl CustomCommand for $name {
            type Result = Value;
            const COMMAND: &'static str = $command;
        }
    };
}

pub mod request {
    use super::*;

    pub enum ClassFileContents {}
    impl CustomRequest for ClassFileContents {
        type Params = lsp::TextDocumentIdentifier;
        type Result = String;
        const METHOD: &'static str = "java/classFileContents";
    }

    pub enum BuildWorkspace {}
    impl CustomRequest for BuildWorkspace {
        type Params = bool;
        type Result = BuildWorkspaceStatus;
        const METHOD: &'static str = "java/buildWorkspace";
    }

    pub enum BuildProjects {}
    impl CustomRequest for BuildProjects {
        type Params = ProjectBuildParams;
        type Result = BuildWorkspaceStatus;
        const METHOD: &'static str = "java/buildProjects";
    }

    pub enum OrganizeImports {}
    impl CustomRequest for OrganizeImports {
        type Params = lsp::CodeActionParams;
        type Result = lsp::WorkspaceEdit;
        const METHOD: &'static str = "java/organizeImports";
    }

    pub enum Cleanup {}
    impl CustomRequest for Cleanup {
        type Params = lsp::TextDocumentIdentifier;
        type Result = lsp::WorkspaceEdit;
        const METHOD: &'static str = "java/cleanup";
    }

    pub enum SearchSymbols {}
    impl CustomRequest for SearchSymbols {
        type Params = SearchSymbolParams;
        type Result = Vec<lsp::SymbolInformation>;
        const METHOD: &'static str = "java/searchSymbols";
    }

    json_request!(ListOverridableMethods, "java/listOverridableMethods");
    json_request!(AddOverridableMethods, "java/addOverridableMethods");
    json_request!(CheckHashCodeEqualsStatus, "java/checkHashCodeEqualsStatus");
    json_request!(GenerateHashCodeEquals, "java/generateHashCodeEquals");
    json_request!(CheckToStringStatus, "java/checkToStringStatus");
    json_request!(GenerateToString, "java/generateToString");
    json_request!(
        ResolveUnimplementedAccessors,
        "java/resolveUnimplementedAccessors"
    );
    json_request!(GenerateAccessors, "java/generateAccessors");
    json_request!(CheckConstructorsStatus, "java/checkConstructorsStatus");
    json_request!(GenerateConstructors, "java/generateConstructors");
    json_request!(
        CheckDelegateMethodsStatus,
        "java/checkDelegateMethodsStatus"
    );
    json_request!(GenerateDelegateMethods, "java/generateDelegateMethods");
    json_request!(GetRefactorEdit, "java/getRefactorEdit");
    json_request!(GetChangeSignatureInfo, "java/getChangeSignatureInfo");
    json_request!(InferSelection, "java/inferSelection");
    json_request!(GetMoveDestinations, "java/getMoveDestinations");
    json_request!(Move, "java/move");
    json_request!(FindLinks, "java/findLinks");
    json_request!(
        CheckExtractInterfaceStatus,
        "java/checkExtractInterfaceStatus"
    );
    json_request!(ExtendedDocumentSymbol, "java/extendedDocumentSymbol");
}

pub mod client_notification {
    use super::*;

    pub enum ProjectConfigurationUpdate {}
    impl CustomNotification for ProjectConfigurationUpdate {
        type Params = lsp::TextDocumentIdentifier;
        const METHOD: &'static str = "java/projectConfigurationUpdate";
    }

    pub enum ProjectConfigurationsUpdate {}
    impl CustomNotification for ProjectConfigurationsUpdate {
        type Params = ProjectConfigurationsUpdateParams;
        const METHOD: &'static str = "java/projectConfigurationsUpdate";
    }

    pub enum ValidateDocument {}
    impl CustomNotification for ValidateDocument {
        type Params = ValidateDocumentParams;
        const METHOD: &'static str = "java/validateDocument";
    }
}

pub mod command {
    use super::*;

    json_command!(OrganizeImports, "java.edit.organizeImports");
    json_command!(StringFormatting, "java.edit.stringFormatting");
    json_command!(HandlePasteEvent, "java.edit.handlePasteEvent");
    json_command!(SmartSemicolonDetection, "java.edit.smartSemicolonDetection");
    json_command!(
        ResolveSourceAttachment,
        "java.project.resolveSourceAttachment"
    );
    json_command!(
        UpdateSourceAttachment,
        "java.project.updateSourceAttachment"
    );
    json_command!(AddToSourcePath, "java.project.addToSourcePath");
    json_command!(RemoveFromSourcePath, "java.project.removeFromSourcePath");
    json_command!(ListSourcePaths, "java.project.listSourcePaths");
    json_command!(GetProjectSettings, "java.project.getSettings");
    json_command!(GetClasspaths, "java.project.getClasspaths");
    json_command!(UpdateClasspaths, "java.project.updateClassPaths");
    json_command!(UpdateProjectSettings, "java.project.updateSettings");

    pub enum IsTestFile {}
    impl CustomCommand for IsTestFile {
        type Result = bool;
        const COMMAND: &'static str = "java.project.isTestFile";
    }

    json_command!(GetAllProjects, "java.project.getAll");
    json_command!(RefreshDiagnostics, "java.project.refreshDiagnostics");
    json_command!(ImportProjects, "java.project.import");
    json_command!(
        ChangeImportedProjects,
        "java.project.changeImportedProjects"
    );
    json_command!(
        ResolveStackTraceLocation,
        "java.project.resolveStackTraceLocation"
    );
    json_command!(OpenTypeHierarchy, "java.navigate.openTypeHierarchy");
    json_command!(ResolveTypeHierarchy, "java.navigate.resolveTypeHierarchy");
    json_command!(UpgradeGradle, "java.project.upgradeGradle");
    json_command!(
        ResolveWorkspaceSymbol,
        "java.project.resolveWorkspaceSymbol"
    );
    json_command!(UpdateJdk, "java.project.updateJdk");
    json_command!(GenerateProtobufSources, "java.protobuf.generateSources");
    json_command!(CreateModuleInfo, "java.project.createModuleInfo");

    pub enum ReloadBundles {}
    impl CustomCommand for ReloadBundles {
        type Result = bool;
        const COMMAND: &'static str = "java.reloadBundles";
    }

    json_command!(CompletionSelected, "java.completion.onDidSelect");
    json_command!(Decompile, "java.decompile");
    json_command!(GetVmInstalls, "java.vm.getAllInstalls");
    json_command!(GetTroubleshootingInfo, "java.getTroubleshootingInfo");
    json_command!(ResolveText, "java.project.resolveText");
    json_command!(GetFullyQualifiedName, "java.getFullyQualifiedName");
}

pub mod notification {
    pub const STATUS: &str = "language/status";
    pub const ACTIONABLE: &str = "language/actionableNotification";
    pub const EVENT: &str = "language/eventNotification";
    pub const PROGRESS: &str = "language/progressReport";
    pub const WORKSPACE_NOTIFY: &str = "workspace/notify";
    pub const EXECUTE_CLIENT_COMMAND: &str = "workspace/executeClientCommand";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildWorkspaceStatus {
    Failed,
    Succeed,
    WithError,
    Cancelled,
    Unknown,
}

impl Serialize for BuildWorkspaceStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::Failed => 0,
            Self::Succeed => 1,
            Self::WithError => 2,
            Self::Cancelled => 3,
            Self::Unknown => -1,
        };
        serializer.serialize_i64(value)
    }
}

impl<'de> Deserialize<'de> for BuildWorkspaceStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireValue {
            Number(i64),
            Name(String),
        }

        let status = match WireValue::deserialize(deserializer)? {
            WireValue::Number(0) => Self::Failed,
            WireValue::Number(1) => Self::Succeed,
            WireValue::Number(2) => Self::WithError,
            WireValue::Number(3) => Self::Cancelled,
            WireValue::Name(name) => match name.as_str() {
                "FAILED" | "0" => Self::Failed,
                "SUCCEED" | "1" => Self::Succeed,
                "WITH_ERROR" | "2" => Self::WithError,
                "CANCELLED" | "3" => Self::Cancelled,
                _ => Self::Unknown,
            },
            WireValue::Number(_) => Self::Unknown,
        };
        Ok(status)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigurationsUpdateParams {
    pub identifiers: Vec<lsp::TextDocumentIdentifier>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBuildParams {
    pub identifiers: Vec<lsp::TextDocumentIdentifier>,
    pub is_full_build: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidateDocumentParams {
    pub text_document: lsp::TextDocumentIdentifier,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchSymbolParams {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_only: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    #[serde(rename = "type")]
    pub status_type: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActionableNotification {
    pub severity: Value,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub commands: Vec<lsp::Command>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventNotification {
    pub event_type: Value,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressReport {
    pub id: String,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub sub_task: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub total_work: Option<u64>,
    #[serde(default)]
    pub work_done: Option<u64>,
    #[serde(default)]
    pub complete: bool,
}

pub const CUSTOM_CODE_ACTION_KINDS: &[&str] = &[
    "source.generate",
    "source.generate.accessors",
    "source.generate.hashCodeEquals",
    "source.generate.toString",
    "source.generate.constructors",
    "source.generate.delegateMethods",
    "source.generate.finalModifiers",
    "source.overrideMethods",
    "source.sortMembers",
    "refactor.extract.function",
    "refactor.extract.constant",
    "refactor.extract.variable",
    "refactor.extract.field",
    "refactor.extract.interface",
    "refactor.move",
    "refactor.assign.variable",
    "refactor.assign.field",
    "refactor.introduce.parameter",
    "refactor.change.signature",
    "quickassist",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_status_tolerates_new_server_values() {
        let status: BuildWorkspaceStatus = serde_json::from_str("99").unwrap();
        assert_eq!(status, BuildWorkspaceStatus::Unknown);
    }

    #[test]
    fn build_status_uses_lsp4j_numeric_wire_values() {
        let status: BuildWorkspaceStatus = serde_json::from_str("2").unwrap();
        assert_eq!(status, BuildWorkspaceStatus::WithError);
        assert_eq!(serde_json::to_string(&status).unwrap(), "2");
    }

    #[test]
    fn project_build_params_use_jdt_ls_camel_case_fields() {
        let params = ProjectBuildParams {
            identifiers: vec![lsp::TextDocumentIdentifier {
                uri: lsp::Url::parse("file:///workspace/project").unwrap(),
            }],
            is_full_build: true,
        };

        assert_eq!(
            serde_json::to_value(params).unwrap(),
            serde_json::json!({
                "identifiers": [{ "uri": "file:///workspace/project" }],
                "isFullBuild": true
            })
        );
    }

    #[test]
    fn code_action_kinds_are_unique() {
        let unique = CUSTOM_CODE_ACTION_KINDS
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), CUSTOM_CODE_ACTION_KINDS.len());
    }
}
