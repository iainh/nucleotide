# Nucleotide Layered Architecture

## Overview
This document defines the canonical layer structure for the Nucleotide codebase refactoring. Each layer has clear dependencies and responsibilities.

## Layer Hierarchy (Bottom to Top)

### Layer 1: nucleotide-types
**Purpose**: Pure data types with no cross-crate dependencies
**Dependencies**: None (only external deps like serde, etc.)
**Contents**:
- Font configurations (FontSettings, UiFontConfig, EditorFontConfig)
- Editor status types (EditorStatus)
- Completion triggers (CompletionTrigger)
- Basic UI types without behavior
- Diagnostic severity types

### Layer 2: nucleotide-events  
**Purpose**: Cross-crate event definitions
**Dependencies**: nucleotide-types, helix crates
**Contents**:
- CoreEvent, UiEvent, WorkspaceEvent, LspEvent
- AppEvent aggregator
- EventBus and EventHandler traits
- Message severity enums

### Layer 3: nucleotide-core
**Purpose**: Core editor abstractions and bridges
**Dependencies**: nucleotide-types, nucleotide-events, helix crates
**Contents**:
- Capability traits (EditorReadAccess, EditorWriteAccess, etc.)
- Event bridges (event_bridge.rs, gpui_to_helix_bridge.rs)
- Command system
- Document manager abstractions
- Preview tracker

### Layer 4: nucleotide-editor
**Purpose**: Text editing and rendering logic
**Dependencies**: nucleotide-types, nucleotide-events, nucleotide-core, helix crates, gpui
**Contents**:
- Line cache
- Scroll manager
- Document renderer
- Editor view components
- Text layout and rendering

### Layer 5: nucleotide-ui
**Purpose**: UI components and styling
**Dependencies**: nucleotide-types, nucleotide-events, nucleotide-core, gpui
**Contents**:
- Picker components
- Prompt components  
- Theme management
- UI utilities (buttons, lists, etc.)
- Completion UI
- Info boxes and notifications

### Layer 6: nucleotide-workspace
**Purpose**: Workspace and layout management
**Dependencies**: nucleotide-types, nucleotide-events, nucleotide-core, nucleotide-editor, nucleotide-ui, gpui
**Contents**:
- Workspace manager
- Tab management
- Layout management
- Panel coordination

### Layer 7: nucleotide-lsp
**Purpose**: Language Server Protocol integration
**Dependencies**: nucleotide-types, nucleotide-events, nucleotide-core, helix-lsp
**Contents**:
- LSP manager
- LSP state management
- Document synchronization
- LSP status tracking

### Layer 8: nucleotide (main application)
**Purpose**: Application entry point and integration
**Dependencies**: All other nucleotide crates, gpui
**Contents**:
- Application main loop
- GPUI integration
- Configuration loading
- File tree implementation
- Overlay management
- Status line
- Main entry point

## Dependency Rules

1. **Unidirectional Dependencies**: Lower layers cannot depend on higher layers
2. **Minimal Cross-Layer Dependencies**: Each layer should minimize dependencies on others
3. **No Circular Dependencies**: Strict hierarchy must be maintained
4. **External Dependencies**: Only specified external crates allowed per layer

## Current Status (Phase 2.5 Complete)

- ✅ nucleotide-types: Pure type definitions with optional feature flags for external deps
- ✅ nucleotide-events: Event system definitions
- ✅ nucleotide-core: Event bridges and capabilities without circular deps
- ✅ nucleotide-editor: Text rendering logic (no longer depends on nucleotide-ui)
- ✅ nucleotide-ui: UI components
- ✅ nucleotide-workspace: Workspace management logic
- ✅ nucleotide-lsp: LSP integration
- ✅ nucleotide: Main application glue

## Achievements

- ✅ Unidirectional dependency graph maintained
- ✅ nucleotide-editor → nucleotide-ui dependency removed
- ✅ nucleotide-types compiles with zero heavy dependencies (gpui/helix optional)
- ✅ All crates use standardized workspace metadata
- ✅ Clean layered architecture established