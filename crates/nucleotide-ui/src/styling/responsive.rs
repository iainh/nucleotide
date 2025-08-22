// ABOUTME: Responsive design system for nucleotide-ui components
// ABOUTME: Provides breakpoints, responsive tokens, and adaptive styling

use gpui::{Pixels, Size, px};
use std::collections::HashMap;

/// Standard responsive breakpoints
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Breakpoint {
    ExtraSmall, // < 640px
    Small,      // >= 640px
    Medium,     // >= 768px
    Large,      // >= 1024px
    ExtraLarge, // >= 1280px
    XXLarge,    // >= 1536px
}

impl Breakpoint {
    /// Get the minimum width for this breakpoint
    pub fn min_width(self) -> Pixels {
        match self {
            Self::ExtraSmall => px(0.0),
            Self::Small => px(640.0),
            Self::Medium => px(768.0),
            Self::Large => px(1024.0),
            Self::ExtraLarge => px(1280.0),
            Self::XXLarge => px(1536.0),
        }
    }

    /// Get the maximum width for this breakpoint (exclusive)
    pub fn max_width(self) -> Option<Pixels> {
        match self {
            Self::ExtraSmall => Some(px(640.0)),
            Self::Small => Some(px(768.0)),
            Self::Medium => Some(px(1024.0)),
            Self::Large => Some(px(1280.0)),
            Self::ExtraLarge => Some(px(1536.0)),
            Self::XXLarge => None, // No upper limit
        }
    }

    /// Get breakpoint from viewport width
    pub fn from_width(width: Pixels) -> Self {
        if width.0 >= 1536.0 {
            Self::XXLarge
        } else if width.0 >= 1280.0 {
            Self::ExtraLarge
        } else if width.0 >= 1024.0 {
            Self::Large
        } else if width.0 >= 768.0 {
            Self::Medium
        } else if width.0 >= 640.0 {
            Self::Small
        } else {
            Self::ExtraSmall
        }
    }

    /// Get short identifier for this breakpoint
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExtraSmall => "xs",
            Self::Small => "sm",
            Self::Medium => "md",
            Self::Large => "lg",
            Self::ExtraLarge => "xl",
            Self::XXLarge => "2xl",
        }
    }

    /// Get all breakpoints in order
    pub fn all() -> &'static [Breakpoint] {
        &[
            Self::ExtraSmall,
            Self::Small,
            Self::Medium,
            Self::Large,
            Self::ExtraLarge,
            Self::XXLarge,
        ]
    }
}

/// Responsive value that changes based on breakpoint
#[derive(Debug, Clone)]
pub struct ResponsiveValue<T> {
    values: HashMap<Breakpoint, T>,
    default: T,
}

impl<T: Clone> ResponsiveValue<T> {
    /// Create a new responsive value with a default
    pub fn new(default: T) -> Self {
        Self {
            values: HashMap::new(),
            default,
        }
    }

    /// Set value for a specific breakpoint
    pub fn set(mut self, breakpoint: Breakpoint, value: T) -> Self {
        self.values.insert(breakpoint, value);
        self
    }

    /// Get value for current breakpoint
    pub fn get(&self, current_breakpoint: Breakpoint) -> &T {
        // Find the highest breakpoint that's <= current_breakpoint
        let mut best_match = None;
        let mut best_breakpoint = None;

        for (breakpoint, value) in &self.values {
            if *breakpoint <= current_breakpoint
                && (best_breakpoint.is_none() || *breakpoint > best_breakpoint.unwrap())
            {
                best_match = Some(value);
                best_breakpoint = Some(*breakpoint);
            }
        }

        best_match.unwrap_or(&self.default)
    }

    /// Create from breakpoint-value pairs
    pub fn from_pairs(default: T, pairs: &[(Breakpoint, T)]) -> Self
    where
        T: Clone,
    {
        let mut responsive = Self::new(default);
        for (breakpoint, value) in pairs {
            responsive = responsive.set(*breakpoint, value.clone());
        }
        responsive
    }
}

/// Responsive sizing tokens
#[derive(Debug, Clone)]
pub struct ResponsiveSizes {
    pub space_1: ResponsiveValue<Pixels>,
    pub space_2: ResponsiveValue<Pixels>,
    pub space_3: ResponsiveValue<Pixels>,
    pub space_4: ResponsiveValue<Pixels>,
    pub space_5: ResponsiveValue<Pixels>,
    pub space_6: ResponsiveValue<Pixels>,
    pub space_8: ResponsiveValue<Pixels>,
    pub space_10: ResponsiveValue<Pixels>,
    pub space_12: ResponsiveValue<Pixels>,
    pub space_16: ResponsiveValue<Pixels>,
}

