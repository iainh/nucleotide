// ABOUTME: LRU caching system for completion results to improve performance
// ABOUTME: Provides intelligent caching with configurable size and invalidation logic

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::completion_v2::{Position, StringMatch};

/// Cache key for completion results
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    /// The query string used for filtering
    pub query: String,
    /// The position where completion was triggered
    pub position: Option<Position>,
    /// Hash of the completion items (to detect when items change)
    pub items_hash: u64,
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.query.hash(state);
        self.position.hash(state);
        self.items_hash.hash(state);
    }
}

impl CacheKey {
    pub fn new(query: String, position: Option<Position>, items_hash: u64) -> Self {
        Self {
            query,
            position,
            items_hash,
        }
    }
}

/// Cached completion result with metadata
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The cached completion results
    pub matches: Vec<StringMatch>,
    /// When this entry was created
    pub created_at: Instant,
    /// How many times this entry has been accessed
    pub access_count: u64,
    /// Last time this entry was accessed
    pub last_accessed: Instant,
}

impl CacheEntry {
    pub fn new(matches: Vec<StringMatch>) -> Self {
        let now = Instant::now();
        Self {
            matches,
            created_at: now,
            access_count: 1,
            last_accessed: now,
        }
    }

    /// Mark this entry as accessed
    pub fn touch(&mut self) {
        self.access_count += 1;
        self.last_accessed = Instant::now();
    }

    /// Check if this entry has expired
    pub fn is_expired(&self, max_age: Duration) -> bool {
        self.created_at.elapsed() > max_age
    }
}

/// LRU cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum number of entries to store
    pub max_entries: usize,
    /// Maximum age for cache entries
    pub max_age: Duration,
    /// Whether to enable cache metrics
    pub enable_metrics: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 100,
            max_age: Duration::from_secs(300), // 5 minutes
            enable_metrics: true,
        }
    }
}

/// Cache metrics for monitoring performance
#[derive(Debug, Clone, Default)]
pub struct CacheMetrics {
    /// Total number of cache lookups
    pub total_lookups: u64,
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of entries evicted due to size limit
    pub evictions: u64,
    /// Number of entries expired due to age
    pub expirations: u64,
}

