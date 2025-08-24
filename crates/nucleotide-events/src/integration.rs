// ABOUTME: Integration events for cross-domain coordination
// ABOUTME: Handles communication between bounded contexts without breaking domain boundaries

use helix_lsp::LanguageServerId;
use helix_view::{DocumentId, ViewId};

/// Integration events for coordinating between bounded contexts
/// These events enable cross-domain communication while maintaining clean domain boundaries
#[derive(Debug, Clone)]
pub enum Event {
    /// Document-View synchronization
    DocumentViewSync {
        doc_id: DocumentId,
        view_id: ViewId,
        sync_type: SyncType,
    },

    /// LSP-Document association  
    LspDocumentAssociation {
        doc_id: DocumentId,
        server_id: LanguageServerId,
        association_type: AssociationType,
    },

    /// Completion coordination across domains
    CompletionCoordination {
        request_id: crate::completion::CompletionRequestId,
        coordination_type: CoordinationType,
    },

    /// UI-Editor state synchronization
    UiEditorSync {
        sync_type: UiEditorSyncType,
        data: UiEditorSyncData,
    },

    /// Workspace-LSP project coordination
    WorkspaceLspCoordination {
        workspace_root: std::path::PathBuf,
        coordination_type: WorkspaceLspCoordinationType,
    },

    /// Error recovery coordination
    ErrorRecoveryCoordination {
        error_type: ErrorType,
        recovery_action: RecoveryAction,
    },
}

/// Types of document-view synchronization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncType {
    SelectionToView,
    ViewToDocument,
    ScrollSync,
    FocusSync,
}

/// Types of LSP-document association
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationType {
    Attach,
    Detach,
    LanguageChange,
}

/// Types of completion coordination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinationType {
    RequestToLsp,
    ResultsToUi,
    CancelRequest,
    FilterUpdate,
}

/// Types of UI-editor synchronization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEditorSyncType {
    ThemeChange,
    FontChange,
    ModeSync,
    StatusSync,
    DocumentViewRefresh,
    SaveIndicatorUpdate,
    DiagnosticIndicatorUpdate,
    FileTreeUpdate,
    TabBarUpdate,
}

/// Data for UI-editor synchronization
#[derive(Debug, Clone)]
pub enum UiEditorSyncData {
    ThemeData { theme_name: String, is_dark: bool },
    FontData { family: String, size: f32 },
    ModeData { mode: helix_view::document::Mode },
    StatusData { message: String, severity: String },
    DocumentViewData { doc_id: DocumentId, revision: u64 },
    SaveIndicatorData { doc_id: DocumentId, is_modified: bool },
    DiagnosticData { doc_id: DocumentId, error_count: usize, warning_count: usize },
    FileTreeData { doc_id: DocumentId, action: FileTreeAction },
    TabBarData { doc_id: DocumentId, action: TabBarAction },
}

/// Actions for file tree updates
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTreeAction {
    Refresh,
    HighlightDocument,
    ShowDocument,
}

/// Actions for tab bar updates
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarAction {
    AddTab,
    RemoveTab,
    UpdateTab,
    HighlightTab,
    ShowSaveIndicator,
    HideSaveIndicator,
}

/// Types of workspace-LSP coordination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLspCoordinationType {
    ProjectDetection,
    ServerStartup,
    WorkspaceChange,
    ProjectClosure,
}

/// Types of errors requiring coordination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    LspServerFailure,
    DocumentLoadFailure,
    CompletionTimeout,
    ViewSyncFailure,
}

/// Recovery actions for error coordination
#[derive(Debug, Clone)]
pub enum RecoveryAction {
    RestartLspServer {
        server_id: LanguageServerId,
    },
    ReloadDocument {
        doc_id: DocumentId,
    },
    CancelCompletion {
        request_id: crate::completion::CompletionRequestId,
    },
    RefreshView {
        view_id: ViewId,
    },
    ShowUserError {
        message: String,
    },
}

