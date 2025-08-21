// ABOUTME: Theme transition animation system for smooth theme switching
// ABOUTME: Provides interpolation, easing functions, and performance-optimized animations

use crate::Theme;
use gpui::{Hsla, Pixels};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Theme animator for smooth theme transitions
#[derive(Debug)]
pub struct ThemeAnimator {
    /// Current animation state
    animation_state: Arc<RwLock<AnimationState>>,
    /// Animation configuration
    config: AnimationConfig,
    /// Performance monitoring
    performance_monitor: PerformanceMonitor,
}

/// Animation state tracking
#[derive(Debug, Clone)]
pub struct AnimationState {
    /// Whether an animation is currently running
    pub is_animating: bool,
    /// Animation start time
    pub start_time: Option<Instant>,
    /// Animation duration
    pub duration: Duration,
    /// Source theme
    pub from_theme: Option<Theme>,
    /// Target theme
    pub to_theme: Option<Theme>,
    /// Current interpolated theme
    pub current_theme: Option<Theme>,
    /// Animation progress (0.0 to 1.0)
    pub progress: f32,
    /// Easing function
    pub easing: EasingFunction,
    /// Properties being animated
    pub animated_properties: Vec<AnimatedProperty>,
}

/// Animation configuration
#[derive(Debug, Clone)]
pub struct AnimationConfig {
    /// Default animation duration
    pub default_duration: Duration,
    /// Default easing function
    pub default_easing: EasingFunction,
    /// Enable GPU acceleration when possible
    pub enable_gpu_acceleration: bool,
    /// Maximum animation fps
    pub max_fps: u32,
    /// Reduced motion support
    pub respect_reduced_motion: bool,
    /// Performance thresholds
    pub performance_thresholds: PerformanceThresholds,
}

/// Performance monitoring for animations
#[derive(Debug, Clone)]
pub struct PerformanceMonitor {
    /// Frame time measurements
    frame_times: Vec<Duration>,
    /// Dropped frame count
    dropped_frames: u32,
    /// Performance warnings issued
    warnings_issued: u32,
}

/// Performance thresholds
#[derive(Debug, Clone)]
pub struct PerformanceThresholds {
    /// Maximum acceptable frame time
    pub max_frame_time: Duration,
    /// Maximum dropped frames before degrading
    pub max_dropped_frames: u32,
    /// Memory usage threshold
    pub max_memory_mb: usize,
}

/// Properties that can be animated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimatedProperty {
    /// Background colors
    BackgroundColor,
    /// Text colors
    TextColor,
    /// Border colors
    BorderColor,
    /// Primary colors
    PrimaryColor,
    /// Secondary colors
    SecondaryColor,
    /// Surface colors
    SurfaceColor,
    /// Error colors
    ErrorColor,
    /// Warning colors
    WarningColor,
    /// Success colors
    SuccessColor,
    /// Opacity values
    Opacity,
    /// Size values
    Sizes,
}

/// Easing functions for smooth animations
#[derive(Debug, Clone, Copy)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInSine,
    EaseOutSine,
    EaseInOutSine,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInQuart,
    EaseOutQuart,
    EaseInOutQuart,
    EaseInBack,
    EaseOutBack,
    EaseInOutBack,
    Custom { control_points: [f32; 4] },
}

/// Animation step result
#[derive(Debug, Clone)]
pub struct AnimationStep {
    /// Current interpolated theme
    pub theme: Theme,
    /// Animation progress
    pub progress: f32,
    /// Whether animation is complete
    pub is_complete: bool,
    /// Performance metrics for this step
    pub performance: StepPerformance,
}

/// Performance metrics for a single animation step
#[derive(Debug, Clone)]
pub struct StepPerformance {
    /// Time taken to compute this step
    pub computation_time: Duration,
    /// Memory allocated for this step
    pub memory_used: usize,
    /// Whether this step was optimized
    pub was_optimized: bool,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            default_duration: Duration::from_millis(300),
            default_easing: EasingFunction::EaseOutCubic,
            enable_gpu_acceleration: true,
            max_fps: 60,
            respect_reduced_motion: true,
            performance_thresholds: PerformanceThresholds::default(),
        }
    }
}

