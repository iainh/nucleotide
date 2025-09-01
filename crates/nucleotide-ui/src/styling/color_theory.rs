// ABOUTME: Color theory utilities for intelligent color selection and contrast optimization
// ABOUTME: Provides WCAG-compliant contrast calculations and contextual color awareness

use crate::DesignTokens;
use core::f32::consts::PI;
use gpui::{Hsla, hsla};

/// WCAG contrast ratios for accessibility compliance
pub struct ContrastRatios;

impl ContrastRatios {
    pub const AAA_NORMAL: f32 = 7.0;
    pub const AA_NORMAL: f32 = 4.5;
    pub const AA_LARGE: f32 = 3.0;
    pub const MIN_READABLE: f32 = 2.0;
}

/// Color theory utilities for intelligent styling
pub struct ColorTheory;

impl ColorTheory {
    // ==========================
    // Linear sRGB companding
    // ==========================
    fn srgb_to_linear(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    fn linear_to_srgb(v: f32) -> f32 {
        if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    }

    // ==========================
    // RGB <-> HSL helpers
    // ==========================
    fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        let max = r.max(g.max(b));
        let min = r.min(g.min(b));
        let l = (max + min) * 0.5;
        if (max - min).abs() < 1e-6 {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if (max - r).abs() < 1e-6 {
            ((g - b) / d) % 6.0
        } else if (max - g).abs() < 1e-6 {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        // Normalize h to [0,1)
        let mut h = if h < 0.0 { h + 1.0 } else { h };
        if h >= 1.0 {
            h -= 1.0;
        }
        (h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0))
    }
    /// Calculate relative luminance for contrast calculations
    /// Based on WCAG 2.1 specification
    pub fn relative_luminance(color: Hsla) -> f32 {
        let (r, g, b) = Self::hsl_to_rgb(color.h, color.s, color.l);
        // Convert to linear RGB
        let r_linear = Self::srgb_to_linear(r);
        let g_linear = Self::srgb_to_linear(g);
        let b_linear = Self::srgb_to_linear(b);

        // Calculate luminance
        0.2126 * r_linear + 0.7152 * g_linear + 0.0722 * b_linear
    }

    /// Calculate contrast ratio between two colors
    pub fn contrast_ratio(color1: Hsla, color2: Hsla) -> f32 {
        let lum1 = Self::relative_luminance(color1);
        let lum2 = Self::relative_luminance(color2);

        let lighter = lum1.max(lum2);
        let darker = lum1.min(lum2);

        (lighter + 0.05) / (darker + 0.05)
    }

    /// Find the best text color for a given background
    pub fn best_text_color(background: Hsla, tokens: &DesignTokens) -> Hsla {
        // For transparent backgrounds, use primary text
        if background.a < 0.1 {
            return tokens.colors.text_primary;
        }

        // Build a candidate set that includes theme texts and OKLab-neutral extremes
        let oklab_white = Self::oklch_to_hsla(
            Oklch {
                L: 0.97,
                C: 0.0,
                h: 0.0,
            },
            1.0,
        );
        let oklab_black = Self::oklch_to_hsla(
            Oklch {
                L: 0.03,
                C: 0.0,
                h: 0.0,
            },
            1.0,
        );

        let candidates = [
            tokens.colors.text_primary,
            tokens.colors.text_secondary,
            tokens.colors.text_on_primary,
            oklab_white, // Perceptual near-white
            oklab_black, // Perceptual near-black
        ];

        let mut best_color = tokens.colors.text_primary;
        let mut best_contrast = 0.0;

        for &candidate in &candidates {
            let contrast = Self::contrast_ratio(background, candidate);
            if contrast > best_contrast {
                best_contrast = contrast;
                best_color = candidate;
            }
        }

        // If still not good enough, pick an OKLab neutral that meets or maximizes contrast
        if best_contrast < ContrastRatios::AA_NORMAL {
            let candidate = Self::oklab_high_contrast_text(background, ContrastRatios::AA_NORMAL);
            let cand_contrast = Self::contrast_ratio(background, candidate);
            if cand_contrast > best_contrast {
                candidate
            } else {
                best_color
            }
        } else {
            best_color
        }
    }

    /// Generate a high-contrast neutral text color using OKLab (neutral chroma, optimized L)
    fn oklab_high_contrast_text(background: Hsla, min_ratio: f32) -> Hsla {
        // Start with perceptual near-white and near-black neutrals
        let mut best = Self::oklch_to_hsla(
            Oklch {
                L: 0.97,
                C: 0.0,
                h: 0.0,
            },
            1.0,
        );
        let mut best_c = Self::contrast_ratio(background, best);
        let black = Self::oklch_to_hsla(
            Oklch {
                L: 0.03,
                C: 0.0,
                h: 0.0,
            },
            1.0,
        );
        let black_c = Self::contrast_ratio(background, black);
        if black_c > best_c {
            best = black;
            best_c = black_c;
        }

        // If not enough, search along OKLab L for a neutral that maximizes contrast
        if best_c < min_ratio {
            let mut local_best = best;
            let mut local_best_c = best_c;
            // Search with a few samples biased to extremes
            for i in 0..=12 {
                let l = i as f32 / 12.0; // 0..1
                let l = l.clamp(0.0, 1.0);
                let candidate = Self::oklch_to_hsla(
                    Oklch {
                        L: l,
                        C: 0.0,
                        h: 0.0,
                    },
                    1.0,
                );
                let c = Self::contrast_ratio(background, candidate);
                if c > local_best_c {
                    local_best = candidate;
                    local_best_c = c;
                }
                if local_best_c >= min_ratio {
                    break;
                }
            }
            local_best
        } else {
            best
        }
    }

    /// Get contextually appropriate colors based on surrounding UI
    pub fn contextual_colors(
        variant: &str,
        is_dark_theme: bool,
        context: crate::tokens::ColorContext,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        match context {
            crate::tokens::ColorContext::OnSurface => Self::on_surface_colors(variant, tokens),
            crate::tokens::ColorContext::OnPrimary => Self::on_primary_colors(variant, tokens),
            crate::tokens::ColorContext::Floating => {
                Self::floating_colors(variant, is_dark_theme, tokens)
            }
            crate::tokens::ColorContext::Overlay => {
                Self::overlay_colors(variant, is_dark_theme, tokens)
            }
        }
    }

    /// Colors for elements on surface backgrounds
    fn on_surface_colors(variant: &str, tokens: &DesignTokens) -> ContextualColors {
        let (background, foreground) = match variant {
            "primary" => (
                tokens.colors.primary,
                Self::best_text_color(tokens.colors.primary, tokens),
            ),
            "secondary" => (tokens.colors.surface, tokens.colors.text_primary),
            "ghost" => {
                // Ghost variant: transparent background, foreground based on surface
                let bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent
                let fg = Self::best_text_color(tokens.colors.surface, tokens);
                (bg, fg)
            }
            "danger" => (
                tokens.colors.error,
                Self::best_text_color(tokens.colors.error, tokens),
            ),
            "success" => (
                tokens.colors.success,
                Self::best_text_color(tokens.colors.success, tokens),
            ),
            "warning" => (
                tokens.colors.warning,
                Self::best_text_color(tokens.colors.warning, tokens),
            ),
            _ => (tokens.colors.surface, tokens.colors.text_primary),
        };

        let border = if variant == "ghost" {
            hsla(0.0, 0.0, 0.0, 0.0) // Transparent border for ghost
        } else {
            Self::subtle_border_color(background, tokens)
        };

        ContextualColors {
            background,
            foreground,
            border,
        }
    }

    /// Colors for elements on primary backgrounds
    fn on_primary_colors(variant: &str, tokens: &DesignTokens) -> ContextualColors {
        let background = match variant {
            "ghost" => hsla(0.0, 0.0, 1.0, 0.1), // Subtle overlay
            _ => Self::adjust_for_primary_context(tokens.colors.primary, tokens),
        };

        ContextualColors {
            background,
            foreground: tokens.colors.text_on_primary,
            border: Self::subtle_border_color(background, tokens),
        }
    }

    /// Colors for floating elements (modals, popups)
    fn floating_colors(
        variant: &str,
        is_dark_theme: bool,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        let base_bg = if is_dark_theme {
            tokens.colors.surface_elevated
        } else {
            tokens.colors.surface_overlay
        };

        let (background, foreground) = match variant {
            "ghost" => {
                // Ghost variant: transparent background, foreground based on floating surface
                let bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent
                let fg = Self::best_text_color(base_bg, tokens);
                (bg, fg)
            }
            _ => {
                let bg = base_bg;
                let fg = Self::best_text_color(bg, tokens);
                (bg, fg)
            }
        };

        let border = if variant == "ghost" {
            hsla(0.0, 0.0, 0.0, 0.0) // Transparent border for ghost
        } else {
            tokens.colors.border_default
        };

        ContextualColors {
            background,
            foreground,
            border,
        }
    }

    /// Colors for overlay elements
    fn overlay_colors(
        variant: &str,
        is_dark_theme: bool,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        let background = match variant {
            "primary" => tokens.colors.primary,
            "ghost" => hsla(0.0, 0.0, 0.0, 0.0),
            _ => {
                if is_dark_theme {
                    hsla(0.0, 0.0, 0.1, 0.95) // Dark overlay
                } else {
                    hsla(0.0, 0.0, 0.95, 0.95) // Light overlay
                }
            }
        };

        ContextualColors {
            background,
            foreground: Self::best_text_color(background, tokens),
            border: tokens.colors.border_default,
        }
    }

    /// Create a subtle border color that works with the background
    pub fn subtle_border_color(background: Hsla, _tokens: &DesignTokens) -> Hsla {
        let bg_luminance = Self::relative_luminance(background);

        // Aim for at least 3:1 contrast ratio for borders
        let mut border_color = if bg_luminance > 0.5 {
            // Light background - use darker border
            Self::darken(background, 0.15)
        } else {
            // Dark background - use lighter border
            Self::lighten(background, 0.15)
        };

        // Check contrast and adjust if needed
        let mut contrast = Self::contrast_ratio(background, border_color);
        let mut adjustment = 0.05;

        while contrast < 3.0 && adjustment < 0.5 {
            border_color = if bg_luminance > 0.5 {
                Self::darken(background, 0.15 + adjustment)
            } else {
                Self::lighten(background, 0.15 + adjustment)
            };
            contrast = Self::contrast_ratio(background, border_color);
            adjustment += 0.05;
        }

        border_color
    }

    /// Adjust color for primary context
    fn adjust_for_primary_context(primary: Hsla, tokens: &DesignTokens) -> Hsla {
        // Create a variant that works well on primary background
        Self::mix(primary, tokens.colors.surface, 0.15)
    }

    /// Create a lighter variant (perceptual OKLab L increase)
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        Self::adjust_oklab_lightness(color, amount)
    }

