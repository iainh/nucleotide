// ABOUTME: Common UI utilities and helper functions for nucleotide-ui components
// ABOUTME: Provides layout, styling, and interaction utilities for component development

use gpui::{ElementId, Hsla, Pixels, Point, SharedString, Size, px};
use std::time::Duration;

/// Layout helper utilities
pub struct LayoutHelpers;

impl LayoutHelpers {
    /// Calculate responsive size based on available space
    pub fn responsive_size(base_size: Pixels, available_space: Pixels, factor: f32) -> Pixels {
        let computed = base_size.0 * factor;
        px(computed.min(available_space.0).max(base_size.0 * 0.5))
    }

    /// Calculate grid dimensions for a given number of items
    pub fn calculate_grid_dimensions(
        item_count: usize,
        preferred_columns: usize,
    ) -> (usize, usize) {
        if item_count == 0 {
            return (0, 0);
        }

        let columns = preferred_columns.min(item_count);
        let rows = item_count.div_ceil(columns); // Ceiling division
        (columns, rows)
    }

    /// Calculate item position in a grid
    pub fn grid_position(index: usize, columns: usize) -> (usize, usize) {
        let row = index / columns;
        let col = index % columns;
        (row, col)
    }

    /// Calculate total size needed for a grid
    pub fn grid_total_size(
        item_size: Size<Pixels>,
        gap: Pixels,
        columns: usize,
        rows: usize,
    ) -> Size<Pixels> {
        let width =
            px(columns as f32 * item_size.width.0 + (columns.saturating_sub(1)) as f32 * gap.0);
        let height = px(rows as f32 * item_size.height.0 + (rows.saturating_sub(1)) as f32 * gap.0);
        Size { width, height }
    }

    /// Check if a point is within bounds
    pub fn point_in_bounds(point: Point<Pixels>, bounds: Size<Pixels>) -> bool {
        point.x.0 >= 0.0
            && point.y.0 >= 0.0
            && point.x.0 <= bounds.width.0
            && point.y.0 <= bounds.height.0
    }

    /// Calculate distance between two points
    pub fn distance_between_points(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
        let dx = a.x.0 - b.x.0;
        let dy = a.y.0 - b.y.0;
        (dx * dx + dy * dy).sqrt()
    }

    /// Constrain a size to fit within bounds
    pub fn constrain_size(size: Size<Pixels>, max_size: Size<Pixels>) -> Size<Pixels> {
        Size {
            width: px(size.width.0.min(max_size.width.0)),
            height: px(size.height.0.min(max_size.height.0)),
        }
    }

    /// Calculate aspect ratio-preserving size
    pub fn aspect_fit_size(
        content_size: Size<Pixels>,
        container_size: Size<Pixels>,
    ) -> Size<Pixels> {
        let content_aspect = content_size.width.0 / content_size.height.0;
        let container_aspect = container_size.width.0 / container_size.height.0;

        if content_aspect > container_aspect {
            // Content is wider, fit to width
            Size {
                width: container_size.width,
                height: px(container_size.width.0 / content_aspect),
            }
        } else {
            // Content is taller, fit to height
            Size {
                width: px(container_size.height.0 * content_aspect),
                height: container_size.height,
            }
        }
    }
}

/// Color manipulation utilities
pub struct ColorHelpers;

