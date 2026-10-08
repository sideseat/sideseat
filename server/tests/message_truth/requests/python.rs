//! Python's `repr` of a JSON value: how a client that sends a tool's result as `str(result)` wrote it.

use serde_json::Value;

/// What Python's `repr` writes for this value, or `None` where this rendering does not claim to match it: a
/// float outside the range in which Python and Rust write the same digits.
pub(super) fn repr(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(signed), _) => signed.to_string(),
            (None, Some(unsigned)) => unsigned.to_string(),
            (None, None) => float(number.as_f64()?)?,
        },
        Value::String(text) => string(text),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(repr)
                .collect::<Option<Vec<_>>>()?
                .join(", ")
        ),
        Value::Object(members) => format!(
            "{{{}}}",
            members
                .iter()
                .map(|(key, item)| Some(format!("{}: {}", string(key), repr(item)?)))
                .collect::<Option<Vec<_>>>()?
                .join(", ")
        ),
    })
}

/// Python writes a float's shortest round-tripping digits, as Rust does, but in exponent notation below 1e-4
/// and from 1e16, where Rust does not; outside that range nothing is claimed.
fn float(value: f64) -> Option<String> {
    let magnitude = value.abs();
    if !value.is_finite() || (magnitude != 0.0 && !(1e-4..1e16).contains(&magnitude)) {
        return None;
    }
    let text = format!("{value}");
    Some(if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    })
}

/// A `str`'s repr: single quotes unless the text holds one and no double quote, backslash escapes for the
/// quote, the backslash and the common controls, `\x`/`\u`/`\U` for what Python does not print.
fn string(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        let code = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if code < 0x20 || code == 0x7f => out.push_str(&format!("\\x{code:02x}")),
            c if code < 0x7f || printable(c) => out.push(c),
            _ if code <= 0xff => out.push_str(&format!("\\x{code:02x}")),
            _ if code <= 0xffff => out.push_str(&format!("\\u{code:04x}")),
            _ => out.push_str(&format!("\\U{code:08x}")),
        }
    }
    out.push(quote);
    out
}

/// Python's `str.isprintable` beyond ASCII, erring towards escaping: controls, separators and the
/// invisible format characters are written escaped, as Python writes them.
fn printable(c: char) -> bool {
    !c.is_control()
        && !c.is_whitespace()
        && !matches!(
            c as u32,
            0xad | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 | 0xfeff
        )
}
