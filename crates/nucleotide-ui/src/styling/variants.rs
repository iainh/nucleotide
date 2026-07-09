/// Standard component variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Success,
    Warning,
    Info,
    Accent,
}

impl StyleVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Ghost => "ghost",
            Self::Danger => "danger",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Accent => "accent",
        }
    }
}

/// Standard component sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleSize {
    ExtraSmall,
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl StyleSize {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExtraSmall => "xs",
            Self::Small => "sm",
            Self::Medium => "md",
            Self::Large => "lg",
            Self::ExtraLarge => "xl",
        }
    }
}
