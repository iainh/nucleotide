// ABOUTME: Rendering utilities and optimization helpers for nucleotide-ui components
// ABOUTME: Provides caching, memoization, and rendering performance optimizations

use gpui::{AnyElement, Hsla, IntoElement, Pixels};
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// Render cache for expensive computations
#[derive(Debug)]
pub struct RenderCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    cache: HashMap<K, CacheEntry<V>>,
    max_size: usize,
    default_ttl: Duration,
}

#[derive(Debug, Clone)]
struct CacheEntry<V> {
    value: V,
    created_at: Instant,
    ttl: Duration,
    access_count: usize,
    last_accessed: Instant,
}

impl<K, V> RenderCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Create a new render cache
    pub fn new(max_size: usize, default_ttl: Duration) -> Self {
        Self {
            cache: HashMap::new(),
            max_size,
            default_ttl,
        }
    }

    /// Get a value from the cache
    pub fn get(&mut self, key: &K) -> Option<V> {
        let now = Instant::now();

        if let Some(entry) = self.cache.get_mut(key) {
            // Check if entry has expired
            if now.duration_since(entry.created_at) > entry.ttl {
                self.cache.remove(key);
                return None;
            }

            // Update access statistics
            entry.access_count += 1;
            entry.last_accessed = now;

            Some(entry.value.clone())
        } else {
            None
        }
    }

    /// Insert a value into the cache
    pub fn insert(&mut self, key: K, value: V) {
        self.insert_with_ttl(key, value, self.default_ttl);
    }

    /// Insert a value with custom TTL
    pub fn insert_with_ttl(&mut self, key: K, value: V, ttl: Duration) {
        let now = Instant::now();

        // Evict old entries if cache is full
        if self.cache.len() >= self.max_size && !self.cache.contains_key(&key) {
            self.evict_lru();
        }

        let entry = CacheEntry {
            value,
            created_at: now,
            ttl,
            access_count: 1,
            last_accessed: now,
        };

        self.cache.insert(key, entry);
    }

    /// Remove expired entries
    pub fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.cache
            .retain(|_, entry| now.duration_since(entry.created_at) <= entry.ttl);
    }

    /// Evict least recently used entry
    fn evict_lru(&mut self) {
        if let Some((lru_key, _)) = self
            .cache
            .iter()
            .min_by_key(|(_, entry)| (entry.last_accessed, entry.access_count))
            .map(|(k, v)| (k.clone(), v.clone()))
        {
            self.cache.remove(&lru_key);
        }
    }

    /// Clear all cached entries
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            size: self.cache.len(),
            max_size: self.max_size,
            hit_ratio: 0.0, // Would need hit/miss tracking for accurate calculation
            total_entries: self.cache.len(),
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub max_size: usize,
    pub hit_ratio: f32,
    pub total_entries: usize,
}

/// Memoization utilities for expensive computations
pub struct MemoizedComputation<Args, Result>
where
    Args: Hash + Eq + Clone,
    Result: Clone,
{
    cache: RenderCache<Args, Result>,
    computation: Box<dyn Fn(&Args) -> Result>,
}

