# LSP Extension Implementation Plan for Nucleotide

## Overview

This document outlines the implementation of rust-analyzer's custom LSP extensions in Nucleotide using an extension pattern that avoids forking the helix-lsp crate. The approach leverages Helix's existing LSP infrastructure while adding rust-analyzer-specific functionality through a trait-based extension system.

## Architecture Approach

### Extension Pattern (Recommended)

We will implement rust-analyzer extensions in the existing `nucleotide-lsp` crate using a trait-based extension pattern:

```rust
// nucleotide-lsp/src/rust_analyzer/mod.rs
pub trait RustAnalyzerExt {
    async fn expand_macro(&self, position: Position) -> Result<String>;
    async fn view_syntax_tree(&self, doc: TextDocumentIdentifier) -> Result<String>;
    async fn run_test(&self, test_id: String) -> Result<TestResult>;
    // ... other rust-analyzer specific methods
}

impl RustAnalyzerExt for helix_lsp::Client {
    // Implement custom requests using the existing Client's call method
}
```

### Benefits of This Approach

1. **No Fork Required** - Uses Helix's existing LSP infrastructure
2. **Clean Separation** - Extensions are isolated and optional
3. **Maintainability** - Easy to update as Helix evolves
4. **Modularity** - Can conditionally enable features per server
5. **Extensibility** - Pattern can be applied to other LSP servers

## rust-analyzer Extensions Catalog

Based on analysis of rust-analyzer's LSP implementation, the following 30+ custom methods are available:

### 1. Code Intelligence & Visualization (High Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| `rust-analyzer/expandMacro` | Show macro expansions | Popup/Panel | **High** |
| `rust-analyzer/viewSyntaxTree` | View parse tree | Tree viewer | Medium |
| `rust-analyzer/viewHir` | View HIR representation | Structured viewer | Medium |
| `rust-analyzer/viewMir` | View MIR representation | Structured viewer | Low |
| `rust-analyzer/interpretFunction` | Const eval debugging | Debug panel | Low |
| `rust-analyzer/viewCrateGraph` | Dependency visualization | Graph widget | Medium |
| `rust-analyzer/viewItemTree` | Module structure view | Tree viewer | Medium |
| `rust-analyzer/viewRecursiveMemoryLayout` | Memory layout analysis | Structured viewer | Low |

### 2. Enhanced Testing (High Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| `rust-analyzer/runTest` | Direct test execution | Test runner panel | **High** |
| `rust-analyzer/discoverTest` | Test discovery | Test tree view | **High** |
| `rust-analyzer/relatedTests` | Find related tests | Quick picker | Medium |
| Test state notifications | Track test execution | Status indicators | Medium |

### 3. Advanced Editing (Medium Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| `rust-analyzer/ssr` | Structural Search & Replace | Search/Replace dialog | **High** |
| `rust-analyzer/moveItem` | Move items up/down | Keyboard shortcuts | Medium |
| `rust-analyzer/joinLines` | Smart line joining | Editor command | Medium |
| `rust-analyzer/matchingBrace` | Enhanced brace matching | Visual indicator | Low |
| `rust-analyzer/parentModule` | Navigate to parent | Navigation command | Medium |
| `rust-analyzer/childModules` | Find child modules | Quick picker | Medium |

### 4. Workspace Management (Low Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| `rust-analyzer/reloadWorkspace` | Force workspace reload | Command palette | Low |
| `rust-analyzer/rebuildProcMacros` | Rebuild proc macros | Command palette | Low |
| `rust-analyzer/fetchDependencyList` | List dependencies | Dependency viewer | Low |
| `rust-analyzer/openCargoToml` | Open Cargo.toml | File opener | Medium |

### 5. Diagnostics & Analysis (Medium Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| `rust-analyzer/analyzerStatus` | Server health info | Status panel | Medium |
| `rust-analyzer/memoryUsage` | Memory profiling | Debug info | Low |
| Flycheck controls | Diagnostic management | Status bar controls | Medium |
| Server status notifications | Health monitoring | Status indicators | Medium |

### 6. Enhanced Standard Features (Medium Priority)

| Method | Purpose | UI Component | Implementation Priority |
|--------|---------|--------------|------------------------|
| Enhanced hover with ranges | Better hover info | Hover popup | Medium |
| Code actions with group IDs | Organized actions | Action menu | Medium |
| Workspace symbols with filtering | Better symbol search | Symbol picker | Medium |
| External documentation links | Quick docs access | Browser integration | Medium |