impl CacheMetrics {
    /// Calculate hit ratio as a percentage
    pub fn hit_ratio(&self) -> f64 {
        if self.total_lookups == 0 {
            0.0
        } else {
            (self.hits as f64 / self.total_lookups as f64) * 100.0
        }
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// LRU cache for completion results
pub struct CompletionCache {
    /// The actual cache storage
    cache: HashMap<CacheKey, CacheEntry>,
    /// Access order for LRU eviction
    access_order: Vec<CacheKey>,
    /// Cache configuration
    config: CacheConfig,
    /// Performance metrics
    metrics: CacheMetrics,
}

impl CompletionCache {
    /// Create a new completion cache with default configuration
    pub fn new() -> Self {
        Self::with_config(CacheConfig::default())
    }

    /// Create a new completion cache with custom configuration
    pub fn with_config(config: CacheConfig) -> Self {
        Self {
            cache: HashMap::new(),
            access_order: Vec::new(),
            config,
            metrics: CacheMetrics::default(),
        }
    }

    /// Get cached results for a key
    pub fn get(&mut self, key: &CacheKey) -> Option<Vec<StringMatch>> {
        if self.config.enable_metrics {
            self.metrics.total_lookups += 1;
        }

        // Check if key exists in cache
        if let Some(entry) = self.cache.get_mut(key) {
            // Check if entry has expired
            if entry.is_expired(self.config.max_age) {
                // Remove expired entry
                self.cache.remove(key);
                self.access_order.retain(|k| k != key);
                if self.config.enable_metrics {
                    self.metrics.expirations += 1;
                    self.metrics.misses += 1;
                }
                return None;
            }

            // Mark as accessed
            entry.touch();

            // Update access order (move to front)
            self.access_order.retain(|k| k != key);
            self.access_order.push(key.clone());

            if self.config.enable_metrics {
                self.metrics.hits += 1;
            }

            Some(entry.matches.clone())
        } else {
            if self.config.enable_metrics {
                self.metrics.misses += 1;
            }
            None
        }
    }

    /// Insert results into cache
    pub fn insert(&mut self, key: CacheKey, matches: Vec<StringMatch>) {
        // Remove existing entry if present
        if self.cache.contains_key(&key) {
            self.access_order.retain(|k| k != &key);
        }

        // Check if we need to evict entries
        while self.cache.len() >= self.config.max_entries {
            self.evict_oldest();
        }

        // Insert new entry
        let entry = CacheEntry::new(matches);
        self.cache.insert(key.clone(), entry);
        self.access_order.push(key);
    }

    /// Evict the oldest (least recently used) entry
    fn evict_oldest(&mut self) {
        if let Some(oldest_key) = self.access_order.first().cloned() {
            self.cache.remove(&oldest_key);
            self.access_order.remove(0);
            if self.config.enable_metrics {
                self.metrics.evictions += 1;
            }
        }
    }

    /// Clear all cache entries
    pub fn clear(&mut self) {
        self.cache.clear();
        self.access_order.clear();
        if self.config.enable_metrics {
            self.metrics.reset();
        }
    }

    /// Remove expired entries
    pub fn cleanup_expired(&mut self) {
        let expired_keys: Vec<CacheKey> = self
            .cache
            .iter()
            .filter(|(_, entry)| entry.is_expired(self.config.max_age))
            .map(|(key, _)| key.clone())
            .collect();

        for key in expired_keys {
            self.cache.remove(&key);
            self.access_order.retain(|k| k != &key);
            if self.config.enable_metrics {
                self.metrics.expirations += 1;
            }
        }
    }

    /// Get current cache size
    pub fn size(&self) -> usize {
        self.cache.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Get cache metrics
    pub fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }

    /// Invalidate cache entries that depend on a specific items hash
    pub fn invalidate_items(&mut self, items_hash: u64) {
        let keys_to_remove: Vec<CacheKey> = self
            .cache
            .keys()
            .filter(|key| key.items_hash == items_hash)
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.cache.remove(&key);
            self.access_order.retain(|k| k != &key);
        }
    }

    /// Check if a query can be optimized using cached results
    pub fn can_optimize_query(&self, base_query: &str, new_query: &str) -> bool {
        // New query should be an extension of base query
        if !new_query.starts_with(base_query) {
            return false;
        }

        // Check if we have cached results for the base query
        self.cache.keys().any(|key| key.query == base_query)
    }

    /// Get cached results for query optimization
    pub fn get_optimization_base(
        &mut self,
        base_query: &str,
        items_hash: u64,
    ) -> Option<Vec<StringMatch>> {
        // Find the best matching cache entry for the base query
        let matching_key = self
            .cache
            .keys()
            .find(|key| key.query == base_query && key.items_hash == items_hash)
            .cloned();

        if let Some(key) = matching_key {
            self.get(&key)
        } else {
            None
        }
    }
}

impl Default for CompletionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion_v2::StringMatch;

    #[test]
    fn test_cache_basic_operations() {
        let mut cache = CompletionCache::new();

        let key = CacheKey::new("test".to_string(), None, 123);
        let matches = vec![StringMatch::new(1, 100, vec![0, 1, 2])];

        // Initially empty
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.size(), 0);

        // Insert and retrieve
        cache.insert(key.clone(), matches.clone());
        assert_eq!(cache.size(), 1);

        let retrieved = cache.get(&key).unwrap();
        assert_eq!(retrieved.len(), matches.len());
        assert_eq!(retrieved[0].candidate_id, matches[0].candidate_id);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let config = CacheConfig {
            max_entries: 2,
            max_age: Duration::from_secs(60),
            enable_metrics: true,
        };
        let mut cache = CompletionCache::with_config(config);

        let key1 = CacheKey::new("test1".to_string(), None, 123);
        let key2 = CacheKey::new("test2".to_string(), None, 123);
        let key3 = CacheKey::new("test3".to_string(), None, 123);

        let matches = vec![StringMatch::new(1, 100, vec![0])];

