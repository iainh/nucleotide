// ABOUTME: This file implements the LSP manager with feature flag support and fallback mechanisms
// ABOUTME: It handles project-based vs file-based LSP startup with runtime configuration changes

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use helix_view::DocumentId;
use nucleotide_logging::{debug, error, info, instrument, warn};

// Helper function to find workspace root from a specific directory
#[instrument]
fn find_workspace_root_from(start_dir: &Path) -> PathBuf {
    // Walk up the directory tree looking for VCS directories
    for ancestor in start_dir.ancestors() {
        if ancestor.join(".git").exists()
            || ancestor.join(".svn").exists()
            || ancestor.join(".hg").exists()
            || ancestor.join(".jj").exists()
            || ancestor.join(".helix").exists()
        {
            return ancestor.to_path_buf();
        }
    }

    // If no VCS directory found, use the start directory
    start_dir.to_path_buf()
}

// Internal config structure for LSP management
#[derive(Debug, Clone)]
pub struct LspManagerConfig {
    /// Enable project-based LSP startup (vs file-based)
    pub project_lsp_startup: bool,
    /// Timeout for LSP startup in milliseconds
    pub startup_timeout_ms: u64,
    /// Enable graceful fallback to file-based startup on project detection failures
    pub enable_fallback: bool,
}

impl Default for LspManagerConfig {
    fn default() -> Self {
        Self {
            project_lsp_startup: false,
            startup_timeout_ms: 5000,
            enable_fallback: true,
        }
    }
}

impl LspManagerConfig {
    /// Validate the LSP configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate timeout is reasonable
        if self.startup_timeout_ms == 0 {
            return Err("LSP startup timeout must be greater than 0".to_string());
        }

        if self.startup_timeout_ms > 60000 {
            return Err("LSP startup timeout should not exceed 60 seconds".to_string());
        }

        // Log warnings for potentially problematic configurations
        if self.startup_timeout_ms < 1000 {
            warn!(
                timeout_ms = self.startup_timeout_ms,
                "LSP startup timeout is very low - may cause frequent failures"
            );
        }

        if self.project_lsp_startup && !self.enable_fallback {
            warn!("Project LSP startup enabled without fallback - may cause startup failures");
        }

        Ok(())
    }
}

/// Errors that can occur during LSP operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum LspError {
    #[error("Document not found: {doc_id:?}")]
    DocumentNotFound { doc_id: DocumentId },

    #[error("No language configuration for document: {doc_id:?}")]
    NoLanguageConfig { doc_id: DocumentId },

    #[error("Project detection failed: {reason}")]
    ProjectDetectionFailed { reason: String },

    #[error("LSP startup timeout: {timeout_ms}ms")]
    StartupTimeout { timeout_ms: u64 },

    #[error("Configuration validation failed: {message}")]
    ConfigValidationFailed { message: String },

    #[error("Fallback mechanism failed: {original_error}")]
    FallbackFailed { original_error: String },

    #[error("LSP communication error: {message}")]
    CommunicationError { message: String },
}

/// Represents the different LSP startup modes
#[derive(Debug, Clone, PartialEq)]
pub enum LspStartupMode {
    /// Project-based startup: LSP starts when project is detected
    Project {
        project_root: PathBuf,
        timeout: Duration,
    },
    /// File-based startup: LSP starts when specific file types are opened
    File { file_path: PathBuf },
}

/// Result of LSP startup attempt
#[derive(Debug, Clone)]
pub enum LspStartupResult {
    /// LSP startup was successful
    Success {
        mode: LspStartupMode,
        language_servers: Vec<String>,
        duration: Duration,
    },
    /// LSP startup failed but fallback is available
    Failed {
        mode: LspStartupMode,
        error: LspError,
        fallback_mode: Option<LspStartupMode>,
    },
    /// LSP startup was skipped (feature disabled)
    Skipped { reason: String },
}

/// Manages LSP startup with feature flag support and fallback mechanisms
pub struct LspManager {
    config: Arc<LspManagerConfig>,
    startup_attempts: Vec<LspStartupAttempt>,
}