## Implementation Architecture

### Directory Structure

```
nucleotide-lsp/
├── src/
│   ├── lib.rs
│   ├── rust_analyzer/
│   │   ├── mod.rs              # Main module
│   │   ├── protocol.rs         # LSP protocol types
│   │   ├── extensions.rs       # Extension trait
│   │   ├── handlers.rs         # Request handlers
│   │   └── ui_integration.rs   # UI component bridges
│   ├── helix_lsp_bridge.rs     # Existing bridge (extend)
│   └── ... (other existing files)
```

### Core Components

#### 1. Protocol Definitions

```rust
// nucleotide-lsp/src/rust_analyzer/protocol.rs
use lsp_types::request::Request;
use serde::{Deserialize, Serialize};

pub mod request {
    use super::*;

    pub enum ExpandMacro {}
    impl Request for ExpandMacro {
        type Params = ExpandMacroParams;
        type Result = ExpandMacroResult;
        const METHOD: &'static str = "rust-analyzer/expandMacro";
    }

    pub enum ViewSyntaxTree {}
    impl Request for ViewSyntaxTree {
        type Params = ViewSyntaxTreeParams;
        type Result = String;
        const METHOD: &'static str = "rust-analyzer/viewSyntaxTree";
    }

    pub enum RunTest {}
    impl Request for RunTest {
        type Params = RunTestParams;
        type Result = RunTestResult;
        const METHOD: &'static str = "rust-analyzer/runTest";
    }

    // ... other request types
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExpandMacroParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub position: lsp_types::Position,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ExpandMacroResult {
    pub name: String,
    pub expansion: String,
}

// ... other parameter/result types
```

#### 2. Extension Trait

```rust
// nucleotide-lsp/src/rust_analyzer/extensions.rs
use anyhow::Result;
use helix_lsp::Client;
use lsp_types::{Position, TextDocumentIdentifier};

use super::protocol::{request::*, *};

#[async_trait::async_trait]
pub trait RustAnalyzerExt {
    /// Expand macro at position
    async fn expand_macro(
        &self,
        doc: TextDocumentIdentifier,
        position: Position,
    ) -> Result<ExpandMacroResult>;

    /// View syntax tree for document
    async fn view_syntax_tree(&self, doc: TextDocumentIdentifier) -> Result<String>;

    /// Run a specific test
    async fn run_test(&self, test_id: String) -> Result<RunTestResult>;

    /// Discover tests in workspace
    async fn discover_tests(&self) -> Result<Vec<TestItem>>;

    /// Perform structural search and replace
    async fn ssr(&self, query: String, parse_only: bool) -> Result<SsrResult>;

    /// Get analyzer status
    async fn analyzer_status(&self, doc: Option<TextDocumentIdentifier>) -> Result<String>;

    /// Check if client is rust-analyzer
    fn is_rust_analyzer(&self) -> bool;
}

#[async_trait::async_trait]
impl RustAnalyzerExt for Client {
    async fn expand_macro(
        &self,
        doc: TextDocumentIdentifier,
        position: Position,
    ) -> Result<ExpandMacroResult> {
        if !self.is_rust_analyzer() {
            return Err(anyhow::anyhow!("Not a rust-analyzer client"));
        }

        let params = ExpandMacroParams {
            text_document: doc,
            position,
        };

        let result = self.request::<ExpandMacro>(params).await?;
        Ok(result)
    }

    async fn view_syntax_tree(&self, doc: TextDocumentIdentifier) -> Result<String> {
        if !self.is_rust_analyzer() {
            return Err(anyhow::anyhow!("Not a rust-analyzer client"));
        }

        let params = ViewSyntaxTreeParams {
            text_document: doc,
        };

        let result = self.request::<ViewSyntaxTree>(params).await?;
        Ok(result)
    }

    async fn run_test(&self, test_id: String) -> Result<RunTestResult> {
        if !self.is_rust_analyzer() {
            return Err(anyhow::anyhow!("Not a rust-analyzer client"));
        }

        let params = RunTestParams { test_id };
        let result = self.request::<RunTest>(params).await?;
        Ok(result)
    }

    // ... other implementations

    fn is_rust_analyzer(&self) -> bool {
        self.name().contains("rust-analyzer")
    }
}
```