impl ResponsiveSizes {
    /// Create responsive sizes with mobile-first approach
    pub fn mobile_first() -> Self {
        Self {
            space_1: ResponsiveValue::from_pairs(
                px(4.0),
                &[(Breakpoint::Medium, px(4.0)), (Breakpoint::Large, px(6.0))],
            ),
            space_2: ResponsiveValue::from_pairs(
                px(8.0),
                &[(Breakpoint::Medium, px(8.0)), (Breakpoint::Large, px(10.0))],
            ),
            space_3: ResponsiveValue::from_pairs(
                px(12.0),
                &[
                    (Breakpoint::Medium, px(12.0)),
                    (Breakpoint::Large, px(16.0)),
                ],
            ),
            space_4: ResponsiveValue::from_pairs(
                px(16.0),
                &[
                    (Breakpoint::Medium, px(16.0)),
                    (Breakpoint::Large, px(20.0)),
                ],
            ),
            space_5: ResponsiveValue::from_pairs(
                px(20.0),
                &[
                    (Breakpoint::Medium, px(20.0)),
                    (Breakpoint::Large, px(24.0)),
                ],
            ),
            space_6: ResponsiveValue::from_pairs(
                px(24.0),
                &[
                    (Breakpoint::Medium, px(24.0)),
                    (Breakpoint::Large, px(32.0)),
                ],
            ),
            space_8: ResponsiveValue::from_pairs(
                px(32.0),
                &[
                    (Breakpoint::Medium, px(32.0)),
                    (Breakpoint::Large, px(40.0)),
                ],
            ),
            space_10: ResponsiveValue::from_pairs(
                px(40.0),
                &[
                    (Breakpoint::Medium, px(40.0)),
                    (Breakpoint::Large, px(48.0)),
                ],
            ),
            space_12: ResponsiveValue::from_pairs(
                px(48.0),
                &[
                    (Breakpoint::Medium, px(48.0)),
                    (Breakpoint::Large, px(64.0)),
                ],
            ),
            space_16: ResponsiveValue::from_pairs(
                px(64.0),
                &[
                    (Breakpoint::Medium, px(64.0)),
                    (Breakpoint::Large, px(80.0)),
                ],
            ),
        }
    }

    /// Create responsive sizes optimized for desktop-first
    pub fn desktop_first() -> Self {
        Self {
            space_1: ResponsiveValue::from_pairs(
                px(6.0),
                &[
                    (Breakpoint::ExtraSmall, px(4.0)),
                    (Breakpoint::Small, px(4.0)),
                ],
            ),
            space_2: ResponsiveValue::from_pairs(
                px(10.0),
                &[
                    (Breakpoint::ExtraSmall, px(8.0)),
                    (Breakpoint::Small, px(8.0)),
                ],
            ),
            space_3: ResponsiveValue::from_pairs(
                px(16.0),
                &[
                    (Breakpoint::ExtraSmall, px(12.0)),
                    (Breakpoint::Small, px(12.0)),
                ],
            ),
            space_4: ResponsiveValue::from_pairs(
                px(20.0),
                &[
                    (Breakpoint::ExtraSmall, px(16.0)),
                    (Breakpoint::Small, px(16.0)),
                ],
            ),
            space_5: ResponsiveValue::from_pairs(
                px(24.0),
                &[
                    (Breakpoint::ExtraSmall, px(20.0)),
                    (Breakpoint::Small, px(20.0)),
                ],
            ),
            space_6: ResponsiveValue::from_pairs(
                px(32.0),
                &[
                    (Breakpoint::ExtraSmall, px(24.0)),
                    (Breakpoint::Small, px(24.0)),
                ],
            ),
            space_8: ResponsiveValue::from_pairs(
                px(40.0),
                &[
                    (Breakpoint::ExtraSmall, px(32.0)),
                    (Breakpoint::Small, px(32.0)),
                ],
            ),
            space_10: ResponsiveValue::from_pairs(
                px(48.0),
                &[
                    (Breakpoint::ExtraSmall, px(40.0)),
                    (Breakpoint::Small, px(40.0)),
                ],
            ),
            space_12: ResponsiveValue::from_pairs(
                px(64.0),
                &[
                    (Breakpoint::ExtraSmall, px(48.0)),
                    (Breakpoint::Small, px(48.0)),
                ],
            ),
            space_16: ResponsiveValue::from_pairs(
                px(80.0),
                &[
                    (Breakpoint::ExtraSmall, px(64.0)),
                    (Breakpoint::Small, px(64.0)),
                ],
            ),
        }
    }
}

/// Responsive typography tokens
#[derive(Debug, Clone)]
pub struct ResponsiveTypography {
    pub text_xs: ResponsiveValue<Pixels>,
    pub text_sm: ResponsiveValue<Pixels>,
    pub text_base: ResponsiveValue<Pixels>,
    pub text_lg: ResponsiveValue<Pixels>,
    pub text_xl: ResponsiveValue<Pixels>,
    pub text_2xl: ResponsiveValue<Pixels>,
    pub text_3xl: ResponsiveValue<Pixels>,
}

