// ABOUTME: Language-aware prefix extraction for completion systems
// ABOUTME: Combines Helix's LSP integration with Zed's sophisticated character classification

use std::collections::HashSet;

/// Language-aware completion character classifier inspired by Zed's approach
/// but optimized for LSP integration like Helix
pub struct PrefixExtractor {
    /// Characters that can be part of identifiers (alphanumeric + language-specific)
    identifier_chars: HashSet<char>,
    /// Characters that trigger method/property completion (dots, arrows, etc)
    trigger_chars: HashSet<char>,
    /// Characters that separate completion contexts (whitespace, operators)
    separator_chars: HashSet<char>,
}

impl Default for PrefixExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PrefixExtractor {
    pub fn new() -> Self {
        // Base identifier characters (Helix approach)
        let mut identifier_chars = HashSet::new();
        for c in 'a'..='z' {
            identifier_chars.insert(c);
        }
        for c in 'A'..='Z' {
            identifier_chars.insert(c);
        }
        for c in '0'..='9' {
            identifier_chars.insert(c);
        }
        identifier_chars.insert('_');

        // Language-specific identifier extensions (Zed approach)
        identifier_chars.insert('-'); // CSS properties, Lisp
        identifier_chars.insert('$'); // PHP, JavaScript
        identifier_chars.insert('@'); // Annotations, decorators

        // Completion trigger characters
        let mut trigger_chars = HashSet::new();
        trigger_chars.insert('.'); // Method/property access
        trigger_chars.insert(':'); // CSS, namespace resolution
        trigger_chars.insert('>'); // Arrow operator (part of ->)

        // Separator characters (end completion context)
        let mut separator_chars = HashSet::new();
        separator_chars.insert(' ');
        separator_chars.insert('\t');
        separator_chars.insert('\n');
        separator_chars.insert('(');
        separator_chars.insert(')');
        separator_chars.insert('[');
        separator_chars.insert(']');
        separator_chars.insert('{');
        separator_chars.insert('}');
        separator_chars.insert(',');
        separator_chars.insert(';');

        Self {
            identifier_chars,
            trigger_chars,
            separator_chars,
        }
    }

    /// Extract completion prefix from text at cursor position
    /// Returns (prefix, is_trigger_completion)
    pub fn extract_prefix(&self, line_text: &str, cursor_col: usize) -> (String, bool) {
        if cursor_col == 0 || line_text.is_empty() {
            return (String::new(), false);
        }

        let line_text_to_cursor = &line_text[..cursor_col.min(line_text.len())];
        let chars: Vec<char> = line_text_to_cursor.chars().collect();

        if chars.is_empty() {
            return (String::new(), false);
        }

        // Check if we're in a trigger completion context (e.g., "obj.method")
        let is_trigger_completion = self.is_trigger_context(&chars);

        if is_trigger_completion {
            // For trigger completions, prefix starts after the trigger character
            self.extract_trigger_prefix(&chars)
        } else {
            // For normal completions, extract the current identifier
            self.extract_identifier_prefix(&chars)
        }
    }

    fn is_trigger_context(&self, chars: &[char]) -> bool {
        // Look for trigger characters walking backwards until we hit a separator
        for i in (0..chars.len()).rev() {
            let c = chars[i];
            if self.trigger_chars.contains(&c) {
                return true;
            }
            if self.separator_chars.contains(&c) {
                break; // Hit separator before trigger
            }
        }
        false
    }

    fn extract_trigger_prefix(&self, chars: &[char]) -> (String, bool) {
        // Find the most recent trigger character
        for i in (0..chars.len()).rev() {
            if self.trigger_chars.contains(&chars[i]) {
                // Extract everything after the trigger as prefix
                let prefix: String = chars[i + 1..].iter().collect();
                return (prefix, true);
            }
        }

        // Fallback to identifier extraction if no trigger found
        self.extract_identifier_prefix(chars)
    }

    fn extract_identifier_prefix(&self, chars: &[char]) -> (String, bool) {
        // Walk backwards to find the start of the current identifier
        let mut start_pos = chars.len();

        for i in (0..chars.len()).rev() {
            let c = chars[i];
            if self.identifier_chars.contains(&c) {
                start_pos = i;
            } else {
                break;
            }
        }

        let prefix: String = chars[start_pos..].iter().collect();
        (prefix, false)
    }

    /// Language-specific configuration for different file types
    pub fn configure_for_language(&mut self, language: &str) {
        match language {
            "rust" => {
                // Don't add ':' to identifier_chars - it should trigger completion
                self.trigger_chars.insert(':');
            }
            "javascript" | "typescript" => {
                self.identifier_chars.insert('$');
                self.trigger_chars.insert('.');
            }
            "css" | "scss" | "less" => {
                self.identifier_chars.insert('-');
                self.trigger_chars.insert(':');
            }
            "php" => {
                self.identifier_chars.insert('$');
                self.trigger_chars.insert('-'); // For ->
            }
            "c" | "cpp" => {
                self.trigger_chars.insert('-'); // For ->
                self.trigger_chars.insert(':'); // For ::
            }
            _ => {} // Use defaults
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_identifier_extraction() {
        let extractor = PrefixExtractor::new();

        // Basic word completion
        let (prefix, is_trigger) = extractor.extract_prefix("let variable_name", 17);
        assert_eq!(prefix, "variable_name");
        assert!(!is_trigger);

        // Partial word
        let (prefix, is_trigger) = extractor.extract_prefix("let var", 7);
        assert_eq!(prefix, "var");
        assert!(!is_trigger);
    }

    #[test]
    fn test_dot_notation_completion() {
        let extractor = PrefixExtractor::new();

        // Method completion after dot
        let (prefix, is_trigger) = extractor.extract_prefix("client.method", 13);
        assert_eq!(prefix, "method");
        assert!(is_trigger);

        // Empty prefix right after dot
        let (prefix, is_trigger) = extractor.extract_prefix("client.", 7);
        assert_eq!(prefix, "");
        assert!(is_trigger);

        // Chained method calls
        let (prefix, is_trigger) = extractor.extract_prefix("obj.method().another", 20);
        assert_eq!(prefix, "another");
        assert!(is_trigger);
    }

    #[test]
    fn test_language_specific_features() {
        let mut extractor = PrefixExtractor::new();
        extractor.configure_for_language("rust");

        // Rust namespace resolution
        let (prefix, is_trigger) = extractor.extract_prefix("std::collections::HashMap", 25);
        assert_eq!(prefix, "HashMap");
        assert!(is_trigger);
    }

    #[test]
    fn test_separators_end_completion() {
        let extractor = PrefixExtractor::new();

        // Parentheses should end completion context
        let (prefix, is_trigger) = extractor.extract_prefix("function(param", 14);
        assert_eq!(prefix, "param");
        assert!(!is_trigger);

        // Comma should end completion context
        let (prefix, is_trigger) = extractor.extract_prefix("func(a, b", 9);
        assert_eq!(prefix, "b");
        assert!(!is_trigger);
    }
}