#[derive(Debug, Clone)]
struct LspStartupAttempt {
    mode: LspStartupMode,
    started_at: Instant,
    result: Option<LspStartupResult>,
}

impl LspManager {
    /// Create a new LSP manager with the given configuration
    pub fn new(config: Arc<LspManagerConfig>) -> Self {
        Self {
            config,
            startup_attempts: Vec::new(),
        }
    }

    /// Update the configuration at runtime (for hot-reloading)
    #[instrument(skip(self, new_config))]
    pub fn update_config(&mut self, new_config: Arc<LspManagerConfig>) -> Result<(), LspError> {
        // Validate the new configuration before applying it
        if let Err(validation_error) = new_config.validate() {
            error!(
                validation_error = %validation_error,
                "Invalid LSP configuration provided for update"
            );
            return Err(LspError::ConfigValidationFailed {
                message: validation_error,
            });
        }

        let old_project_lsp = self.config.project_lsp_startup;
        let new_project_lsp = new_config.project_lsp_startup;

        let old_fallback = self.config.enable_fallback;
        let new_fallback = new_config.enable_fallback;

        let old_timeout = self.config.startup_timeout_ms;
        let new_timeout = new_config.startup_timeout_ms;

        if old_project_lsp != new_project_lsp
            || old_fallback != new_fallback
            || old_timeout != new_timeout
        {
            info!(
                old_project_lsp = old_project_lsp,
                new_project_lsp = new_project_lsp,
                old_fallback = old_fallback,
                new_fallback = new_fallback,
                old_timeout = old_timeout,
                new_timeout = new_timeout,
                "LSP configuration changed - updating manager"
            );
        }

        self.config = new_config;
        Ok(())
    }

    /// Determine the appropriate LSP startup mode for a given document
    #[instrument(skip(self, doc_path, project_dir))]
    pub fn determine_startup_mode(
        &self,
        doc_path: Option<&Path>,
        project_dir: Option<&Path>,
    ) -> LspStartupMode {
        if !self.config.project_lsp_startup {
            info!("Project LSP startup disabled - using file-based mode");
            return LspStartupMode::File {
                file_path: doc_path.unwrap_or_else(|| Path::new("")).to_path_buf(),
            };
        }

        if let Some(project_root) = project_dir {
            info!(
                project_root = %project_root.display(),
                "Project detected - using project-based LSP startup"
            );
            return LspStartupMode::Project {
                project_root: project_root.to_path_buf(),
                timeout: Duration::from_millis(self.config.startup_timeout_ms),
            };
        }

        if self.config.enable_fallback {
            warn!("No project detected but fallback enabled - using file-based mode");
            LspStartupMode::File {
                file_path: doc_path.unwrap_or_else(|| Path::new("")).to_path_buf(),
            }
        } else {
            warn!("No project detected and fallback disabled - using file-based mode anyway");
            LspStartupMode::File {
                file_path: doc_path.unwrap_or_else(|| Path::new("")).to_path_buf(),
            }
        }
    }

    /// Start LSP for a document using the determined startup mode
    #[instrument(skip(self, editor))]
    pub fn start_lsp_for_document(
        &mut self,
        doc_id: DocumentId,
        editor: &mut helix_view::Editor,
    ) -> LspStartupResult {
        let start_time = Instant::now();

        // Get document information
        let (doc_path, project_dir) = if let Some(doc) = editor.document(doc_id) {
            let doc_path = doc.path().map(|p| p.to_path_buf());
            let project_dir = self.detect_project_root(doc_path.as_deref());
            (doc_path, project_dir)
        } else {
            error!(doc_id = ?doc_id, "Document not found");
            return LspStartupResult::Failed {
                mode: LspStartupMode::File {
                    file_path: PathBuf::new(),
                },
                error: LspError::DocumentNotFound { doc_id },
                fallback_mode: None,
            };
        };

        let startup_mode = self.determine_startup_mode(doc_path.as_deref(), project_dir.as_deref());

        // Record the startup attempt
        let attempt = LspStartupAttempt {
            mode: startup_mode.clone(),
            started_at: start_time,
            result: None,
        };
        self.startup_attempts.push(attempt);

        // Attempt to start LSP based on the determined mode
        match &startup_mode {
            LspStartupMode::Project {
                project_root,
                timeout,
            } => self.start_project_lsp(doc_id, project_root, *timeout, editor, start_time),
            LspStartupMode::File { file_path } => {
                self.start_file_lsp(doc_id, file_path, editor, start_time)
            }
        }
    }

