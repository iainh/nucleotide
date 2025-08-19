# Global Input System View Hierarchy Refactoring Plan

## Executive Summary

This plan outlines a comprehensive refactoring of Nucleotide's view hierarchy to properly integrate with the global input system. The current implementation has focus routing issues where key events don't reach the global input dispatcher due to components capturing focus independently. We'll redesign the architecture to use a unified input routing system that maintains compatibility with GPUI's native patterns while enabling global shortcuts and navigation.

## Current State Analysis

### Problems Identified:
1. **Fragmented Focus Management**: Components (Workspace, DocumentView, FileTree) each manage focus independently
2. **Key Event Interception**: DocumentElement captures focus but doesn't forward global shortcuts
3. **Inconsistent Input Routing**: Global shortcuts work only when specific components have focus
4. **Architectural Mismatch**: Custom global input system conflicts with GPUI's native action system

### Current Architecture:
```
Application
├── Workspace (has focus management + key handlers)
│   ├── DocumentView (delegates to DocumentElement)
│   │   └── DocumentElement (track_focus, no key handlers)
│   ├── FileTree (has own focus + key handlers)
│   └── GlobalInputDispatcher (isolated, not integrated)
```

## Target Architecture

### Design Principles:
1. **Unified Input Flow**: Single entry point for all key events
2. **Hierarchical Processing**: Global → Context → Component level handling
3. **GPUI Native Integration**: Leverage GPUI actions and focus system
4. **Component Independence**: Components don't manage global concerns
5. **Extensibility**: Easy to add new global shortcuts and navigation

### New Architecture:
```
Application
├── InputCoordinator (GPUI action handlers + global routing)
├── Workspace (delegates input to coordinator)
│   ├── DocumentView (pure rendering, no input handling)
│   ├── FileTree (component-specific input only)
│   └── UI Components (pure rendering)
```

## Detailed Implementation Plan

### Phase 1: Input Coordinator Foundation
Create a centralized input coordination system that works with GPUI's native patterns.

#### Step 1.1: Create InputCoordinator Structure
- Design `InputCoordinator` as the single source of truth for input routing
- Integrate with GPUI's global action system
- Create action definitions for all global shortcuts
- Implement hierarchical input processing (Global → Context → Component)

#### Step 1.2: Define Input Contexts and Priorities
- Create `InputContext` enum (Normal, Completion, FileTree, Picker, etc.)
- Implement context switching logic
- Define priority system for context resolution
- Create context-aware shortcut registration

#### Step 1.3: GPUI Action Integration
- Convert all global shortcuts to GPUI actions
- Register actions with proper contexts
- Implement action handlers in InputCoordinator
- Ensure compatibility with existing menu system

### Phase 2: View Hierarchy Restructuring

#### Step 2.1: Workspace Input Delegation
- Remove direct key handling from Workspace
- Implement input delegation to InputCoordinator
- Maintain focus restoration logic for UI state
- Ensure workspace actions (file tree toggle, etc.) work properly

#### Step 2.2: Document View Input Cleanup
- Remove focus tracking from DocumentElement
- Delegate editor input to Helix core through InputCoordinator
- Implement proper editor context switching
- Maintain scroll and selection behavior

#### Step 2.3: Component Input Standardization
- Standardize FileTree input handling
- Remove redundant focus management from components
- Implement component-specific input contexts
- Ensure component actions integrate with global system

### Phase 3: Global Navigation System

#### Step 3.1: Focus Group Management
- Implement focus groups (Editor, FileTree, Overlays, etc.)
- Create focus navigation between groups (Tab, Shift+Tab)
- Implement directional navigation within groups
- Add visual focus indicators

#### Step 3.2: Context-Aware Shortcuts
- Implement context-sensitive shortcut behavior
- Add escape key handling for modal contexts
- Create shortcut conflict resolution
- Implement customizable keybindings

