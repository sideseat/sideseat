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

/// Check if a string is a valid sideseat file URI.
/// Consistent with `parse_file_uri` -- returns true only if parsing succeeds.
pub fn is_file_uri(s: &str) -> bool {
    parse_file_uri(s).is_some()
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

    #[test]
    fn test_is_file_uri_consistency_with_parse() {
        // is_file_uri must agree with parse_file_uri for all edge cases
        let cases = [
            "#!B64!#::",                // empty hash
            "#!B64!#image/png::",       // empty hash with mime
            "#!B64!#no-separator",      // no :: separator
            "",                         // empty string
            "#!B64!#::abc123",          // valid
            "#!B64!#image/png::abc123", // valid with mime
        ];
        for case in cases {
            assert_eq!(
                is_file_uri(case),
                parse_file_uri(case).is_some(),
                "is_file_uri and parse_file_uri disagree on {:?}",
                case
            );
        }
    }

    #[test]
    fn test_roundtrip_build_parse() {
        // Build then parse should produce the same components
        let uri = build_file_uri("deadbeef1234", Some("image/png"));
        let parsed = parse_file_uri(&uri).unwrap();
        assert_eq!(parsed.hash, "deadbeef1234");
        assert_eq!(parsed.media_type, Some("image/png"));

        let uri = build_file_uri("abc123", None);
        let parsed = parse_file_uri(&uri).unwrap();
        assert_eq!(parsed.hash, "abc123");
        assert_eq!(parsed.media_type, None);
    }

    #[test]
    fn test_parse_with_double_separator_in_hash() {
        // Hash containing :: should be preserved as-is (first :: is the separator)
        let parsed = parse_file_uri("#!B64!#image/png::hash::extra");
        assert!(parsed.is_some());
        let p = parsed.unwrap();
        assert_eq!(p.hash, "hash::extra");
        assert_eq!(p.media_type, Some("image/png"));
    }

    #[test]
    fn test_build_with_empty_hash_is_not_parseable() {
        // build_file_uri with empty hash produces a URI that parse_file_uri rejects
        let uri = build_file_uri("", Some("image/png"));
        assert_eq!(uri, "#!B64!#image/png::");
        assert!(
            parse_file_uri(&uri).is_none(),
            "empty-hash URI should not parse"
        );
        assert!(!is_file_uri(&uri));
    }
}
