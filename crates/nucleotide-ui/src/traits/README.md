# Component Trait System

The nucleotide-ui trait system provides consistent APIs and patterns across all components. It establishes a unified architecture for component creation, styling, interaction, and composition.

## Core Architecture

### Trait Hierarchy

1. **Component** - Base trait for component identity and lifecycle
2. **Styled** - Theme-aware styling with variants and sizes  
3. **Interactive** - Event handling and user interaction
4. **Composable** - Parent-child element relationships
5. **Extension Traits** - Specialized functionality (tooltips, validation, etc.)

## Core Traits

### Component Trait

Every nucleotide-ui component implements the `Component` trait for consistent identity and lifecycle management.

```rust
use nucleotide_ui::{Component, Button, ButtonVariant};

let button = Button::new("my-button", "Click me")
    .with_id("updated-id")
    .disabled(true);

assert_eq!(button.id().as_ref(), "updated-id");
assert!(button.is_disabled());
```

**Key Methods:**
- `id()` - Get component identifier
- `with_id()` - Set identifier (builder pattern)
- `is_disabled()` - Check disabled state
- `disabled()` - Set disabled state

### Styled Trait

Provides consistent theme integration and variant management.

```rust
use nucleotide_ui::{Styled, Button, ButtonVariant, ButtonSize};

let button = Button::new("btn", "Submit")
    .with_variant(ButtonVariant::Primary)
    .with_size(ButtonSize::Large);

assert_eq!(*button.variant(), ButtonVariant::Primary);
assert_eq!(*button.size(), ButtonSize::Large);

// Theme-aware styling
let theme = Theme::from_tokens(nucleotide_ui::DesignTokens::dark());
let styles = button.apply_theme_styling(&theme);
```

**Key Features:**
- Associated types for `Variant` and `Size`
- Builder pattern methods
- Automatic theme integration
- Style state computation (hover, active, disabled)

### Interactive Trait

Manages user interaction and event handling.

```rust
use nucleotide_ui::{Interactive, Button};

let button = Button::new("btn", "Click")
    .on_click(|event, window, cx| {
        println!("Button clicked!");
    })
    .on_secondary_click(|event, window, cx| {
        println!("Right-clicked!");
    });

assert!(button.is_focusable());
```

**Key Features:**
- Primary and secondary click handlers
- Focus management
- Type-safe event handling

## Extension Traits

### Tooltipped

Add tooltip support to any component.

```rust
use nucleotide_ui::{Tooltipped, Button};

let button = Button::new("btn", "Save")
    .tooltip("Save the current document");

assert_eq!(button.get_tooltip().unwrap(), "Save the current document");
```

### Composable

Support parent-child element relationships.

```rust
use nucleotide_ui::{Composable, div};

let container = div()
    .child("First child")
    .child("Second child")
    .children(vec!["Third", "Fourth"]);
```

### Slotted

Slot-based composition for flexible layouts.

```rust
use nucleotide_ui::{Slotted, ListItem, FileIcon};

let item = ListItem::new("item")
    .start_slot(FileIcon::new("folder"))
    .end_slot("badge")
    .child("Main content");
```

### Validatable

Support validation states and error messages.

```rust
use nucleotide_ui::{Validatable, ValidationState, TextInput};

let input = TextInput::new("email")
    .with_validation_state(ValidationState::Error("Invalid email".to_string()));

assert!(input.has_error());
assert_eq!(input.error_message(), Some("Invalid email"));
```

## Helper Traits

### ComponentFactory

Simplified component creation with common patterns.

```rust
use nucleotide_ui::{ComponentFactory, Button, ButtonVariant, ButtonSize};

// Standard creation
let button = Button::new("btn");

// With variant
let primary_button = Button::with_variant("btn", ButtonVariant::Primary);

// With size
let large_button = Button::with_size("btn", ButtonSize::Large);
```

### ThemedContext

Easy access to theme and design tokens from GPUI contexts.