#### Step 3.3: Advanced Navigation Features
- Add quick navigation (Ctrl+1, Ctrl+2 for different areas)
- Implement search and picker integration
- Add accessibility improvements (screen reader support)
- Create keyboard-only navigation modes

### Phase 4: Integration and Polish

#### Step 4.1: Component Integration
- Wire all components to use InputCoordinator
- Test focus restoration across all scenarios
- Ensure no input handling gaps
- Validate component interaction flows

#### Step 4.2: Testing and Validation
- Test global shortcuts from all contexts
- Validate modal behavior (completion, pickers)
- Test focus restoration on window activation
- Ensure editor functionality remains intact

#### Step 4.3: Performance and Cleanup
- Remove redundant global input system code
- Optimize input routing performance
- Clean up debug logging
- Document new architecture patterns

## Implementation Breakdown

### Chunk 1: InputCoordinator Foundation (Steps 1.1-1.3)
**Size**: Medium - Core architecture changes but well-defined scope
**Risk**: Medium - New central component but clear interfaces
**Dependencies**: None
**Outcome**: Centralized input system with GPUI integration

### Chunk 2: Workspace Refactoring (Step 2.1)  
**Size**: Small - Single component changes
**Risk**: Low - Well-understood component with clear delegation pattern
**Dependencies**: Chunk 1
**Outcome**: Workspace delegates to InputCoordinator

### Chunk 3: Document View Cleanup (Step 2.2)
**Size**: Small-Medium - Input handling changes in document system
**Risk**: Medium - Core editor functionality, needs careful testing
**Dependencies**: Chunks 1-2
**Outcome**: Clean document input flow through coordinator

### Chunk 4: Component Standardization (Step 2.3)
**Size**: Small - Component-by-component updates
**Risk**: Low - Isolated changes to individual components
**Dependencies**: Chunks 1-3
**Outcome**: All components use standard input patterns

### Chunk 5: Focus Groups (Step 3.1)
**Size**: Medium - New navigation system
**Risk**: Low-Medium - Well-defined scope, clear UX patterns
**Dependencies**: Chunks 1-4
**Outcome**: Tab-based focus group navigation

### Chunk 6: Advanced Shortcuts (Steps 3.2-3.3)
**Size**: Large - Multiple advanced features
**Risk**: Low - Building on established foundation
**Dependencies**: Chunks 1-5
**Outcome**: Full-featured global navigation system

### Chunk 7: Integration & Testing (Steps 4.1-4.3)
**Size**: Medium - Integration work and cleanup
**Risk**: Low - Validation and polish work
**Dependencies**: All previous chunks
**Outcome**: Production-ready refactored system

## Implementation Prompts

Each section below contains a specific prompt for implementing the corresponding chunk. These prompts build on each other and should be executed in sequence.

---

## Prompt 1: InputCoordinator Foundation

```
Create a new InputCoordinator system that will serve as the central hub for all keyboard input routing in Nucleotide. This replaces the fragmented focus management approach with a unified system.

Requirements:
1. Create `crates/nucleotide/src/input_coordinator.rs` that defines:
   - `InputCoordinator` struct with GPUI action integration
   - `InputContext` enum (Normal, Completion, FileTree, Picker, Modal)
   - `ContextPriority` system for handling overlapping contexts
   - Action handler registration system

2. Integrate with GPUI's action system:
   - Define actions for all global shortcuts (ToggleFileTree, ShowFileFinder, etc.)
   - Use `gpui::actions!` macro to declare actions
   - Implement action handlers that work with InputContext

3. Design the coordinator to handle:
   - Global shortcuts that work from any context
   - Context-specific shortcuts (e.g., completion navigation)
   - Priority-based context resolution
   - Escape key handling for modal dismissal

4. Create the foundation for focus group management:
   - Define `FocusGroup` enum (Editor, FileTree, Overlays, StatusBar)
   - Basic focus group switching logic
   - Integration hooks for components to register with groups

5. Integration points:
   - Method to handle GPUI KeyDownEvents
   - Context switching API for components
   - Action handler registration for workspace operations
   - Focus group navigation (Tab/Shift+Tab between groups)

Design this as a clean, well-documented foundation that other components can integrate with. Use Rust best practices and ensure thread safety. The coordinator should be created once and shared across the application.

Focus on creating the core architecture - don't implement all shortcuts yet, but create the framework that makes adding them straightforward.
```

