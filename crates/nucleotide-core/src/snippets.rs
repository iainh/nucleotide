// ABOUTME: LSP snippet parsing and cursor positioning support
// ABOUTME: Handles LSP snippet format with tabstops and placeholders

use nucleotide_logging::{debug, instrument, warn};

/// A parsed LSP snippet with tabstops and text parts
#[derive(Debug, Clone, PartialEq)]
pub struct SnippetTemplate {
    pub text_parts: Vec<TextPart>,
    pub tabstops: Vec<Tabstop>,
    pub final_cursor_pos: Option<usize>, // Position of $0 tabstop
}

/// Parts of a snippet - either literal text or a tabstop
#[derive(Debug, Clone, PartialEq)]
pub enum TextPart {
    /// Plain text to be inserted
    Literal(String),
    /// Reference to a tabstop
    Tabstop {
        index: usize,
        placeholder: Option<String>,
    },
}

/// A tabstop in the snippet
#[derive(Debug, Clone, PartialEq)]
pub struct Tabstop {
    pub index: usize,
    pub placeholder: Option<String>,
    pub position: usize, // Character position in final rendered text
}

impl SnippetTemplate {
    /// Parse an LSP snippet string into a SnippetTemplate
    #[instrument]
    pub fn parse(snippet: &str) -> Result<Self, SnippetParseError> {
        debug!(snippet = %snippet, "Parsing LSP snippet");

        let parser = SnippetParser::new(snippet);
        parser.parse()
    }

    /// Render the snippet as plain text without tabstop markers
    pub fn render_plain_text(&self) -> String {
        let mut result = String::new();

        for part in &self.text_parts {
            match part {
                TextPart::Literal(text) => result.push_str(text),
                TextPart::Tabstop { placeholder, .. } => {
                    if let Some(placeholder_text) = placeholder {
                        result.push_str(placeholder_text);
                    }
                }
            }
        }

        result
    }

    /// Calculate the absolute cursor position after insertion
    /// Returns the position where $0 should be placed, or None if no $0 tabstop
    pub fn calculate_final_cursor_position(&self, insertion_start: usize) -> Option<usize> {
        let plain_text = self.render_plain_text();

        if let Some(relative_pos) = self.final_cursor_pos {
            Some(insertion_start + relative_pos)
        } else {
            // If no $0 tabstop, position cursor at end of insertion
            Some(insertion_start + plain_text.chars().count())
        }
    }
}

/// Error type for snippet parsing
#[derive(Debug, Clone)]
pub struct SnippetParseError {
    pub message: String,
    pub position: usize,
}

impl std::fmt::Display for SnippetParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Snippet parse error at position {}: {}",
            self.position, self.message
        )
    }
}

impl std::error::Error for SnippetParseError {}

/// Parser for LSP snippet syntax
struct SnippetParser<'a> {
    input: &'a str,
    chars: std::str::Chars<'a>,
    position: usize,
    current_char: Option<char>,
}

impl<'a> SnippetParser<'a> {
    fn new(input: &'a str) -> Self {
        let mut chars = input.chars();
        let current_char = chars.next();
        Self {
            input,
            chars,
            position: 0,
            current_char,
        }
    }

    fn parse(mut self) -> Result<SnippetTemplate, SnippetParseError> {
        let mut text_parts = Vec::new();
        let mut tabstops = Vec::new();
        let mut current_text = String::new();
        let mut final_cursor_pos = None;
        let mut char_position = 0;

        while let Some(ch) = self.current_char {
            match ch {
                '$' => {
                    // Save any accumulated text
                    if !current_text.is_empty() {
                        text_parts.push(TextPart::Literal(current_text.clone()));
                        char_position += current_text.chars().count();
                        current_text.clear();
                    }

                    // Parse the tabstop
                    self.advance();
                    let (tabstop_index, placeholder) = self.parse_tabstop()?;

                    if tabstop_index == 0 {
                        // $0 is the final cursor position
                        final_cursor_pos = Some(char_position);
                    } else {
                        // Regular tabstop
                        let tabstop = Tabstop {
                            index: tabstop_index,
                            placeholder: placeholder.clone(),
                            position: char_position,
                        };

                        text_parts.push(TextPart::Tabstop {
                            index: tabstop_index,
                            placeholder: placeholder.clone(),
                        });

                        tabstops.push(tabstop);

                        // Add placeholder text length to position tracking
                        if let Some(ref placeholder_text) = placeholder {
                            char_position += placeholder_text.chars().count();
                        }
                    }
                }
                '\\' => {
                    // Escaped character
                    self.advance();
                    if let Some(escaped_ch) = self.current_char {
                        current_text.push(escaped_ch);
                        self.advance();
                    }
                }
                _ => {
                    // Regular character
                    current_text.push(ch);
                    self.advance();
                }
            }
        }

        // Add any remaining text
        if !current_text.is_empty() {
            text_parts.push(TextPart::Literal(current_text));
        }

        Ok(SnippetTemplate {
            text_parts,
            tabstops,
            final_cursor_pos,
        })
    }