impl<Args, Result> MemoizedComputation<Args, Result>
where
    Args: Hash + Eq + Clone,
    Result: Clone,
{
    /// Create a new memoized computation
    pub fn new<F>(computation: F, max_cache_size: usize, ttl: Duration) -> Self
    where
        F: Fn(&Args) -> Result + 'static,
    {
        Self {
            cache: RenderCache::new(max_cache_size, ttl),
            computation: Box::new(computation),
        }
    }

    /// Compute result, using cache if available
    pub fn compute(&mut self, args: Args) -> Result {
        if let Some(cached_result) = self.cache.get(&args) {
            cached_result
        } else {
            let result = (self.computation)(&args);
            self.cache.insert(args, result.clone());
            result
        }
    }

    /// Invalidate cached result for specific arguments
    pub fn invalidate(&mut self, args: &Args) {
        self.cache.cache.remove(args);
    }

    /// Clear all cached results
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

/// Viewport and visibility utilities
pub struct ViewportUtils;

impl ViewportUtils {
    /// Check if an element is likely visible in the viewport
    pub fn is_likely_visible(
        element_top: Pixels,
        element_height: Pixels,
        viewport_top: Pixels,
        viewport_height: Pixels,
        buffer_zone: Pixels,
    ) -> bool {
        let element_bottom = Pixels(element_top.0 + element_height.0);
        let viewport_bottom = Pixels(viewport_top.0 + viewport_height.0);

        // Add buffer zone for pre-loading
        let buffered_viewport_top = Pixels(viewport_top.0 - buffer_zone.0);
        let buffered_viewport_bottom = Pixels(viewport_bottom.0 + buffer_zone.0);

        // Check if element intersects with buffered viewport
        element_bottom.0 >= buffered_viewport_top.0 && element_top.0 <= buffered_viewport_bottom.0
    }

    /// Calculate visible portion of an element
    pub fn visible_portion(
        element_top: Pixels,
        element_height: Pixels,
        viewport_top: Pixels,
        viewport_height: Pixels,
    ) -> f32 {
        let element_bottom = element_top.0 + element_height.0;
        let viewport_bottom = viewport_top.0 + viewport_height.0;

        let visible_top = element_top.0.max(viewport_top.0);
        let visible_bottom = element_bottom.min(viewport_bottom);

        if visible_bottom <= visible_top {
            0.0 // Not visible
        } else {
            (visible_bottom - visible_top) / element_height.0
        }
    }

    /// Calculate intersection ratio between element and viewport
    pub fn intersection_ratio(
        element_top: Pixels,
        element_height: Pixels,
        viewport_top: Pixels,
        viewport_height: Pixels,
    ) -> f32 {
        Self::visible_portion(element_top, element_height, viewport_top, viewport_height)
    }
}

/// Conditional rendering utilities
pub struct ConditionalRenderer;

impl ConditionalRenderer {
    /// Render element only if condition is true
    pub fn render_if<T>(condition: bool, element: T) -> Option<AnyElement>
    where
        T: IntoElement,
    {
        if condition {
            Some(element.into_any_element())
        } else {
            None
        }
    }

    /// Render one of two elements based on condition
    pub fn render_either<T, U>(condition: bool, if_true: T, if_false: U) -> AnyElement
    where
        T: IntoElement,
        U: IntoElement,
    {
        if condition {
            if_true.into_any_element()
        } else {
            if_false.into_any_element()
        }
    }

    /// Render element with loading state
    pub fn render_with_loading<T, L>(is_loading: bool, content: T, loading_element: L) -> AnyElement
    where
        T: IntoElement,
        L: IntoElement,
    {
        Self::render_either(is_loading, loading_element, content)
    }

    /// Render list of optional elements, filtering out None values
    pub fn render_list(elements: Vec<Option<AnyElement>>) -> Vec<AnyElement> {
        elements.into_iter().flatten().collect()
    }
}

/// Lazy rendering utilities for performance optimization
pub struct LazyRenderer<T> {
    generator: Box<dyn Fn() -> T>,
    cached_element: Option<T>,
    is_dirty: bool,
}

impl<T> LazyRenderer<T>
where
    T: Clone,
{
    /// Create a new lazy renderer
    pub fn new<F>(generator: F) -> Self
    where
        F: Fn() -> T + 'static,
    {
        Self {
            generator: Box::new(generator),
            cached_element: None,
            is_dirty: true,
        }
    }

    /// Get the rendered element, generating if necessary
    pub fn render(&mut self) -> T {
        if self.is_dirty || self.cached_element.is_none() {
            let element = (self.generator)();
            self.cached_element = Some(element.clone());
            self.is_dirty = false;
            element
        } else {
            self.cached_element.as_ref().unwrap().clone()
        }
    }

    /// Mark the renderer as dirty (needs re-rendering)
    pub fn invalidate(&mut self) {
        self.is_dirty = true;
    }

    /// Check if the renderer needs to re-render
    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }
}

