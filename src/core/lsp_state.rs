// ABOUTME: LSP state management for reactive UI updates
// ABOUTME: Provides a GPUI Model for LSP status, diagnostics, and progress

use gpui::*;
use helix_lsp::{LanguageServerId, LspProgressMap};
use helix_core::Uri;
use helix_core::diagnostic::Diagnostic;
use std::collections::{HashMap, BTreeMap};

/// Progress information for a single LSP operation
#[derive(Clone, Debug)]
pub struct LspProgress {
    pub server_id: LanguageServerId,
    pub token: String,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

/// Status of a language server
#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    Starting,
    Initializing,
    Running,
    Failed(String),
    Stopped,
}

/// Information about a language server
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub id: LanguageServerId,
    pub name: String,
    pub status: ServerStatus,
    pub root_uri: Option<String>,
    pub capabilities: Option<serde_json::Value>,
}

/// Diagnostic information with source server
#[derive(Clone, Debug)]
pub struct DiagnosticInfo {
    pub diagnostic: Diagnostic,
    pub server_id: LanguageServerId,
}

/// LSP state that can be observed by UI components
pub struct LspState {
    /// Active language servers
    pub servers: HashMap<LanguageServerId, ServerInfo>,
    
    /// Current progress operations
    pub progress: HashMap<String, LspProgress>,
    
    /// Diagnostics by file URI
    pub diagnostics: BTreeMap<Uri, Vec<DiagnosticInfo>>,
    
    /// Last status message
    pub status_message: Option<String>,
    
    /// Progress map from helix
    pub progress_map: LspProgressMap,
}

impl LspState {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            progress: HashMap::new(),
            diagnostics: BTreeMap::new(),
            status_message: None,
            progress_map: LspProgressMap::new(),
        }
    }
    
    /// Register a new language server
    pub fn register_server(&mut self, id: LanguageServerId, name: String, root_uri: Option<String>) {
        self.servers.insert(id, ServerInfo {
            id,
            name,
            status: ServerStatus::Starting,
            root_uri,
            capabilities: None,
        });
    }
    
    /// Update server status
    pub fn update_server_status(&mut self, id: LanguageServerId, status: ServerStatus) {
        if let Some(server) = self.servers.get_mut(&id) {
            server.status = status;
        }
    }
    
    /// Update server capabilities
    pub fn update_server_capabilities(&mut self, id: LanguageServerId, capabilities: serde_json::Value) {
        if let Some(server) = self.servers.get_mut(&id) {
            server.capabilities = Some(capabilities);
            server.status = ServerStatus::Running;
        }
    }
    
    /// Remove a language server
    pub fn remove_server(&mut self, id: LanguageServerId) {
        self.servers.remove(&id);
        
        // Remove all progress for this server
        self.progress.retain(|_, p| p.server_id != id);
        
        // Remove all diagnostics from this server
        for diagnostics in self.diagnostics.values_mut() {
            diagnostics.retain(|d| d.server_id != id);
        }
        self.diagnostics.retain(|_, diags| !diags.is_empty());
    }
    
    /// Start a progress operation
    pub fn start_progress(&mut self, server_id: LanguageServerId, token: String, title: String) {
        let key = format!("{}-{}", server_id, token);
        self.progress.insert(key, LspProgress {
            server_id,
            token,
            title,
            message: None,
            percentage: None,
        });
    }
    
    /// Update progress
    pub fn update_progress(&mut self, server_id: LanguageServerId, token: String, message: Option<String>, percentage: Option<u32>) {
        let key = format!("{}-{}", server_id, token);
        if let Some(progress) = self.progress.get_mut(&key) {
            if let Some(msg) = message {
                progress.message = Some(msg);
            }
            if let Some(pct) = percentage {
                progress.percentage = Some(pct);
            }
        }
    }
    
    /// End a progress operation
    pub fn end_progress(&mut self, server_id: LanguageServerId, token: String) {
        let key = format!("{}-{}", server_id, token);
        self.progress.remove(&key);
    }
    
    /// Set diagnostics for a file
    pub fn set_diagnostics(&mut self, uri: Uri, diagnostics: Vec<DiagnosticInfo>) {
        if diagnostics.is_empty() {
            self.diagnostics.remove(&uri);
        } else {
            self.diagnostics.insert(uri, diagnostics);
        }
    }
    
    /// Get diagnostics for a file
    pub fn get_diagnostics(&self, uri: &Uri) -> Option<&Vec<DiagnosticInfo>> {
        self.diagnostics.get(uri)
    }
    
    /// Get all active progress operations
    pub fn active_progress(&self) -> Vec<&LspProgress> {
        self.progress.values().collect()
    }
    
    /// Get running servers count
    pub fn running_servers_count(&self) -> usize {
        self.servers.values()
            .filter(|s| matches!(s.status, ServerStatus::Running))
            .count()
    }
    
    /// Check if any server is busy
    pub fn is_busy(&self) -> bool {
        !self.progress.is_empty()
    }
    
    /// Get a formatted status string
    pub fn status_string(&self) -> Option<String> {
        if let Some(msg) = &self.status_message {
            return Some(msg.clone());
        }
        
        if !self.progress.is_empty() {
            let progress_items: Vec<String> = self.progress.values()
                .map(|p| {
                    let mut status = p.title.clone();
                    if let Some(msg) = &p.message {
                        status.push_str(": ");
                        status.push_str(msg);
                    }
                    if let Some(pct) = p.percentage {
                        status.push_str(&format!(" ({}%)", pct));
                    }
                    status
                })
                .collect();
            return Some(progress_items.join(", "));
        }
        
        None
    }
}

impl Default for LspState {
    fn default() -> Self {
        Self::new()
    }
}