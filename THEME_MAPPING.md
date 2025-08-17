# Nucleotide Design System Theme Mapping

This document provides a comprehensive mapping of all design tokens in the nucleotide-ui design system, their default values in light and dark system appearances, and how they map to Helix theme keys.

## Table of Contents

1. [Design Token Overview](#design-token-overview)
2. [Semantic Color Tokens](#semantic-color-tokens)
3. [Size and Spacing Tokens](#size-and-spacing-tokens)
4. [Helix Theme Mapping](#helix-theme-mapping)
5. [Component Usage Examples](#component-usage-examples)
6. [Debugging Guide](#debugging-guide)

## Design Token Overview

The nucleotide-ui design system uses a two-tier token architecture:

1. **Base Colors**: Raw color palette (neutral_50-950, primary_50-900, semantic colors)
2. **Semantic Tokens**: Meaningful names mapped to base colors (background, surface, text_primary, etc.)

This allows for consistent theming while maintaining flexibility for different appearance modes.

## Semantic Color Tokens

### Surface Colors

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.background` | `neutral_50` (98% lightness) | `neutral_50` (5% lightness) | `ui.background` | Main app background |
| `colors.surface` | `neutral_100` (96% lightness) | `neutral_100` (8% lightness) | `ui.menu` → `ui.background` | Component backgrounds |
| `colors.surface_elevated` | `neutral_200` (94% lightness) | `neutral_200` (12% lightness) | Computed from base | Elevated surfaces (modals, dropdowns) |
| `colors.surface_overlay` | `hsla(0,0,100%,95%)` | `hsla(0,0,0%,95%)` | None | Overlay backdrop |

### Interactive States

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.surface_hover` | `neutral_200` (94% lightness) | `neutral_200` (12% lightness) | None | Hover states |
| `colors.surface_active` | `neutral_300` (91% lightness) | `neutral_300` (16% lightness) | None | Active/pressed states |
| `colors.surface_selected` | `primary_100` | `primary_200` | `ui.selection` | Selected items |
| `colors.surface_disabled` | `neutral_100` (96% lightness) | `neutral_100` (8% lightness) | None | Disabled elements |

### Text Colors

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.text_primary` | `neutral_900` (15% lightness) | `neutral_900` (89% lightness) | `ui.text` | Primary text |
| `colors.text_secondary` | `neutral_700` (42% lightness) | `neutral_700` (64% lightness) | None | Secondary text |
| `colors.text_tertiary` | `neutral_500` (64% lightness) | `neutral_500` (38% lightness) | None | Tertiary text |
| `colors.text_disabled` | `neutral_400` (78% lightness) | `neutral_400` (24% lightness) | None | Disabled text |
| `colors.text_on_primary` | `neutral_50` (98% lightness) | `neutral_50` (5% lightness) | None | Text on primary colors |

### Border Colors

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.border_default` | `neutral_300` (91% lightness) | `neutral_300` (16% lightness) | `ui.window` → computed | Default borders |
| `colors.border_muted` | `neutral_200` (94% lightness) | `neutral_200` (12% lightness) | None | Subtle borders |
| `colors.border_strong` | `neutral_400` (78% lightness) | `neutral_400` (24% lightness) | None | Prominent borders |
| `colors.border_focus` | `primary_500` | `primary_500` | `ui.cursor.primary` | Focus indicators |

### Brand Colors

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.primary` | `primary_500` (55% lightness, 220° hue) | `primary_500` (69% lightness, 220° hue) | `ui.selection` → `ui.cursor.primary` | Primary brand color |
| `colors.primary_hover` | `primary_600` (44% lightness) | `primary_400` (55% lightness) | None | Primary hover state |
| `colors.primary_active` | `primary_700` (35% lightness) | `primary_300` (44% lightness) | None | Primary active state |

### Semantic Feedback Colors

| Token | Light Theme Value | Dark Theme Value | Helix Key | Usage |
|-------|------------------|------------------|-----------|-------|
| `colors.success` | `success_500` (50% lightness, 120° hue) | `success_500` (60% lightness, 120° hue) | None | Success states |
| `colors.warning` | `warning_500` (50% lightness, 40° hue) | `warning_500` (60% lightness, 40° hue) | `warning` | Warning states |
| `colors.error` | `error_500` (50% lightness, 0° hue) | `error_500` (60% lightness, 0° hue) | `error` | Error states |
| `colors.info` | `info_500` (50% lightness, 200° hue) | `info_500` (60% lightness, 200° hue) | `info` | Info states |

## Size and Spacing Tokens

### Spacing Scale

| Token | Value | Usage |
|-------|-------|-------|
| `sizes.space_0` | `0px` | No spacing |
| `sizes.space_1` | `2px` | Minimal spacing |
| `sizes.space_2` | `4px` | Small spacing |
| `sizes.space_3` | `8px` | Base spacing unit |
| `sizes.space_4` | `12px` | Medium spacing |
| `sizes.space_5` | `16px` | Large spacing |
| `sizes.space_6` | `20px` | Extra large spacing |
| `sizes.space_7` | `24px` | 2x large spacing |
| `sizes.space_8` | `32px` | 3x large spacing (overlay positioning) |
| `sizes.space_9` | `40px` | 4x large spacing |
| `sizes.space_10` | `48px` | 5x large spacing |

### Component Sizes

| Token | Value | Usage |
|-------|-------|-------|
| `sizes.button_height_sm` | `28px` | Small buttons, status line |
| `sizes.button_height_md` | `36px` | Medium buttons, tab bar |
| `sizes.button_height_lg` | `44px` | Large buttons |

### Border Radius

| Token | Value | Usage |
|-------|-------|-------|
| `sizes.radius_sm` | `4px` | Small radius |
| `sizes.radius_md` | `6px` | Medium radius (buttons, tabs) |
| `sizes.radius_lg` | `8px` | Large radius (dropdowns, modals) |
| `sizes.radius_full` | `9999px` | Fully rounded |

### Font Sizes

| Token | Value | Usage |
|-------|-------|-------|
| `sizes.text_xs` | `11px` | Very small text |
| `sizes.text_sm` | `12px` | Small text (status line) |
| `sizes.text_md` | `14px` | Default text size |
| `sizes.text_lg` | `16px` | Large text |
| `sizes.text_xl` | `18px` | Extra large text |

## Helix Theme Mapping

### Primary Mappings

| Nucleotide Token | Helix Theme Key | Fallback Logic |
|------------------|-----------------|----------------|
| `background` | `ui.background` | System appearance: Light(98%), Dark(5%) |
| `surface` | `ui.menu` → `ui.background` (+5% lightness) | System appearance: Light(95%), Dark(10%) |
| `text_primary` | `ui.text` | System appearance: Light(10%), Dark(90%) |
| `border_default` | `ui.window` → `ui.text` (50% sat, 50% light, 80% alpha) | System appearance: Light(80%), Dark(20%) |
| `primary` | `ui.selection` → `ui.cursor.primary` | Default: Blue (220°, 60%, 50%) |
| `error` | `error` | Default: Red (0°, 80%, 50%/60%) |
| `warning` | `warning` | Default: Orange (40°, 80%, 50%/60%) |
| `info` | `info` | Default: Blue (200°, 70%, 50%/60%) |

### Computed Colors

Some design tokens are computed from Helix theme values:

- **Surface**: `ui.menu` background, or `ui.background` + 5% lightness
- **Border**: `ui.window` foreground with reduced saturation/lightness and transparency
- **Interactive States**: Computed from base colors using lightness adjustments

### Fallback System

When Helix theme keys are missing, the system uses:

1. **System Appearance Detection**: Light vs Dark mode
2. **Base Color Palette**: Neutral and primary color scales
3. **Semantic Defaults**: Consistent colors across themes

## Component Usage Examples

### Tab Component
```rust
// Uses multiple design tokens
.bg(tokens.colors.background)           // ui.background
.text_color(tokens.colors.text_primary) // ui.text
.border_color(tokens.colors.border_default) // ui.window
.px(tokens.sizes.space_4)               // 12px padding
.rounded(tokens.sizes.radius_md)        // 6px radius
```

### Status Line
```rust
// Focus-based theming
let status_bg = if focused {
    tokens.colors.surface               // ui.menu
} else {
    tokens.colors.surface_disabled     // computed
};
```

### Overlay System
```rust
// Consistent overlay styling
.bg(tokens.colors.surface_overlay)     // 95% opacity backdrop
.pt(tokens.sizes.space_8)              // 32px top padding
```

## Debugging Guide

### Environment Variables

```bash
# Force fallback colors (ignore all Helix theme colors)
export NUCLEOTIDE_DISABLE_THEME_LOADING=1

# Enable detailed theme logging
export NUCLEOTIDE_LOG=debug
export RUST_LOG=nucleotide_ui::theme_manager=trace
```

### Theme Inspection

Use the theme manager's debug methods:

```rust
// Check if theme is dark
let is_dark = theme_manager.is_dark_theme();

// Get current system appearance
let appearance = theme_manager.get_system_appearance();

// Access design tokens
let theme = cx.theme();
let background = theme.tokens.colors.background;
```

### Common Issues

1. **Missing Helix Theme Key**: Check fallback values in the mapping table
2. **Incorrect Lightness**: Verify light/dark theme values match expectations
3. **Inconsistent Spacing**: Ensure using design tokens instead of hardcoded px values
4. **Wrong Semantic Color**: Check if using appropriate semantic token for the use case

### Color Value Format

All colors use HSLA format:
- **Hue**: 0.0-1.0 (multiply by 360 for degrees)
- **Saturation**: 0.0-1.0 (0% to 100%)
- **Lightness**: 0.0-1.0 (0% to 100%)
- **Alpha**: 0.0-1.0 (0% to 100% opacity)

Example: `hsla(220.0/360.0, 0.6, 0.5, 1.0)` = Blue (220°, 60%, 50%, 100%)

## Proposed Additional Theme Keys

Based on comprehensive analysis of Helix theme documentation, the following theme keys could be added to enhance the design system:

### Cursor and Selection System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.cursor_normal` | `ui.cursor.normal` → `ui.cursor` | `primary_500` | `primary_500` | Default cursor state |
| `colors.cursor_insert` | `ui.cursor.insert` | `success_500` | `success_500` | Insert mode cursor |
| `colors.cursor_select` | `ui.cursor.select` | `warning_500` | `warning_500` | Selection mode cursor |
| `colors.cursor_match` | `ui.cursor.match` | `info_500` | `info_500` | Bracket matching cursor |
| `colors.selection_primary` | `ui.selection` | `primary_100` | `primary_200` | Primary text selection |
| `colors.selection_secondary` | `ui.highlight` | `neutral_100` | `neutral_200` | Secondary highlighting |

### Gutter and Line Number System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.gutter_background` | `ui.gutter` | `neutral_50` | `neutral_50` | Editor gutter background |
| `colors.gutter_selected` | `ui.gutter.selected` | `neutral_100` | `neutral_100` | Active line gutter |
| `colors.line_number` | `ui.linenr` | `neutral_500` | `neutral_500` | Line numbers |
| `colors.line_number_active` | `ui.linenr.selected` | `neutral_700` | `neutral_700` | Current line number |

### Enhanced Status and Buffer System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.statusline_active` | `ui.statusline` | `surface` | `surface` | Active window status |
| `colors.statusline_inactive` | `ui.statusline.inactive` | `surface_disabled` | `surface_disabled` | Inactive window status |
| `colors.bufferline_background` | `ui.bufferline` → computed | `neutral_100` | `neutral_100` | Buffer bar background |
| `colors.bufferline_active` | `ui.bufferline.active` | `background` | `background` | Active buffer tab |
| `colors.bufferline_inactive` | `ui.bufferline` | `surface` | `surface` | Inactive buffer tabs |

### Enhanced Popup and Menu System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.popup_background` | `ui.popup` | `surface_elevated` | `surface_elevated` | Documentation popups |
| `colors.popup_border` | `ui.popup` → computed | `border_default` | `border_default` | Popup borders |
| `colors.menu_background` | `ui.menu` | `surface_elevated` | `surface_elevated` | Menu/completion background |
| `colors.menu_selected` | `ui.menu.selected` | `surface_selected` | `surface_selected` | Selected menu items |
| `colors.menu_separator` | `ui.background.separator` | `border_muted` | `border_muted` | Menu dividers |

### Enhanced Diagnostic System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.diagnostic_error` | `error` | `error_500` | `error_500` | Error text/icons |
| `colors.diagnostic_warning` | `warning` | `warning_500` | `warning_500` | Warning text/icons |
| `colors.diagnostic_info` | `info` | `info_500` | `info_500` | Info text/icons |
| `colors.diagnostic_hint` | `hint` | `neutral_600` | `neutral_600` | Hint text/icons |
| `colors.diagnostic_error_bg` | `error` → computed (10% opacity) | `error_500` + alpha | `error_500` + alpha | Error backgrounds |
| `colors.diagnostic_warning_bg` | `warning` → computed (10% opacity) | `warning_500` + alpha | `warning_500` + alpha | Warning backgrounds |
| `colors.diagnostic_info_bg` | `info` → computed (10% opacity) | `info_500` + alpha | `info_500` + alpha | Info backgrounds |
| `colors.diagnostic_hint_bg` | `hint` → computed (10% opacity) | `neutral_600` + alpha | `neutral_600` + alpha | Hint backgrounds |

### Separator and UI Enhancement System

| Proposed Token | Helix Key | Light Theme Value | Dark Theme Value | Usage |
|----------------|-----------|------------------|------------------|-------|
| `colors.separator_horizontal` | `ui.background.separator` | `border_muted` | `border_muted` | Horizontal dividers |
| `colors.separator_vertical` | `ui.background.separator` | `border_muted` | `border_muted` | Vertical dividers |
| `colors.separator_subtle` | `ui.background.separator` → computed (50% opacity) | `border_muted` + alpha | `border_muted` + alpha | Subtle separations |
| `colors.focus_ring` | `ui.cursor.primary` | `primary_500` | `primary_500` | Focus indicators |
| `colors.focus_ring_error` | `error` | `error_500` | `error_500` | Error state focus |
| `colors.focus_ring_warning` | `warning` | `warning_500` | `warning_500` | Warning state focus |

## Implementation Roadmap

### Phase 1: Core Editor Experience
1. **Cursor and Selection System** - Essential for editing modes
2. **Enhanced Diagnostics** - Critical for LSP integration
3. **Gutter and Line Numbers** - Editor-specific functionality

### Phase 2: UI Hierarchy and Navigation  
4. **Status and Buffer System** - Window and tab management
5. **Enhanced Popups and Menus** - Command and completion UX

### Phase 3: Polish and Accessibility
6. **Separators and Focus** - Visual hierarchy and accessibility
7. **Component Integration** - Update existing components to use new tokens

### Implementation Benefits

- **Enhanced Editor Fidelity**: More accurate Helix terminal → GUI mapping
- **Improved User Experience**: Better visual distinction between UI states
- **Design System Completeness**: Comprehensive coverage of Helix UI elements
- **Better Debugging**: More granular theme key mapping for troubleshooting

---

This document should be updated whenever design tokens or Helix theme mappings change. For the most current values, refer to:
- `/crates/nucleotide-ui/src/tokens/mod.rs` - Design token definitions
- `/crates/nucleotide-ui/src/theme_manager.rs` - Helix theme mapping logic