impl Default for PerformanceThresholds {
    fn default() -> Self {
        Self {
            max_frame_time: Duration::from_millis(16), // 60fps
            max_dropped_frames: 5,
            max_memory_mb: 50,
        }
    }
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self {
            frame_times: Vec::with_capacity(60), // Track last 60 frames
            dropped_frames: 0,
            warnings_issued: 0,
        }
    }
}

impl ThemeAnimator {
    /// Create a new theme animator
    pub fn new() -> Self {
        Self {
            animation_state: Arc::new(RwLock::new(AnimationState::new())),
            config: AnimationConfig::default(),
            performance_monitor: PerformanceMonitor::default(),
        }
    }

    /// Create animator with custom configuration
    pub fn with_config(config: AnimationConfig) -> Self {
        Self {
            animation_state: Arc::new(RwLock::new(AnimationState::new())),
            config,
            performance_monitor: PerformanceMonitor::default(),
        }
    }

    /// Start animating from one theme to another
    pub fn animate_theme_transition(
        &mut self,
        from_theme: Arc<RwLock<Theme>>,
        to_theme: Theme,
        duration: Duration,
    ) -> Result<(), AnimationError> {
        let from_theme_value = from_theme
            .read()
            .map_err(|_| AnimationError::LockError("Failed to acquire from_theme lock".into()))?
            .clone();

        let actual_duration = if self.config.respect_reduced_motion {
            // Check system preferences for reduced motion
            self.check_reduced_motion_preference()
                .unwrap_or(false)
                .then(|| Duration::ZERO)
                .unwrap_or(duration)
        } else {
            duration
        };

        if let Ok(mut state) = self.animation_state.write() {
            *state = AnimationState {
                is_animating: actual_duration > Duration::ZERO,
                start_time: Some(Instant::now()),
                duration: actual_duration,
                from_theme: Some(from_theme_value),
                to_theme: Some(to_theme),
                current_theme: None,
                progress: 0.0,
                easing: self.config.default_easing,
                animated_properties: vec![
                    AnimatedProperty::BackgroundColor,
                    AnimatedProperty::TextColor,
                    AnimatedProperty::BorderColor,
                    AnimatedProperty::PrimaryColor,
                    AnimatedProperty::SecondaryColor,
                    AnimatedProperty::SurfaceColor,
                ],
            };
        } else {
            return Err(AnimationError::LockError(
                "Failed to acquire animation state lock".into(),
            ));
        }

        // If no animation needed, apply immediately
        if actual_duration == Duration::ZERO {
            if let Ok(_target_theme) = from_theme.read() {
                // Immediate application would happen here
            }
            return Ok(());
        }

        nucleotide_logging::info!(
            duration_ms = actual_duration.as_millis(),
            "Theme animation started"
        );

        Ok(())
    }

    /// Update animation and get current interpolated theme
    pub fn update_animation(&mut self) -> Option<AnimationStep> {
        let step_start = Instant::now();

        let state_clone = {
            if let Ok(state) = self.animation_state.read() {
                if !state.is_animating {
                    return None;
                }
                state.clone()
            } else {
                return None;
            }
        };

        let elapsed = state_clone.start_time?.elapsed();

        let progress = if state_clone.duration == Duration::ZERO {
            1.0
        } else {
            (elapsed.as_secs_f32() / state_clone.duration.as_secs_f32()).min(1.0)
        };

        let eased_progress = self.apply_easing(progress, state_clone.easing);
        let is_complete = progress >= 1.0;

        // Interpolate theme
        let interpolated_theme =
            if let (Some(from), Some(to)) = (&state_clone.from_theme, &state_clone.to_theme) {
                self.interpolate_themes(from, to, eased_progress, &state_clone.animated_properties)
            } else {
                return None;
            };

        // Update state
        if let Ok(mut state) = self.animation_state.write() {
            state.progress = progress;
            state.current_theme = Some(interpolated_theme.clone());

            if is_complete {
                state.is_animating = false;
                nucleotide_logging::debug!("Theme animation completed");
            }
        }

        let computation_time = step_start.elapsed();

        // Monitor performance
        self.monitor_performance(computation_time);

        Some(AnimationStep {
            theme: interpolated_theme,
            progress: eased_progress,
            is_complete,
            performance: StepPerformance {
                computation_time,
                memory_used: std::mem::size_of::<Theme>(), // Simplified
                was_optimized: computation_time < self.config.performance_thresholds.max_frame_time,
            },
        })
    }