    /// Create a darker variant (perceptual OKLab L decrease)
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        Self::adjust_oklab_lightness(color, -amount)
    }

    /// Mix two colors
    pub fn mix(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
        let ratio = ratio.clamp(0.0, 1.0);
        hsla(
            color1.h + (color2.h - color1.h) * ratio,
            color1.s + (color2.s - color1.s) * ratio,
            color1.l + (color2.l - color1.l) * ratio,
            color1.a + (color2.a - color1.a) * ratio,
        )
    }

    // ==========================
    // OKLab / OKLCH conversions (D65, per OKLab definition)
    // Source: Wikipedia: Oklab color space — "Conversion from CIE XYZ" and OKLab/OKLCH formulas
    // Implements direct linear sRGB <-> OKLab transforms with cube-root nonlinearity.
    // ==========================

    pub fn hsla_to_oklab(color: Hsla) -> Oklab {
        let (r_srgb, g_srgb, b_srgb) = Self::hsl_to_rgb(color.h, color.s, color.l);
        let r = Self::srgb_to_linear(r_srgb);
        let g = Self::srgb_to_linear(g_srgb);
        let b = Self::srgb_to_linear(b_srgb);

        // Linear sRGB -> LMS (OKLab M1)
        let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
        let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
        let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

        // Nonlinearity (cube root)
        let l_ = l.cbrt();
        let m_ = m.cbrt();
        let s_ = s.cbrt();

        // LMS' -> OKLab (OKLab M2)
        let L = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
        let a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
        let b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

        Oklab { L, a, b }
    }

    pub fn oklab_to_hsla(lab: Oklab, alpha: f32) -> Hsla {
        // OKLab -> LMS'
        let l_ = lab.L + 0.3963377774 * lab.a + 0.2158037573 * lab.b;
        let m_ = lab.L - 0.1055613458 * lab.a - 0.0638541728 * lab.b;
        let s_ = lab.L - 0.0894841775 * lab.a - 1.2914855480 * lab.b;

        // Inverse nonlinearity
        let l = l_.powi(3);
        let m = m_.powi(3);
        let s = s_.powi(3);

        // LMS -> linear sRGB
        let r_lin = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g_lin = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let b_lin = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

        // Linear -> gamma sRGB
        let r = Self::linear_to_srgb(r_lin).clamp(0.0, 1.0);
        let g = Self::linear_to_srgb(g_lin).clamp(0.0, 1.0);
        let b = Self::linear_to_srgb(b_lin).clamp(0.0, 1.0);

        // sRGB -> HSL (gpui::Hsla uses HSL)
        let (h, s, l) = Self::rgb_to_hsl(r, g, b);
        hsla(h, s, l, alpha)
    }

    pub fn hsla_to_oklch(color: Hsla) -> Oklch {
        let lab = Self::hsla_to_oklab(color);
        let C = (lab.a * lab.a + lab.b * lab.b).sqrt();
        let mut h = lab.b.atan2(lab.a); // atan2(y=b, x=a)
        // Normalize hue to [0, 2π)
        if h < 0.0 {
            h += 2.0 * PI;
        }
        Oklch { L: lab.L, C, h }
    }

    pub fn oklch_to_hsla(lch: Oklch, alpha: f32) -> Hsla {
        let a = lch.C * lch.h.cos();
        let b = lch.C * lch.h.sin();
        Self::oklab_to_hsla(Oklab { L: lch.L, a, b }, alpha)
    }

    // Shortest-arc hue interpolation in OKLCH
    pub fn mix_oklch(a: Hsla, b: Hsla, t: f32) -> Hsla {
        let t = t.clamp(0.0, 1.0);
        let a_lch = Self::hsla_to_oklch(a);
        let b_lch = Self::hsla_to_oklch(b);

        let mut dh = b_lch.h - a_lch.h;
        if dh.abs() > PI {
            dh -= (2.0 * PI) * dh.signum();
        }

        let L = a_lch.L + (b_lch.L - a_lch.L) * t;
        let C = a_lch.C + (b_lch.C - a_lch.C) * t;
        let h = a_lch.h + dh * t;

        // Preserve alpha by linear interpolation
        let alpha = a.a + (b.a - a.a) * t;
        Self::oklch_to_hsla(Oklch { L, C, h }, alpha)
    }

    // Reduce chroma until the color fits sRGB gamut (linear), preserving hue and roughly L
    pub fn clamp_oklch_to_srgb_gamut(mut lch: Oklch) -> Oklch {
        for _ in 0..16 {
            let hsla = Self::oklch_to_hsla(lch, 1.0);
            // Convert to linear sRGB and check component bounds
            let (r, g, b) = Self::hsl_to_rgb(hsla.h, hsla.s, hsla.l);
            let (r, g, b) = (
                Self::srgb_to_linear(r),
                Self::srgb_to_linear(g),
                Self::srgb_to_linear(b),
            );
            if r >= 0.0 && r <= 1.0 && g >= 0.0 && g <= 1.0 && b >= 0.0 && b <= 1.0 {
                return lch;
            }
            lch.C *= 0.9; // iteratively reduce chroma
            if lch.C < 1e-4 {
                break;
            }
        }
        lch.C = lch.C.max(0.0);
        lch
    }

    // Adjust OKLab lightness by delta, keeping hue; moderates chroma if needed
    pub fn adjust_oklab_lightness(color: Hsla, delta_L: f32) -> Hsla {
        let mut lch = Self::hsla_to_oklch(color);
        lch.L = (lch.L + delta_L).clamp(0.0, 1.0);
        let lch = Self::clamp_oklch_to_srgb_gamut(lch);
        Self::oklch_to_hsla(lch, color.a)
    }

    /// Create a surface color variant by slightly adjusting lightness
    pub fn surface_variant(surface: Hsla, amount: f32) -> Hsla {
        // Determine if we should lighten or darken based on current lightness
        if surface.l > 0.5 {
            // Light theme - darken slightly for variants
            Self::darken(surface, amount)
        } else {
            // Dark theme - lighten slightly for variants
            Self::lighten(surface, amount)
        }
    }

    /// Convert HSL to RGB
    fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;

        let (r, g, b) = if h < 1.0 / 6.0 {
            (c, x, 0.0)
        } else if h < 2.0 / 6.0 {
            (x, c, 0.0)
        } else if h < 3.0 / 6.0 {
            (0.0, c, x)
        } else if h < 4.0 / 6.0 {
            (0.0, x, c)
        } else if h < 5.0 / 6.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };

        (r + m, g + m, b + m)
    }

    /// Create a completely transparent color
    pub fn transparent() -> Hsla {
        hsla(0.0, 0.0, 0.0, 0.0)
    }

    /// Create a color with specified alpha (preserving hue, saturation, lightness)
    pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
        hsla(color.h, color.s, color.l, alpha.clamp(0.0, 1.0))
    }

    /// Ensure text has sufficient contrast against background using OKLab-guided adjustments
    pub fn ensure_contrast(background: Hsla, text: Hsla, min_ratio: f32) -> Hsla {
        let mut best = text;
        let mut best_c = Self::contrast_ratio(background, text);
        if best_c >= min_ratio {
            return text;
        }

        // Try OKLab lightness adjustments on the provided text (preserve hue, reduce chroma a bit)
        let mut lch = Self::hsla_to_oklch(text);
        lch.C = (lch.C * 0.5).min(0.05); // neutralize chroma for readability
        // Determine direction based on background luminance
        let bg_is_dark = Self::relative_luminance(background) < 0.5;
        let search = if bg_is_dark {
            [0.6, 0.7, 0.8, 0.9, 0.97]
        } else {
            [0.4, 0.3, 0.2, 0.1, 0.03]
        };
        for &target_l in &search {
            let candidate = Self::oklch_to_hsla(
                Oklch {
                    L: target_l,
                    C: lch.C,
                    h: lch.h,
                },
                text.a,
            );
            let c = Self::contrast_ratio(background, candidate);
            if c > best_c {
                best = candidate;
                best_c = c;
            }
            if best_c >= min_ratio {
                return best;
            }
        }

        // Fallback to OKLab neutral search across L extremes
        Self::oklab_high_contrast_text(background, min_ratio)
    }

    /// Derive chrome colors from a surface color for UI components
    /// Returns computed colors based on surface brightness with WCAG contrast validation
    pub fn derive_chrome_colors(surface_color: Hsla) -> ChromeColors {
        let is_dark_theme = surface_color.l < 0.5;

        nucleotide_logging::debug!(
            surface_color = ?surface_color,
            surface_lightness = surface_color.l,
            is_dark_theme = is_dark_theme,
            "Computing chrome colors from surface color"
        );

        // Compute titlebar and footer backgrounds (darker/lighter than surface)
        let titlebar_background = if is_dark_theme {
            // Dark theme: make titlebar lighter than surface
            Self::lighten(surface_color, 0.12)
        } else {
            // Light theme: make titlebar darker than surface
            Self::darken(surface_color, 0.12)
        };

        // Footer uses same approach as titlebar for consistency
        let footer_background = titlebar_background;

        // File tree and tab backgrounds: subtle variation from surface
        let file_tree_background = if is_dark_theme {
            // Dark theme: slightly lighter than surface
            Self::lighten(surface_color, 0.05)
        } else {
            // Light theme: slightly darker than surface
            Self::darken(surface_color, 0.05)
        };

        // Tab empty areas use same approach as file tree
        let tab_empty_background = file_tree_background;

        // Compute separator color with proper contrast
        let separator_color = if is_dark_theme {
            // Dark theme: lighter separator with reduced saturation
            hsla(
                surface_color.h,
                surface_color.s * 0.3,
                (surface_color.l + 0.15).min(1.0),
                0.8,
            )
        } else {
            // Light theme: darker separator with reduced saturation
            hsla(
                surface_color.h,
                surface_color.s * 0.3,
                (surface_color.l - 0.15).max(0.0),
                0.8,
            )
        };

        let chrome_colors = ChromeColors {
            titlebar_background,
            footer_background,
            file_tree_background,
            tab_empty_background,
            separator_color,
        };

        // Validate contrast ratios for accessibility
        Self::validate_chrome_colors(&chrome_colors, surface_color);

        nucleotide_logging::info!(
            titlebar_bg = ?chrome_colors.titlebar_background,
            file_tree_bg = ?chrome_colors.file_tree_background,
            separator = ?chrome_colors.separator_color,
            theme_type = if is_dark_theme { "dark" } else { "light" },
            "Chrome colors computed successfully"
        );

        chrome_colors
    }

    /// Validate that chrome colors meet accessibility standards
    fn validate_chrome_colors(chrome_colors: &ChromeColors, surface_color: Hsla) {
        // Chrome backgrounds need lower contrast than text (1.2:1 minimum for visual distinction)
        const CHROME_MIN_CONTRAST: f32 = 1.2;

        let validations = [
            ("titlebar_background", chrome_colors.titlebar_background),
            ("footer_background", chrome_colors.footer_background),
            ("file_tree_background", chrome_colors.file_tree_background),
            ("tab_empty_background", chrome_colors.tab_empty_background),
        ];

        for (name, color) in &validations {
            let contrast = Self::contrast_ratio(surface_color, *color);

            if contrast < CHROME_MIN_CONTRAST {
                nucleotide_logging::warn!(
                    color_name = name,
                    contrast_ratio = contrast,
                    min_required = CHROME_MIN_CONTRAST,
                    surface_color = ?surface_color,
                    chrome_color = ?color,
                    "Chrome color contrast below minimum visual distinction threshold"
                );
            } else {
                nucleotide_logging::debug!(
                    color_name = name,
                    contrast_ratio = contrast,
                    "Chrome color contrast validation passed"
                );
            }
        }
    }
}