---

## Prompt 2: Workspace Input Delegation

```
Refactor the Workspace component to delegate all input handling to the InputCoordinator created in Prompt 1. Remove the existing fragmented key handling and focus management.

Requirements:
1. Modify `crates/nucleotide/src/workspace.rs`:
   - Remove the existing `handle_key`, `handle_global_shortcuts_only`, and `handle_global_input_shortcuts` methods
   - Remove the `GlobalInputDispatcher` field and related code
   - Keep focus restoration logic for UI state management

2. Integrate with InputCoordinator:
   - Add `input_coordinator: Arc<InputCoordinator>` field to Workspace
   - Modify the constructor to accept and store the coordinator
   - Register workspace-specific action handlers with the coordinator

3. Simplify the render method:
   - Remove the complex conditional key handler attachment
   - Use a single, simple key handler that delegates to InputCoordinator
   - Keep track_focus for UI focus state but don't use it for input routing
   - Remove the debug logging added during troubleshooting

4. Update workspace actions:
   - Register `ToggleFileTree`, `ShowFileFinder`, `ShowCommandPalette` actions with coordinator
   - Implement workspace-specific methods called by action handlers
   - Ensure file tree toggle, picker opening, etc. work through the coordinator

5. Context management:
   - Call coordinator methods to switch contexts when overlays appear/disappear
   - Set context to "Completion" when completion is active
   - Set context to "Picker" when file finder or command palette is open

6. Update constructor calls:
   - Modify main.rs to create InputCoordinator and pass it to Workspace
   - Ensure coordinator is properly initialized before workspace creation

The result should be a much cleaner Workspace that focuses on UI layout and state management, with all input concerns delegated to the InputCoordinator. The existing functionality (file tree toggle, etc.) should continue to work, but through the new centralized system.
```

---

## Prompt 3: Document View Input Cleanup

```
Refactor the DocumentView and DocumentElement to work with the InputCoordinator system. Remove focus tracking from DocumentElement and ensure editor input flows properly through the coordinator while maintaining Helix editor functionality.

Requirements:
1. Modify `crates/nucleotide/src/document.rs`:
   - Remove focus tracking from DocumentElement (remove `track_focus` call)
   - Keep the focus handle for UI state but don't use it for input routing
   - Ensure DocumentElement remains purely a rendering component

2. Editor input integration:
   - Create a bridge between InputCoordinator and Helix editor input
   - Register editor-specific actions (if needed) with the coordinator
   - Ensure editor context is properly set when a document has focus

3. Context switching:
   - Implement methods to notify coordinator when document gets/loses focus
   - Set input context to "Editor" when document is active
   - Handle completion context switching when autocomplete appears

4. Maintain editor functionality:
   - Ensure all Helix key bindings continue to work
   - Preserve scroll, selection, and cursor behavior
   - Keep interaction with the underlying Helix editor core intact

5. Focus restoration:
   - Update focus restoration logic in workspace to work with new system
   - Ensure documents can still receive focus for UI purposes
   - Maintain visual focus indicators

6. Integration with coordinator:
   - Call coordinator methods to handle editor-specific input contexts
   - Ensure global shortcuts (Ctrl+B, Ctrl+S, etc.) work when editor is focused
   - Pass through editor-specific keys to Helix core

The goal is to clean up the input handling while preserving all editor functionality. The DocumentView should become a pure rendering component that coordinates with the InputCoordinator for input routing, while maintaining its integration with Helix editor core for actual text editing.
```

