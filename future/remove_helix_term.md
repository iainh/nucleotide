# Plan to Remove `helix-term` Dependency

This document outlines a plan to remove the `helix-term` dependency from the Nucleotide project. This is a significant architectural change that will streamline the application, reduce its binary size, and eliminate unnecessary terminal-specific libraries.

## Prerequisites

**Capture baseline metrics before starting:**
```bash
# Clean build to ensure accurate measurements
cargo clean
time cargo build --release
ls -lh target/release/nucl

# Count total dependencies
cargo tree --edges=features --prefix=none | sort -u | wc -l

# Record the metrics below
```

**Baseline Metrics (TO BE FILLED IN):**
- Build time: _____ seconds
- Binary size: _____ MB
- Total dependency count: `cargo tree --edges=features --prefix=none | wc -l`
- Terminal-specific dependencies to be removed:
  - `crossterm` and its ~141 dependencies (terminal I/O)
  - `helix-tui` and its ~1,804 dependencies (terminal UI rendering)
  - `helix-dap` and its ~947 dependencies (debug adapter protocol)
  - `signal-hook` and its dependencies (terminal signal handling)

**Create a feature branch before starting:**
```bash
git checkout -b feature/no_helix_term
```

**Development Approach:**
- Practice Test-Driven Development (TDD) throughout the migration
- Write tests for existing functionality before extracting/replacing components
- Ensure each phase maintains full functionality with comprehensive tests
- Keep the main branch stable by only merging completed, tested phases

## Current Usage Analysis

The project currently uses `helix-term` for:
1. **Configuration** (`helix_term::config::Config`) - Already partially replaced
2. **Commands** (`TYPABLE_COMMAND_MAP`, `TYPABLE_COMMAND_LIST`, command execution)
3. **Compositor** (Component trait, EventResult, Context) - Core event handling
4. **UI Components** (Prompt, Picker, FilePickerData, EditorView)
5. **Events** (OnModeSwitch, PostCommand registration)
6. **Keymaps** (KeymapResult, Keymaps)
7. **Jobs** (async task management)
8. **Args** (command-line argument parsing)

## Phase 1: Extract Core Types (Low Risk) - Days 1-2

Start with the simplest, most isolated components to build confidence and establish patterns.

### 1.1 Args Structure
*   **Write tests** for current command-line argument parsing behavior
*   **Extract** `helix_term::args::Args` struct to `src/args.rs`
*   **Update** `src/main.rs` to use local Args
*   **Verify** all tests pass

### 1.2 Jobs System
*   **Write tests** for async job management scenarios
*   **Extract** `helix_term::job::Jobs` to `src/jobs.rs`
*   **Simplify** for GUI use (remove terminal-specific concerns)
*   **Update** all Jobs usage in application.rs
*   **Verify** async operations still work correctly

### 1.3 Complete Configuration Migration
*   **Write tests** for all configuration loading scenarios
*   **Complete extraction** of remaining config types from `helix_term::config`
*   **Remove** all `helix_term::config` imports
*   **Verify** configuration loading and merging works correctly

## Phase 2: Build Command Infrastructure (Medium Complexity) - Days 3-5

Create a robust command system that preserves all existing functionality.

### 2.1 Command Registry Design
*   **Write tests** for command execution patterns
*   **Define traits**: `MappableCommand` and `TypableCommand` in `src/commands/mod.rs`
*   **Create registry**: Build command maps for both command types
*   **Test** command lookup and execution

### 2.2 Extract Command Definitions
*   **Write tests** for each command category
*   **Extract** command definitions from helix-term source
*   **Organize** commands by category (movement, editing, file operations, etc.)
*   **Use** `helix_view` and `helix_core` directly for command implementation
*   **Test** each command thoroughly

### 2.3 GPUI Command Context
*   **Write tests** for context creation and usage
*   **Create** GPUI-native command context that wraps helix's Context
*   **Update** all command execution sites
*   **Verify** commands execute correctly with new context

## Phase 3: Replace UI Components (Already Started) - Days 6-7

Build on the existing `prompt_view.rs` pattern to replace remaining UI components.

### 3.1 Complete Prompt Migration
*   ✅ Already done - `prompt_view.rs` replaces `helix_term::ui::Prompt`
*   **Write tests** to ensure feature parity
*   **Remove** remaining `helix_term::ui::Prompt` references

### 3.2 File Picker
*   **Write tests** for file picker functionality
*   **Create** `src/ui/file_picker.rs` using GPUI
*   **Implement** fuzzy search using existing `nucleo` dependency
*   **Replace** `helix_term::ui::Picker` usage
*   **Test** file navigation and selection

### 3.3 EditorView Methods
*   **Write tests** for syntax highlighting methods
*   **Extract** the 3 highlight methods from EditorView
*   **Move** to `src/utils/highlights.rs`
*   **Update** document.rs to use local methods
*   **Verify** syntax highlighting still works

## Phase 4: Event System (Critical) - Day 8

Maintain event compatibility while removing terminal-specific registration.

### 4.1 Event Registration
*   **Write tests** for event flow
*   **Keep** `helix_event` for core event handling
*   **Create** local registration in `src/events/register.rs`
*   **Extract** OnModeSwitch and PostCommand event types
*   **Replace** `helix_term::events::register()` call
*   **Test** all event handlers still fire correctly

