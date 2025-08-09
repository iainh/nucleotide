// ABOUTME: LSP state management for reactive UI updates
// ABOUTME: Provides a GPUI Model for LSP status, diagnostics, and progress

use helix_lsp::LanguageServerId;
use helix_core::Uri;
use helix_core::diagnostic::Diagnostic;
use std::collections::{HashMap, BTreeMap};
use std::time::{Duration, Instant};

/// Spinner frames matching helix-term
pub const SPINNER_FRAMES: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

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
    
    /// Spinner state
    pub spinner_frame: usize,
    pub last_spinner_update: Instant,
}

impl LspState {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            progress: HashMap::new(),
            diagnostics: BTreeMap::new(),
            status_message: None,
            spinner_frame: 0,
            last_spinner_update: Instant::now(),
        }
    }
    
    /// Get the current spinner frame, advancing if needed
    pub fn get_spinner_frame(&mut self) -> &'static str {
        let now = Instant::now();
        if now.duration_since(self.last_spinner_update) >= SPINNER_INTERVAL {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.last_spinner_update = now;
        }
        SPINNER_FRAMES[self.spinner_frame]
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
    
    /// Check if we should show spinner (when there's LSP activity)
    pub fn should_show_spinner(&self) -> bool {
        // Show spinner if:
        // - There are active progress operations
        // - Any server is starting/initializing
        let has_progress = !self.progress.is_empty();
        let has_busy_servers = self.servers.values().any(|s| matches!(s.status, ServerStatus::Starting | ServerStatus::Initializing));
        
        has_progress || has_busy_servers
    }
    
    /// Check if any LSP servers are present (regardless of activity)
    pub fn has_any_lsp_server(&self) -> bool {
        !self.servers.is_empty()
    }
    
    /// Get the appropriate LSP indicator character
    pub fn get_lsp_indicator(&mut self) -> Option<String> {
        if !self.has_any_lsp_server() {
            return None;
        }
        
        let indicator = if self.should_show_spinner() {
            // If there's activity, use the animated spinner frame
            self.get_spinner_frame().to_string()
        } else {
            // Otherwise, use a static indicator showing LSP is present but idle
            "◉".to_string()  // Static dot indicating LSP presence
        };
        
        // Add any progress message if available
        if !self.progress.is_empty() {
            // Get the most recent progress message
            if let Some(progress) = self.progress.values().next() {
                let mut result = indicator;
                result.push(' ');
                
                // Build the message
                let mut message = progress.title.clone();
                if let Some(msg) = &progress.message {
                    message.push_str(": ");
                    message.push_str(msg);
                }
                if let Some(pct) = progress.percentage {
                    message.push_str(&format!(" ({}%)", pct));
                }
                
                // Truncate if too long (max 40 chars for the message part)
                const MAX_MESSAGE_LEN: usize = 40;
                if message.len() > MAX_MESSAGE_LEN {
                    message.truncate(MAX_MESSAGE_LEN - 3);
                    message.push_str("...");
                }
                
                result.push_str(&message);
                return Some(result);
            }
        }
        
        // Just return the indicator if no progress messages
        Some(indicator)
    }
}

impl Default for LspState {
    fn default() -> Self {
        Self::new()
    }
}