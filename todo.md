# Global Input System Refactoring - Todo Tracker

## Status: Planning Complete
**Created**: 2025-08-18
**Last Updated**: 2025-08-18

## Overview
This tracks the implementation of the comprehensive global input system refactoring outlined in `plan.md`. The refactoring addresses fundamental architectural issues with keyboard input routing and focus management.

## Chunk Progress

### ✅ Planning Phase
- [x] Analyze current architecture issues
- [x] Design new InputCoordinator-based architecture  
- [x] Break down implementation into manageable chunks
- [x] Create detailed implementation prompts
- [x] Define success criteria and validation checklist

### 🔲 Chunk 1: InputCoordinator Foundation
**Status**: Not Started  
**Dependencies**: None  
**Estimated Complexity**: Medium  
**Risk Level**: Medium  

**Tasks**:
- [ ] Create `crates/nucleotide/src/input_coordinator.rs`
- [ ] Define InputCoordinator struct with GPUI integration
- [ ] Implement InputContext enum and priority system
- [ ] Create action handler registration system
- [ ] Set up basic focus group foundation
- [ ] Integrate with GPUI actions system

**Success Criteria**:
- [ ] InputCoordinator compiles and can be instantiated
- [ ] Basic action registration works
- [ ] Context switching mechanism functional
- [ ] Thread-safe shared coordinator instance

### 🔲 Chunk 2: Workspace Input Delegation  
**Status**: Not Started  
**Dependencies**: Chunk 1  
**Estimated Complexity**: Small  
**Risk Level**: Low  

**Tasks**:
- [ ] Remove existing key handlers from Workspace
- [ ] Add InputCoordinator integration to Workspace
- [ ] Update workspace constructor to accept coordinator
- [ ] Register workspace actions with coordinator
- [ ] Implement context switching for overlays
- [ ] Update main.rs to create and pass coordinator

**Success Criteria**:
- [ ] Workspace compiles with new architecture
- [ ] File tree toggle works through coordinator
- [ ] Focus restoration still functions
- [ ] No regression in workspace functionality

### 🔲 Chunk 3: Document View Input Cleanup
**Status**: Not Started  
**Dependencies**: Chunks 1-2  
**Estimated Complexity**: Small-Medium  
**Risk Level**: Medium  

**Tasks**:
- [ ] Remove focus tracking from DocumentElement
- [ ] Create editor input bridge to coordinator
- [ ] Implement editor context switching
- [ ] Preserve Helix editor functionality
- [ ] Update focus restoration logic
- [ ] Ensure global shortcuts work when editor focused

**Success Criteria**:
- [ ] Editor input flows to Helix core properly
- [ ] No regression in text editing functionality
- [ ] Global shortcuts work from editor context
- [ ] Scroll and selection behavior preserved

### 🔲 Chunk 4: Component Input Standardization
**Status**: Not Started  
**Dependencies**: Chunks 1-3  
**Estimated Complexity**: Small  
**Risk Level**: Low  

**Tasks**:
- [ ] Standardize FileTree input handling
- [ ] Update completion system integration
- [ ] Standardize picker input handling
- [ ] Remove redundant input systems
- [ ] Implement context switching for all components
- [ ] Register component-specific actions

**Success Criteria**:
- [ ] All components use consistent input patterns
- [ ] Context switching works between components
- [ ] Component-specific shortcuts still functional
- [ ] No independent input handling systems remain

### 🔲 Chunk 5: Focus Groups and Tab Navigation
**Status**: Not Started  
**Dependencies**: Chunks 1-4  
**Estimated Complexity**: Medium  
**Risk Level**: Low-Medium  

**Tasks**:
- [ ] Complete focus group implementation
- [ ] Add visual focus indicators
- [ ] Implement Tab/Shift+Tab navigation
- [ ] Create focus group registration API
- [ ] Handle dynamic group availability
- [ ] Integrate with existing components

