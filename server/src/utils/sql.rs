//! SQL utility functions

/// Escape SQL LIKE metacharacters (%, _, \) in user input
///
/// Use this when building LIKE patterns from user input to prevent
/// unintended pattern matching.
///
/// # Example
///
/// ```
/// use sideseat_server::utils::sql::escape_like_pattern;
///
/// let user_input = "100% match_test";
/// let pattern = format!("%{}%", escape_like_pattern(user_input));
/// assert_eq!(pattern, "%100\\% match\\_test%");
/// ```
pub fn escape_like_pattern(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// True when `name` can be interpolated into SQL as a column reference.
///
/// Filter column names are checked against a per-view allowlist when a request is parsed, so a
/// filter reaching a query builder should already be safe. This is the check at the point the SQL
/// is actually assembled, where the consequence of a miss is injection rather than a confusing
/// error: a real column is always a bare identifier, and no injection payload is. A new caller
/// that forgets the allowlist therefore cannot turn into an injection, only into a rejected
/// filter.
///
/// # Example
///
/// ```
/// use sideseat_server::utils::sql::is_plain_identifier;
///
/// assert!(is_plain_identifier("gen_ai_usage_total_tokens"));
/// assert!(!is_plain_identifier("id; DROP TABLE otel_spans"));
/// assert!(!is_plain_identifier("count(*)"));
/// assert!(!is_plain_identifier(""));
/// ```
pub fn is_plain_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with('.')
        && !name.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_identifiers_accept_columns_and_reject_everything_else() {
        for accepted in ["tags", "gen_ai_cost_total", "s.timestamp_start", "column_2"] {
            assert!(is_plain_identifier(accepted), "{accepted} is a column name");
        }
        for rejected in [
            "",
            ".",
            "s.",
            ".tags",
            "tags; DROP TABLE otel_spans",
            "tags' OR '1'='1",
            "count(*)",
            "tags--",
            "tags\n",
            "tags OR 1=1",
            "таги",
        ] {
            assert!(
                !is_plain_identifier(rejected),
                "{rejected:?} must not reach a query"
            );
        }
    }

    #[test]
    fn test_escape_like_pattern_no_special_chars() {
        assert_eq!(escape_like_pattern("hello"), "hello");
    }

    #[test]
    fn test_escape_like_pattern_percent() {
        assert_eq!(escape_like_pattern("100%"), "100\\%");
    }

    #[test]
    fn test_escape_like_pattern_underscore() {
        assert_eq!(escape_like_pattern("foo_bar"), "foo\\_bar");
    }

    #[test]
    fn test_escape_like_pattern_backslash() {
        assert_eq!(escape_like_pattern("path\\file"), "path\\\\file");
    }

    #[test]
    fn test_escape_like_pattern_multiple() {
        assert_eq!(escape_like_pattern("100%_\\test"), "100\\%\\_\\\\test");
    }

    #[test]
    fn test_escape_like_pattern_empty() {
        assert_eq!(escape_like_pattern(""), "");
    }
}
