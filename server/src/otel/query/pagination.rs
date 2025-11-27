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
