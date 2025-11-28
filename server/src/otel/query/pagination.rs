//! Cursor-based pagination

use base64::Engine;
use serde::{Deserialize, Serialize};

/// Pagination cursor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    /// Timestamp for ordering
    pub timestamp: i64,
    /// ID for uniqueness
    pub id: String,
}

impl Cursor {
    /// Encode cursor to string
    pub fn encode(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
    }

    /// Decode cursor from string
    pub fn decode(s: &str) -> Option<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()?;
        let json = String::from_utf8(bytes).ok()?;
        serde_json::from_str(&json).ok()
    }
}

/// Paginated result
#[derive(Debug, Clone, Serialize)]
pub struct PageResult<T> {
    /// Items in this page
    pub items: Vec<T>,

    /// Cursor for next page (if more results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,

    /// Whether there are more results
    pub has_more: bool,
}

impl<T> PageResult<T> {
    /// Create an empty result
    pub fn empty() -> Self {
        Self { items: vec![], next_cursor: None, has_more: false }
    }

    /// Get the encoded next cursor string
    pub fn next_cursor_string(&self) -> Option<String> {
        self.next_cursor.as_ref().map(|c| c.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_encode_decode_roundtrip() {
        let cursor = Cursor { timestamp: 1234567890123456789, id: "abc123".to_string() };
        let encoded = cursor.encode();
        let decoded = Cursor::decode(&encoded).unwrap();
        assert_eq!(decoded.timestamp, cursor.timestamp);
        assert_eq!(decoded.id, cursor.id);
    }

    #[test]
    fn test_cursor_decode_invalid() {
        assert!(Cursor::decode("not-valid-base64!!!").is_none());
        assert!(Cursor::decode("").is_none());
    }

    #[test]
    fn test_cursor_decode_invalid_json() {
        let invalid = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        assert!(Cursor::decode(&invalid).is_none());
    }

    #[test]
    fn test_cursor_serialization() {
        let cursor = Cursor { timestamp: 100, id: "test".to_string() };
        let json = serde_json::to_string(&cursor).unwrap();
        let deserialized: Cursor = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.timestamp, 100);
        assert_eq!(deserialized.id, "test");
    }

    #[test]
    fn test_page_result_empty() {
        let result: PageResult<String> = PageResult::empty();
        assert!(result.items.is_empty());
        assert!(result.next_cursor.is_none());
        assert!(!result.has_more);
    }

    #[test]
    fn test_page_result_next_cursor_string() {
        let result: PageResult<i32> = PageResult {
            items: vec![1, 2, 3],
            next_cursor: Some(Cursor { timestamp: 999, id: "last".to_string() }),
            has_more: true,
        };
        let cursor_str = result.next_cursor_string();
        assert!(cursor_str.is_some());

        // Verify we can decode it back
        let decoded = Cursor::decode(&cursor_str.unwrap()).unwrap();
        assert_eq!(decoded.timestamp, 999);
        assert_eq!(decoded.id, "last");
    }

    #[test]
    fn test_page_result_no_cursor() {
        let result: PageResult<i32> =
            PageResult { items: vec![1], next_cursor: None, has_more: false };
        assert!(result.next_cursor_string().is_none());
    }

    #[test]
    fn test_page_result_serialization() {
        let result: PageResult<String> = PageResult {
            items: vec!["a".to_string(), "b".to_string()],
            next_cursor: None,
            has_more: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"items\":[\"a\",\"b\"]"));
        assert!(json.contains("\"has_more\":false"));
        assert!(!json.contains("next_cursor")); // skip_serializing_if
    }

    #[test]
    fn test_cursor_clone() {
        let cursor = Cursor { timestamp: 123, id: "test".to_string() };
        let cloned = cursor.clone();
        assert_eq!(cloned.timestamp, cursor.timestamp);
        assert_eq!(cloned.id, cursor.id);
    }
}
