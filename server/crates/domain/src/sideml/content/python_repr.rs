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
pub(crate) fn try_parse_python_repr(s: &str) -> Option<JsonValue> {
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

const PYTHON_REPR_MAX_DEPTH: usize = 64;
const PYTHON_CONSTRUCTOR_MEMBER: &str = "__python_constructor";
const PYTHON_POSITIONAL_MEMBER: &str = "__python_args";

/// Parse one Python constructor `repr` into a JSON tree.
///
/// This is intentionally a sealed subset rather than Python syntax:
///
/// - constructor calls with positional and keyword arguments;
/// - lists, tuples, dictionaries, strings, JSON numbers, booleans, and `None`;
/// - enum reprs such as `<State.SUCCESS: 'success'>`;
/// - nested constructor calls.
///
/// Constructor names are retained under `__python_constructor`, and positional arguments under
/// `__python_args`. A rule may then select only the producer fields that carry product data. Unsupported
/// syntax fails closed, leaving the original telemetry available as text.
pub(crate) fn try_parse_python_constructor_repr(s: &str) -> Option<JsonValue> {
    // Some telemetry serializers JSON-encode the repr before putting it in an OTLP string attribute. Decode
    // exactly one string layer; objects and arrays are not constructor reprs and stay with their normal parser.
    let decoded = serde_json::from_str::<String>(s).ok();
    let source = decoded.as_deref().unwrap_or(s);
    let mut parser = PythonConstructorParser::new(source);
    let value = parser.parse_constructor(0)?;
    parser.skip_whitespace();
    (parser.position == source.len()).then_some(value)
}

/// Parse text that is several constructor `repr`s written back to back, into an array of their trees.
///
/// What a log of Python objects looks like when each was printed without a separator. All or nothing: text
/// between the reprs that is not whitespace means the carrier is some other shape. A single repr is a
/// sequence of one.
pub(crate) fn try_parse_python_constructor_repr_sequence(s: &str) -> Option<JsonValue> {
    let mut parser = PythonConstructorParser::new(s);
    let mut items = Vec::new();
    parser.skip_whitespace();
    while parser.position < s.len() {
        items.push(parser.parse_constructor(0)?);
        parser.skip_whitespace();
    }
    (!items.is_empty()).then_some(JsonValue::Array(items))
}

/// Parse the Python `str()` of a dict, list or tuple - strings in either quote, numbers, `True`, `False`,
/// `None`, nested containers - into a JSON tree, through the same sealed and depth-bounded parser as
/// constructor reprs with constructors, enum reprs and bare names refused. The whole text must be one
/// container; anything else is `None`, leaving the telemetry to be read as text.
pub(crate) fn try_parse_python_literal(s: &str) -> Option<JsonValue> {
    let source = s.trim();
    if !matches!(source.chars().next()?, '{' | '[' | '(') {
        return None;
    }
    let mut parser = PythonConstructorParser::new(source);
    parser.literals_only = true;
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    (parser.position == source.len()).then_some(value)
}

/// Normalize a JSON-decoded list whose members are constructor repr strings.
///
/// The caller already knows the surrounding value is a tool response. This helper still requires at least
/// one constructor to become a recognised content block; an ordinary JSON array remains ordinary JSON
/// rather than being reinterpreted because it happened to contain strings.
pub(crate) fn try_normalize_python_constructor_content(value: &JsonValue) -> Option<JsonValue> {
    fn constructor_block(raw: &str) -> Option<JsonValue> {
        let parsed = try_parse_python_constructor_repr(raw)?;
        parsed.get("type").and_then(JsonValue::as_str)?;
        let normalized = super::normalize_returned_value_block(&parsed)?;
        (!matches!(
            normalized.get("type").and_then(JsonValue::as_str),
            Some("json" | "unknown") | None
        ))
        .then_some(normalized)
    }

    match value {
        JsonValue::String(raw) => constructor_block(raw).map(|block| json!([block])),
        JsonValue::Array(items) => {
            let mut found_constructor = false;
            let mut normalized = Vec::with_capacity(items.len());
            for item in items {
                if let Some(block) = item.as_str().and_then(constructor_block) {
                    found_constructor = true;
                    normalized.push(block);
                } else if let Some(block) = super::normalize_returned_value_block(item) {
                    normalized.push(block);
                }
            }
            found_constructor.then_some(JsonValue::Array(normalized))
        }
        _ => None,
    }
}

struct PythonConstructorParser<'a> {
    source: &'a str,
    position: usize,
    /// Refuse constructors, enum reprs and bare identifiers: the plain-literal subset only.
    literals_only: bool,
}