```rust
use nucleotide_ui::{ThemedContext, DesignTokens};

// In component render method
fn render(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let tokens = cx.tokens();
    
    div()
        .bg(tokens.chrome.surface)
        .text_color(tokens.chrome.text_on_chrome)
        .p(tokens.sizes.space_3)
}
```

### KeyboardNavigable

Support keyboard navigation and accessibility.

```rust
use nucleotide_ui::{KeyboardNavigable, Button};

impl KeyboardNavigable for MyComponent {
    fn handle_key_event(&mut self, event: &KeyDownEvent) -> bool {
        match event.key {
            "Enter" | " " => {
                self.trigger_action();
                true // Event handled
            }
            _ => false // Event not handled
        }
    }
    
    fn tab_index(&self) -> Option<i32> {
        Some(0) // Include in tab navigation
    }
}
```

## Macro Helpers

### impl_component!

Automatically implement the `Component` trait for structs with `id` and `disabled` fields.

```rust
use nucleotide_ui::impl_component;

pub struct MyComponent {
    id: ElementId,
    disabled: bool,
    // ... other fields
}

impl_component!(MyComponent);

// Now MyComponent implements Component trait automatically
```

### impl_tooltipped!

Automatically implement the `Tooltipped` trait for structs with a `tooltip` field.

```rust
use nucleotide_ui::impl_tooltipped;

pub struct MyComponent {
    tooltip: Option<SharedString>,
    // ... other fields
}

impl_tooltipped!(MyComponent);

// Now MyComponent implements Tooltipped trait automatically
```

## Component States

### ComponentState Enum

Represents common component states that affect styling.

```rust
use nucleotide_ui::{ComponentState, compute_component_state};

let state = compute_component_state(
    false, // disabled
    false, // loading  
    true,  // focused
    false, // hovered
    false  // active
);

assert_eq!(state, ComponentState::Focused);
assert!(state.is_interactive());
assert!(!state.prevents_interaction());
```

**Available States:**
- `Default` - Normal state
- `Hover` - Mouse hovering
- `Active` - Being pressed/activated
- `Focused` - Has keyboard focus
- `Disabled` - Cannot be interacted with
- `Loading` - In loading state

## Styling Integration

### ComponentStyles

Computed styles based on theme and component state.

```rust
use nucleotide_ui::{ComponentStyles, Theme, ButtonVariant, ButtonSize};

let theme = Theme::from_tokens(nucleotide_ui::DesignTokens::dark());
let base_styles = ComponentStyles::from_theme(&theme, &ButtonVariant::Primary, &ButtonSize::Medium);

// Create state variants
let hover_styles = base_styles.hover_state(&theme);
let active_styles = base_styles.active_state(&theme);
let disabled_styles = base_styles.disabled_state(&theme);
```

**Style Properties:**
- `background` - Background color
- `text_color` - Text color
- `border_color` - Border color
- `padding` - Internal spacing
- `border_radius` - Corner rounding

## Usage Patterns

### Creating a New Component