        // Fill cache to capacity
        cache.insert(key1.clone(), matches.clone());
        cache.insert(key2.clone(), matches.clone());
        assert_eq!(cache.size(), 2);

        // Access key1 to make it more recently used
        cache.get(&key1);

        // Insert key3, should evict key2 (least recently used)
        cache.insert(key3.clone(), matches);
        assert_eq!(cache.size(), 2);

        // key1 and key3 should exist, key2 should be evicted
        assert!(cache.get(&key1).is_some());
        assert!(cache.get(&key2).is_none());
        assert!(cache.get(&key3).is_some());
    }

    #[test]
    fn test_cache_expiration() {
        let config = CacheConfig {
            max_entries: 10,
            max_age: Duration::from_millis(10), // Short but reliable expiration
            enable_metrics: true,
        };
        let mut cache = CompletionCache::with_config(config);

        let key = CacheKey::new("test".to_string(), None, 123);
        let matches = vec![StringMatch::new(1, 100, vec![0])];

        cache.insert(key.clone(), matches);
        assert!(cache.get(&key).is_some());

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(15));

        // Should be expired now
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_cache_metrics() {
        let mut cache = CompletionCache::new();
        let key = CacheKey::new("test".to_string(), None, 123);
        let matches = vec![StringMatch::new(1, 100, vec![0])];

        // Initial metrics
        let metrics = cache.metrics();
        assert_eq!(metrics.total_lookups, 0);
        assert_eq!(metrics.hits, 0);
        assert_eq!(metrics.misses, 0);

        // Cache miss
        cache.get(&key);
        assert_eq!(cache.metrics().total_lookups, 1);
        assert_eq!(cache.metrics().misses, 1);
        assert_eq!(cache.metrics().hit_ratio(), 0.0);

        // Insert and hit
        cache.insert(key.clone(), matches);
        cache.get(&key);

        assert_eq!(cache.metrics().total_lookups, 2);
        assert_eq!(cache.metrics().hits, 1);
        assert_eq!(cache.metrics().misses, 1);
        assert_eq!(cache.metrics().hit_ratio(), 50.0);
    }

    #[test]
    fn test_cache_invalidation() {
        let mut cache = CompletionCache::new();

        let key1 = CacheKey::new("test1".to_string(), None, 123);
        let key2 = CacheKey::new("test2".to_string(), None, 456);
        let matches = vec![StringMatch::new(1, 100, vec![0])];

        cache.insert(key1.clone(), matches.clone());
        cache.insert(key2.clone(), matches);
        assert_eq!(cache.size(), 2);

        // Invalidate entries with items_hash 123
        cache.invalidate_items(123);
        assert_eq!(cache.size(), 1);
        assert!(cache.get(&key1).is_none());
        assert!(cache.get(&key2).is_some());
    }

    #[test]
    fn test_query_optimization_detection() {
        let mut cache = CompletionCache::new();

        let key = CacheKey::new("test".to_string(), None, 123);
        let matches = vec![StringMatch::new(1, 100, vec![0])];

        cache.insert(key, matches);

        // Should detect optimization opportunity
        assert!(cache.can_optimize_query("test", "testing"));
        assert!(cache.can_optimize_query("test", "test_func"));

        // Should not optimize for non-extensions
        assert!(!cache.can_optimize_query("test", "other"));
        assert!(!cache.can_optimize_query("testing", "test"));
    }

    #[test]
    fn test_cleanup_expired() {
        let config = CacheConfig {
            max_entries: 10,
            max_age: Duration::from_millis(1),
            enable_metrics: true,
        };
        let mut cache = CompletionCache::with_config(config);

        let key1 = CacheKey::new("test1".to_string(), None, 123);
        let key2 = CacheKey::new("test2".to_string(), None, 123);
        let matches = vec![StringMatch::new(1, 100, vec![0])];

        cache.insert(key1, matches.clone());

        // Wait for first entry to expire
        std::thread::sleep(Duration::from_millis(2));

        cache.insert(key2.clone(), matches);
        assert_eq!(cache.size(), 2);

        // Clean up expired entries
        cache.cleanup_expired();
        assert_eq!(cache.size(), 1);
        assert!(cache.get(&key2).is_some());
    }
}