impl<'a> PythonConstructorParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
            literals_only: false,
        }
    }

    fn parse_constructor(&mut self, depth: usize) -> Option<JsonValue> {
        self.guard_depth(depth)?;
        self.skip_whitespace();
        let name = self.parse_identifier(true)?;
        self.skip_whitespace();
        self.consume('(')?;

        let mut positional = Vec::new();
        let mut keyword = serde_json::Map::new();
        self.skip_whitespace();
        while self.peek()? != ')' {
            let checkpoint = self.position;
            let named = self.parse_identifier(false).and_then(|field| {
                self.skip_whitespace();
                self.consume('=').map(|_| field)
            });
            if let Some(field) = named {
                self.skip_whitespace();
                let value = self.parse_value(depth + 1)?;
                if keyword.insert(field, value).is_some() {
                    return None;
                }
            } else {
                self.position = checkpoint;
                positional.push(self.parse_value(depth + 1)?);
            }

            self.skip_whitespace();
            match self.peek()? {
                ',' => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek()? == ')' {
                        break;
                    }
                }
                ')' => break,
                _ => return None,
            }
        }
        self.consume(')')?;

        let mut object = serde_json::Map::new();
        object.insert(
            PYTHON_CONSTRUCTOR_MEMBER.to_string(),
            JsonValue::String(name),
        );
        if !positional.is_empty() {
            object.insert(
                PYTHON_POSITIONAL_MEMBER.to_string(),
                JsonValue::Array(positional),
            );
        }
        for (field, value) in keyword {
            object.insert(field, value);
        }
        Some(JsonValue::Object(object))
    }

    fn parse_value(&mut self, depth: usize) -> Option<JsonValue> {
        self.guard_depth(depth)?;
        self.skip_whitespace();
        match self.peek()? {
            '\'' | '"' => self.parse_string().map(JsonValue::String),
            '[' => self.parse_array(depth + 1, '[', ']').map(JsonValue::Array),
            '(' => self.parse_array(depth + 1, '(', ')').map(JsonValue::Array),
            '{' => self.parse_object(depth + 1),
            '<' if self.literals_only => None,
            '<' => self.parse_enum_repr(depth + 1),
            '-' | '0'..='9' => self.parse_number(),
            _ => {
                let checkpoint = self.position;
                let identifier = self.parse_identifier(true)?;
                match identifier.as_str() {
                    "True" => Some(JsonValue::Bool(true)),
                    "False" => Some(JsonValue::Bool(false)),
                    "None" => Some(JsonValue::Null),
                    _ if self.literals_only => None,
                    _ => {
                        self.skip_whitespace();
                        if self.peek() == Some('(') {
                            self.position = checkpoint;
                            self.parse_constructor(depth + 1)
                        } else {
                            Some(JsonValue::String(identifier))
                        }
                    }
                }
            }
        }
    }

    fn parse_array(&mut self, depth: usize, open: char, close: char) -> Option<Vec<JsonValue>> {
        self.guard_depth(depth)?;
        self.consume(open)?;
        self.skip_whitespace();
        let mut values = Vec::new();
        while self.peek()? != close {
            values.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            match self.peek()? {
                ',' => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek()? == close {
                        break;
                    }
                }
                found if found == close => break,
                _ => return None,
            }
        }
        self.consume(close)?;
        Some(values)
    }

    fn parse_object(&mut self, depth: usize) -> Option<JsonValue> {
        self.guard_depth(depth)?;
        self.consume('{')?;
        self.skip_whitespace();
        let mut object = serde_json::Map::new();
        while self.peek()? != '}' {
            let key = match self.parse_value(depth + 1)? {
                JsonValue::String(value) => value,
                JsonValue::Number(value) => value.to_string(),
                JsonValue::Bool(value) => value.to_string(),
                JsonValue::Null => "null".to_string(),
                _ => return None,
            };
            self.skip_whitespace();
            self.consume(':')?;
            self.skip_whitespace();
            let value = self.parse_value(depth + 1)?;
            if object.insert(key, value).is_some() {
                return None;
            }
            self.skip_whitespace();
            match self.peek()? {
                ',' => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek()? == '}' {
                        break;
                    }
                }
                '}' => break,
                _ => return None,
            }
        }
        self.consume('}')?;
        Some(JsonValue::Object(object))
    }

    fn parse_enum_repr(&mut self, depth: usize) -> Option<JsonValue> {
        self.guard_depth(depth)?;
        self.consume('<')?;
        self.skip_whitespace();
        self.parse_identifier(true)?;
        self.skip_whitespace();
        self.consume(':')?;
        self.skip_whitespace();
        let value = self.parse_value(depth + 1)?;
        self.skip_whitespace();
        self.consume('>')?;
        Some(value)
    }

    fn parse_string(&mut self) -> Option<String> {
        let quote = self.bump()?;
        let mut out = String::new();
        loop {
            let character = self.bump()?;
            match character {
                found if found == quote => return Some(out),
                '\\' => {
                    let escaped = self.bump()?;
                    match escaped {
                        '\\' => out.push('\\'),
                        '\'' => out.push('\''),
                        '"' => out.push('"'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000c}'),
                        'a' => out.push('\u{0007}'),
                        'v' => out.push('\u{000b}'),
                        'x' => out.push(self.parse_escape(2)?),
                        'u' => out.push(self.parse_escape(4)?),
                        'U' => out.push(self.parse_escape(8)?),
                        _ => return None,
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn parse_escape(&mut self, digits: usize) -> Option<char> {
        let start = self.position;
        for _ in 0..digits {
            self.bump()?.is_ascii_hexdigit().then_some(())?;
        }
        let value = u32::from_str_radix(&self.source[start..self.position], 16).ok()?;
        char::from_u32(value)
    }

    fn parse_number(&mut self) -> Option<JsonValue> {
        let start = self.position;
        while self.peek().is_some_and(|character| {
            character.is_ascii_digit() || matches!(character, '+' | '-' | '.' | 'e' | 'E')
        }) {
            self.bump();
        }
        self.source[start..self.position]
            .parse::<serde_json::Number>()
            .ok()
            .map(JsonValue::Number)
    }

    fn parse_identifier(&mut self, qualified: bool) -> Option<String> {
        let start = self.position;
        let first = self.peek()?;
        if !(first.is_alphabetic() || first == '_') {
            return None;
        }
        self.bump();
        while self.peek().is_some_and(|character| {
            character.is_alphanumeric() || character == '_' || (qualified && character == '.')
        }) {
            self.bump();
        }
        Some(self.source[start..self.position].to_string())
    }

    fn guard_depth(&self, depth: usize) -> Option<()> {
        (depth <= PYTHON_REPR_MAX_DEPTH).then_some(())
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn consume(&mut self, expected: char) -> Option<()> {
        (self.bump()? == expected).then_some(())
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }
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