```rust
use nucleotide_ui::{Component, Styled, Interactive, impl_component};
use gpui::{ElementId, IntoElement, RenderOnce};

#[derive(IntoElement)]
pub struct MyButton {
    id: ElementId,
    label: String,
    variant: MyButtonVariant,
    size: MyButtonSize,
    disabled: bool,
    on_click: Option<Box<dyn Fn() + 'static>>,
}

#[derive(Clone, Default)]
pub enum MyButtonVariant { #[default] Primary, Secondary }

#[derive(Clone, Default)]  
pub enum MyButtonSize { Small, #[default] Medium, Large }

impl_component!(MyButton);

impl Styled for MyButton {
    type Variant = MyButtonVariant;
    type Size = MyButtonSize;
    
    fn variant(&self) -> &Self::Variant { &self.variant }
    fn with_variant(mut self, variant: Self::Variant) -> Self {
        self.variant = variant;
        self
    }
    
    fn size(&self) -> &Self::Size { &self.size }
    fn with_size(mut self, size: Self::Size) -> Self {
        self.size = size;
        self
    }
}

impl Interactive for MyButton {
    type ClickHandler = Box<dyn Fn() + 'static>;
    
    fn on_click(mut self, handler: Self::ClickHandler) -> Self {
        self.on_click = Some(handler);
        self
    }
    
    fn on_secondary_click(self, _handler: Self::ClickHandler) -> Self {
        self // Not supported for this component
    }
}

impl RenderOnce for MyButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let styles = self.apply_theme_styling(cx.theme());
        
        div()
            .id(self.id.clone())
            .bg(styles.background)
            .text_color(styles.text_color)
            .border_1()
            .border_color(styles.border_color)
            .p(styles.padding)
            .rounded_px(styles.border_radius)
            .when(!self.disabled, |this| {
                this.hover(|this| {
                    let hover_styles = styles.hover_state(cx.theme());
                    this.bg(hover_styles.background)
                })
            })
            .child(self.label)
    }
}
```

### Using Components with Traits

```rust
use nucleotide_ui::*;

// Create component using trait methods
let button = Button::new("save-btn", "Save Document")
    .with_variant(ButtonVariant::Primary)
    .with_size(ButtonSize::Large)
    .disabled(false)
    .tooltip("Save the current document to disk")
    .on_click(|event, window, cx| {
        // Handle save action
    });

// Use in GPUI element tree
div()
    .child(button)
    .child(
        ListItem::new("item")
            .start_slot(FileIcon::new("document"))
            .child("Document.txt")
            .end_slot("Modified")
    )
```

## Migration Guide

### From Current Button Implementation

```rust
// Before (current implementation)
let button = Button::new("btn", "Click me")
    .variant(ButtonVariant::Primary)
    .size(ButtonSize::Large)
    .disabled(true)
    .tooltip("Click this button");

// After (with traits) - same API!
let button = Button::new("btn", "Click me")
    .with_variant(ButtonVariant::Primary)  // New method name
    .with_size(ButtonSize::Large)          // New method name
    .disabled(true)                        // Same
    .tooltip("Click this button");         // Same
```

**Key Changes:**
- `variant()` → `with_variant()` 
- `size()` → `with_size()`
- All other methods remain the same
- Additional trait methods available

## Benefits

### 1. Consistency
- Uniform APIs across all components
- Predictable method names and patterns
- Consistent builder pattern usage

### 2. Composability  
- Mix and match traits as needed
- Automatic implementation via macros
- Clean separation of concerns

### 3. Extensibility
- Easy to add new traits
- Components can opt into additional functionality
- Framework for future enhancements

### 4. Type Safety
- Associated types for variants and sizes
- Compile-time validation
- Clear error messages

### 5. Integration
- Seamless GPUI integration
- Theme system integration
- Design token compatibility

## Best Practices

### 1. Trait Implementation
```rust
// ✅ Good - use associated types
impl Styled for MyComponent {
    type Variant = MyVariant;
    type Size = MySize;
    // ...
}

// ❌ Avoid - generic parameters make usage complex
impl<V, S> Styled<V, S> for MyComponent { /* ... */ }
```

### 2. Builder Methods
```rust
// ✅ Good - consistent naming
.with_variant(ButtonVariant::Primary)
.with_size(ButtonSize::Large)

// ❌ Avoid - inconsistent naming
.set_variant(ButtonVariant::Primary)
.size(ButtonSize::Large)
```

### 3. Macro Usage
```rust
// ✅ Good - use macros for simple cases
impl_component!(MyButton);
impl_tooltipped!(MyButton);

// ❌ Avoid - manual implementation when macro suffices
impl Component for MyButton {
    // ... repetitive boilerplate
}
```

The trait system provides a solid foundation for consistent, composable, and extensible component development in nucleotide-ui.
