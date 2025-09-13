# Refactor Nucleotide into Multiple Crates

Based on my analysis, here's a plan to refactor Nucleotide into a multi-crate workspace structure:

## Proposed Crate Structure

1. **nucleotide-core** - Core abstractions and bridges
   - `Application` struct (wrapper for Helix Editor)
   - Event bridges (`event_bridge`, `gpui_to_helix_bridge`)
   - Core types and input handling
   - Dependencies: helix-*, tokio

2. **nucleotide-ui** - Reusable UI components
   - All modules from `src/ui/`
   - Theme system (`Theme`, `Themed` trait)
   - Common UI patterns (button, list_item, overlay, scrollbar)
   - Dependencies: gpui only

3. **nucleotide-widgets** - Complex UI widgets
   - File tree (`src/file_tree/`)
   - Picker system (`picker`, `picker_view`)
   - Overlay views (`overlay`, `prompt`, `prompt_view`)
   - Info/notification views (`info_box`, `notification`, `key_hint_view`)
   - Dependencies: nucleotide-ui, gpui, notify

4. **nucleotide-editor** - Editor view components
   - Document view (`document.rs`)
   - Scroll management (`scroll_manager.rs`)
   - Line cache (`line_cache.rs`)
   - Completion (`completion.rs`)
   - Dependencies: nucleotide-ui, nucleotide-core, helix-*, gpui

5. **nucleotide-lsp** - LSP integration
   - All modules from `src/core/` (lsp_manager, lsp_state, document_manager)
   - LSP status view (`lsp_status.rs`)
   - Dependencies: nucleotide-core, helix-lsp, gpui

6. **nucleotide-workspace** - Main application workspace
   - Workspace (`workspace.rs`)
   - Titlebar components (`src/titlebar/`)
   - Statusline (`statusline.rs`)
   - Command system (`command_system.rs`)
   - Dependencies: all other crates

7. **nucleotide** - Main application
   - Main entry point (`main.rs`)
   - Configuration (`config.rs`)
   - Actions (`actions.rs`)
   - Assets (`assets.rs`)
   - Utilities (`utils.rs`, `theme_manager.rs`)
   - Dependencies: all other crates

## Migration Steps

### Phase 0: Create feature branch
1. Create a new branch for the refactoring work: `git checkout -b refactor/multi-crate-workspace`
2. All refactoring work should be done on this branch
3. Regularly commit progress to track changes
4. Merge back to main only when refactoring is complete and tested

### Phase 1: Set up workspace structure
1. Create workspace `Cargo.toml` at project root
2. Create `crates/` directory
3. Move current code to `crates/nucleotide/`
4. Update `.gitignore` and build scripts

### Phase 2: Extract nucleotide-ui (lowest dependency)
1. Create `crates/nucleotide-ui/` with its own `Cargo.toml`
2. Move `ui/` modules to new crate
3. Define and export public API
4. Remove old ui modules from main crate

### Phase 3: Extract nucleotide-core
1. Create `crates/nucleotide-core/` 
2. Move `Application`, bridges, and input types
3. Define public traits and types for component communication
4. Remove old core modules from main crate

### Phase 4: Extract specialized crates (can be done in parallel)
1. Extract nucleotide-lsp (LSP components)
2. Extract nucleotide-widgets (file tree, pickers, overlays)  
3. Extract nucleotide-editor (document view, scroll management)
4. Remove extracted modules from main crate

### Phase 5: Create nucleotide-workspace
1. Create `crates/nucleotide-workspace/`
2. Move workspace and related components
3. Wire up all crate dependencies
4. Remove workspace modules from main crate

### Phase 6: Finalize main crate
1. Keep only main entry point, config, and top-level coordination
2. Re-export key public APIs from sub-crates for convenience
3. Update bundle scripts for new structure
4. Update README with new architecture

## Benefits
- **Better separation of concerns** - Each crate has a clear responsibility
- **Faster incremental builds** - Changes to UI don't rebuild core logic
- **Reusability** - UI components can be used independently
- **Testability** - Easier to test components in isolation
- **Parallel development** - Teams can work on different crates simultaneously
- **Clean architecture** - No legacy compatibility code needed

## Notes
- Since we're not maintaining backward compatibility, we can make clean API boundaries
- This is a breaking change that will require updating all imports
- Can optimize crate interfaces without compatibility constraints
- Update CI/CD pipelines for workspace builds
- Document inter-crate dependencies and architecture