impl ColorHelpers {
    /// Lighten a color by a percentage (0.0 to 1.0)
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        Hsla {
            h: color.h,
            s: color.s,
            l: (color.l + amount).min(1.0),
            a: color.a,
        }
    }

    /// Darken a color by a percentage (0.0 to 1.0)
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        Hsla {
            h: color.h,
            s: color.s,
            l: (color.l - amount).max(0.0),
            a: color.a,
        }
    }

    /// Adjust color opacity
    pub fn with_opacity(color: Hsla, opacity: f32) -> Hsla {
        Hsla {
            h: color.h,
            s: color.s,
            l: color.l,
            a: opacity.clamp(0.0, 1.0),
        }
    }

    /// Mix two colors with a ratio (0.0 = color1, 1.0 = color2)
    pub fn mix_colors(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
        let ratio = ratio.clamp(0.0, 1.0);
        let inv_ratio = 1.0 - ratio;

        Hsla {
            h: color1.h * inv_ratio + color2.h * ratio,
            s: color1.s * inv_ratio + color2.s * ratio,
            l: color1.l * inv_ratio + color2.l * ratio,
            a: color1.a * inv_ratio + color2.a * ratio,
        }
    }

    /// Get contrast color (black or white) for best readability
    pub fn contrast_color(background: Hsla) -> Hsla {
        // Calculate relative luminance
        let luminance = 0.299 * background.l + 0.587 * background.l + 0.114 * background.l;

        if luminance > 0.5 {
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0,
            } // Black
        } else {
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 1.0,
            } // White
        }
    }

    /// Check if a color is considered "dark"
    pub fn is_dark_color(color: Hsla) -> bool {
        color.l < 0.5
    }

    /// Generate a color variant for different states
    pub fn state_variant(base_color: Hsla, state: &str) -> Hsla {
        match state {
            "hover" => Self::lighten(base_color, 0.1),
            "active" => Self::darken(base_color, 0.1),
            "disabled" => Self::with_opacity(base_color, 0.5),
            "selected" => Self::lighten(base_color, 0.2),
            "error" => Hsla {
                h: 0.0,
                s: 0.8,
                l: 0.5,
                a: base_color.a,
            },
            "warning" => Hsla {
                h: 45.0,
                s: 0.8,
                l: 0.5,
                a: base_color.a,
            },
            "success" => Hsla {
                h: 120.0,
                s: 0.6,
                l: 0.4,
                a: base_color.a,
            },
            _ => base_color,
        }
    }
}

/// Animation and timing utilities
pub struct AnimationHelpers;

impl AnimationHelpers {
    /// Easing function: ease-in-out
    pub fn ease_in_out(t: f32) -> f32 {
        if t < 0.5 {
            2.0 * t * t
        } else {
            -1.0 + (4.0 - 2.0 * t) * t
        }
    }

    /// Easing function: ease-in
    pub fn ease_in(t: f32) -> f32 {
        t * t
    }

    /// Easing function: ease-out
    pub fn ease_out(t: f32) -> f32 {
        t * (2.0 - t)
    }

    /// Easing function: elastic
    pub fn ease_elastic(t: f32) -> f32 {
        if t == 0.0 || t == 1.0 {
            t
        } else {
            let c4 = (2.0 * std::f32::consts::PI) / 3.0;
            -(2.0_f32.powf(10.0 * t - 10.0)) * ((t * 10.0 - 10.75) * c4).sin()
        }
    }

    /// Easing function: bounce
    pub fn ease_bounce(t: f32) -> f32 {
        const N1: f32 = 7.5625;
        const D1: f32 = 2.75;

        if t < 1.0 / D1 {
            N1 * t * t
        } else if t < 2.0 / D1 {
            let t = t - 1.5 / D1;
            N1 * t * t + 0.75
        } else if t < 2.5 / D1 {
            let t = t - 2.25 / D1;
            N1 * t * t + 0.9375
        } else {
            let t = t - 2.625 / D1;
            N1 * t * t + 0.984375
        }
    }

    /// Calculate animation progress (0.0 to 1.0) based on elapsed time
    pub fn calculate_progress(elapsed: Duration, total_duration: Duration) -> f32 {
        if total_duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / total_duration.as_secs_f32()).clamp(0.0, 1.0)
        }
    }

    /// Interpolate between two values using an easing function
    pub fn interpolate_with_easing<T>(start: T, end: T, progress: f32, easing: fn(f32) -> f32) -> T
    where
        T: Clone
            + std::ops::Add<Output = T>
            + std::ops::Sub<Output = T>
            + std::ops::Mul<f32, Output = T>,
    {
        let eased_progress = easing(progress);
        start.clone() + (end - start.clone()) * eased_progress
    }

    /// Common animation durations
    pub fn duration_fast() -> Duration {
        Duration::from_millis(100)
    }

    pub fn duration_normal() -> Duration {
        Duration::from_millis(200)
    }

    pub fn duration_slow() -> Duration {
        Duration::from_millis(300)
    }
}