/// Computed chrome colors for UI components based on surface color
#[derive(Debug, Clone, Copy)]
pub struct ChromeColors {
    /// Titlebar background color (darker/lighter than surface)
    pub titlebar_background: Hsla,
    /// Footer background color (consistent with titlebar)
    pub footer_background: Hsla,
    /// File tree background color (subtle variation from surface)
    pub file_tree_background: Hsla,
    /// Tab bar empty areas background (consistent with file tree)
    pub tab_empty_background: Hsla,
    /// Separator color with proper contrast
    pub separator_color: Hsla,
}

impl ChromeColors {
    /// Create ChromeColors with all colors set to a single color (for testing)
    pub fn uniform(color: Hsla) -> Self {
        Self {
            titlebar_background: color,
            footer_background: color,
            file_tree_background: color,
            tab_empty_background: color,
            separator_color: color,
        }
    }
}

/// Contextually computed colors
#[derive(Debug, Clone, Copy)]
pub struct ContextualColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
}

/// OKLab color (L, a, b)
#[derive(Debug, Clone, Copy)]
pub struct Oklab {
    pub L: f32,
    pub a: f32,
    pub b: f32,
}

/// OKLCH color (L, C, h [radians])
#[derive(Debug, Clone, Copy)]
pub struct Oklch {
    pub L: f32,
    pub C: f32,
    pub h: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DesignTokens;

