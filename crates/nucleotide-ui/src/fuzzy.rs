// ABOUTME: Fuzzy matching implementation using nucleo (same as Helix)
// ABOUTME: High-performance fuzzy matcher with prefix preference and sophisticated scoring

use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Import nucleo components like Helix does
use nucleo::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo::{Config, Utf32Str};
use parking_lot::Mutex;

use crate::completion_v2::{StringMatch, StringMatchCandidate};

/// LazyMutex implementation exactly like Helix
pub struct LazyMutex<T> {
    inner: Mutex<Option<T>>,
    init: fn() -> T,
}

impl<T> LazyMutex<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            inner: Mutex::new(None),
            init,
        }
    }

    pub fn lock(&self) -> impl DerefMut<Target = T> + '_ {
        parking_lot::MutexGuard::map(self.inner.lock(), |val| val.get_or_insert_with(self.init))
    }
}

/// Global matcher instance, exactly like Helix
pub static MATCHER: LazyMutex<nucleo::Matcher> = LazyMutex::new(nucleo::Matcher::default);

/// Configuration for fuzzy matching behavior - using nucleo defaults
#[derive(Debug, Clone)]
pub struct FuzzyConfig {
    /// Whether to prefer prefix matches (like Helix)
    pub prefer_prefix: bool,
    /// Case matching strategy
    pub case_matching: CaseMatching,
    /// Text normalization strategy  
    pub normalization: Normalization,
}

impl Default for FuzzyConfig {
    fn default() -> Self {
        Self {
            prefer_prefix: true,                 // Key setting from Helix!
            case_matching: CaseMatching::Ignore, // Like Helix for completions
            normalization: Normalization::Smart,
        }
    }
}

/// Perform fuzzy matching on a collection of candidates
///
/// # Arguments
/// * `candidates` - The candidates to match against
/// * `query` - The search query
/// * `config` - Matching configuration
/// * `max_results` - Maximum number of results to return
/// * `cancel_flag` - Flag to check for cancellation
///
/// # Returns
/// Vector of matches sorted by score (highest first)
pub async fn match_strings(
    candidates: Vec<StringMatchCandidate>,
    query: String,
    config: FuzzyConfig,
    max_results: usize,
    cancel_flag: Arc<AtomicBool>,
) -> Vec<StringMatch> {
    if query.is_empty() {
        // Return all candidates with equal score when query is empty
        return candidates
            .into_iter()
            .take(max_results)
            .map(|candidate| StringMatch::new(candidate.id, 100, vec![]))
            .collect();
    }

    // Use nucleo matcher exactly like Helix
    let mut matcher = MATCHER.lock();
    matcher.config = Config::DEFAULT;
    matcher.config.prefer_prefix = config.prefer_prefix;

    // Create atom pattern like Helix does for completions
    let pattern = Atom::new(
        &query,
        config.case_matching,
        config.normalization,
        AtomKind::Fuzzy,
        false,
    );

    let mut matches = Vec::new();
    let mut utf32_buf = Vec::new();

    // Calculate minimum score threshold like Helix does
    let min_score = (7 + query.len() as u32 * 14) / 3;

    for candidate in candidates {
        // Check for cancellation periodically
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        let text = &candidate.text;

        // Score using nucleo
        if let Some(score) = pattern.score(Utf32Str::new(text, &mut utf32_buf), &mut matcher) {
            // Normalize score like Helix (divide by 3 for full matches)
            let normalized_score = (score as u32) / 3;

            // Only include matches above minimum threshold
            if normalized_score > min_score {
                // For now, we don't extract positions from nucleo (would require more complex integration)
                // TODO: Extract match positions for highlighting
                matches.push(StringMatch::new(
                    candidate.id,
                    normalized_score as u16,
                    vec![],
                ));
            }
        }

        // Yield control periodically for async cancellation
        if matches.len() % 100 == 0 {
            tokio::task::yield_now().await;
        }
    }

    // Sort by score (higher first)
    matches.sort_by(|a, b| b.score.cmp(&a.score));

    // Limit results
    matches.truncate(max_results);

    matches
}

/// Convenience function for simple fuzzy matching (following Helix pattern)
/// This is similar to Helix's fuzzy_match function but adapted for our types
pub fn fuzzy_match<T: AsRef<str>>(
    pattern: &str,
    items: impl IntoIterator<Item = T>,
    prefer_prefix: bool,
) -> Vec<(T, u16)> {
    let mut matcher = MATCHER.lock();
    matcher.config = Config::DEFAULT;
    matcher.config.prefer_prefix = prefer_prefix;

    let pattern = Atom::new(
        pattern,
        CaseMatching::Ignore, // Like Helix for completions
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );

    // Use nucleo's built-in match_list method
    pattern.match_list(items, &mut matcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_simple() {
        let items: Vec<&str> = vec!["hello", "help", "world"];
        let matches = fuzzy_match("hel", items, true);

        // Should find matches for "hello" and "help"
        assert!(matches.len() >= 2);
        assert!(matches.iter().any(|(item, _)| *item == "hello"));
        assert!(matches.iter().any(|(item, _)| *item == "help"));
    }

    #[test]
    fn test_fuzzy_match_prefix_preference() {
        let items: Vec<&str> = vec!["print_hello", "hello_print", "printf"];
        let matches = fuzzy_match("print", items, true);

        // Should prefer prefix matches
        assert!(!matches.is_empty());
        // print_hello and printf should score higher than hello_print
        let print_hello_score = matches
            .iter()
            .find(|(item, _)| *item == "print_hello")
            .map(|(_, score)| *score);
        let hello_print_score = matches
            .iter()
            .find(|(item, _)| *item == "hello_print")
            .map(|(_, score)| *score);

        if let (Some(prefix_score), Some(suffix_score)) = (print_hello_score, hello_print_score) {
            assert!(prefix_score >= suffix_score);
        }
    }

    #[tokio::test]
    async fn test_async_matching() {
        let candidates = vec![
            StringMatchCandidate::new(1, "hello_world".to_string()),
            StringMatchCandidate::new(2, "hello_rust".to_string()),
            StringMatchCandidate::new(3, "goodbye_world".to_string()),
            StringMatchCandidate::new(4, "test_hello".to_string()),
        ];

        let config = FuzzyConfig::default();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let matches = match_strings(candidates, "hello".to_string(), config, 10, cancel_flag).await;

        // Should find 3 matches (all containing "hello")
        assert_eq!(matches.len(), 3);

        // Results should be sorted by score
        assert!(matches[0].score >= matches[1].score);
        assert!(matches[1].score >= matches[2].score);
    }

    #[tokio::test]
    async fn test_cancellation() {
        let candidates = vec![
            StringMatchCandidate::new(1, "test1".to_string()),
            StringMatchCandidate::new(2, "test2".to_string()),
        ];

        let config = FuzzyConfig::default();
        let cancel_flag = Arc::new(AtomicBool::new(true)); // Pre-cancelled

        let matches = match_strings(candidates, "test".to_string(), config, 10, cancel_flag).await;

        // Should return empty due to cancellation
        assert!(matches.is_empty());
    }

    #[test]
    fn test_empty_query() {
        let items: Vec<&str> = vec!["hello", "world"];
        let matches = fuzzy_match("", items, false);

        // Empty query should return all items
        assert_eq!(matches.len(), 2);
    }
}