/// Color palette utilities for consistent theming
pub struct PaletteUtils;

impl PaletteUtils {
    /// Generate a color palette from a base color
    pub fn generate_palette(base_color: Hsla, count: usize) -> Vec<Hsla> {
        let mut palette = Vec::with_capacity(count);

        for i in 0..count {
            let factor = if count == 1 {
                0.0
            } else {
                i as f32 / (count - 1) as f32
            };

            // Vary lightness while keeping hue and saturation
            let lightness = (base_color.l * 0.3) + (factor * 0.7);

            palette.push(Hsla {
                h: base_color.h,
                s: base_color.s,
                l: lightness.clamp(0.0, 1.0),
                a: base_color.a,
            });
        }

        palette
    }

    /// Generate complementary colors
    pub fn complementary_color(color: Hsla) -> Hsla {
        Hsla {
            h: (color.h + 180.0) % 360.0,
            s: color.s,
            l: color.l,
            a: color.a,
        }
    }

    /// Generate triadic colors
    pub fn triadic_colors(color: Hsla) -> (Hsla, Hsla) {
        let color1 = Hsla {
            h: (color.h + 120.0) % 360.0,
            s: color.s,
            l: color.l,
            a: color.a,
        };

        let color2 = Hsla {
            h: (color.h + 240.0) % 360.0,
            s: color.s,
            l: color.l,
            a: color.a,
        };

        (color1, color2)
    }

    /// Generate analogous colors
    pub fn analogous_colors(color: Hsla, count: usize, spread: f32) -> Vec<Hsla> {
        let mut colors = Vec::with_capacity(count);
        let step = spread / (count as f32 - 1.0);

        for i in 0..count {
            let hue_offset = -spread / 2.0 + (i as f32 * step);
            colors.push(Hsla {
                h: (color.h + hue_offset) % 360.0,
                s: color.s,
                l: color.l,
                a: color.a,
            });
        }

        colors
    }
}

/// Performance measurement for render operations
pub struct RenderProfiler {
    timings: HashMap<String, Vec<Duration>>,
    current_operations: HashMap<String, Instant>,
}

impl RenderProfiler {
    /// Create a new render profiler
    pub fn new() -> Self {
        Self {
            timings: HashMap::new(),
            current_operations: HashMap::new(),
        }
    }

    /// Start timing a render operation
    pub fn start_operation(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.current_operations.insert(name, Instant::now());
    }

    /// End timing a render operation
    pub fn end_operation(&mut self, name: &str) -> Option<Duration> {
        if let Some(start_time) = self.current_operations.remove(name) {
            let duration = start_time.elapsed();
            self.timings
                .entry(name.to_string())
                .or_default()
                .push(duration);
            Some(duration)
        } else {
            None
        }
    }

    /// Get timing statistics for an operation
    pub fn get_stats(&self, name: &str) -> Option<RenderStats> {
        self.timings.get(name).map(|durations| {
            let count = durations.len();
            let total: Duration = durations.iter().sum();
            let average = if count > 0 {
                total / count as u32
            } else {
                Duration::ZERO
            };
            let min = durations.iter().min().copied().unwrap_or(Duration::ZERO);
            let max = durations.iter().max().copied().unwrap_or(Duration::ZERO);

            RenderStats {
                operation_name: name.to_string(),
                count,
                total_time: total,
                average_time: average,
                min_time: min,
                max_time: max,
            }
        })
    }

    /// Clear all timing data
    pub fn clear(&mut self) {
        self.timings.clear();
        self.current_operations.clear();
    }
}

/// Render timing statistics
#[derive(Debug, Clone)]
pub struct RenderStats {
    pub operation_name: String,
    pub count: usize,
    pub total_time: Duration,
    pub average_time: Duration,
    pub min_time: Duration,
    pub max_time: Duration,
}

