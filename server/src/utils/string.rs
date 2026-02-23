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

/// Check if value is a placeholder (not actual content).
///
/// Frameworks often replace binary content with placeholder strings during
/// logging or when content is too large to include.
pub fn is_placeholder_value(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }

    const PLACEHOLDERS: &[&str] = &[
        "<replaced>",
        "<binary>",
        "<truncated>",
        "<omitted>",
        "<redacted>",
        "<image>",
        "<audio>",
        "<video>",
        "<file>",
        "[binary]",
        "[replaced]",
        "[truncated]",
        "[omitted]",
        "[redacted]",
        "[image]",
        "[audio]",
        "[video]",
        "[file]",
        "...",
        "\u{2026}",
    ];

    let trimmed = s.trim();
    if PLACEHOLDERS.contains(&trimmed) {
        return true;
    }

    // Generic angle bracket placeholders like <...>, <base64 data>, etc.
    if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() < 50 {
        return true;
    }

    // Generic square bracket placeholders
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() < 50 {
        return true;
    }

    false
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

    // ========================================================================
    // is_placeholder_value tests
    // ========================================================================

    #[test]
    fn test_placeholder_empty_string() {
        assert!(is_placeholder_value(""));
    }

    #[test]
    fn test_placeholder_exact_matches() {
        let expected_placeholders = [
            "<replaced>",
            "<binary>",
            "<truncated>",
            "<omitted>",
            "<redacted>",
            "<image>",
            "<audio>",
            "<video>",
            "<file>",
            "[binary]",
            "[replaced]",
            "[truncated]",
            "[omitted]",
            "[redacted]",
            "[image]",
            "[audio]",
            "[video]",
            "[file]",
            "...",
            "\u{2026}", // Unicode ellipsis
        ];
        for p in expected_placeholders {
            assert!(is_placeholder_value(p), "should be placeholder: {:?}", p);
        }
    }

    #[test]
    fn test_placeholder_with_whitespace_trimming() {
        assert!(is_placeholder_value("  <replaced>  "));
        assert!(is_placeholder_value("\t<binary>\n"));
        assert!(is_placeholder_value("   ...   "));
    }

    #[test]
    fn test_placeholder_generic_angle_brackets() {
        assert!(is_placeholder_value("<base64 data>"));
        assert!(is_placeholder_value("<content removed>"));
        assert!(is_placeholder_value("<...>"));
    }

    #[test]
    fn test_placeholder_generic_square_brackets() {
        assert!(is_placeholder_value("[base64 data]"));
        assert!(is_placeholder_value("[content removed]"));
        assert!(is_placeholder_value("[...]"));
    }

    #[test]
    fn test_placeholder_length_boundary() {
        // Generic patterns must be < 50 chars
        let short = format!("<{}>", "a".repeat(46)); // 49 chars total: < + 46 + > = 48 < 50
        assert!(is_placeholder_value(&short));

        let at_limit = format!("<{}>", "a".repeat(47)); // 49 chars: < + 47 + > = 49 < 50
        assert!(is_placeholder_value(&at_limit));

        let over_limit = format!("<{}>", "a".repeat(48)); // 50 chars: not < 50
        assert!(!is_placeholder_value(&over_limit));
    }

    #[test]
    fn test_placeholder_not_matching_long_content() {
        // Real content should not be matched
        assert!(!is_placeholder_value(
            "Hello, this is actual content that should not match"
        ));
        assert!(!is_placeholder_value("SGVsbG8gV29ybGQ=")); // base64
        assert!(!is_placeholder_value("data:image/png;base64,abc"));
    }

    #[test]
    fn test_placeholder_not_matching_long_brackets() {
        // Strings > 50 chars with brackets should NOT match
        let long_bracketed = format!("<{}>", "a".repeat(100));
        assert!(!is_placeholder_value(&long_bracketed));
        let long_square = format!("[{}]", "a".repeat(100));
        assert!(!is_placeholder_value(&long_square));
    }

    #[test]
    fn test_placeholder_whitespace_only() {
        // Whitespace-only: s.is_empty() returns false, but trimmed is empty.
        // Neither exact match nor generic patterns match empty trimmed string.
        // This is NOT a placeholder per the current implementation.
        assert!(!is_placeholder_value("   "));
        assert!(!is_placeholder_value("\t\n"));
    }
}