**Success Criteria**:
- [ ] Tab navigation works between UI areas
- [ ] Visual focus indicators functional
- [ ] Focus groups integrate with components
- [ ] Context-aware navigation behavior

### 🔲 Chunk 6: Advanced Shortcuts and Navigation
**Status**: Not Started  
**Dependencies**: Chunks 1-5  
**Estimated Complexity**: Large  
**Risk Level**: Low  

**Tasks**:
- [ ] Implement quick navigation (Ctrl+1, Ctrl+2, etc.)
- [ ] Add context-aware shortcut behavior
- [ ] Enhance Escape key handling with context stack
- [ ] Add advanced picker integration
- [ ] Create customizable keybinding system
- [ ] Implement accessibility improvements

**Success Criteria**:
- [ ] All advanced shortcuts functional
- [ ] Context stack works for modal dismissal
- [ ] Customizable keybindings system works
- [ ] Accessibility features functional

### 🔲 Chunk 7: Integration, Testing, and Cleanup
**Status**: Not Started  
**Dependencies**: All previous chunks  
**Estimated Complexity**: Medium  
**Risk Level**: Low  

**Tasks**:
- [ ] Complete final integration
- [ ] Perform comprehensive testing
- [ ] Optimize performance
- [ ] Clean up old code and debug logs
- [ ] Add error handling and robustness
- [ ] Update documentation

**Success Criteria**:
- [ ] All validation checklist items pass
- [ ] Performance matches or exceeds current system
- [ ] Old input system code completely removed
- [ ] Documentation reflects new architecture

## Current Issues to Address
Based on previous session debugging:

### High Priority
1. **DocumentElement Focus Capture**: DocumentElement has focus but no key handlers
2. **Fragmented Input Routing**: Components handle input independently
3. **Global Shortcut Failures**: Ctrl+B and other shortcuts don't work from all contexts

### Medium Priority
1. **Complex Focus Restoration**: Current logic is complex and error-prone
2. **Debug Logging Cleanup**: Remove temporary debug output
3. **Performance**: Multiple event handlers may impact performance

### Low Priority
1. **Code Consistency**: Standardize patterns across components
2. **Documentation**: Update architectural documentation
3. **Testing Coverage**: Add comprehensive input system tests

## Architecture Goals

### Before (Current Issues)
```
Application
├── Workspace (complex key handling + focus management)
│   ├── DocumentView → DocumentElement (focus capture, no handlers)
│   ├── FileTree (independent key handling)
│   └── GlobalInputDispatcher (isolated, not integrated)
```

### After (Target State)
```
Application  
├── InputCoordinator (centralized routing + GPUI integration)
├── Workspace (clean UI logic, delegates to coordinator)
│   ├── DocumentView (pure rendering)
│   ├── FileTree (component logic only)
│   └── Other Components (pure rendering)
```

## Key Principles for Implementation

1. **Incremental Changes**: Each chunk should compile and run
2. **Preserve Functionality**: No regressions in existing features
3. **Clean Interfaces**: Components shouldn't know about global input concerns
4. **GPUI Native**: Use GPUI patterns rather than fighting them
5. **Testable**: Each chunk should be independently testable

## Notes and Decisions

### Design Decisions
- Use GPUI actions instead of custom event routing for global shortcuts
- Centralize all input logic in InputCoordinator rather than distributed handling
- Keep focus handles for UI state but not for input routing
- Implement context stack for proper modal behavior

### Implementation Notes
- InputCoordinator should be created once and shared via Arc
- Context switching should be explicit and auditable
- Components register actions rather than handling keys directly
- Priority system prevents context conflicts

### Risk Mitigation
- Test each chunk thoroughly before proceeding
- Keep rollback capability by preserving working states
- Focus on editor functionality preservation (highest risk area)
- Use feature flags if needed for gradual rollout

---

**Next Step**: Begin Chunk 1 - InputCoordinator Foundation
**Prompt to Use**: Prompt 1 from plan.md