    #[test]
    fn test_contrast_calculation() {
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        let black = hsla(0.0, 0.0, 0.0, 1.0);

        let contrast = ColorTheory::contrast_ratio(white, black);
        assert!(contrast > 20.0); // Should be ~21:1
    }

    #[test]
    fn test_best_text_color() {
        let tokens = DesignTokens::light();

        // Test on light background
        let light_bg = hsla(0.0, 0.0, 0.9, 1.0);
        let text_color = ColorTheory::best_text_color(light_bg, &tokens);
        let contrast = ColorTheory::contrast_ratio(light_bg, text_color);
        assert!(contrast >= ContrastRatios::AA_NORMAL);

        // Test on dark background
        let dark_bg = hsla(0.0, 0.0, 0.1, 1.0);
        let text_color = ColorTheory::best_text_color(dark_bg, &tokens);
        let contrast = ColorTheory::contrast_ratio(dark_bg, text_color);
        assert!(contrast >= ContrastRatios::AA_NORMAL);
    }

    #[test]
    fn test_chrome_colors_light_theme() {
        let light_surface = hsla(0.0, 0.0, 0.95, 1.0); // Light surface
        let chrome_colors = ColorTheory::derive_chrome_colors(light_surface);

        // In light theme, chrome colors should be darker than surface
        assert!(chrome_colors.titlebar_background.l < light_surface.l);
        assert!(chrome_colors.footer_background.l < light_surface.l);
        assert!(chrome_colors.file_tree_background.l < light_surface.l);
        assert!(chrome_colors.tab_empty_background.l < light_surface.l);

        // Titlebar and footer should be consistent
        assert_eq!(
            chrome_colors.titlebar_background,
            chrome_colors.footer_background
        );

        // File tree and tabs should be consistent
        assert_eq!(
            chrome_colors.file_tree_background,
            chrome_colors.tab_empty_background
        );

        // Validate some contrast ratios - chrome colors need less contrast than text
        let titlebar_contrast =
            ColorTheory::contrast_ratio(light_surface, chrome_colors.titlebar_background);
        // Chrome backgrounds need at least 1.2:1 contrast to be visually distinguishable
        assert!(titlebar_contrast >= 1.2);
    }