#### 3. Integration with Existing Bridge

```rust
// Extension to existing nucleotide-lsp/src/helix_lsp_bridge.rs
use crate::rust_analyzer::RustAnalyzerExt;

impl HelixLspBridge {
    /// Get rust-analyzer extensions for a language server
    pub async fn get_rust_analyzer_ext(
        &self,
        editor: &Editor,
        server_id: LanguageServerId,
    ) -> Option<Arc<dyn RustAnalyzerExt>> {
        if let Some(client) = editor.language_servers.get_by_id(server_id) {
            if client.is_rust_analyzer() {
                return Some(client);
            }
        }
        None
    }

    /// Expand macro at cursor position
    pub async fn expand_macro_at_cursor(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
    ) -> Result<Option<ExpandMacroResult>, ProjectLspError> {
        // Get active document and cursor position
        let (view, doc) = current_ref!(editor);
        let text_doc = doc.identifier();
        let position = doc.selection(view.id).primary().cursor(doc.text().slice(..));

        // Find rust-analyzer server
        if let Some(server_id) = doc.language_servers().next() {
            if let Some(ext) = self.get_rust_analyzer_ext(editor, server_id).await {
                let result = ext.expand_macro(text_doc, position).await
                    .map_err(ProjectLspError::RequestFailed)?;
                return Ok(Some(result));
            }
        }

        Ok(None)
    }

    // ... other convenience methods
}
```

#### 4. UI Integration Points

```rust
// nucleotide-lsp/src/rust_analyzer/ui_integration.rs
use nucleotide_events::{CoreEvent, UiEvent};
use nucleotide_ui::{InfoBox, TreeView, TestRunner};

pub struct MacroExpansionViewer {
    expansion_text: String,
    macro_name: String,
}

impl MacroExpansionViewer {
    pub fn new(result: ExpandMacroResult) -> Self {
        Self {
            expansion_text: result.expansion,
            macro_name: result.name,
        }
    }

    pub fn render(&self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        InfoBox::new(cx)
            .title(format!("Macro: {}", self.macro_name))
            .content(&self.expansion_text)
            .syntax_highlight("rust")
    }
}

pub struct TestRunner {
    tests: Vec<TestItem>,
    running_tests: HashSet<String>,
    test_results: HashMap<String, TestResult>,
}

impl TestRunner {
    pub async fn discover_tests(&mut self, lsp_bridge: &HelixLspBridge) -> Result<()> {
        // Use lsp_bridge to discover tests
        // Update self.tests
        Ok(())
    }

    pub async fn run_test(&mut self, test_id: String, lsp_bridge: &HelixLspBridge) -> Result<()> {
        self.running_tests.insert(test_id.clone());
        // Use lsp_bridge to run test
        // Handle result and update self.test_results
        Ok(())
    }
}

// Event handlers for UI updates
pub fn handle_test_result_event(event: TestResultEvent) {
    // Update UI state
    // Send UI refresh events
}
```

## Integration with Nucleotide's Event System

### Event Types

```rust
// Add to nucleotide-events/src/lsp.rs
#[derive(Debug, Clone)]
pub enum RustAnalyzerEvent {
    MacroExpansionRequested {
        document_id: DocumentId,
        position: Position,
    },
    MacroExpansionResult {
        result: ExpandMacroResult,
    },
    TestDiscoveryRequested,
    TestDiscoveryResult {
        tests: Vec<TestItem>,
    },
    TestRunRequested {
        test_id: String,
    },
    TestRunResult {
        test_id: String,
        result: TestResult,
    },
    SyntaxTreeRequested {
        document_id: DocumentId,
    },
    SyntaxTreeResult {
        content: String,
    },
}
```

### Command Integration

```rust
// Add to nucleotide/src/actions.rs
pub fn expand_macro_at_cursor(cx: &mut WindowContext) {
    // Get current editor state
    // Send MacroExpansionRequested event
    // UI will handle the response
}

pub fn run_current_test(cx: &mut WindowContext) {
    // Discover test at cursor
    // Send TestRunRequested event
}

pub fn show_syntax_tree(cx: &mut WindowContext) {
    // Send SyntaxTreeRequested event
}
```

