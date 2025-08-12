# Further Refactoring Opportunities

## Modules That Can Be Moved to Lower Layers

### 1. Actions → nucleotide-ui
**Current Location**: `/crates/nucleotide/src/actions.rs`
**Target**: `/crates/nucleotide-ui/src/actions.rs` (merge with existing)
**Content**:
- prompt actions
- file_tree actions  
- editor actions
- help actions
- workspace actions
- window actions
- test actions
- common navigation actions

**Benefit**: Consolidates all GPUI actions in the UI layer where they belong.

### 2. Config Types → nucleotide-types
**Current Location**: `/crates/nucleotide/src/config.rs` (partial)
**Target**: `/crates/nucleotide-types/src/config.rs`
**Content**:
- `FontWeight` enum
- `FontConfig` struct
- `UiFontConfig` struct (if not already there)
- `EditorFontConfig` struct (if not already there)
- Other pure configuration data structures

**Benefit**: Places all configuration types in the foundational types layer.

### 3. Utility Functions → Split between layers
**Current Location**: `/crates/nucleotide/src/utils.rs`

#### → nucleotide-ui
- `color_to_hsla()` - Helix to GPUI color conversion

#### → nucleotide-core  
- `translate_key()` - GPUI to Helix keystroke conversion
- `handle_key_result()` - Helix keymap result handling
- `detect_bundle_runtime()` - Runtime path detection

#### Keep in main crate
- `load_tutor()` - Requires editor manipulation, appropriate for app layer

**Benefit**: Places utilities at the appropriate abstraction level.

### 4. Remove Re-export Files
**Files to remove**:
- `/crates/nucleotide/src/event_bridge.rs` (just re-exports from nucleotide-core)
- `/crates/nucleotide/src/gpui_to_helix_bridge.rs` (just re-exports from nucleotide-core)

**Action**: Update imports to use nucleotide-core directly.

## Implementation Order

1. **Move actions.rs content to nucleotide-ui**
   - Merge with existing actions.rs in nucleotide-ui
   - Update all imports in main crate

2. **Extract config types to nucleotide-types**
   - Create config.rs in nucleotide-types
   - Move pure data types only
   - Keep config loading logic in main crate

3. **Split utils.rs across layers**
   - Create utils modules in target crates
   - Move functions to appropriate layers
   - Update imports

4. **Clean up re-export files**
   - Remove redundant bridge files
   - Update direct imports

## Benefits of These Moves

1. **Reduced main crate size**: Fewer modules in the main application crate
2. **Better cohesion**: Related functionality grouped together
3. **Improved reusability**: Utilities available to all dependent crates
4. **Cleaner architecture**: Each layer contains appropriate abstractions

## Modules That Should Stay in Main Crate

These modules have circular dependencies with Core and should remain:
- `application.rs` - Core application logic
- `workspace.rs` - Main workspace coordination
- `overlay.rs` - Overlay state management
- `statusline.rs` - Status display logic
- `document.rs` - Document view management
- `file_tree/` - File browser implementation
- `editor_*.rs` - Editor integration modules
- `main.rs` - Entry point

## Risk Assessment

**Low Risk Moves**: Actions, config types, and utilities have minimal dependencies and clear ownership.

**Medium Risk**: Some utilities might have hidden dependencies that need careful review.

**Recommendation**: These moves are safe and will improve the architecture without introducing complexity.