    /// Stop current animation
    pub fn stop_animation(&mut self) {
        if let Ok(mut state) = self.animation_state.write() {
            state.is_animating = false;
            nucleotide_logging::debug!("Theme animation stopped");
        }
    }

    /// Check if animation is currently running
    pub fn is_animating(&self) -> bool {
        self.animation_state
            .read()
            .map(|state| state.is_animating)
            .unwrap_or(false)
    }

    /// Get current animation progress
    pub fn get_progress(&self) -> f32 {
        self.animation_state
            .read()
            .map(|state| state.progress)
            .unwrap_or(0.0)
    }

    /// Configure animation properties
    pub fn configure<F>(&mut self, configurator: F)
    where
        F: FnOnce(&mut AnimationConfig),
    {
        configurator(&mut self.config);
    }

    /// Apply easing function to progress
    fn apply_easing(&self, t: f32, easing: EasingFunction) -> f32 {
        match easing {
            EasingFunction::Linear => t,
            EasingFunction::EaseIn => t * t,
            EasingFunction::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            EasingFunction::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t) * (1.0 - t)
                }
            }
            EasingFunction::EaseInSine => 1.0 - (t * std::f32::consts::PI / 2.0).cos(),
            EasingFunction::EaseOutSine => (t * std::f32::consts::PI / 2.0).sin(),
            EasingFunction::EaseInOutSine => -(((t * std::f32::consts::PI).cos() - 1.0) / 2.0),
            EasingFunction::EaseInQuad => t * t,
            EasingFunction::EaseOutQuad => 1.0 - (1.0 - t) * (1.0 - t),
            EasingFunction::EaseInOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t).powi(2)
                }
            }
            EasingFunction::EaseInCubic => t * t * t,
            EasingFunction::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            EasingFunction::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            EasingFunction::EaseInQuart => t.powi(4),
            EasingFunction::EaseOutQuart => 1.0 - (1.0 - t).powi(4),
            EasingFunction::EaseInOutQuart => {
                if t < 0.5 {
                    8.0 * t.powi(4)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(4) / 2.0
                }
            }
            EasingFunction::EaseInBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t * t * t - c1 * t * t
            }
            EasingFunction::EaseOutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            EasingFunction::EaseInOutBack => {
                let c1 = 1.70158;
                let c2 = c1 * 1.525;
                if t < 0.5 {
                    ((2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2)) / 2.0
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (t * 2.0 - 2.0) + c2) + 2.0) / 2.0
                }
            }
            EasingFunction::Custom { control_points } => {
                // Simplified cubic bezier approximation
                let [_x1, y1, _x2, y2] = control_points;
                // This is a simplified version - real implementation would need proper cubic bezier calculation
                let t2 = t * t;
                let t3 = t2 * t;
                let mt = 1.0 - t;
                let mt2 = mt * mt;
                let mt3 = mt2 * mt;

                mt3 * 0.0 + 3.0 * mt2 * t * y1 + 3.0 * mt * t2 * y2 + t3 * 1.0
            }
        }
    }

    /// Interpolate between two themes
    fn interpolate_themes(
        &self,
        from: &Theme,
        to: &Theme,
        progress: f32,
        properties: &[AnimatedProperty],
    ) -> Theme {
        let mut result = from.clone();

        for property in properties {
            match property {
                AnimatedProperty::BackgroundColor => {
                    result.tokens.colors.background = self.interpolate_color(
                        from.tokens.colors.background,
                        to.tokens.colors.background,
                        progress,
                    );
                }
                AnimatedProperty::TextColor => {
                    result.tokens.colors.text_primary = self.interpolate_color(
                        from.tokens.colors.text_primary,
                        to.tokens.colors.text_primary,
                        progress,
                    );
                    result.tokens.colors.text_secondary = self.interpolate_color(
                        from.tokens.colors.text_secondary,
                        to.tokens.colors.text_secondary,
                        progress,
                    );
                }
                AnimatedProperty::BorderColor => {
                    result.tokens.colors.border_default = self.interpolate_color(
                        from.tokens.colors.border_default,
                        to.tokens.colors.border_default,
                        progress,
                    );
                }
                AnimatedProperty::PrimaryColor => {
                    result.tokens.colors.primary = self.interpolate_color(
                        from.tokens.colors.primary,
                        to.tokens.colors.primary,
                        progress,
                    );
                }
                AnimatedProperty::SecondaryColor => {
                    result.tokens.colors.text_secondary = self.interpolate_color(
                        from.tokens.colors.text_secondary,
                        to.tokens.colors.text_secondary,
                        progress,
                    );
                }
                AnimatedProperty::SurfaceColor => {
                    result.tokens.colors.surface = self.interpolate_color(
                        from.tokens.colors.surface,
                        to.tokens.colors.surface,
                        progress,
                    );
                }
                AnimatedProperty::ErrorColor => {
                    result.tokens.colors.error = self.interpolate_color(
                        from.tokens.colors.error,
                        to.tokens.colors.error,
                        progress,
                    );
                }
                AnimatedProperty::WarningColor => {
                    result.tokens.colors.warning = self.interpolate_color(
                        from.tokens.colors.warning,
                        to.tokens.colors.warning,
                        progress,
                    );
                }
                AnimatedProperty::SuccessColor => {
                    result.tokens.colors.success = self.interpolate_color(
                        from.tokens.colors.success,
                        to.tokens.colors.success,
                        progress,
                    );
                }
                AnimatedProperty::Sizes => {
                    result.tokens.sizes.space_1 = self.interpolate_pixels(
                        from.tokens.sizes.space_1,
                        to.tokens.sizes.space_1,
                        progress,
                    );
                    result.tokens.sizes.space_2 = self.interpolate_pixels(
                        from.tokens.sizes.space_2,
                        to.tokens.sizes.space_2,
                        progress,
                    );
                    result.tokens.sizes.space_3 = self.interpolate_pixels(
                        from.tokens.sizes.space_3,
                        to.tokens.sizes.space_3,
                        progress,
                    );
                    result.tokens.sizes.space_4 = self.interpolate_pixels(
                        from.tokens.sizes.space_4,
                        to.tokens.sizes.space_4,
                        progress,
                    );
                }
                AnimatedProperty::Opacity => {
                    // Opacity would be handled if we had opacity in our color system
                }
            }
        }

        // Rebuild derived fields
        result = Theme::from_tokens(result.tokens);
        result
    }

    /// Interpolate between two colors
    fn interpolate_color(&self, from: Hsla, to: Hsla, progress: f32) -> Hsla {
        // Handle hue interpolation (shortest path around color wheel)
        let hue_diff = to.h - from.h;
        let adjusted_hue_diff = if hue_diff > 180.0 {
            hue_diff - 360.0
        } else if hue_diff < -180.0 {
            hue_diff + 360.0
        } else {
            hue_diff
        };

        let interpolated_hue = (from.h + adjusted_hue_diff * progress) % 360.0;
        let interpolated_hue = if interpolated_hue < 0.0 {
            interpolated_hue + 360.0
        } else {
            interpolated_hue
        };

        Hsla {
            h: interpolated_hue,
            s: from.s + (to.s - from.s) * progress,
            l: from.l + (to.l - from.l) * progress,
            a: from.a + (to.a - from.a) * progress,
        }
    }

    /// Interpolate between two pixel values
    fn interpolate_pixels(&self, from: Pixels, to: Pixels, progress: f32) -> Pixels {
        gpui::px(from.0 + (to.0 - from.0) * progress)
    }

    /// Monitor animation performance
    fn monitor_performance(&mut self, frame_time: Duration) {
        self.performance_monitor.frame_times.push(frame_time);

        // Keep only recent measurements
        if self.performance_monitor.frame_times.len() > 60 {
            self.performance_monitor.frame_times.remove(0);
        }

        // Check for performance issues
        if frame_time > self.config.performance_thresholds.max_frame_time {
            self.performance_monitor.dropped_frames += 1;

            if self.performance_monitor.dropped_frames
                > self.config.performance_thresholds.max_dropped_frames
                && self.performance_monitor.warnings_issued < 3
            {
                nucleotide_logging::warn!(
                    dropped_frames = self.performance_monitor.dropped_frames,
                    frame_time_ms = frame_time.as_millis(),
                    "Theme animation performance degraded"
                );
                self.performance_monitor.warnings_issued += 1;
            }
        }
    }

    /// Check system preference for reduced motion
    fn check_reduced_motion_preference(&self) -> Option<bool> {
        // This would check system settings - simplified for now
        std::env::var("PREFER_REDUCED_MOTION")
            .ok()
            .and_then(|val| val.parse().ok())
    }

    /// Get performance statistics
    pub fn get_performance_stats(&self) -> AnimationPerformanceStats {
        let avg_frame_time = if self.performance_monitor.frame_times.is_empty() {
            Duration::ZERO
        } else {
            let total: Duration = self.performance_monitor.frame_times.iter().sum();
            total / self.performance_monitor.frame_times.len() as u32
        };

        AnimationPerformanceStats {
            average_frame_time: avg_frame_time,
            dropped_frames: self.performance_monitor.dropped_frames,
            warnings_issued: self.performance_monitor.warnings_issued,
            current_fps: if avg_frame_time > Duration::ZERO {
                1000.0 / avg_frame_time.as_millis() as f32
            } else {
                0.0
            },
        }
    }
}