/// Text and content utilities
pub struct TextHelpers;

impl TextHelpers {
    /// Truncate text to fit within a character limit
    pub fn truncate(text: &str, max_chars: usize) -> String {
        if text.len() <= max_chars {
            text.to_string()
        } else if max_chars <= 3 {
            "...".to_string()
        } else {
            format!("{}...", &text[..max_chars - 3])
        }
    }

    /// Truncate text with custom ellipsis
    pub fn truncate_with_ellipsis(text: &str, max_chars: usize, ellipsis: &str) -> String {
        if text.len() <= max_chars {
            text.to_string()
        } else if max_chars <= ellipsis.len() {
            ellipsis.to_string()
        } else {
            format!("{}{}", &text[..max_chars - ellipsis.len()], ellipsis)
        }
    }

    /// Wrap text to fit within a width (simplified)
    pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current_line = String::new();

        for word in text.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }

        lines
    }

    /// Generate a human-readable label from a snake_case or kebab-case string
    pub fn humanize_label(input: &str) -> String {
        input
            .replace(['_', '-'], " ")
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Generate a slug from text (lowercase, alphanumeric, hyphens)
    pub fn slugify(text: &str) -> String {
        text.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Count words in text
    pub fn word_count(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// Estimate reading time in minutes
    pub fn reading_time_minutes(text: &str, words_per_minute: usize) -> usize {
        let word_count = Self::word_count(text);
        ((word_count as f32 / words_per_minute as f32).ceil() as usize).max(1)
    }
}

/// State management utilities
pub struct StateHelpers;

impl StateHelpers {
    /// Debounce a value change (returns true if enough time has passed)
    pub fn should_debounce(
        last_change: &mut Option<std::time::Instant>,
        debounce_duration: Duration,
    ) -> bool {
        let now = std::time::Instant::now();

        match last_change {
            Some(last) => {
                if now.duration_since(*last) >= debounce_duration {
                    *last_change = Some(now);
                    true
                } else {
                    false
                }
            }
            None => {
                *last_change = Some(now);
                true
            }
        }
    }

    /// Throttle calls (returns true if enough time has passed since last call)
    pub fn should_throttle(
        last_call: &mut Option<std::time::Instant>,
        throttle_duration: Duration,
    ) -> bool {
        let now = std::time::Instant::now();

        match last_call {
            Some(last) => {
                if now.duration_since(*last) >= throttle_duration {
                    *last_call = Some(now);
                    true
                } else {
                    false
                }
            }
            None => {
                *last_call = Some(now);
                true
            }
        }
    }

    /// Toggle a boolean value
    pub fn toggle(value: &mut bool) -> bool {
        *value = !*value;
        *value
    }

    /// Cycle through a list of values
    pub fn cycle_value<T: Clone>(current: &T, options: &[T]) -> Option<T> {
        if let Some(index) = options.iter().position(|x| std::ptr::eq(x, current)) {
            let next_index = (index + 1) % options.len();
            options.get(next_index).cloned()
        } else {
            options.first().cloned()
        }
    }
}

/// Element ID utilities
pub struct ElementIdHelpers;

impl ElementIdHelpers {
    /// Generate a unique element ID with a prefix
    pub fn unique_id(prefix: &str) -> ElementId {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix_string: SharedString = prefix.to_string().into();
        ElementId::from((prefix_string, id))
    }

    /// Generate a scoped element ID
    pub fn scoped_id(scope: &str, name: &str) -> ElementId {
        let scoped_name: SharedString = format!("{}::{}", scope, name).into();
        ElementId::from(scoped_name)
    }

    /// Generate a hierarchical element ID
    pub fn child_id(parent: &ElementId, child_name: &str) -> ElementId {
        let child_id: SharedString = format!("{}::{}", parent, child_name).into();
        ElementId::from(child_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_helpers() {
        let (cols, rows) = LayoutHelpers::calculate_grid_dimensions(10, 3);
        assert_eq!(cols, 3);
        assert_eq!(rows, 4);

        let (row, col) = LayoutHelpers::grid_position(5, 3);
        assert_eq!(row, 1);
        assert_eq!(col, 2);

        let size = LayoutHelpers::grid_total_size(
            Size {
                width: px(50.0),
                height: px(30.0),
            },
            px(10.0),
            3,
            2,
        );
        assert_eq!(size.width.0, 170.0); // 3*50 + 2*10
        assert_eq!(size.height.0, 70.0); // 2*30 + 1*10
    }

    #[test]
    fn test_color_helpers() {
        let color = Hsla {
            h: 200.0,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };

        let lighter = ColorHelpers::lighten(color, 0.2);
        assert_eq!(lighter.l, 0.7);

        let darker = ColorHelpers::darken(color, 0.2);
        assert_eq!(darker.l, 0.3);

        let transparent = ColorHelpers::with_opacity(color, 0.5);
        assert_eq!(transparent.a, 0.5);

        assert!(!ColorHelpers::is_dark_color(color));

        let dark_color = Hsla {
            h: 200.0,
            s: 0.5,
            l: 0.3,
            a: 1.0,
        };
        assert!(ColorHelpers::is_dark_color(dark_color));
    }

    #[test]
    fn test_animation_helpers() {
        assert_eq!(AnimationHelpers::ease_in_out(0.0), 0.0);
        assert_eq!(AnimationHelpers::ease_in_out(1.0), 1.0);
        assert!(
            AnimationHelpers::ease_in_out(0.5) > 0.4 && AnimationHelpers::ease_in_out(0.5) < 0.6
        );

        let progress = AnimationHelpers::calculate_progress(
            Duration::from_millis(500),
            Duration::from_millis(1000),
        );
        assert_eq!(progress, 0.5);
    }

    #[test]
    fn test_text_helpers() {
        assert_eq!(TextHelpers::truncate("Hello, World!", 10), "Hello,...");
        assert_eq!(TextHelpers::truncate("Hi", 10), "Hi");

        let wrapped = TextHelpers::wrap_text("This is a long line of text", 10);
        assert!(wrapped.len() > 1);
        assert!(wrapped[0].len() <= 10);

        assert_eq!(TextHelpers::humanize_label("user_name"), "User Name");
        assert_eq!(TextHelpers::humanize_label("kebab-case"), "Kebab Case");

        assert_eq!(TextHelpers::slugify("Hello World!"), "hello-world");

        assert_eq!(TextHelpers::word_count("Hello world test"), 3);

        assert_eq!(TextHelpers::reading_time_minutes("Hello world", 200), 1);
    }

    #[test]
    fn test_state_helpers() {
        let mut value = false;
        assert_eq!(StateHelpers::toggle(&mut value), true);
        assert_eq!(StateHelpers::toggle(&mut value), false);

        let options = vec!["a", "b", "c"];
        let current = "a";
        let next = StateHelpers::cycle_value(&current, &options);
        assert_eq!(next, Some("b"));

        let current = "c";
        let next = StateHelpers::cycle_value(&current, &options);
        assert_eq!(next, Some("a"));
    }

    #[test]
    fn test_element_id_helpers() {
        let id1 = ElementIdHelpers::unique_id("button");
        let id2 = ElementIdHelpers::unique_id("button");
        assert_ne!(id1, id2);

        let scoped = ElementIdHelpers::scoped_id("dialog", "close-button");
        assert_eq!(scoped.to_string(), "dialog::close-button");

        let parent: ElementId = "parent".into();
        let child = ElementIdHelpers::child_id(&parent, "child");
        assert_eq!(child.to_string(), "parent::child");
    }
}