---

## Prompt 4: Component Input Standardization

```
Standardize input handling across all remaining components (FileTree, Pickers, Completion, etc.) to use the InputCoordinator system. Remove redundant focus management and ensure consistent input behavior.

Requirements:
1. FileTree input standardization (`crates/nucleotide/src/file_tree/view.rs`):
   - Remove existing key handler if present
   - Register FileTree-specific actions with InputCoordinator
   - Implement context switching when FileTree gains/loses focus
   - Ensure tree navigation (up/down/expand/collapse) works through coordinator

2. Completion system integration:
   - Update completion components to use InputCoordinator contexts
   - Register completion navigation actions (up/down/accept/dismiss)
   - Implement proper context switching for completion state
   - Ensure Escape key dismisses completion through coordinator

3. Picker components (file finder, command palette):
   - Standardize picker input handling through InputCoordinator
   - Register picker navigation and filtering actions
   - Implement modal context management for pickers
   - Ensure consistent Escape key behavior across all pickers

4. Remove redundant systems:
   - Remove any remaining direct key handlers in components
   - Clean up old focus management code that conflicts with coordinator
   - Remove debug logging related to the old input system

5. Context integration:
   - Each component should notify coordinator when it becomes active
   - Implement proper context priority for overlapping components
   - Ensure context switches cleanly between components

6. Action registration:
   - Create component-specific action enums
   - Register actions with appropriate contexts in coordinator
   - Implement action handlers that call into component methods

7. Testing and validation:
   - Ensure all component-specific keyboard shortcuts still work
   - Verify that global shortcuts work from any component context
   - Test context switching between different UI areas

The result should be a consistent input handling pattern across all components, where each component registers its specific actions with the InputCoordinator and handles context switching properly. No component should have its own independent input handling system.
```

---

## Prompt 5: Focus Groups and Tab Navigation

```
Implement the focus group navigation system that allows Tab/Shift+Tab navigation between major UI areas (Editor, FileTree, etc.) and provides visual focus indicators.

Requirements:
1. Complete focus group implementation in InputCoordinator:
   - Implement `FocusGroup` enum: Editor, FileTree, StatusBar, Overlays
   - Add focus group navigation methods (next_group, previous_group)
   - Implement Tab/Shift+Tab global shortcuts for group navigation
   - Add visual focus group indicators

2. Focus group registration:
   - Create API for components to register with focus groups
   - Implement focus group activation/deactivation
   - Handle focus group priority when overlays appear

3. Visual focus indicators:
   - Integrate with existing `FocusIndicator` traits from the UI library
   - Add subtle visual indicators when focus groups are active
   - Implement different indicator styles for different group types
   - Ensure indicators are accessible and don't interfere with UI

4. Group navigation logic:
   - Tab cycles forward through groups: Editor → FileTree → StatusBar → back to Editor
   - Shift+Tab cycles backward through groups
   - Skip disabled or hidden groups
   - Handle overlay groups with higher priority

5. Integration with existing components:
   - Update Workspace to register Editor group when documents are present
   - Update FileTree to register with FileTree group when visible
   - Handle dynamic group availability (e.g., FileTree group only when tree is shown)

6. Keyboard shortcuts:
   - Register Tab/Shift+Tab as global shortcuts
   - Ensure Tab works for focus group navigation when appropriate
   - Handle Tab within groups (e.g., within completion items) vs between groups

7. Context-aware navigation:
   - Don't interfere with Tab behavior in contexts where it has specific meaning
   - Handle modal contexts (completion, pickers) appropriately
   - Ensure focus group navigation works with existing focus restoration

The result should be intuitive keyboard navigation between major UI areas, with clear visual feedback about which area has focus. This creates a foundation for full keyboard-only navigation throughout the application.
```

---

## Prompt 6: Advanced Shortcuts and Navigation

