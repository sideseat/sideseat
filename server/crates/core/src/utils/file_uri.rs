//! File URI utilities for `#!B64!#` reference format.

/// URI prefix for file references.
///
/// Format: `#!B64!#[mime/type]::hash`
/// - With MIME:    `#!B64!#image/png::abc123`
/// - Without MIME: `#!B64!#::abc123`
pub const FILE_URI_PREFIX: &str = "#!B64!#";
pub const FILE_HASH_HEX_LENGTH: usize = 64;

pub fn is_valid_file_hash(hash: &str) -> bool {
    hash.len() == FILE_HASH_HEX_LENGTH && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The two characters every reference begins with, which an escaped producer string never holds unescaped.
const REFERENCE_LEAD: &str = "#!";

/// A producer's own string, escaped for a store where SideSeat's references sit beside it: every `#!` becomes `#!!`.
///
/// A reference begins with `#!`, so an escaped string can never contain one - not whole, not at a boundary where an
/// escaped occurrence meets the text after it - and only a reference SideSeat wrote reads as one. Escaping the
/// lead itself rather than the whole prefix is what makes that hold: `#!B64!#!B64!#` escaped by prefix re-forms a
/// prefix from the first one's tail and the second's head. Lossless: [`unescape_producer_text`] gives back the
/// original exactly. Borrowed where there is nothing to escape, which is nearly always.
pub fn escape_producer_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(REFERENCE_LEAD) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace(REFERENCE_LEAD, "#!!"))
}

/// A producer string as it was before [`escape_producer_text`]: each `#!!` is `#!` again, and a `#!` that is not
/// followed by `!` - the lead of a reference SideSeat wrote - is left as it is.
pub fn unescape_producer_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("#!!") {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace("#!!", REFERENCE_LEAD))
}

/// Parsed components of a `#!B64!#` file URI.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    fn storage_hashes_are_exactly_64_hex_characters() {
        assert!(is_valid_file_hash(&"a".repeat(FILE_HASH_HEX_LENGTH)));
        assert!(!is_valid_file_hash(&"a".repeat(FILE_HASH_HEX_LENGTH - 1)));
        assert!(!is_valid_file_hash(&"g".repeat(FILE_HASH_HEX_LENGTH)));
        assert!(!is_valid_file_hash(&format!(
            "{}.",
            "a".repeat(FILE_HASH_HEX_LENGTH - 1)
        )));
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

    /// Escaping is lossless, and an escaped producer string holds no reference whatever it was spelled like.
    #[test]
    fn an_escaped_producer_string_holds_no_reference_and_unescapes_exactly() {
        let reference = format!(
            "{FILE_URI_PREFIX}image/png::{}",
            "a".repeat(FILE_HASH_HEX_LENGTH)
        );
        for original in [
            reference.clone(),
            format!("see {reference} here"),
            format!("{FILE_URI_PREFIX}!B64!#"),
            "#!!".to_string(),
            "#!#!!#".to_string(),
            "#!/bin/sh".to_string(),
            "plain".to_string(),
        ] {
            let escaped = escape_producer_text(&original);
            assert!(
                !escaped.contains(FILE_URI_PREFIX),
                "{original:?} -> {escaped:?}"
            );
            assert_eq!(unescape_producer_text(&escaped), original);
        }
        // A reference SideSeat wrote beside an escaped string survives the unescape.
        let mixed = format!("{} {reference}", escape_producer_text("#!!"));
        assert_eq!(unescape_producer_text(&mixed), format!("#!! {reference}"));
    }
}
