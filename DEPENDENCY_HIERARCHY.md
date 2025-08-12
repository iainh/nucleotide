# Nucleotide Dependency Hierarchy

## Verified Dependency Structure (Phase 6)

After refactoring, the crate dependencies form a clean directed acyclic graph (DAG):

```
Layer 1: nucleotide-types
         └── (no nucleotide dependencies)

Layer 2: nucleotide-events
         └── nucleotide-types

Layer 3: nucleotide-core
         ├── nucleotide-types
         └── nucleotide-events

Layer 4: nucleotide-ui
         ├── nucleotide-types
         ├── nucleotide-events
         └── nucleotide-core

Layer 4: nucleotide-editor
         ├── nucleotide-core
         └── nucleotide-ui

Layer 5: nucleotide-lsp
         └── nucleotide-core

Layer 6: nucleotide-workspace (shell only)
         └── (no dependencies currently)

Layer 7: nucleotide (main app)
         ├── nucleotide-types
         ├── nucleotide-events
         ├── nucleotide-core
         ├── nucleotide-ui
         ├── nucleotide-editor
         └── nucleotide-lsp
```

## Verification Results

✅ **No circular dependencies detected**
✅ **All crates compile successfully**
✅ **Workspace builds without errors**
✅ **Dependency tree shows proper layering**

## Key Achievements

1. **Clean separation of concerns**: Each crate has a specific responsibility
2. **Unidirectional dependencies**: Lower layers don't depend on higher layers
3. **Reusable components**: Lower layer crates can be used independently
4. **Type safety**: Types flow from nucleotide-types through all layers
5. **Event decoupling**: Events in nucleotide-events enable loose coupling

## Remaining in Main Crate

The following remain in the main `nucleotide` crate as the application integration layer:
- workspace.rs - Main workspace coordination
- overlay.rs - Overlay management
- statusline.rs - Status display
- document.rs - Document view
- file_tree/ - File browser
- application.rs - Core application loop

These components are tightly integrated with GPUI entity management and form the application shell.