impl ResponsiveTypography {
    /// Create responsive typography with mobile-first scaling
    pub fn mobile_first() -> Self {
        Self {
            text_xs: ResponsiveValue::from_pairs(px(12.0), &[(Breakpoint::Large, px(12.0))]),
            text_sm: ResponsiveValue::from_pairs(px(14.0), &[(Breakpoint::Large, px(14.0))]),
            text_base: ResponsiveValue::from_pairs(px(16.0), &[(Breakpoint::Large, px(16.0))]),
            text_lg: ResponsiveValue::from_pairs(px(18.0), &[(Breakpoint::Large, px(20.0))]),
            text_xl: ResponsiveValue::from_pairs(px(20.0), &[(Breakpoint::Large, px(24.0))]),
            text_2xl: ResponsiveValue::from_pairs(px(24.0), &[(Breakpoint::Large, px(30.0))]),
            text_3xl: ResponsiveValue::from_pairs(px(30.0), &[(Breakpoint::Large, px(36.0))]),
        }
    }
}

/// Viewport context for responsive calculations
#[derive(Debug, Clone)]
pub struct ViewportContext {
    pub size: Size<Pixels>,
    pub breakpoint: Breakpoint,
    pub is_mobile: bool,
    pub is_tablet: bool,
    pub is_desktop: bool,
}

impl ViewportContext {
    /// Create viewport context from size
    pub fn from_size(size: Size<Pixels>) -> Self {
        let breakpoint = Breakpoint::from_width(size.width);

        Self {
            size,
            breakpoint,
            is_mobile: matches!(breakpoint, Breakpoint::ExtraSmall),
            is_tablet: matches!(breakpoint, Breakpoint::Small | Breakpoint::Medium),
            is_desktop: matches!(
                breakpoint,
                Breakpoint::Large | Breakpoint::ExtraLarge | Breakpoint::XXLarge
            ),
        }
    }

    /// Check if viewport matches a breakpoint condition
    pub fn matches_breakpoint(&self, condition: BreakpointCondition) -> bool {
        match condition {
            BreakpointCondition::Exact(bp) => self.breakpoint == bp,
            BreakpointCondition::Min(bp) => self.breakpoint >= bp,
            BreakpointCondition::Max(bp) => self.breakpoint <= bp,
            BreakpointCondition::Range(min, max) => {
                self.breakpoint >= min && self.breakpoint <= max
            }
        }
    }
}

/// Breakpoint matching conditions
#[derive(Debug, Clone, Copy)]
pub enum BreakpointCondition {
    Exact(Breakpoint),
    Min(Breakpoint),
    Max(Breakpoint),
    Range(Breakpoint, Breakpoint),
}

/// Responsive style utilities
pub struct ResponsiveStyler;

impl ResponsiveStyler {
    /// Get responsive spacing value
    pub fn spacing(
        &self,
        sizes: &ResponsiveSizes,
        space_key: &str,
        viewport: &ViewportContext,
    ) -> Pixels {
        match space_key {
            "1" => *sizes.space_1.get(viewport.breakpoint),
            "2" => *sizes.space_2.get(viewport.breakpoint),
            "3" => *sizes.space_3.get(viewport.breakpoint),
            "4" => *sizes.space_4.get(viewport.breakpoint),
            "5" => *sizes.space_5.get(viewport.breakpoint),
            "6" => *sizes.space_6.get(viewport.breakpoint),
            "8" => *sizes.space_8.get(viewport.breakpoint),
            "10" => *sizes.space_10.get(viewport.breakpoint),
            "12" => *sizes.space_12.get(viewport.breakpoint),
            "16" => *sizes.space_16.get(viewport.breakpoint),
            _ => px(8.0), // Default fallback
        }
    }

    /// Get responsive typography value
    pub fn typography(
        &self,
        typography: &ResponsiveTypography,
        text_key: &str,
        viewport: &ViewportContext,
    ) -> Pixels {
        match text_key {
            "xs" => *typography.text_xs.get(viewport.breakpoint),
            "sm" => *typography.text_sm.get(viewport.breakpoint),
            "base" => *typography.text_base.get(viewport.breakpoint),
            "lg" => *typography.text_lg.get(viewport.breakpoint),
            "xl" => *typography.text_xl.get(viewport.breakpoint),
            "2xl" => *typography.text_2xl.get(viewport.breakpoint),
            "3xl" => *typography.text_3xl.get(viewport.breakpoint),
            _ => px(16.0), // Default fallback
        }
    }

    /// Check if mobile-optimized styles should be used
    pub fn should_use_mobile_styles(&self, viewport: &ViewportContext) -> bool {
        viewport.is_mobile
    }

    /// Check if touch-friendly sizing should be used
    pub fn should_use_touch_sizing(&self, viewport: &ViewportContext) -> bool {
        viewport.is_mobile || viewport.is_tablet
    }
}