```
Implement advanced keyboard navigation features including context-aware shortcuts, quick navigation (Ctrl+1, Ctrl+2), and customizable keybindings.

Requirements:
1. Quick navigation shortcuts:
   - Ctrl+1: Focus Editor area
   - Ctrl+2: Focus FileTree (open if hidden)
   - Ctrl+3: Focus Status/Command area
   - Ctrl+0: Return to last focused area
   - Make these work from any context

2. Context-aware shortcut behavior:
   - Implement shortcut conflict resolution
   - Different behavior for same key in different contexts
   - Priority system for overlapping shortcuts
   - Context-specific help system

3. Enhanced Escape key handling:
   - Context stack for proper modal dismissal
   - Escape dismisses current modal/overlay
   - Multiple escapes to navigate back through context stack
   - Return to appropriate focus after dismissal

4. Advanced picker integration:
   - Ctrl+P for file finder with fuzzy search
   - Ctrl+Shift+P for command palette
   - Ctrl+O for buffer/document picker
   - Consistent navigation within all pickers

5. Search and navigation integration:
   - Implement search shortcuts that work globally
   - Navigation shortcuts for search results
   - Integration with Helix search when in editor context

6. Customizable keybindings:
   - Create configuration system for custom shortcuts
   - Support for user-defined keybinding overrides
   - Validation system for keybinding conflicts
   - Runtime keybinding updates

7. Accessibility improvements:
   - Screen reader integration for navigation state
   - High contrast focus indicators option
   - Keyboard shortcuts for accessibility features
   - Announce context changes for assistive technology

8. Advanced context management:
   - Context history for smart focus restoration
   - Context-specific menus and help
   - Dynamic context-based shortcut registration
   - Performance optimization for context switching

The result should be a fully-featured keyboard navigation system that rivals modern IDEs, with intuitive shortcuts, customizable behavior, and excellent accessibility support.
```

---

## Prompt 7: Integration, Testing, and Cleanup

```
Complete the integration of the new input system, perform comprehensive testing, clean up old code, and optimize performance.

Requirements:
1. Final integration:
   - Ensure all components are properly wired to InputCoordinator
   - Remove any remaining references to the old GlobalInputDispatcher
   - Update all constructor calls and dependency injection
   - Verify no input handling gaps exist

2. Comprehensive testing:
   - Test all global shortcuts from every possible context
   - Verify modal behavior (completion, pickers, etc.)
   - Test focus restoration on window activation/deactivation
   - Ensure editor functionality remains completely intact
   - Test Tab navigation through all focus groups
   - Verify context switching works correctly

3. Performance optimization:
   - Profile input routing performance
   - Optimize hot paths in InputCoordinator
   - Reduce allocations in input handling
   - Cache commonly-used action handlers

4. Code cleanup:
   - Remove old GlobalInputDispatcher and related code
   - Clean up debug logging from troubleshooting sessions
   - Remove unused imports and dependencies
   - Update documentation to reflect new architecture

5. Error handling and robustness:
   - Add proper error handling to InputCoordinator
   - Handle edge cases in context switching
   - Graceful degradation if coordinator fails
   - Recovery mechanisms for lost focus states

6. Documentation and examples:
   - Document the new input architecture
   - Create examples for adding new shortcuts
   - Document context switching patterns
   - Update CLAUDE.md with new patterns

7. Validation checklist:
   - [ ] All existing shortcuts work from all contexts
   - [ ] Editor input flows properly to Helix core
   - [ ] Tab navigation works between focus groups
   - [ ] Completion system works with new input routing
   - [ ] File tree navigation works properly
   - [ ] Pickers (file finder, command palette) work correctly
   - [ ] Focus restoration works on window events
   - [ ] No performance regressions in input handling
   - [ ] Memory usage is reasonable
   - [ ] All old input system code is removed

The final result should be a production-ready, well-tested, and performant input system that provides seamless global navigation while maintaining all existing functionality. The codebase should be cleaner and more maintainable than before the refactoring.
```