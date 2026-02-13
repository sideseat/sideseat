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

#[cfg(test)]
mod tests {
    use super::*;

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
