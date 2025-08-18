// ABOUTME: Fuzzy matching implementation for completion filtering
// ABOUTME: Based on modern fuzzy matching algorithms with score calculation and highlighting

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::completion_v2::{StringMatch, StringMatchCandidate};

/// Configuration for fuzzy matching behavior
#[derive(Debug, Clone)]
pub struct FuzzyConfig {
    /// Whether matching should be case sensitive
    pub case_sensitive: bool,
    /// Bonus score for consecutive character matches
    pub consecutive_bonus: u16,
    /// Bonus score for word boundary matches
    pub word_boundary_bonus: u16,
    /// Bonus score for prefix matches
    pub prefix_bonus: u16,
    /// Penalty for gaps between matches
    pub gap_penalty: u16,
}

impl Default for FuzzyConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            consecutive_bonus: 15,
            word_boundary_bonus: 30,
            prefix_bonus: 50,
            gap_penalty: 3,
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

    let mut matches = Vec::new();
    let query_lower = if config.case_sensitive {
        query.clone()
    } else {
        query.to_lowercase()
    };

    for candidate in candidates {
        // Check for cancellation periodically
        if cancel_flag.load(Ordering::Relaxed) {
            return Vec::new();
        }

        let text = if config.case_sensitive {
            candidate.text.clone()
        } else {
            candidate.text.to_lowercase()
        };

        if let Some((score, positions)) = fuzzy_match_score(&text, &query_lower, &config) {
            matches.push(StringMatch::new(candidate.id, score, positions));
        }

        // Yield control periodically for async cancellation
        if matches.len() % 100 == 0 {
            tokio::task::yield_now().await;
        }
    }

    // Sort by score (highest first)
    matches.sort();

    // Limit results
    matches.truncate(max_results);

    matches
}

/// Calculate fuzzy match score and positions for a single string
///
/// Returns None if the query doesn't match the text at all
/// Returns Some((score, positions)) where positions are character indices that matched
fn fuzzy_match_score(text: &str, query: &str, config: &FuzzyConfig) -> Option<(u16, Vec<usize>)> {
    if query.is_empty() {
        return Some((100, vec![]));
    }

    let text_chars: Vec<char> = text.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();

    // Check if all query characters exist in the text
    let mut text_idx = 0;
    let mut matched_positions = Vec::new();

    for &query_char in &query_chars {
        let mut found = false;
        while text_idx < text_chars.len() {
            if text_chars[text_idx] == query_char {
                matched_positions.push(text_idx);
                text_idx += 1;
                found = true;
                break;
            }
            text_idx += 1;
        }
        if !found {
            return None; // Query character not found
        }
    }

    // Calculate score based on match quality
    let score = calculate_match_score(&text_chars, &matched_positions, config);

    Some((score, matched_positions))
}

/// Calculate the quality score for a match
fn calculate_match_score(text_chars: &[char], positions: &[usize], config: &FuzzyConfig) -> u16 {
    if positions.is_empty() {
        return 0;
    }

    let mut score = 100; // Base score

    // Prefix bonus - higher score if match starts at beginning
    if positions[0] == 0 {
        score += config.prefix_bonus;
    }

    // Consecutive character bonus
    let mut consecutive_count = 0;
    for i in 1..positions.len() {
        if positions[i] == positions[i - 1] + 1 {
            consecutive_count += 1;
            score += config.consecutive_bonus;
        } else {
            consecutive_count = 0;
        }
    }

    // Word boundary bonus
    for &pos in positions {
        if is_word_boundary(text_chars, pos) {
            score += config.word_boundary_bonus;
        }
    }

    // Gap penalty - reduce score for large gaps between matches
    for i in 1..positions.len() {
        let gap = positions[i] - positions[i - 1] - 1;
        if gap > 0 {
            score = score.saturating_sub(config.gap_penalty * gap as u16);
        }
    }

    // Length bonus - shorter strings with matches are better
    let length_penalty = (text_chars.len() as u16).saturating_sub(100) / 10;
    score = score.saturating_sub(length_penalty);

    score
}

/// Check if a position is at a word boundary
fn is_word_boundary(text_chars: &[char], pos: usize) -> bool {
    if pos == 0 {
        return true; // Start of string is always word boundary
    }

    let current_char = text_chars[pos];
    let prev_char = text_chars[pos - 1];

    // Word boundary if:
    // - Previous char is not alphanumeric and current is
    // - Previous char is lowercase and current is uppercase
    // - Previous char is separator (_ - . / \ etc.)

    if !prev_char.is_alphanumeric() && current_char.is_alphanumeric() {
        return true;
    }

    if prev_char.is_lowercase() && current_char.is_uppercase() {
        return true;
    }

    matches!(prev_char, '_' | '-' | '.' | '/' | '\\' | ' ' | '\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let candidates = vec![StringMatchCandidate::new(1, "hello".to_string())];
        let config = FuzzyConfig::default();

        let result = fuzzy_match_score("hello", "hello", &config);
        assert!(result.is_some());
        let (score, positions) = result.unwrap();
        assert!(score > 200); // Should have high score due to prefix bonus
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_prefix_match() {
        let config = FuzzyConfig::default();

        let result = fuzzy_match_score("hello_world", "hello", &config);
        assert!(result.is_some());
        let (score, positions) = result.unwrap();
        assert!(score > 200); // Should have high score due to prefix bonus
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_subsequence_match() {
        let config = FuzzyConfig::default();

        let result = fuzzy_match_score("hello_world", "hlwrd", &config);
        assert!(result.is_some());
        let (score, positions) = result.unwrap();
        assert!(score > 100); // Should match but with lower score
        assert_eq!(positions, vec![0, 2, 6, 8, 10]);
    }

    #[test]
    fn test_no_match() {
        let config = FuzzyConfig::default();

        let result = fuzzy_match_score("hello", "xyz", &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_case_sensitivity() {
        let config_sensitive = FuzzyConfig {
            case_sensitive: true,
            ..Default::default()
        };

        let config_insensitive = FuzzyConfig {
            case_sensitive: false,
            ..Default::default()
        };

        // Case sensitive should not match
        assert!(fuzzy_match_score("Hello", "hello", &config_sensitive).is_none());

        // Case insensitive should match
        assert!(fuzzy_match_score("hello", "hello", &config_insensitive).is_some());
    }

    #[test]
    fn test_word_boundary_detection() {
        // Start of string
        assert!(is_word_boundary(&['h', 'e', 'l', 'l', 'o'], 0));

        // After underscore
        assert!(is_word_boundary(&['h', 'e', '_', 'w', 'o'], 3));

        // After dash
        assert!(is_word_boundary(&['h', 'e', '-', 'w', 'o'], 3));

        // Camel case
        assert!(is_word_boundary(&['h', 'e', 'l', 'W', 'o'], 3));

        // Not word boundary
        assert!(!is_word_boundary(&['h', 'e', 'l', 'l', 'o'], 2));
    }

    #[test]
    fn test_consecutive_bonus() {
        let config = FuzzyConfig::default();

        // "hel" should get consecutive bonus
        let result1 = fuzzy_match_score("hello", "hel", &config);
        assert!(result1.is_some());

        // "heo" should not get consecutive bonus
        let result2 = fuzzy_match_score("hello", "heo", &config);
        assert!(result2.is_some());

        // Consecutive match should score higher
        assert!(result1.unwrap().0 > result2.unwrap().0);
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
        let config = FuzzyConfig::default();

        let result = fuzzy_match_score("hello", "", &config);
        assert!(result.is_some());
        let (score, positions) = result.unwrap();
        assert_eq!(score, 100);
        assert!(positions.is_empty());
    }
}