    #[test]
    fn test_chrome_colors_dark_theme() {
        let dark_surface = hsla(0.0, 0.0, 0.1, 1.0); // Dark surface
        let chrome_colors = ColorTheory::derive_chrome_colors(dark_surface);

        // In dark theme, chrome colors should be lighter than surface
        assert!(chrome_colors.titlebar_background.l > dark_surface.l);
        assert!(chrome_colors.footer_background.l > dark_surface.l);
        assert!(chrome_colors.file_tree_background.l > dark_surface.l);
        assert!(chrome_colors.tab_empty_background.l > dark_surface.l);

        // Titlebar and footer should be consistent
        assert_eq!(
            chrome_colors.titlebar_background,
            chrome_colors.footer_background
        );

        // File tree and tabs should be consistent
        assert_eq!(
            chrome_colors.file_tree_background,
            chrome_colors.tab_empty_background
        );

        // Validate some contrast ratios - chrome colors need less contrast than text
        let titlebar_contrast =
            ColorTheory::contrast_ratio(dark_surface, chrome_colors.titlebar_background);
        // Chrome backgrounds need at least 1.2:1 contrast to be visually distinguishable
        assert!(titlebar_contrast >= 1.2);
    }

    #[test]
    fn test_chrome_colors_preserve_hue() {
        // Test with a blue surface to ensure hue is preserved
        let blue_surface = hsla(240.0 / 360.0, 0.3, 0.5, 1.0); // Blue surface
        let chrome_colors = ColorTheory::derive_chrome_colors(blue_surface);

        // All chrome colors should preserve the blue hue
        assert!((chrome_colors.titlebar_background.h - blue_surface.h).abs() < 0.01);
        assert!((chrome_colors.file_tree_background.h - blue_surface.h).abs() < 0.01);

        // Separator may have different hue due to saturation reduction, but should be similar
        let hue_diff = (chrome_colors.separator_color.h - blue_surface.h).abs();
        assert!(hue_diff < 0.1 || hue_diff > (1.0 - 0.1)); // Allow for hue wrap-around
    }