impl Default for RenderProfiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Macro for automatic render profiling
#[macro_export]
macro_rules! profile_render {
    ($profiler:expr, $name:expr, $block:block) => {{
        $profiler.start_operation($name);
        let result = $block;
        $profiler.end_operation($name);
        result
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_cache() {
        let mut cache = RenderCache::new(2, Duration::from_secs(1));

        cache.insert("key1".to_string(), "value1".to_string());
        cache.insert("key2".to_string(), "value2".to_string());

        assert_eq!(cache.get(&"key1".to_string()), Some("value1".to_string()));
        assert_eq!(cache.get(&"key2".to_string()), Some("value2".to_string()));

        // Adding third item should evict LRU
        cache.insert("key3".to_string(), "value3".to_string());
        assert_eq!(cache.cache.len(), 2);
    }

    #[test]
    fn test_memoized_computation() {
        let mut memo = MemoizedComputation::new(|x: &i32| x * x, 10, Duration::from_secs(1));

        let result1 = memo.compute(5);
        let result2 = memo.compute(5); // Should use cache

        assert_eq!(result1, 25);
        assert_eq!(result2, 25);
    }

    #[test]
    fn test_viewport_utils() {
        let is_visible = ViewportUtils::is_likely_visible(
            Pixels(100.0), // element_top
            Pixels(50.0),  // element_height
            Pixels(120.0), // viewport_top
            Pixels(200.0), // viewport_height
            Pixels(20.0),  // buffer_zone
        );

        assert!(is_visible);

        let portion = ViewportUtils::visible_portion(
            Pixels(100.0), // element_top
            Pixels(50.0),  // element_height
            Pixels(120.0), // viewport_top
            Pixels(200.0), // viewport_height
        );

        assert!(portion > 0.0 && portion <= 1.0);
    }

    #[test]
    fn test_lazy_renderer() {
        use std::sync::{Arc, RwLock};

        let counter = Arc::new(RwLock::new(0));
        let counter_clone = counter.clone();

        let mut lazy = LazyRenderer::new(move || {
            let mut c = counter_clone.write().unwrap();
            *c += 1;
            format!("Generated {}", *c)
        });

        let result1 = lazy.render();
        let result2 = lazy.render(); // Should use cache

        assert_eq!(result1, result2);
        assert_eq!(*counter.read().unwrap(), 1); // Only generated once

        lazy.invalidate();
        let _result3 = lazy.render(); // Should regenerate
        assert_eq!(*counter.read().unwrap(), 2); // Generated twice now
    }

    #[test]
    fn test_palette_utils() {
        let base_color = Hsla {
            h: 200.0,
            s: 0.6,
            l: 0.5,
            a: 1.0,
        };
        let palette = PaletteUtils::generate_palette(base_color, 5);

        assert_eq!(palette.len(), 5);
        assert!(
            palette
                .iter()
                .all(|c| c.h == base_color.h && c.s == base_color.s)
        );

        let complementary = PaletteUtils::complementary_color(base_color);
        assert_eq!(complementary.h, 20.0); // 200 + 180 - 360

        let (triadic1, triadic2) = PaletteUtils::triadic_colors(base_color);
        assert_eq!(triadic1.h, 320.0); // 200 + 120
        assert_eq!(triadic2.h, 80.0); // 200 + 240 - 360
    }

    #[test]
    fn test_render_profiler() {
        let mut profiler = RenderProfiler::new();

        profiler.start_operation("test_render");
        std::thread::sleep(Duration::from_millis(1));
        let duration = profiler.end_operation("test_render");

        assert!(duration.is_some());
        assert!(duration.unwrap() >= Duration::from_millis(1));

        let stats = profiler.get_stats("test_render").unwrap();
        assert_eq!(stats.count, 1);
        assert!(stats.total_time >= Duration::from_millis(1));
    }
}
