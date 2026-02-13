//! String utility functions

/// Default maximum length for preview text (in characters)
pub const PREVIEW_MAX_LENGTH: usize = 200;

/// Truncate text to max length with ellipsis
pub fn truncate_preview(text: &str, max_len: usize) -> String {
    let text = text.trim();
    if text.chars().count() > max_len {
        format!("{}...", text.chars().take(max_len).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Truncate optional text with default max length
pub fn truncate_preview_opt(text: Option<String>) -> Option<String> {
    text.map(|t| truncate_preview(&t, PREVIEW_MAX_LENGTH))
}

/// Parse a string that may be a JSON array or comma-separated values into a Vec<String>.
///
/// Handles:
/// - JSON arrays: `["a", "b", "c"]`
/// - Comma-separated: `a, b, c`
/// - Mixed/malformed JSON: falls back to comma splitting
pub fn parse_string_array(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('[') {
        // Try JSON array first
        serde_json::from_str(trimmed).unwrap_or_else(|_| {
            // Fallback to comma-separated
            trimmed
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
    } else {
        // Comma-separated
        trimmed
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_string_array_json() {
        let result = parse_string_array(r#"["a", "b", "c"]"#);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_string_array_csv() {
        let result = parse_string_array("a, b, c");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_string_array_csv_no_spaces() {
        let result = parse_string_array("a,b,c");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_string_array_empty() {
        let result = parse_string_array("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_string_array_whitespace() {
        let result = parse_string_array("   ");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_string_array_malformed_json() {
        // Malformed JSON falls back to comma splitting
        let result = parse_string_array("[a, b, c]");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_string_array_single_value() {
        let result = parse_string_array("single");
        assert_eq!(result, vec!["single"]);
    }

    #[test]
    fn test_parse_string_array_json_single() {
        let result = parse_string_array(r#"["single"]"#);
        assert_eq!(result, vec!["single"]);
    }

    #[test]
    fn test_truncate_preview_short() {
        assert_eq!(truncate_preview("hello", PREVIEW_MAX_LENGTH), "hello");
    }

    #[test]
    fn test_truncate_preview_long() {
        let long_text = "a".repeat(300);
        let truncated = truncate_preview(&long_text, PREVIEW_MAX_LENGTH);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= PREVIEW_MAX_LENGTH + 3);
    }

    #[test]
    fn test_truncate_preview_trims_whitespace() {
        assert_eq!(truncate_preview("  hello  ", 100), "hello");
    }

    #[test]
    fn test_truncate_preview_opt_some() {
        let result = truncate_preview_opt(Some("hello".to_string()));
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn test_truncate_preview_opt_none() {
        let result = truncate_preview_opt(None);
        assert_eq!(result, None);
    }
}
