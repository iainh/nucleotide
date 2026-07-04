// ABOUTME: Shared input styling options
// ABOUTME: Interactive text fields are implemented by text_input::TextInput

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum InputVariant {
    #[default]
    Default,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum InputSize {
    Small,
    #[default]
    Medium,
    Large,
}