## Implementation Phases

### Phase 1: Foundation (Week 1-2)
- [ ] Set up rust-analyzer module structure
- [ ] Implement core protocol types
- [ ] Create extension trait skeleton
- [ ] Add basic macro expansion support
- [ ] Integrate with command system

### Phase 2: Core Features (Week 3-4)
- [ ] Implement test discovery and running
- [ ] Add structural search & replace
- [ ] Create UI components for results display
- [ ] Wire up event system integration

### Phase 3: Advanced Features (Week 5-6)
- [ ] Add syntax tree viewer
- [ ] Implement HIR/MIR viewers
- [ ] Add crate graph visualization
- [ ] Enhance navigation commands

### Phase 4: Polish & Integration (Week 7-8)
- [ ] Add workspace management commands
- [ ] Implement status monitoring
- [ ] Add comprehensive error handling
- [ ] Performance optimization
- [ ] Documentation and examples

## Testing Strategy

### Unit Tests

```rust
// nucleotide-lsp/src/rust_analyzer/tests.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_macro_expansion() {
        // Mock rust-analyzer client
        // Test expansion request/response
    }

    #[tokio::test]
    async fn test_test_discovery() {
        // Test test discovery functionality
    }

    #[test]
    fn test_protocol_serialization() {
        // Test LSP message serialization/deserialization
    }
}
```

### Integration Tests

```rust
// Test with real rust-analyzer server
#[tokio::test]
async fn test_real_rust_analyzer_integration() {
    // Start rust-analyzer server
    // Test actual LSP communication
    // Verify responses match expected format
}
```

### UI Tests

```rust
// Test UI components respond correctly to LSP events
#[gpui::test]
async fn test_macro_expansion_ui(cx: &mut TestAppContext) {
    // Simulate macro expansion event
    // Verify UI updates correctly
}
```

## Performance Considerations

### Caching Strategy
- Cache syntax tree results for unchanged documents
- Implement smart invalidation on document changes
- Use weak references to avoid memory leaks

### Async Handling
- All LSP requests are non-blocking
- Use timeout configurations for long-running requests
- Implement cancellation for user-interrupted operations

### Memory Management
- Stream large results (like crate graphs) instead of loading entirely
- Implement pagination for test results
- Use efficient data structures for tree representations

## Configuration

### User Configuration

```toml
# ~/.config/helix/nucleotide.toml
[lsp.rust-analyzer]
# Enable rust-analyzer extensions
enable_extensions = true

# Specific feature toggles
enable_macro_expansion = true
enable_test_runner = true
enable_syntax_tree = true
enable_ssr = true

# UI preferences
macro_expansion_popup_size = "large"
test_runner_panel_position = "bottom"
```

### Server Detection

```rust
// Automatic detection of rust-analyzer capabilities
impl Client {
    fn detect_rust_analyzer_capabilities(&self) -> RustAnalyzerCapabilities {
        // Query server capabilities
        // Return supported features
    }
}
```

## Error Handling

