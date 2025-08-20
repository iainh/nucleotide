# Nucleotide LSP Startup Flow Guide

This guide provides a comprehensive overview of how Language Server Protocol (LSP) servers are initialized and managed in Nucleotide from application startup to server activation. Understanding this flow is essential for developers working on LSP functionality, debugging server startup issues, or extending the LSP system.

## Table of Contents

1. [Overview and Key Concepts](#overview-and-key-concepts)
2. [Startup Flow Phases](#startup-flow-phases)
3. [Component Architecture](#component-architecture)
4. [Decision Points and Branching Logic](#decision-points-and-branching-logic)
5. [Component Interactions](#component-interactions)
6. [Recent Fixes and Improvements](#recent-fixes-and-improvements)
7. [Visual Flow Diagrams](#visual-flow-diagrams)
8. [Troubleshooting Common Issues](#troubleshooting-common-issues)

## Overview and Key Concepts

### Core Philosophy

Nucleotide implements a **dual-mode LSP system** that supports both:
- **Project-based startup**: Proactive LSP server initialization when a project is detected
- **File-based startup**: Traditional LSP startup when files are opened (Helix's default behavior)

### Key Components

- **`LspManager`** (`src/lsp_manager.rs`): High-level LSP coordination with feature flag support
- **`ProjectLspManager`** (`nucleotide-lsp/src/project_lsp_manager.rs`): Project-aware LSP management
- **`HelixLspBridge`** (`nucleotide-lsp/src/helix_lsp_bridge.rs`): Integration with Helix's LSP registry
- **`ProjectStatusService`** (`src/project_status_service.rs`): Project detection and status management
- **Event System**: Structured event flow for LSP lifecycle management

### Configuration-Driven Behavior

LSP startup behavior is controlled by configuration flags:
- `project_lsp_startup`: Enable/disable project-based LSP startup
- `enable_fallback`: Allow fallback to file-based startup on project startup failure
- `startup_timeout_ms`: Timeout for LSP server initialization
- `project_markers`: Custom project detection configuration

## Startup Flow Phases

### Phase 1: Application Initialization

**Location**: `src/main.rs` → `init_editor()` → `application::init_editor()`

```
main() → init_editor() → determine_workspace_root() → application::init_editor()
```

**Key Activities:**
1. **Workspace Root Detection**: Determine project directory from CLI arguments
   - Priority: `--working-dir` → directory arg → first file parent → git repo root → current dir
2. **Working Directory Setup**: Set working directory before Editor creation
3. **Helix Editor Creation**: Initialize core Helix components
4. **LSP Infrastructure Setup**: 
   - Create completion channels
   - Register event hooks
   - Initialize event bridges (Helix ↔ GPUI)

**Critical Code Path:**
```rust
// main.rs line ~1171
let workspace_root = determine_workspace_root(&args)?;
if let Some(root) = &workspace_root {
    helix_stdx::env::set_current_working_dir(root)?;
}

// application.rs line ~2375
let lsp_manager = crate::lsp_manager::LspManager::new(Arc::new(gui_config.clone()));
```

### Phase 2: GUI Initialization

**Location**: `src/main.rs` → `gui_main()`

**Key Activities:**
1. **GPUI Application Setup**: Create GPUI app with UI framework
2. **Global Service Registration**: Set up theme, font, and configuration globals
3. **Project Status Service**: Initialize project detection service
4. **LSP State Entity**: Create reactive LSP state for UI updates

**Critical Code Path:**
```rust
// main.rs line ~714
let lsp_state = cx.new(|_| nucleotide_lsp::LspState::new());
```

### Phase 3: Application Entity Creation

**Location**: `src/main.rs` → window creation callback

**Key Activities:**
1. **Application Entity**: Wrap Helix Editor in GPUI-managed Application
2. **Event Subscription**: Set up input and timer event handlers
3. **LSP State Binding**: Link LSP state to Application
4. **Completion System**: Initialize completion coordination

### Phase 4: Workspace Creation

**Location**: `src/workspace.rs` → `Workspace::with_views()`

**Key Activities:**
1. **ProjectLspManager Initialization**: Create project-level LSP management
2. **Project Root Assignment**: Set current project root
3. **Event Subscriptions**: Set up workspace-level event handling

**Critical Decision Point:**
```rust
// workspace.rs line ~199
let project_lsp_manager = if let Some(ref root) = root_path_for_manager {
    // Initialize ProjectLspManager for proactive startup
    info!(project_root = %root.display(), "Initializing ProjectLspManager");
    // ... configuration and creation
} else { None };
```

### Phase 5: LSP Server Activation

**Trigger Points:**
- **File Opening**: Document opened via UI or CLI arguments
- **Project Detection**: ProjectLspManager detects project and starts servers proactively
- **Feature Flag Changes**: Runtime configuration updates

**Flow Branches:**

#### A. Project-Based Startup (when `project_lsp_startup = true`)
1. **Project Detection**: `ProjectDetector` analyzes workspace root
2. **Server Requirement Analysis**: Determine needed language servers
3. **Proactive Startup**: Start servers before files are opened
4. **Health Monitoring**: Background health checks

#### B. File-Based Startup (traditional)
1. **Document Opening**: User opens a file
2. **Language Detection**: Helix detects file language
3. **Server Startup**: LSP server started for specific language
4. **Registration**: Server registered with document

#### C. Fallback Mechanism
1. **Primary Failure**: Project-based startup fails
2. **Fallback Decision**: Check `enable_fallback` configuration
3. **Fallback Execution**: Switch to file-based startup
4. **Error Recovery**: Log failure and continue with fallback

## Component Architecture

### LspManager (High-Level Coordinator)

**Responsibilities:**
- Feature flag evaluation
- Startup mode determination (project vs file-based)
- Fallback mechanism coordination
- Configuration hot-reloading

**Key Methods:**
- `determine_startup_mode()`: Choose startup approach
- `start_lsp_for_document()`: Document-triggered LSP startup
- `update_config()`: Runtime configuration updates

### ProjectLspManager (Project-Level Management)

**Responsibilities:**
- Project detection and analysis
- Proactive server startup
- Server lifecycle management
- Health monitoring

**Key Components:**
- `ProjectDetector`: Project type analysis
- `ServerLifecycleManager`: Server start/stop operations
- Background health check tasks
- Event processing system

### HelixLspBridge (Integration Layer)

**Responsibilities:**
- Bridge between project management and Helix's LSP registry
- Server startup through Helix's existing infrastructure
- Document-server associations

**Integration Points:**
- `helix_view::Editor` direct interaction
- `helix_lsp::Registry` server management
- Event propagation to ProjectLspManager

## Decision Points and Branching Logic

### Primary Decision Tree

```
Document Opening
├── Check: project_lsp_startup enabled?
│   ├── YES: Project-based startup
│   │   ├── Check: Project detected?
│   │   │   ├── YES: Use project servers
│   │   │   └── NO: Check fallback enabled?
│   │   │       ├── YES: Fall back to file-based
│   │   │       └── NO: Proceed with file-based anyway
│   │   └── Startup failure?
│   │       ├── Check: enable_fallback?
│   │       │   ├── YES: Attempt file-based startup
│   │       │   └── NO: Report failure
│   │       └── Continue
│   └── NO: File-based startup (traditional Helix behavior)
```

### Configuration-Based Branching

**Feature Flags Impact:**
- `project_lsp_startup = false`: Skip all project-based logic
- `enable_fallback = false`: No fallback on project startup failure
- `startup_timeout_ms`: Controls server initialization timeout
- Custom project markers enable alternative project detection

### Runtime Decision Points

1. **Project Detection**: Can we identify a project structure?
2. **Server Availability**: Are required language servers configured?
3. **Timeout Handling**: Has server startup exceeded timeout?
4. **Fallback Viability**: Should we attempt fallback startup?

## Component Interactions

### Startup Sequence Interactions

```
main.rs
├── Determines workspace root
├── Creates Application (with LspManager)
└── Creates Workspace
    ├── Initializes ProjectLspManager (if project detected)
    ├── Links to Application's LspManager
    └── Sets up event handling

File Opening Event
├── Workspace receives OpenFile event
├── Delegates to Application.handle_document_with_project_lsp()
├── LspManager.start_lsp_for_document()
│   ├── Determines startup mode
│   ├── Attempts project-based or file-based startup
│   └── Handles fallback if needed
└── Updates UI state via LspState entity
```

### Event Flow

**Reactive Event System:**
- **Document events**: File opening, closing, modification
- **LSP events**: Server initialization, progress, health status
- **Project events**: Project detection, server startup requests
- **UI events**: Status updates, progress indicators

**Key Event Channels:**
- `completion_rx`: Helix completion events
- `event_bridge_rx`: Helix → GPUI event forwarding
- `project_event_tx`: Project-level LSP coordination

### Data Flow

```
Configuration → LspManager → Startup Decision
                      ↓
          ProjectLspManager ← Project Detection
                      ↓
          HelixLspBridge → helix_lsp::Registry
                      ↓
          LSP Servers ← Network/Process Communication
                      ↓
          UI Updates ← LspState Entity
```

## Recent Fixes and Improvements

### Critical Startup Fixes

1. **Working Directory Timing**: Fixed race condition where working directory wasn't set before Editor creation
2. **Event Hook Registration**: Ensured proper LSP event hook registration before server startup
3. **Completion Channel Setup**: Fixed completion system initialization order
4. **Theme Loading**: Resolved theme loading issues that affected LSP indicators

### Feature Flag Integration

- **Runtime Configuration**: Hot-reloading of LSP configuration without restart
- **Graceful Fallback**: Improved fallback mechanism with proper error handling
- **Startup Mode Determination**: Clear logic for choosing startup approach

### Project Detection Enhancements

- **Custom Project Markers**: Configurable project detection rules
- **Priority-based Selection**: Handle multiple project markers with priorities
- **Builtin Fallback**: Fall back to standard project detection when custom fails

## Visual Flow Diagrams

### Overall Startup Flow

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│  Application    │    │   Workspace      │    │  ProjectLsp     │
│  Startup        │    │   Creation       │    │  Manager        │
│                 │    │                  │    │                 │
├─ Workspace Root │    ├─ Project Root   │    ├─ Project        │
│  Detection      │───▶│  Assignment      │───▶│  Detection      │
│                 │    │                  │    │                 │
├─ Editor         │    ├─ Event          │    ├─ Server         │
│  Initialization │    │  Subscriptions   │    │  Startup        │
│                 │    │                  │    │                 │
└─ LSP Manager   │    └─ UI Integration  │    └─ Health        │
  Creation        │                      │      Monitoring     │
└─────────────────┘    └──────────────────┘    └─────────────────┘
```

### LSP Activation Decision Tree

```
Document Opened
       │
       ▼
┌─────────────────┐
│ project_lsp_    │  NO
│ startup         │────┐
│ enabled?        │    │
└─────────────────┘    │
       │ YES            │
       ▼                │
┌─────────────────┐    │
│ Project         │    │
│ detected?       │    │
└─────────────────┘    │
    │ YES       │ NO    │
    ▼           ▼       │
┌─────────┐ ┌─────────┐ │
│Project  │ │Fallback │ │
│Startup  │ │enabled? │ │
└─────────┘ └─────────┘ │
    │         │ YES      │
    │         ▼          │
    │    ┌─────────┐    │
    │    │File     │    │
    │    │Startup  │    │
    │    └─────────┘    │
    │         │         │
    ▼         ▼         ▼
┌───────────────────────┐
│   Final LSP Startup   │
└───────────────────────┘
```

### Component Interaction Flow

```
┌────────────┐  config  ┌─────────────┐
│LspManager  │◄────────►│ProjectLsp   │
│            │  events  │Manager      │
└──────┬─────┘          └──────┬──────┘
       │                       │
       │ delegates              │ coordinates
       ▼                       ▼
┌────────────┐          ┌─────────────┐
│Helix       │          │HelixLsp     │
│Editor      │◄────────►│Bridge       │
│            │ registry │             │
└────────────┘          └─────────────┘
```

## Troubleshooting Common Issues

### Server Not Starting

**Symptoms**: LSP features not working, no server indicators in status bar

**Check List**:
1. **Configuration**: Verify `project_lsp_startup` setting
2. **Project Detection**: Check if project root is correctly identified
3. **Server Configuration**: Ensure language server is installed and configured in Helix
4. **Working Directory**: Verify working directory is set correctly
5. **Feature Flags**: Check if fallback is enabled for project startup failures

**Debug Commands**:
```rust
// Check project detection
let project_root = workspace.get_project_root();

// Verify LSP manager state  
let startup_stats = app.lsp_manager.get_startup_stats();

// Check active servers
let active_servers = editor.language_servers.iter_clients().collect();
```

### Project Not Detected

**Symptoms**: File-based startup used instead of project-based

**Common Causes**:
1. **Working Directory**: Wrong working directory set at startup
2. **VCS Markers**: No `.git`, `.svn`, etc. directories found
3. **Custom Markers**: Custom project markers not configured correctly
4. **Permission Issues**: Cannot read project directory

**Solutions**:
- Use `--working-dir` CLI argument to specify project directory
- Ensure VCS directories are present and readable
- Configure custom project markers in configuration
- Check directory permissions

### Fallback Not Working

**Symptoms**: LSP startup fails completely instead of falling back

**Check List**:
1. **Fallback Configuration**: Ensure `enable_fallback = true`
2. **Error Handling**: Check logs for fallback failure reasons
3. **Server Availability**: Verify language servers are available for fallback
4. **Timeout Issues**: Check if primary startup is timing out correctly

### Performance Issues

**Symptoms**: Slow startup, high resource usage

**Optimization Areas**:
1. **Proactive Startup**: Disable if not needed (`project_lsp_startup = false`)
2. **Health Check Interval**: Increase interval for less frequent checks
3. **Concurrent Startup**: Adjust `max_concurrent_startups` setting
4. **Server Selection**: Optimize project detection to avoid unnecessary servers

### Configuration Hot-Reloading Issues

**Symptoms**: Configuration changes not taking effect

**Troubleshooting**:
1. **Configuration Validation**: Check for validation errors in logs
2. **Update Propagation**: Verify `update_lsp_manager_config()` is called
3. **State Consistency**: Ensure all components receive configuration updates
4. **Restart Requirements**: Some changes may require application restart

### Common Log Patterns

**Successful Startup**:
```
INFO Initializing ProjectLspManager for proactive LSP startup
INFO Project detected and registered project_type=Rust language_servers=["rust-analyzer"]
INFO Language server started successfully server_name=rust-analyzer
```

**Fallback Activation**:
```
WARN Project-based LSP startup failed error=...
INFO Attempting fallback to file-based LSP startup  
INFO Fallback to file-based LSP startup successful
```

**Configuration Issues**:
```
ERROR Invalid LSP configuration provided for update validation_error=...
WARN LSP configuration changed - updating manager
```

### Best Practices

1. **Always Check Logs**: Enable debug logging for LSP troubleshooting
2. **Verify Configuration**: Use configuration validation before applying changes  
3. **Test Fallback**: Ensure fallback works by temporarily disabling project detection
4. **Monitor Performance**: Watch for excessive server startup attempts
5. **Use Feature Flags**: Disable unused features to improve performance

### Emergency Procedures

**Complete LSP Failure**: Set `project_lsp_startup = false` to force file-based startup
**Performance Issues**: Increase timeouts and reduce concurrent startup limit
**Configuration Corruption**: Delete configuration files to reset to defaults
**Server Crashes**: Check server logs and restart with clean state

This guide provides the foundation for understanding and working with Nucleotide's LSP system. For specific implementation details, refer to the source files mentioned throughout this document.