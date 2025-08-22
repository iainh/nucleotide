# UI Components Hybrid Color System Extension Plan

## Overview
Extend our hybrid color system to cover all UI components (buttons, pickers, palettes, modals, etc.) for complete visual consistency across the interface.

## Current Architecture Analysis

### Existing Component System
- `StyleVariant`: Primary, Secondary, Ghost, Danger, Success, Warning, Info, Accent
- `VariantStyler`: Computes variant-specific styles using theme colors
- `ComponentStyles`: Theme-based styling for components
- `VariantColors`: Color computation for variants

### Current Issues
- Components use direct theme colors instead of our hybrid system
- No integration with ChromeTokens for UI chrome consistency
- Button/picker/modal backgrounds may not match titlebar/status bar chrome
- Semantic colors (danger, success, warning) need to integrate with Helix colors

## Implementation Plan

### Phase 1: Component Token Architecture
**Goal**: Create component-specific tokens that integrate with hybrid system

#### 1.1 Button Tokens
```rust
pub struct ButtonTokens {
    // Primary button (main actions)
    primary_background: Hsla,
    primary_background_hover: Hsla,
    primary_background_active: Hsla,
    primary_text: Hsla,
    primary_border: Hsla,
    
    // Secondary button (alternative actions)
    secondary_background: Hsla,
    secondary_background_hover: Hsla,
    secondary_background_active: Hsla,
    secondary_text: Hsla,
    secondary_border: Hsla,
    
    // Ghost button (subtle actions)
    ghost_background: Hsla,
    ghost_background_hover: Hsla,
    ghost_background_active: Hsla,
    ghost_text: Hsla,
    
    // Semantic variants (preserve Helix editor colors)
    danger_background: Hsla,
    danger_text: Hsla,
    success_background: Hsla,
    success_text: Hsla,
    warning_background: Hsla,
    warning_text: Hsla,
    
    // Disabled states
    disabled_background: Hsla,
    disabled_text: Hsla,
    disabled_border: Hsla,
}
```

#### 1.2 Picker/Modal Tokens
```rust
pub struct PickerTokens {
    // Container backgrounds (use chrome colors)
    container_background: Hsla,
    overlay_background: Hsla, // Semi-transparent overlay
    
    // Item states (use editor colors for content)
    item_background: Hsla,
    item_background_hover: Hsla,
    item_background_selected: Hsla, // Use Helix selection
    item_text: Hsla,
    item_text_secondary: Hsla,
    
    // Chrome elements
    header_background: Hsla,
    header_text: Hsla,
    border: Hsla,
    separator: Hsla,
    
    // Input field colors
    input_background: Hsla,
    input_text: Hsla,
    input_border: Hsla,
    input_border_focus: Hsla, // Use Helix focus color
}
```

#### 1.3 Dropdown/Menu Tokens
```rust
pub struct DropdownTokens {
    // Container (chrome colors)
    container_background: Hsla,
    border: Hsla,
    shadow: Hsla,
    
    // Items (editor colors)
    item_background: Hsla,
    item_background_hover: Hsla,
    item_background_selected: Hsla,
    item_text: Hsla,
    item_text_secondary: Hsla,
    
    // Separators
    separator: Hsla,
}
```

### Phase 2: Token Generation Functions
**Goal**: Create token generators that use hybrid color principles

#### 2.1 Component Token Generators
```rust
impl ChromeTokens {
    pub fn button_tokens(&self, editor: &EditorTokens) -> ButtonTokens { ... }
    pub fn picker_tokens(&self, editor: &EditorTokens) -> PickerTokens { ... }
    pub fn dropdown_tokens(&self, editor: &EditorTokens) -> DropdownTokens { ... }
}

impl DesignTokens {
    pub fn button_tokens(&self) -> ButtonTokens { ... }
    pub fn picker_tokens(&self) -> PickerTokens { ... }
    pub fn dropdown_tokens(&self) -> DropdownTokens { ... }
}
```

