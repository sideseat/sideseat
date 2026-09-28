use super::*;

/// Try to parse a Python repr string as JSON.
///
/// Python SDKs (e.g., OpenAI Agents) sometimes serialize tool results using Python's
/// `str()` instead of `json.dumps()`, producing single-quoted dicts:
///   `{'status': 'success', 'content': [{'json': {...}}]}`
///
/// Single-pass conversion handles:
/// - Single-quoted strings → double-quoted (with inner `"` escaped)
/// - Double-quoted strings → pass through (with inner `'` preserved)
/// - `True`/`False`/`None` outside strings → `true`/`false`/`null`
/// - Escape sequences within strings (Python `\'` → literal `'`)
///
/// Only attempts conversion for strings starting with `{` or `[`.
/// Returns `None` if conversion produces invalid JSON (graceful fallback to text).
pub(super) fn try_parse_python_repr(s: &str) -> Option<JsonValue> {
    if !s.starts_with('{') && !s.starts_with('[') {
        return None;
    }

    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 32);
    let mut i = 0;
    // false = outside any string, true = inside a string
    let mut in_string = false;
    // The opening quote character of the current string (b'\'' or b'"')
    let mut quote_char = 0u8;

    while i < len {
        let b = bytes[i];

        if in_string {
            if b == quote_char {
                // Closing quote → always emit JSON double quote
                out.push('"');
                in_string = false;
                i += 1;
            } else if b == b'"' && quote_char == b'\'' {
                // Literal double quote inside a single-quoted Python string
                // Must be escaped for JSON
                out.push_str("\\\"");
                i += 1;
            } else if b == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                match next {
                    // Python \' → literal single quote (safe in JSON double-quoted string)
                    b'\'' => {
                        out.push('\'');
                        i += 2;
                    }
                    // Python \" → literal double quote (must be escaped in JSON)
                    b'"' => {
                        out.push_str("\\\"");
                        i += 2;
                    }
                    // Common escape sequences valid in both Python and JSON
                    b'\\' | b'/' | b'n' | b't' | b'r' | b'b' | b'f' => {
                        out.push('\\');
                        out.push(next as char);
                        i += 2;
                    }
                    // Unicode escape: \uXXXX (valid in both Python and JSON)
                    b'u' => {
                        out.push('\\');
                        out.push('u');
                        i += 2;
                    }
                    // Other Python escapes (\x, \N, \0, etc.) → pass through
                    // May cause JSON parse failure → graceful fallback to text
                    _ => {
                        out.push('\\');
                        i += 1;
                    }
                }
            } else {
                // Regular character inside string (handles multi-byte UTF-8)
                let ch = s[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        } else {
            // Outside any string
            match b {
                b'\'' | b'"' => {
                    out.push('"');
                    in_string = true;
                    quote_char = b;
                    i += 1;
                }
                b'T' if matches_python_literal(bytes, i, b"True") => {
                    out.push_str("true");
                    i += 4;
                }
                b'F' if matches_python_literal(bytes, i, b"False") => {
                    out.push_str("false");
                    i += 5;
                }
                b'N' if matches_python_literal(bytes, i, b"None") => {
                    out.push_str("null");
                    i += 4;
                }
                _ => {
                    // Structure chars, whitespace, numbers (all ASCII outside strings)
                    let ch = s[i..].chars().next()?;
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }

    serde_json::from_str(&out).ok()
}

/// Check if a Python literal (`True`, `False`, `None`) appears at byte position `i`
/// with word boundaries on both sides.
///
/// Word boundary = not preceded/followed by alphanumeric or underscore.
/// This prevents replacing inside identifiers like `Trueness` or `_None`.
#[inline]
pub(super) fn matches_python_literal(bytes: &[u8], i: usize, literal: &[u8]) -> bool {
    let end = i + literal.len();
    if end > bytes.len() || bytes[i..end] != *literal {
        return false;
    }
    // Check boundary after literal
    if end < bytes.len() {
        let after = bytes[end];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    // Check boundary before literal (non-ASCII bytes are always valid boundaries)
    if i > 0 {
        let before = bytes[i - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    true
}