    fn parse_tabstop(&mut self) -> Result<(usize, Option<String>), SnippetParseError> {
        match self.current_char {
            Some(ch) if ch.is_ascii_digit() => {
                // Simple tabstop like $1, $2, $0
                let index = self.parse_number()?;
                Ok((index, None))
            }
            Some('{') => {
                // Complex tabstop like ${1:placeholder}
                self.advance(); // consume '{'
                let index = self.parse_number()?;

                if self.current_char == Some(':') {
                    // Has placeholder
                    self.advance(); // consume ':'
                    let placeholder = self.parse_placeholder()?;

                    if self.current_char != Some('}') {
                        return Err(SnippetParseError {
                            message: "Expected '}' after placeholder".to_string(),
                            position: self.position,
                        });
                    }
                    self.advance(); // consume '}'
                    Ok((index, Some(placeholder)))
                } else if self.current_char == Some('}') {
                    // No placeholder
                    self.advance(); // consume '}'
                    Ok((index, None))
                } else {
                    Err(SnippetParseError {
                        message: "Expected ':' or '}' in tabstop".to_string(),
                        position: self.position,
                    })
                }
            }
            _ => Err(SnippetParseError {
                message: "Expected digit or '{' after '$'".to_string(),
                position: self.position,
            }),
        }
    }

    fn parse_number(&mut self) -> Result<usize, SnippetParseError> {
        let mut number = String::new();

        while let Some(ch) = self.current_char {
            if ch.is_ascii_digit() {
                number.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if number.is_empty() {
            return Err(SnippetParseError {
                message: "Expected number".to_string(),
                position: self.position,
            });
        }

        number.parse().map_err(|_| SnippetParseError {
            message: "Invalid number".to_string(),
            position: self.position,
        })
    }

    fn parse_placeholder(&mut self) -> Result<String, SnippetParseError> {
        let mut placeholder = String::new();
        let mut brace_count = 0;

        while let Some(ch) = self.current_char {
            match ch {
                '{' => {
                    brace_count += 1;
                    placeholder.push(ch);
                    self.advance();
                }
                '}' => {
                    if brace_count == 0 {
                        break; // End of placeholder
                    } else {
                        brace_count -= 1;
                        placeholder.push(ch);
                        self.advance();
                    }
                }
                '\\' => {
                    // Escaped character in placeholder
                    self.advance();
                    if let Some(escaped_ch) = self.current_char {
                        placeholder.push(escaped_ch);
                        self.advance();
                    }
                }
                _ => {
                    placeholder.push(ch);
                    self.advance();
                }
            }
        }

        Ok(placeholder)
    }

    fn advance(&mut self) {
        self.current_char = self.chars.next();
        self.position += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_snippet() {
        let template = SnippetTemplate::parse("concat!($0)").unwrap();

        assert_eq!(template.text_parts.len(), 2);
        assert!(matches!(template.text_parts[0], TextPart::Literal(ref s) if s == "concat!("));
        assert!(matches!(template.text_parts[1], TextPart::Literal(ref s) if s == ")"));
        assert_eq!(template.final_cursor_pos, Some("concat!(".chars().count()));
    }

    #[test]
    fn test_placeholder_snippet() {
        let template = SnippetTemplate::parse("for ${1:item} in ${2:iterator} {\n\t$0\n}").unwrap();

        assert_eq!(template.tabstops.len(), 2);
        assert_eq!(template.tabstops[0].index, 1);
        assert_eq!(template.tabstops[0].placeholder, Some("item".to_string()));
        assert_eq!(template.tabstops[1].index, 2);
        assert_eq!(
            template.tabstops[1].placeholder,
            Some("iterator".to_string())
        );
        assert!(template.final_cursor_pos.is_some());
    }

    #[test]
    fn test_render_plain_text() {
        let template =
            SnippetTemplate::parse("fn ${1:name}() -> ${2:ReturnType} {\n\t$0\n}").unwrap();
        let plain_text = template.render_plain_text();

        assert_eq!(plain_text, "fn name() -> ReturnType {\n\t\n}");
    }

    #[test]
    fn test_escaped_characters() {
        let template = SnippetTemplate::parse("println!(\"\\${} is a placeholder\", $1);").unwrap();
        let plain_text = template.render_plain_text();

        assert!(plain_text.contains("${} is a placeholder"));
    }

    #[test]
    fn test_no_final_tabstop() {
        let template = SnippetTemplate::parse("fn ${1:name}()").unwrap();

        assert!(template.final_cursor_pos.is_none());
        let cursor_pos = template.calculate_final_cursor_position(0);
        assert_eq!(cursor_pos, Some("fn name()".chars().count()));
    }

    #[test]
    fn test_cursor_position_calculation() {
        let template = SnippetTemplate::parse("std::concat!($0)").unwrap();
        let cursor_pos = template.calculate_final_cursor_position(100).unwrap();

        // Should be at 100 + length of "std::concat!("
        assert_eq!(cursor_pos, 100 + "std::concat!(".chars().count());
    }

    #[test]
    fn test_invalid_snippet() {
        let result = SnippetTemplate::parse("invalid ${1 tabstop");
        assert!(result.is_err());
    }

    #[test]
    fn test_concat_snippet_debug() {
        let template = SnippetTemplate::parse("concat!($0)").unwrap();

        println!("Original snippet: concat!($0)");
        println!("Parsed template: {:#?}", template);

        let rendered = template.render_plain_text();
        println!("Rendered plain text: {}", rendered);

        let cursor_pos = template.calculate_final_cursor_position(0);
        println!("Final cursor position: {:?}", cursor_pos);

        // Verify correct parsing
        assert_eq!(rendered, "concat!()");
        assert_eq!(template.final_cursor_pos, Some("concat!(".chars().count()));
        assert_eq!(cursor_pos, Some("concat!(".chars().count()));
    }
}