impl AnimationState {
    fn new() -> Self {
        Self {
            is_animating: false,
            start_time: None,
            duration: Duration::ZERO,
            from_theme: None,
            to_theme: None,
            current_theme: None,
            progress: 0.0,
            easing: EasingFunction::Linear,
            animated_properties: Vec::new(),
        }
    }
}

impl Default for ThemeAnimator {
    fn default() -> Self {
        Self::new()
    }
}

/// Animation performance statistics
#[derive(Debug, Clone)]
pub struct AnimationPerformanceStats {
    /// Average frame computation time
    pub average_frame_time: Duration,
    /// Number of dropped frames
    pub dropped_frames: u32,
    /// Number of performance warnings issued
    pub warnings_issued: u32,
    /// Current effective FPS
    pub current_fps: f32,
}

/// Animation errors
#[derive(Debug, Clone)]
pub enum AnimationError {
    /// Lock acquisition failed
    LockError(String),
    /// Invalid animation parameters
    InvalidParameters(String),
    /// Performance degradation
    PerformanceDegraded(String),
}

impl std::fmt::Display for AnimationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnimationError::LockError(msg) => write!(f, "Lock error: {}", msg),
            AnimationError::InvalidParameters(msg) => write!(f, "Invalid parameters: {}", msg),
            AnimationError::PerformanceDegraded(msg) => write!(f, "Performance degraded: {}", msg),
        }
    }
}

