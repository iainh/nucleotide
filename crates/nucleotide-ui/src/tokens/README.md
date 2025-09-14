# Design Token System

The nucleotide-ui design token system provides a structured, semantic approach to styling components. It replaces hardcoded color and spacing values with a systematic design language.

## Architecture

### Three-Layer Token System

1. **Base Colors** - Raw color definitions (neutral-50 to neutral-950, primary-50 to primary-900, etc.)
2. **Semantic Colors** - Meaningful names for UI elements (background, surface, text-primary, etc.)
3. **Component Tokens** - Ready-to-use values for specific components

### Size Tokens

Consistent spacing and sizing values:
- **Spacing Scale**: `space_0` (0px) to `space_10` (48px)
- **Component Sizes**: Button heights, border radius, font sizes
- **Progressive Scale**: Each step builds on the previous for visual harmony

## Usage Examples

### Basic Usage

```rust
use nucleotide_ui::{DesignTokens, Theme};

// Create theme with design tokens
let tokens = DesignTokens::dark();
let theme = Theme::from_tokens(tokens);

// Access colors semantically
let background = theme.tokens.editor.background;
let primary_text = theme.tokens.editor.text_primary;
let button_color = theme.tokens.chrome.primary;

// Use spacing tokens
let padding = theme.tokens.sizes.space_3; // 8px
let button_height = theme.tokens.sizes.button_height_md; // 36px
```

### Component Styling

```rust
// In your component render method
div()
    .bg(theme.tokens.chrome.surface)
    .text_color(theme.tokens.chrome.text_on_chrome)
    .p(theme.tokens.sizes.space_3)
    .rounded_px(theme.tokens.sizes.radius_md)
    .border_1()
    .border_color(theme.tokens.chrome.border_default)
```

### Color Utilities

```rust
use nucleotide_ui::tokens::{lighten, darken, with_alpha, mix};

let base_color = theme.tokens.chrome.primary;

// Create variations
let lighter = lighten(base_color, 0.1);
let darker = darken(base_color, 0.1);
let transparent = with_alpha(base_color, 0.8);

// Mix colors
let mixed = mix(color1, color2, 0.5); // 50% blend
```

### Surface Elevation

```rust
// Get appropriate surface colors for different elevations
let base_surface = theme.surface_at_elevation(0);      // Background (editor)
let card_surface = theme.surface_at_elevation(1);      // Surface
let modal_surface = theme.surface_at_elevation(2);     // Elevated surface
let tooltip_surface = theme.surface_at_elevation(3);   // Higher elevation
```

## Migration from Hardcoded Values

### Before (Hardcoded)
```rust
.bg(hsla(0.0, 0.0, 0.1, 1.0))
.text_color(hsla(0.0, 0.0, 0.9, 1.0))
.p(px(8.0))
```

### After (Design Tokens)
```rust
.bg(theme.tokens.chrome.surface)
.text_color(theme.tokens.chrome.text_on_chrome)
.p(theme.tokens.sizes.space_3)
```

### Creating Themes

Use tokens explicitly for clarity and consistency:
```rust
let theme = Theme::from_tokens(DesignTokens::dark());
.bg(theme.tokens.editor.background)
.text_color(theme.tokens.chrome.text_on_chrome)
.border_color(theme.tokens.chrome.border_default)
```

## Color System

### Semantic Color Names

- **Surfaces**: `background`, `surface`, `surface_elevated`, `surface_overlay`
- **Interactive**: `surface_hover`, `surface_active`, `surface_selected`
- **Text**: `text_primary`, `text_secondary`, `text_tertiary`, `text_disabled`
- **Borders**: `border_default`, `border_muted`, `border_strong`, `border_focus`
- **Brand**: `primary`, `primary_hover`, `primary_active`
- **Feedback**: `success`, `warning`, `error`, `info`

### Light vs Dark Themes

The system automatically provides appropriate colors for both themes:

```rust
let light_tokens = DesignTokens::light();
let dark_tokens = DesignTokens::dark();

// Same semantic meaning, different values
light_tokens.editor.text_primary; // Dark text for light background
dark_tokens.editor.text_primary;  // Light text for dark background
```

## Size System

### Spacing Scale (8px base unit)
- `space_0`: 0px
- `space_1`: 2px (0.25 × base)
- `space_2`: 4px (0.5 × base)
- `space_3`: 8px (1 × base)
- `space_4`: 12px (1.5 × base)
- `space_5`: 16px (2 × base)
- `space_6`: 20px (2.5 × base)
- `space_7`: 24px (3 × base)
- `space_8`: 32px (4 × base)
- `space_9`: 40px (5 × base)
- `space_10`: 48px (6 × base)

### Component Sizes
- **Buttons**: `button_height_sm` (28px), `button_height_md` (36px), `button_height_lg` (44px)
- **Border Radius**: `radius_sm` (4px), `radius_md` (6px), `radius_lg` (8px), `radius_full` (9999px)
- **Typography**: `text_xs` (11px), `text_sm` (12px), `text_md` (14px), `text_lg` (16px), `text_xl` (18px)

## Best Practices

### 1. Use Semantic Names
```rust
// ✅ Good - semantic meaning
.text_color(theme.tokens.chrome.text_on_chrome)

// ❌ Avoid - raw values
.text_color(hsla(0.0, 0.0, 0.9, 1.0))
```

### 2. Use the Spacing Scale
```rust
// ✅ Good - consistent spacing
.p(theme.tokens.sizes.space_3)
.gap(theme.tokens.sizes.space_2)

// ❌ Avoid - arbitrary values
.p(px(7.0))
.gap(px(3.0))
```

### 3. Leverage Color Utilities
```rust
// ✅ Good - systematic color variations
let hover_color = lighten(theme.tokens.chrome.primary, 0.1);

// ❌ Avoid - manual color calculation
let hover_color = hsla(primary.h, primary.s, primary.l + 0.1, primary.a);
```

### 4. Use Surface Elevation
```rust
// ✅ Good - semantic elevation
.bg(theme.surface_at_elevation(1))

// ❌ Avoid - hardcoded surface variants
.bg(theme.surface_background)
```

## Testing

Run the comprehensive test suite:
```bash
cargo test -p nucleotide-ui --lib tokens
```

Tests cover:
- Token consistency across themes
- Color utility functions
- Theme integration
- Backward compatibility
- Color validation
- Surface elevation logic

## Future Enhancements

- **Animation Tokens**: Duration and easing curves
- **Typography Tokens**: Font families, weights, line heights
- **Shadow Tokens**: Elevation-based shadows
- **Responsive Tokens**: Breakpoint-aware values
- **Custom Theme Generation**: Tools for creating brand-specific themes