#### 2.2 Hybrid Color Principles for Components
- **Primary buttons**: Use computed chrome colors for consistency with titlebar
- **Secondary buttons**: Slightly lighter/darker than primary
- **Ghost buttons**: Transparent with chrome-based hover states
- **Semantic buttons** (danger/success/warning): Use Helix editor colors
- **Container backgrounds**: Use chrome colors for visual hierarchy
- **Content items**: Use Helix selection/cursor colors for familiarity
- **Text colors**: Ensure proper contrast with computed backgrounds

### Phase 3: Integration with Existing System
**Goal**: Integrate new tokens with existing `VariantStyler` and `ComponentStyles`

#### 3.1 Update VariantColors
```rust
impl VariantColors {
    pub fn for_variant_hybrid(variant: StyleVariant, tokens: &DesignTokens) -> Self {
        match variant {
            StyleVariant::Primary => {
                let button_tokens = tokens.button_tokens();
                Self {
                    background: button_tokens.primary_background,
                    background_hover: button_tokens.primary_background_hover,
                    // ... other states
                }
            }
            StyleVariant::Danger => {
                let button_tokens = tokens.button_tokens();
                Self {
                    background: button_tokens.danger_background,
                    // Use Helix error colors
                }
            }
            // ... other variants
        }
    }
}
```

#### 3.2 Update Component Implementations
- Update Button component to use ButtonTokens
- Update Picker components to use PickerTokens
- Update Modal/Overlay components to use hybrid backgrounds
- Update Dropdown/Menu components to use DropdownTokens

### Phase 4: Palette and Advanced Components
**Goal**: Handle color palettes and complex interactive components

#### 4.1 Color Palette Integration
- Theme preview palettes show hybrid colors
- Color picker components use chrome backgrounds
- Syntax highlighting previews preserve Helix colors

#### 4.2 Advanced Component Tokens
```rust
pub struct InputTokens {
    background: Hsla,
    text: Hsla,
    border: Hsla,
    border_focus: Hsla, // Use Helix focus
    placeholder: Hsla,
}

pub struct TooltipTokens {
    background: Hsla, // Chrome-based
    text: Hsla,
    border: Hsla,
    shadow: Hsla,
}

pub struct NotificationTokens {
    info_background: Hsla,
    success_background: Hsla, // Helix success
    warning_background: Hsla, // Helix warning  
    error_background: Hsla,   // Helix error
    text: Hsla,
    border: Hsla,
}
```

### Phase 5: Testing and Validation
**Goal**: Ensure all components follow hybrid color principles

#### 5.1 Component Token Tests
- WCAG contrast validation for all button states
- Visual hierarchy tests (chrome vs content)
- Color consistency tests across all components

#### 5.2 Integration Tests  
- Theme switching works across all components
- Helix theme colors preserved in semantic elements
- Chrome colors consistent across UI chrome

## Migration Strategy

### Step 1: Create Foundation
1. Add ComponentTokens structs to tokens/mod.rs
2. Implement token generation functions
3. Add comprehensive tests

### Step 2: Update Core Components
1. Button component → ButtonTokens
2. Picker components → PickerTokens  
3. Modal/Overlay → chrome backgrounds

### Step 3: Update Styling System
1. Integrate with VariantStyler
2. Update ComponentStyles to use hybrid tokens
3. Maintain backward compatibility

### Step 4: Advanced Components
1. Input fields, tooltips, notifications
2. Color palettes and theme previews
3. Complex interactive components

### Step 5: Validation
1. Run comprehensive tests
2. Visual consistency audit
3. Performance validation

## Expected Benefits

1. **Visual Consistency**: All UI chrome uses computed colors
2. **Helix Integration**: Editor semantic colors preserved where appropriate  
3. **Accessibility**: WCAG compliance across all components
4. **Maintainability**: Centralized color logic for all UI components
5. **Theme Flexibility**: Automatic adaptation to any Helix theme

## Color Distribution Strategy

- **Chrome Elements** (containers, backgrounds, borders): Use computed ChromeTokens
- **Content Elements** (text, selections, cursors): Use Helix EditorTokens  
- **Semantic Elements** (errors, warnings, success): Use Helix colors
- **Interactive States** (hover, focus, active): Computed from base colors
- **Neutral Elements** (separators, borders): Computed from surface color