impl std::error::Error for AnimationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_animator_creation() {
        let animator = ThemeAnimator::new();
        assert!(!animator.is_animating());
        assert_eq!(animator.get_progress(), 0.0);
    }

    #[test]
    fn test_easing_functions() {
        let animator = ThemeAnimator::new();

        // Test linear easing
        assert_eq!(animator.apply_easing(0.5, EasingFunction::Linear), 0.5);

        // Test ease-in (should be slower at start)
        let ease_in_half = animator.apply_easing(0.5, EasingFunction::EaseIn);
        assert!(ease_in_half < 0.5);

        // Test ease-out (should be faster at start)
        let ease_out_half = animator.apply_easing(0.5, EasingFunction::EaseOut);
        assert!(ease_out_half > 0.5);
    }

    #[test]
    fn test_color_interpolation() {
        let animator = ThemeAnimator::new();

        let red = Hsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let blue = Hsla {
            h: 240.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };

        let halfway = animator.interpolate_color(red, blue, 0.5);

        // Should be purple-ish (interpolated hue)
        assert!(halfway.h > 0.0 && halfway.h < 240.0);
        assert_eq!(halfway.s, 1.0);
        assert_eq!(halfway.l, 0.5);
        assert_eq!(halfway.a, 1.0);
    }

    #[test]
    fn test_animation_config() {
        let mut animator = ThemeAnimator::new();

        animator.configure(|config| {
            config.default_duration = Duration::from_millis(500);
            config.default_easing = EasingFunction::EaseInOut;
            config.max_fps = 30;
        });

        assert_eq!(animator.config.default_duration, Duration::from_millis(500));
        assert_eq!(animator.config.max_fps, 30);
    }

    #[test]
    fn test_performance_monitoring() {
        let mut animator = ThemeAnimator::new();

        // Simulate slow frame
        let slow_frame = Duration::from_millis(50);
        animator.monitor_performance(slow_frame);

        let stats = animator.get_performance_stats();
        assert_eq!(stats.dropped_frames, 1);
        assert!(stats.average_frame_time > Duration::ZERO);
    }
}