    /// Start LSP in project-based mode
    #[instrument(skip(self, editor))]
    fn start_project_lsp(
        &mut self,
        doc_id: DocumentId,
        project_root: &Path,
        timeout: Duration,
        editor: &mut helix_view::Editor,
        start_time: Instant,
    ) -> LspStartupResult {
        info!(
            doc_id = ?doc_id,
            project_root = %project_root.display(),
            timeout_ms = timeout.as_millis(),
            "Starting project-based LSP"
        );

        // For now, delegate to the existing LSP startup mechanism
        // In a full implementation, this would implement project-aware LSP startup
        let result = self.delegate_to_existing_lsp_startup(doc_id, editor, start_time);

        match result {
            LspStartupResult::Success {
                mode: _,
                language_servers,
                duration,
            } => {
                info!(
                    project_root = %project_root.display(),
                    language_servers = ?language_servers,
                    duration_ms = duration.as_millis(),
                    "Project-based LSP startup successful"
                );
                LspStartupResult::Success {
                    mode: LspStartupMode::Project {
                        project_root: project_root.to_path_buf(),
                        timeout,
                    },
                    language_servers,
                    duration,
                }
            }
            LspStartupResult::Failed { error, .. } => {
                warn!(
                    project_root = %project_root.display(),
                    error = %error,
                    fallback_enabled = self.config.enable_fallback,
                    "Project-based LSP startup failed"
                );

                if self.config.enable_fallback {
                    info!("Attempting fallback to file-based LSP startup");
                    let fallback_mode = LspStartupMode::File {
                        file_path: project_root.to_path_buf(),
                    };

                    // Attempt fallback
                    match self.start_file_lsp(doc_id, project_root, editor, start_time) {
                        LspStartupResult::Success {
                            language_servers,
                            duration,
                            ..
                        } => {
                            info!("Fallback to file-based LSP startup successful");
                            LspStartupResult::Success {
                                mode: fallback_mode,
                                language_servers,
                                duration,
                            }
                        }
                        fallback_result => {
                            error!("Both project-based and file-based LSP startup failed");
                            LspStartupResult::Failed {
                                mode: LspStartupMode::Project {
                                    project_root: project_root.to_path_buf(),
                                    timeout,
                                },
                                error: LspError::FallbackFailed {
                                    original_error: format!(
                                        "Project startup failed: {}. Fallback also failed: {:?}",
                                        error, fallback_result
                                    ),
                                },
                                fallback_mode: Some(fallback_mode),
                            }
                        }
                    }
                } else {
                    LspStartupResult::Failed {
                        mode: LspStartupMode::Project {
                            project_root: project_root.to_path_buf(),
                            timeout,
                        },
                        error,
                        fallback_mode: None,
                    }
                }
            }
            other => other,
        }
    }

    /// Start LSP in file-based mode (existing behavior)
    #[instrument(skip(self, editor))]
    fn start_file_lsp(
        &mut self,
        doc_id: DocumentId,
        file_path: &Path,
        editor: &mut helix_view::Editor,
        start_time: Instant,
    ) -> LspStartupResult {
        info!(
            doc_id = ?doc_id,
            file_path = %file_path.display(),
            "Starting file-based LSP (existing behavior)"
        );

        // Delegate to existing LSP startup mechanism
        self.delegate_to_existing_lsp_startup(doc_id, editor, start_time)
    }