### Graceful Degradation
- If rust-analyzer extensions fail, fall back to standard LSP
- Show user-friendly error messages for unsupported features
- Retry mechanisms for transient failures

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum RustAnalyzerError {
    #[error("Server does not support extension: {extension}")]
    UnsupportedExtension { extension: String },
    
    #[error("LSP request failed: {message}")]
    RequestFailed { message: String },
    
    #[error("Invalid response format: {details}")]
    InvalidResponse { details: String },
    
    #[error("Feature not available in this context")]
    NotAvailable,
}
```

## Future Extensions

### Other Language Servers
The pattern established for rust-analyzer can be extended to other language servers:

- **gopls** - Go language server extensions
- **typescript-language-server** - TypeScript/JavaScript extensions  
- **pylsp** - Python language server extensions
- **clangd** - C/C++ language server extensions
- **taplo** - TOML language server extensions (see Taplo Extensions section below)

### Custom Extensions
Framework allows adding Nucleotide-specific LSP extensions:

- Project-wide refactoring tools
- Custom diagnostic analyzers
- Integration with external tools (clippy, cargo-audit, etc.)

## Taplo Extensions (TOML Language Server)

### Overview

Taplo is a TOML language server that implements several custom LSP extensions beyond the standard specification. These extensions provide enhanced TOML editing capabilities that Helix cannot currently utilize, presenting an opportunity for Nucleotide to offer superior TOML editing experience.

### Standard LSP Support

Taplo fully implements the standard LSP methods that Helix supports:

**Requests:**
- `initialize`, `folding_range`, `document_symbol`
- `formatting`, `completion`, `hover`
- `document_link`, `semantic_tokens_full`
- `prepare_rename`, `rename`

**Notifications:**
- `initialized`, `did_open_text_document`, `did_change_text_document`
- `did_save_text_document`, `did_close_text_document`
- `did_change_configuration`, `did_change_workspace_folders`

### Custom Extensions (Not Supported by Helix)

Taplo implements **7 custom LSP extensions** that Helix cannot use:

#### Custom Request Methods

| Method | Purpose | Parameters | Response | Priority |
|--------|---------|------------|----------|----------|
| `taplo/convertToJson` | Convert TOML to JSON | Input text string | JSON text or error | **High** |
| `taplo/convertToToml` | Convert JSON to TOML | Input text string | TOML text or error | **High** |
| `taplo/listSchemas` | List available schemas | Document URI | Vector of schema info | Medium |
| `taplo/associatedSchema` | Get document's schema | Document URI | Optional schema info | Medium |

#### Custom Notification Methods

| Method | Purpose | Parameters | Priority |
|--------|---------|------------|----------|
| `taplo/messageWithOutput` | Enhanced messaging | Message kind + string | Medium |
| `taplo/associateSchema` | Associate schema with document | Schema rules (glob/regex/URL) | **High** |
| `taplo/didChangeSchemaAssociation` | Schema association changes | Document URI + schema info | Medium |

### Implementation Strategy

Following the same extension pattern as rust-analyzer:

```rust
// nucleotide-lsp/src/taplo/mod.rs
pub mod protocol {
    use lsp_types::request::Request;
    use serde::{Deserialize, Serialize};

    pub enum ConvertToJson {}
    impl Request for ConvertToJson {
        type Params = ConvertToJsonParams;
        type Result = ConvertToJsonResult;
        const METHOD: &'static str = "taplo/convertToJson";
    }

    pub enum ConvertToToml {}
    impl Request for ConvertToToml {
        type Params = ConvertToTomlParams;
        type Result = ConvertToTomlResult;
        const METHOD: &'static str = "taplo/convertToToml";
    }

    pub enum ListSchemas {}
    impl Request for ListSchemas {
        type Params = ListSchemasParams;
        type Result = Vec<SchemaInfo>;
        const METHOD: &'static str = "taplo/listSchemas";
    }

    pub enum AssociatedSchema {}
    impl Request for AssociatedSchema {
        type Params = AssociatedSchemaParams;
        type Result = Option<SchemaInfo>;
        const METHOD: &'static str = "taplo/associatedSchema";
    }
}

#[async_trait::async_trait]
pub trait TaploExt {
    /// Convert TOML text to JSON
    async fn convert_to_json(&self, text: String) -> Result<String>;
    
    /// Convert JSON text to TOML
    async fn convert_to_toml(&self, text: String) -> Result<String>;
    
    /// List available schemas for document
    async fn list_schemas(&self, uri: Url) -> Result<Vec<SchemaInfo>>;
    
    /// Get schema associated with document
    async fn associated_schema(&self, uri: Url) -> Result<Option<SchemaInfo>>;
    
    /// Associate schema with document
    async fn associate_schema(&self, association: SchemaAssociation) -> Result<()>;
    
    /// Check if client is Taplo
    fn is_taplo(&self) -> bool;
}

#[async_trait::async_trait]
impl TaploExt for helix_lsp::Client {
    async fn convert_to_json(&self, text: String) -> Result<String> {
        if !self.is_taplo() {
            return Err(anyhow::anyhow!("Not a Taplo client"));
        }

        let params = ConvertToJsonParams { text };
        let result = self.request::<ConvertToJson>(params).await?;
        Ok(result.text)
    }

    // ... other implementations

    fn is_taplo(&self) -> bool {
        self.name().contains("taplo") || self.name().contains("toml")
    }
}
```

### UI Components

#### TOML/JSON Converter
```rust
pub struct FormatConverter {
    input: String,
    output: String,
    conversion_type: ConversionType,
}