### 4.2 Event Bridge Updates
*   **Update** `src/event_bridge.rs` to use local event types
*   **Verify** GPUI ↔ Helix event translation works
*   **Test** mode switches and command execution events

## Phase 5: Navigation Stack Architecture (Most Complex) - Days 9-17

Replace the terminal-based compositor with a GPUI-native navigation stack, while **preserving all existing Helix rendering logic**.

### Why Navigation Stack (Not Terminal Virtual DOM)

**Double-diffing Problem:** helix-term builds a 2D grid of cells and diffs it against the terminal. GPUI already provides retained graphics with its own diffing - implementing a terminal compositor would mean doing the work twice and losing GPUI's optimizations.

**Preserve Existing Code:** The navigation stack approach keeps all existing Helix layout, syntax highlighting, and rendering code. We're only replacing the compositor's component management, not the actual rendering logic.

### 5.1 Navigation Infrastructure (Days 9-10)
*   **Write tests** for navigation state management
*   **Create** `src/navigation/mod.rs` with route definitions:
    ```rust
    pub enum Route {
        Editor(EditorView),  // Uses existing document.rs rendering
        FilePicker(FilePickerView),
        Prompt(PromptView),  // Already implemented in prompt_view.rs
        Command(CommandPalette),
    }
    ```
*   **Implement** `NavStack` for push/pop/replace operations
*   **Keep** existing `Application` structure with editor, jobs, etc.
*   **Test** navigation transitions preserve editor state

### 5.2 Compositor to Navigation Adapter (Days 11-12)
*   **Write tests** for compositor compatibility
*   **Create** adapter layer that translates compositor push/pop to navigation
*   **Preserve** existing `EditorView` from helix-term (just wrap it)
*   **Keep** all existing highlight calculation code in document.rs
*   **Maintain** existing scroll_manager.rs logic
*   **Test** that all rendering still uses Helix's layout algorithms

### 5.3 Component Wrapping (Days 13-14)
*   **Write tests** for component lifecycle
*   **Wrap** existing Helix components as GPUI Views (thin wrappers only)
*   **Preserve** `EditorView::render_view` and all its logic
*   **Keep** syntax highlighting via `EditorView::doc_syntax_highlights`
*   **Maintain** diagnostic rendering via existing methods
*   **Test** all Helix rendering features still work

### 5.4 Context Bridge (Days 15-16)
*   **Write tests** for context translation
*   **Create** minimal `CompositorContext` that wraps existing helix Context
*   **Route** commands through navigation while preserving helix command execution
*   **Keep** all `helix_term::commands` execution paths
*   **Test** command execution unchanged

### 5.5 Cleanup (Day 17)
*   **Remove** only the Compositor struct itself
*   **Keep** all rendering, layout, and highlight logic
*   **Verify** no functionality lost
*   **Test** performance unchanged or improved

**Key Point:** We're NOT reimplementing Helix's rendering - we're just replacing the component management layer while keeping all the actual editor logic intact.

## Phase 6: Cleanup & Testing (Days 18-19)

### 6.1 Remove Dependency
*   **Remove** `helix-term` from `Cargo.toml`
*   **Run** `cargo build --release`
*   **Measure** final metrics:
    ```bash
    cargo clean
    time cargo build --release
    ls -lh target/release/nucl
    time ./target/release/nucl --version
    ```
*   **Compare** with baseline metrics
*   **Check** dependency tree for unnecessary packages

### 6.2 Comprehensive Testing
*   **Run** all unit tests
*   **Perform** integration testing of all features
*   **Test** edge cases and error conditions
*   **Performance testing** to ensure no regressions

### 6.3 Documentation
*   **Update** README with architectural changes
*   **Document** new command system API
*   **Add** migration notes for any breaking changes

## Success Metrics

**Quantitative Metrics:**
- ✅ Build time reduction: Target < 50% of baseline
- ✅ Binary size reduction: Target < 70% of baseline
- ✅ Significant dependency reduction (removing ~2,892+ dependencies from terminal-specific crates)
- ✅ No terminal-specific libraries (crossterm, helix-tui, helix-dap, signal-hook)
- ✅ Improved startup time: Measure with `time ./target/release/nucl --version`

**Qualitative Metrics:**
- ✅ All existing functionality preserved
- ✅ Cleaner architecture aligned with GUI paradigms
- ✅ All tests passing
- ✅ No performance regressions
- ✅ Improved maintainability

## Risk Mitigation

1. **Command Compatibility**: Extract exact implementations, comprehensive testing
2. **Compositor Functionality**: Gradual replacement with fallback options
3. **Event System**: Keep helix_event core, only replace registration
4. **Breaking Changes**: Feature branch isolation, extensive testing before merge

## Timeline Estimate

**Total: 19-20 working days (approximately 4 weeks of focused development)**

- Phase 1 (Core Types): 2 days
- Phase 2 (Commands): 3 days  
- Phase 3 (UI Components): 2 days
- Phase 4 (Events): 1 day
- Phase 5 (Navigation Stack): 9 days
- Phase 6 (Cleanup & Testing): 2 days

The extended timeline for Phase 5 reflects the complexity of replacing the compositor while preserving all existing Helix functionality. This is a fundamental architectural change, not just a component swap.

This plan ensures a systematic, testable approach to removing the `helix-term` dependency while maintaining stability and preserving all existing Helix rendering and layout logic.