    #[test]
    fn test_chrome_colors_uniform() {
        let test_color = hsla(120.0 / 360.0, 0.5, 0.6, 1.0);
        let uniform_colors = ChromeColors::uniform(test_color);

        assert_eq!(uniform_colors.titlebar_background, test_color);
        assert_eq!(uniform_colors.footer_background, test_color);
        assert_eq!(uniform_colors.file_tree_background, test_color);
        assert_eq!(uniform_colors.tab_empty_background, test_color);
        assert_eq!(uniform_colors.separator_color, test_color);
    }

    #[test]
    fn test_oklab_oklch_roundtrip() {
        let samples = [
            hsla(0.0, 0.0, 0.0, 1.0),
            hsla(0.0, 0.0, 1.0, 1.0),
            hsla(210.0 / 360.0, 0.5, 0.5, 1.0),
            hsla(0.33, 0.8, 0.4, 1.0),
            hsla(0.85, 0.6, 0.7, 1.0),
        ];
        for c in samples {
            let lch = ColorTheory::hsla_to_oklch(c);
            let c2 = ColorTheory::oklch_to_hsla(lch, c.a);
            // Compare in sRGB space for stability
            let (r1, g1, b1) = ColorTheory::hsl_to_rgb(c.h, c.s, c.l);
            let (r2, g2, b2) = ColorTheory::hsl_to_rgb(c2.h, c2.s, c2.l);
            assert!((r1 - r2).abs() < 1e-3, "r mismatch: {} vs {}", r1, r2);
            assert!((g1 - g2).abs() < 1e-3, "g mismatch: {} vs {}", g1, g2);
            assert!((b1 - b2).abs() < 1e-3, "b mismatch: {} vs {}", b1, b2);
        }
    }

    #[test]
    fn test_oklch_polar_conversion_relations() {
        let lch = Oklch {
            L: 0.6,
            C: 0.1,
            h: 1.2,
        };
        let lab = ColorTheory::hsla_to_oklab(ColorTheory::oklch_to_hsla(lch, 1.0));
        let a_expected = lch.C * lch.h.cos();
        let b_expected = lch.C * lch.h.sin();
        assert!((lab.a - a_expected).abs() < 2e-3);
        assert!((lab.b - b_expected).abs() < 2e-3);
    }

    #[test]
    fn test_mix_oklch_monotonic_lightness() {
        let c1 = hsla(220.0 / 360.0, 0.8, 0.3, 1.0);
        let c2 = hsla(220.0 / 360.0, 0.8, 0.8, 1.0);
        let mut last_L = None;
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let m = ColorTheory::mix_oklch(c1, c2, t);
            let lch = ColorTheory::hsla_to_oklch(m);
            if let Some(prev) = last_L {
                assert!(lch.L >= prev - 1e-5);
            }
            last_L = Some(lch.L);
        }
    }
}