impl Event {
    /// Create a document-view sync event
    pub fn document_view_sync(doc_id: DocumentId, view_id: ViewId, sync_type: SyncType) -> Self {
        Self::DocumentViewSync {
            doc_id,
            view_id,
            sync_type,
        }
    }

    /// Create an LSP-document association event
    pub fn lsp_document_association(
        doc_id: DocumentId,
        server_id: LanguageServerId,
        association_type: AssociationType,
    ) -> Self {
        Self::LspDocumentAssociation {
            doc_id,
            server_id,
            association_type,
        }
    }

    /// Create a completion coordination event
    pub fn completion_coordination(
        request_id: crate::completion::CompletionRequestId,
        coordination_type: CoordinationType,
    ) -> Self {
        Self::CompletionCoordination {
            request_id,
            coordination_type,
        }
    }

    /// Create a UI-editor sync event
    pub fn ui_editor_sync(sync_type: UiEditorSyncType, data: UiEditorSyncData) -> Self {
        Self::UiEditorSync { sync_type, data }
    }

    /// Create a workspace-LSP coordination event
    pub fn workspace_lsp_coordination(
        workspace_root: std::path::PathBuf,
        coordination_type: WorkspaceLspCoordinationType,
    ) -> Self {
        Self::WorkspaceLspCoordination {
            workspace_root,
            coordination_type,
        }
    }