impl FormatConverter {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            conversion_type: ConversionType::TomlToJson,
        }
    }

    pub async fn convert(&mut self, lsp_bridge: &HelixLspBridge) -> Result<()> {
        match self.conversion_type {
            ConversionType::TomlToJson => {
                self.output = lsp_bridge.convert_toml_to_json(&self.input).await?;
            }
            ConversionType::JsonToToml => {
                self.output = lsp_bridge.convert_json_to_toml(&self.input).await?;
            }
        }
        Ok(())
    }
}
```

#### Schema Association Panel
```rust
pub struct SchemaAssociationPanel {
    available_schemas: Vec<SchemaInfo>,
    current_associations: Vec<SchemaAssociation>,
    selected_document: Option<Url>,
}

impl SchemaAssociationPanel {
    pub async fn load_schemas(&mut self, lsp_bridge: &HelixLspBridge, uri: Url) -> Result<()> {
        self.available_schemas = lsp_bridge.list_schemas(uri).await?;
        self.current_associations = lsp_bridge.get_schema_associations().await?;
        Ok(())
    }

    pub async fn associate_schema(
        &mut self, 
        schema_info: SchemaInfo,
        pattern: String,
        lsp_bridge: &HelixLspBridge
    ) -> Result<()> {
        let association = SchemaAssociation {
            schema: schema_info,
            rule: AssociationRule::Glob(pattern),
            priority: None,
            meta: None,
        };
        
        lsp_bridge.associate_schema(association).await?;
        Ok(())
    }
}
```

### Command Integration

```rust
// Add to nucleotide/src/actions.rs

/// Convert TOML document to JSON
pub fn convert_toml_to_json(cx: &mut WindowContext) {
    let event = TaploEvent::ConvertToJsonRequested {
        document_id: get_current_document_id(cx),
    };
    cx.emit(event);
}

/// Convert JSON document to TOML  
pub fn convert_json_to_toml(cx: &mut WindowContext) {
    let event = TaploEvent::ConvertToTomlRequested {
        document_id: get_current_document_id(cx),
    };
    cx.emit(event);
}

/// Show schema association dialog
pub fn show_schema_associations(cx: &mut WindowContext) {
    let event = TaploEvent::ShowSchemaAssociations {
        document_id: get_current_document_id(cx),
    };
    cx.emit(event);
}
```

### Event Types

```rust
// Add to nucleotide-events/src/lsp.rs
#[derive(Debug, Clone)]
pub enum TaploEvent {
    ConvertToJsonRequested {
        document_id: DocumentId,
    },
    ConvertToJsonResult {
        result: Result<String, String>,
    },
    ConvertToTomlRequested {
        document_id: DocumentId,
    },
    ConvertToTomlResult {
        result: Result<String, String>,
    },
    ShowSchemaAssociations {
        document_id: DocumentId,
    },
    SchemasListed {
        schemas: Vec<SchemaInfo>,
    },
    SchemaAssociated {
        association: SchemaAssociation,
    },
}
```

### Benefits for Nucleotide Users

1. **Format Conversion** - Quick TOML ↔ JSON conversion for configuration files
2. **Schema Validation** - Enhanced TOML editing with schema-aware completion and validation
3. **Configuration Management** - Better support for complex TOML configuration files
4. **Developer Productivity** - Reduced context switching between tools

### Implementation Priority

**Phase 1 (High Priority):**
- Format conversion commands (TOML ↔ JSON)
- Basic schema association

**Phase 2 (Medium Priority):**
- Schema listing and browsing
- Enhanced schema validation UI
- Schema association management panel

**Phase 3 (Low Priority):**
- Advanced schema rule patterns
- Schema validation error highlighting
- Integration with external schema repositories

This extension would make Nucleotide particularly valuable for projects with complex TOML configurations like Rust projects (Cargo.toml), Python projects (pyproject.toml), and configuration-heavy applications.

## Conclusion

This implementation plan provides a comprehensive approach to adding rust-analyzer's extended LSP capabilities to Nucleotide while maintaining clean architecture and compatibility with Helix's existing LSP infrastructure. The extension pattern allows for modular development and easy maintenance as both Helix and rust-analyzer evolve.

The phased implementation approach ensures that high-value features like macro expansion and test running are delivered early, while more specialized features can be added in later phases based on user feedback and demand.