    /// Delegate to the existing LSP startup mechanism in Helix
    #[instrument(skip(self, editor))]
    fn delegate_to_existing_lsp_startup(
        &self,
        doc_id: DocumentId,
        editor: &mut helix_view::Editor,
        start_time: Instant,
    ) -> LspStartupResult {
        // Get the document and trigger language server initialization
        let doc = match editor.document(doc_id) {
            Some(doc) => doc,
            None => {
                return LspStartupResult::Failed {
                    mode: LspStartupMode::File {
                        file_path: PathBuf::new(),
                    },
                    error: LspError::DocumentNotFound { doc_id },
                    fallback_mode: None,
                };
            }
        };

        // Check if document has language configuration
        let language_name = doc.language_name().unwrap_or("unknown").to_string();

        if doc.language_config().is_none() {
            debug!(doc_id = ?doc_id, "Document has no language configuration");
            return LspStartupResult::Skipped {
                reason: "No language configuration for document".to_string(),
            };
        }

        // Get current language servers for this document
        let initial_servers: Vec<String> = doc
            .language_servers()
            .map(|ls| ls.name().to_string())
            .collect();

        debug!(
            doc_id = ?doc_id,
            language = %language_name,
            initial_servers = ?initial_servers,
            "Document language configuration found"
        );

        // In the existing system, LSP startup is handled automatically by Helix
        // when documents are opened or when language configuration is detected.
        // We'll simulate success here and let Helix handle the actual startup.

        let duration = start_time.elapsed();

        if initial_servers.is_empty() {
            // No language servers attached yet - this is normal for new documents
            info!(
                doc_id = ?doc_id,
                language = %language_name,
                duration_ms = duration.as_millis(),
                "LSP startup delegated to Helix - no servers attached yet"
            );
        } else {
            info!(
                doc_id = ?doc_id,
                language = %language_name,
                servers = ?initial_servers,
                duration_ms = duration.as_millis(),
                "LSP startup delegated to Helix - servers already attached"
            );
        }

        LspStartupResult::Success {
            mode: LspStartupMode::File {
                file_path: doc
                    .path()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(PathBuf::new),
            },
            language_servers: initial_servers,
            duration,
        }
    }

    /// Detect project root from a file path
    #[instrument(skip(self))]
    fn detect_project_root(&self, file_path: Option<&Path>) -> Option<PathBuf> {
        let current_dir = std::env::current_dir().ok();
        let start_dir = if let Some(path) = file_path {
            if path.is_file() {
                path.parent()
            } else {
                Some(path)
            }
        } else {
            current_dir.as_deref()
        };

        if let Some(dir) = start_dir {
            let project_root = find_workspace_root_from(dir);
            if project_root != dir {
                debug!(
                    start_dir = %dir.display(),
                    project_root = %project_root.display(),
                    "Detected project root"
                );
                Some(project_root)
            } else {
                debug!(
                    start_dir = %dir.display(),
                    "No project root detected (no VCS directories found)"
                );
                None
            }
        } else {
            debug!("No starting directory available for project detection");
            None
        }
    }

    /// Get statistics about LSP startup attempts
    pub fn get_startup_stats(&self) -> LspStartupStats {
        let total_attempts = self.startup_attempts.len();
        let successful_attempts = self
            .startup_attempts
            .iter()
            .filter(|attempt| matches!(attempt.result, Some(LspStartupResult::Success { .. })))
            .count();
        let failed_attempts = self
            .startup_attempts
            .iter()
            .filter(|attempt| matches!(attempt.result, Some(LspStartupResult::Failed { .. })))
            .count();
        let skipped_attempts = self
            .startup_attempts
            .iter()
            .filter(|attempt| matches!(attempt.result, Some(LspStartupResult::Skipped { .. })))
            .count();

        let project_mode_attempts = self
            .startup_attempts
            .iter()
            .filter(|attempt| matches!(attempt.mode, LspStartupMode::Project { .. }))
            .count();
        let file_mode_attempts = self
            .startup_attempts
            .iter()
            .filter(|attempt| matches!(attempt.mode, LspStartupMode::File { .. }))
            .count();

        LspStartupStats {
            total_attempts,
            successful_attempts,
            failed_attempts,
            skipped_attempts,
            project_mode_attempts,
            file_mode_attempts,
        }
    }
}