    /// Create an error recovery coordination event
    pub fn error_recovery(error_type: ErrorType, recovery_action: RecoveryAction) -> Self {
        Self::ErrorRecoveryCoordination {
            error_type,
            recovery_action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_view_sync_creation() {
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        let event = Event::document_view_sync(doc_id, view_id, SyncType::SelectionToView);

        match event {
            Event::DocumentViewSync {
                doc_id: d,
                view_id: v,
                sync_type,
            } => {
                assert_eq!(d, doc_id);
                assert_eq!(v, view_id);
                assert_eq!(sync_type, SyncType::SelectionToView);
            }
            _ => panic!("Expected DocumentViewSync event"),
        }
    }

    #[test]
    fn test_lsp_document_association() {
        let doc_id = DocumentId::default();
        let server_id = LanguageServerId::default();

        let event = Event::lsp_document_association(doc_id, server_id, AssociationType::Attach);

        match event {
            Event::LspDocumentAssociation {
                doc_id: d,
                server_id: s,
                association_type,
            } => {
                assert_eq!(d, doc_id);
                assert_eq!(s, server_id);
                assert_eq!(association_type, AssociationType::Attach);
            }
            _ => panic!("Expected LspDocumentAssociation event"),
        }
    }

    #[test]
    fn test_completion_coordination() {
        let request_id = crate::completion::CompletionRequestId::new(123);

        let event = Event::completion_coordination(request_id, CoordinationType::RequestToLsp);

        match event {
            Event::CompletionCoordination {
                request_id: r,
                coordination_type,
            } => {
                assert_eq!(r, request_id);
                assert_eq!(coordination_type, CoordinationType::RequestToLsp);
            }
            _ => panic!("Expected CompletionCoordination event"),
        }
    }

    #[test]
    fn test_ui_editor_sync() {
        let theme_data = UiEditorSyncData::ThemeData {
            theme_name: "dracula".to_string(),
            is_dark: true,
        };

        let event = Event::ui_editor_sync(UiEditorSyncType::ThemeChange, theme_data);

        match event {
            Event::UiEditorSync { sync_type, data } => {
                assert_eq!(sync_type, UiEditorSyncType::ThemeChange);
                match data {
                    UiEditorSyncData::ThemeData {
                        theme_name,
                        is_dark,
                    } => {
                        assert_eq!(theme_name, "dracula");
                        assert!(is_dark);
                    }
                    _ => panic!("Expected ThemeData"),
                }
            }
            _ => panic!("Expected UiEditorSync event"),
        }
    }

    #[test]
    fn test_sync_types() {
        let sync_types = [
            SyncType::SelectionToView,
            SyncType::ViewToDocument,
            SyncType::ScrollSync,
            SyncType::FocusSync,
        ];

        for sync_type in sync_types {
            let _event =
                Event::document_view_sync(DocumentId::default(), ViewId::default(), sync_type);
        }
    }

    #[test]
    fn test_association_types() {
        let association_types = [
            AssociationType::Attach,
            AssociationType::Detach,
            AssociationType::LanguageChange,
        ];

        for association_type in association_types {
            let _event = Event::lsp_document_association(
                DocumentId::default(),
                LanguageServerId::default(),
                association_type,
            );
        }
    }

    #[test]
    fn test_coordination_types() {
        let coordination_types = [
            CoordinationType::RequestToLsp,
            CoordinationType::ResultsToUi,
            CoordinationType::CancelRequest,
            CoordinationType::FilterUpdate,
        ];

        for coordination_type in coordination_types {
            let _event = Event::completion_coordination(
                crate::completion::CompletionRequestId::new(1),
                coordination_type,
            );
        }
    }

    #[test]
    fn test_error_recovery_coordination() {
        let recovery_action = RecoveryAction::RestartLspServer {
            server_id: LanguageServerId::default(),
        };

        let event = Event::error_recovery(ErrorType::LspServerFailure, recovery_action);

        match event {
            Event::ErrorRecoveryCoordination {
                error_type,
                recovery_action,
            } => {
                assert_eq!(error_type, ErrorType::LspServerFailure);
                match recovery_action {
                    RecoveryAction::RestartLspServer { server_id: _ } => {
                        // Success
                    }
                    _ => panic!("Expected RestartLspServer recovery action"),
                }
            }
            _ => panic!("Expected ErrorRecoveryCoordination event"),
        }
    }

    #[test]
    fn test_workspace_lsp_coordination_types() {
        let coordination_types = [
            WorkspaceLspCoordinationType::ProjectDetection,
            WorkspaceLspCoordinationType::ServerStartup,
            WorkspaceLspCoordinationType::WorkspaceChange,
            WorkspaceLspCoordinationType::ProjectClosure,
        ];

        for coordination_type in coordination_types {
            let _event = Event::workspace_lsp_coordination(
                std::path::PathBuf::from("/test"),
                coordination_type,
            );
        }
    }

    #[test]
    fn test_ui_editor_sync_data_variants() {
        let sync_data_variants = [
            UiEditorSyncData::ThemeData {
                theme_name: "dark".to_string(),
                is_dark: true,
            },
            UiEditorSyncData::FontData {
                family: "Fira Code".to_string(),
                size: 14.0,
            },
            UiEditorSyncData::ModeData {
                mode: helix_view::document::Mode::Normal,
            },
            UiEditorSyncData::StatusData {
                message: "Ready".to_string(),
                severity: "info".to_string(),
            },
            UiEditorSyncData::DocumentViewData {
                doc_id: DocumentId::default(),
                revision: 1,
            },
            UiEditorSyncData::SaveIndicatorData {
                doc_id: DocumentId::default(),
                is_modified: true,
            },
            UiEditorSyncData::DiagnosticData {
                doc_id: DocumentId::default(),
                error_count: 2,
                warning_count: 5,
            },
            UiEditorSyncData::FileTreeData {
                doc_id: DocumentId::default(),
                action: FileTreeAction::HighlightDocument,
            },
            UiEditorSyncData::TabBarData {
                doc_id: DocumentId::default(),
                action: TabBarAction::UpdateTab,
            },
        ];

        let sync_types = [
            UiEditorSyncType::ThemeChange,
            UiEditorSyncType::FontChange,
            UiEditorSyncType::ModeSync,
            UiEditorSyncType::StatusSync,
            UiEditorSyncType::DocumentViewRefresh,
            UiEditorSyncType::SaveIndicatorUpdate,
            UiEditorSyncType::DiagnosticIndicatorUpdate,
            UiEditorSyncType::FileTreeUpdate,
            UiEditorSyncType::TabBarUpdate,
        ];

        for (sync_type, data) in sync_types.into_iter().zip(sync_data_variants.into_iter()) {
            let _event = Event::ui_editor_sync(sync_type, data);
        }
    }
}
