//! File URI utilities for `#!B64!#` reference format.

/// URI prefix for file references.
///
/// Format: `#!B64!#[mime/type]::hash`
/// - With MIME:    `#!B64!#image/png::abc123`
/// - Without MIME: `#!B64!#::abc123`
pub const FILE_URI_PREFIX: &str = "#!B64!#";

/// Parsed components of a `#!B64!#` file URI.
#[derive(Debug, Clone, PartialEq)]
pub struct FileUri<'a> {
    pub hash: &'a str,
    pub media_type: Option<&'a str>,
}

/// Build a `#!B64!#` file URI from hash and optional media type.
pub fn build_file_uri(hash: &str, media_type: Option<&str>) -> String {
    match media_type {
        Some(mt) => format!("{FILE_URI_PREFIX}{mt}::{hash}"),
        None => format!("{FILE_URI_PREFIX}::{hash}"),
    }
}

/// Parse a sideseat file URI into its components.
///
/// Accepts both formats:
/// - `#!B64!#image/png::abc123` -> hash="abc123", media_type=Some("image/png")
/// - `#!B64!#::abc123`          -> hash="abc123", media_type=None
pub fn parse_file_uri(uri: &str) -> Option<FileUri<'_>> {
    let rest = uri.strip_prefix(FILE_URI_PREFIX)?;
    let sep = rest.find("::")?;
    let mime_part = &rest[..sep];
    let hash = &rest[sep + 2..];
    if hash.is_empty() {
        return None;
    }
    let media_type = if mime_part.is_empty() {
        None
    } else {
        Some(mime_part)
    };
    Some(FileUri { hash, media_type })
}

/// Check if a string is a sideseat file URI.
pub fn is_file_uri(s: &str) -> bool {
    s.starts_with(FILE_URI_PREFIX) && s.contains("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_file_uri_with_mime() {
        assert_eq!(
            build_file_uri("abc123", Some("image/png")),
            "#!B64!#image/png::abc123"
        );
    }

    #[test]
    fn test_build_file_uri_without_mime() {
        assert_eq!(build_file_uri("abc123", None), "#!B64!#::abc123");
    }

    #[test]
    fn test_parse_file_uri_with_mime() {
        assert_eq!(
            parse_file_uri("#!B64!#image/png::abc123"),
            Some(FileUri {
                hash: "abc123",
                media_type: Some("image/png")
            })
        );
    }

    #[test]
    fn test_parse_file_uri_without_mime() {
        assert_eq!(
            parse_file_uri("#!B64!#::abc123"),
            Some(FileUri {
                hash: "abc123",
                media_type: None
            })
        );
    }

    #[test]
    fn test_parse_file_uri_invalid() {
        assert!(parse_file_uri("not-a-uri").is_none());
        assert!(parse_file_uri("#!B64!#no-separator").is_none());
        assert!(parse_file_uri("").is_none());
        assert!(parse_file_uri("#!B64!#::").is_none());
        assert!(parse_file_uri("#!B64!#image/png::").is_none());
    }

    #[test]
    fn test_is_file_uri() {
        assert!(is_file_uri("#!B64!#::abc123"));
        assert!(is_file_uri("#!B64!#image/jpeg::abc123"));
        assert!(is_file_uri("#!B64!#application/pdf::hash"));
        assert!(!is_file_uri("data:image/png;base64,abc"));
        assert!(!is_file_uri("https://example.com"));
    }
}