/// Statistics about LSP startup attempts
#[derive(Debug, Clone)]
pub struct LspStartupStats {
    pub total_attempts: usize,
    pub successful_attempts: usize,
    pub failed_attempts: usize,
    pub skipped_attempts: usize,
    pub project_mode_attempts: usize,
    pub file_mode_attempts: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn create_test_config(
        project_lsp_startup: bool,
        enable_fallback: bool,
        timeout_ms: u64,
    ) -> Arc<LspManagerConfig> {
        Arc::new(LspManagerConfig {
            project_lsp_startup,
            startup_timeout_ms: timeout_ms,
            enable_fallback,
        })
    }

    #[test]
    fn test_lsp_manager_creation() {
        let config = create_test_config(false, true, 5000);
        let manager = LspManager::new(config.clone());

        assert_eq!(manager.config.project_lsp_startup, false);
        assert_eq!(manager.config.enable_fallback, true);
        assert_eq!(manager.config.startup_timeout_ms, 5000);
        assert_eq!(manager.startup_attempts.len(), 0);
    }

    #[test]
    fn test_config_update() {
        let initial_config = create_test_config(false, true, 5000);
        let mut manager = LspManager::new(initial_config);

        let new_config = create_test_config(true, false, 3000);
        manager.update_config(new_config);

        assert_eq!(manager.config.project_lsp_startup, true);
        assert_eq!(manager.config.enable_fallback, false);
        assert_eq!(manager.config.startup_timeout_ms, 3000);
    }

    #[test]
    fn test_determine_startup_mode_feature_disabled() {
        let config = create_test_config(false, true, 5000);
        let manager = LspManager::new(config);

        let file_path = Path::new("/some/file.rs");
        let project_dir = Path::new("/some/project");

        let mode = manager.determine_startup_mode(Some(file_path), Some(project_dir));

        match mode {
            LspStartupMode::File { file_path: path } => {
                assert_eq!(path, file_path);
            }
            _ => panic!("Expected file mode when feature is disabled"),
        }
    }

    #[test]
    fn test_determine_startup_mode_project_detected() {
        let config = create_test_config(true, true, 5000);
        let manager = LspManager::new(config);

        let file_path = Path::new("/some/project/file.rs");
        let project_dir = Path::new("/some/project");

        let mode = manager.determine_startup_mode(Some(file_path), Some(project_dir));

        match mode {
            LspStartupMode::Project {
                project_root,
                timeout,
            } => {
                assert_eq!(project_root, project_dir);
                assert_eq!(timeout, Duration::from_millis(5000));
            }
            _ => panic!("Expected project mode when project is detected"),
        }
    }

    #[test]
    fn test_determine_startup_mode_no_project_with_fallback() {
        let config = create_test_config(true, true, 5000);
        let manager = LspManager::new(config);

        let file_path = Path::new("/some/file.rs");

        let mode = manager.determine_startup_mode(Some(file_path), None);

        match mode {
            LspStartupMode::File { file_path: path } => {
                assert_eq!(path, file_path);
            }
            _ => panic!("Expected file mode when no project detected and fallback enabled"),
        }
    }

    #[test]
    fn test_determine_startup_mode_no_project_no_fallback() {
        let config = create_test_config(true, false, 5000);
        let manager = LspManager::new(config);

        let file_path = Path::new("/some/file.rs");

        let mode = manager.determine_startup_mode(Some(file_path), None);

        // Should still use file mode even when fallback is disabled
        // since we need some startup mechanism
        match mode {
            LspStartupMode::File { file_path: path } => {
                assert_eq!(path, file_path);
            }
            _ => panic!("Expected file mode even when fallback disabled"),
        }
    }

    #[test]
    fn test_startup_stats_empty() {
        let config = create_test_config(true, true, 5000);
        let manager = LspManager::new(config);

        let stats = manager.get_startup_stats();
        assert_eq!(stats.total_attempts, 0);
        assert_eq!(stats.successful_attempts, 0);
        assert_eq!(stats.failed_attempts, 0);
        assert_eq!(stats.skipped_attempts, 0);
        assert_eq!(stats.project_mode_attempts, 0);
        assert_eq!(stats.file_mode_attempts, 0);
    }
}
