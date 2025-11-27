//! Input validation for OTLP requests

use crate::otel::error::OtelError;

/// Validate incoming OTLP request
pub fn validate_request(body: &[u8], max_size: usize) -> Result<(), OtelError> {
    // Check size limit
    if body.len() > max_size {
        return Err(OtelError::ValidationError(format!(
            "Request body too large: {} bytes (max: {})",
            body.len(),
            max_size
        )));
    }

    // Check for empty body
    if body.is_empty() {
        return Err(OtelError::ValidationError("Empty request body".to_string()));
    }

    Ok(())
}

/// Truncate a string to max length (for sanitization)
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a valid UTF-8 boundary
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_request_success() {
        let body = b"some valid body";
        assert!(validate_request(body, 1000).is_ok());
    }

    #[test]
    fn test_validate_request_empty_body() {
        let body = b"";
        let result = validate_request(body, 1000);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Empty request body"));
    }

    #[test]
    fn test_validate_request_too_large() {
        let body = b"some body content";
        let result = validate_request(body, 5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn test_validate_request_exact_size() {
        let body = b"12345";
        assert!(validate_request(body, 5).is_ok());
    }

    #[test]
    fn test_truncate_string_no_truncation() {
        assert_eq!(truncate_string("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_string_exact_length() {
        assert_eq!(truncate_string("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_string_truncated() {
        assert_eq!(truncate_string("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_string_utf8_boundary() {
        // "héllo" - é is 2 bytes in UTF-8
        let s = "héllo";
        // Truncate at 2 would cut é in half, should back up
        let result = truncate_string(s, 2);
        assert!(result.is_char_boundary(result.len() - 3)); // -3 for "..."
    }

    #[test]
    fn test_truncate_string_multibyte_chars() {
        // 日本語 - each character is 3 bytes
        let s = "日本語test";
        let result = truncate_string(s, 4);
        // Should truncate at a valid boundary
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_string_empty() {
        assert_eq!(truncate_string("", 10), "");
    }

    #[test]
    fn test_truncate_string_zero_max() {
        assert_eq!(truncate_string("hello", 0), "...");
    }
}
