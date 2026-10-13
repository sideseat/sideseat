//! Python's `repr` of an object graph, and a data model's `str()`, read into a JSON tree.
//!
//! The counterpart of `java_tostring` for Python frameworks that record their own objects as text: a framework
//! that falls back to `str()` when a value is not JSON-serialisable writes `ChatMessage(_role=<ChatRole.USER:
//! 'user'>, _content=[TextContent(text='...')])`, lists of such objects, dicts holding them, and objects it cannot
//! print at all as `<function f at 0x10db9a1f0>`. The grammar is a sealed subset of what `repr` prints, and
//! anything outside it fails closed, leaving the telemetry to be read as text.
//!
//! **A reading is accepted only where printing it again in Python's own repr spacing reproduces the text byte for
//! byte.** Strings print again as their source tokens, so the check decides whether the text is a printed Python
//! value without re-encoding a decoded string; numbers are held to `repr`'s own spelling when read. A text with
//! other spacing, a trailing comma `repr` does not print, a number it would spell otherwise, or a stray token
//! stays text. What the producer printed is read; what it might have meant is not guessed.

use serde_json::{Map, Value as JsonValue};

use super::python_repr::{
    PYTHON_CONSTRUCTOR_MEMBER, PYTHON_POSITIONAL_MEMBER, PYTHON_REPR_MAX_DEPTH,
};

/// The member an enum repr's qualified member name is kept under.
const PYTHON_ENUM_MEMBER: &str = "__python_enum";
/// The member an enum repr's value is kept under, beside its name.
const PYTHON_ENUM_VALUE_MEMBER: &str = "__python_value";
/// The member marking an object `repr` cannot print - `<function f at 0x...>`. The leaf holds no text: the raw
/// carrier keeps the bytes, and a reading has nothing in it to mine.
const PYTHON_OPAQUE_MEMBER: &str = "__python_opaque";

/// Parse one value Python's `repr` printed into a JSON tree.
///
/// - A constructor `Name(a, k=v)` becomes `{"__python_constructor": "Name", "__python_args": [a], "k": v}`, the
///   shape `python_constructor_repr` gives, `__python_args` only where there are positional arguments.
/// - An enum repr `<ChatRole.USER: 'user'>` (or `<Flag: 0>`, a flag with no member name) becomes `{"__python_enum": "ChatRole.USER", "__python_value":
///   "user"}`: its member and its value, both as printed.
/// - An object `repr` cannot print, `<...>` with balanced angle brackets, no quote and no enum header, becomes
///   `{"__python_opaque": true}`: marked, holding nothing to read.
/// - Lists and tuples are arrays, a dict with string keys an object, `True`/`False`/`None` the JSON literals, a
///   string its value with Python's escapes decoded, an integer or a float the JSON number it is.
///
/// Any value may be the whole text. A dict key that is not a string, a repeated key or keyword, an escape `repr`
/// does not print (`\N{...}`, `\a`, octal, a hex escape with a sign), a raw control character, a number spelt
/// as `repr` would not spell it (`01`, `1.`, `-0`, `1E+20`, `1e5`, `inf`), an integer no JSON number holds, and
/// text no reading reprints exactly are refused.
pub(crate) fn try_parse_python_object_repr(source: &str) -> Option<JsonValue> {
    let mut reading = Reading::new(source);
    let node = reading.value(0)?;
    (reading.at == source.len() && reprinted(source, &node, 0) == Some(source.len()))
        .then(|| node.into_json())
        .flatten()
}

/// Parse a data model's `str()` - its fields as `name=value`, separated by single spaces, with no class
/// name - into an object of those fields. No `__python_constructor` is given: the class is not in the text.
///
/// Opt-in per rule: `a=1 b=2` is also prose, so only a producer known to print its models this way is read so.
pub(crate) fn try_parse_python_fields(source: &str) -> Option<JsonValue> {
    let mut reading = Reading::new(source);
    let mut fields = Vec::new();
    loop {
        let name = reading.identifier(false)?;
        reading.consume(b'=')?;
        fields.push((name, reading.value(0)?));
        if reading.at == source.len() {
            break;
        }
        reading.consume(b' ')?;
    }
    let mut at = 0;
    for (index, (name, value)) in fields.iter().enumerate() {
        if index > 0 {
            at = literal(source, at, " ")?;
        }
        at = reprinted(
            source,
            value,
            literal(source, literal(source, at, name)?, "=")?,
        )?;
    }
    (at == source.len())
        .then(|| keyword_object(fields, Map::new()))
        .flatten()
}

/// One reading of the text, borrowing every name and token from it.
enum Node<'a> {
    /// A string: its source token, quotes included, and its decoded value.
    Str(&'a str, String),
    /// An integer, as written: `repr`'s own spelling, checked when read.
    Int(&'a str),
    /// A float, its token `repr`'s own spelling of its value, checked when read.
    Float(&'a str, f64),
    True,
    False,
    None,
    List(Vec<Node<'a>>),
    Tuple(Vec<Node<'a>>),
    Dict(Vec<(Node<'a>, Node<'a>)>),
    Constructor {
        name: &'a str,
        args: Vec<Node<'a>>,
        kwargs: Vec<(&'a str, Node<'a>)>,
    },
    Enum(&'a str, Box<Node<'a>>),
    Opaque(&'a str),
}

impl Node<'_> {
    fn into_json(self) -> Option<JsonValue> {
        Some(match self {
            Node::Str(_, value) => JsonValue::String(value),
            Node::Int(token) => JsonValue::Number(
                token
                    .parse::<i64>()
                    .map(serde_json::Number::from)
                    .or_else(|_| token.parse::<u64>().map(serde_json::Number::from))
                    .ok()?,
            ),
            Node::Float(_, value) => JsonValue::Number(serde_json::Number::from_f64(value)?),
            Node::True => JsonValue::Bool(true),
            Node::False => JsonValue::Bool(false),
            Node::None => JsonValue::Null,
            Node::List(items) | Node::Tuple(items) => JsonValue::Array(
                items
                    .into_iter()
                    .map(Node::into_json)
                    .collect::<Option<_>>()?,
            ),
            Node::Dict(entries) => {
                let mut object = Map::new();
                for (key, value) in entries {
                    let Node::Str(_, key) = key else {
                        return None;
                    };
                    object
                        .insert(key, value.into_json()?)
                        .is_none()
                        .then_some(())?;
                }
                JsonValue::Object(object)
            }
            Node::Constructor { name, args, kwargs } => {
                let mut object = Map::new();
                object.insert(
                    PYTHON_CONSTRUCTOR_MEMBER.to_string(),
                    JsonValue::String(name.to_string()),
                );
                if !args.is_empty() {
                    object.insert(
                        PYTHON_POSITIONAL_MEMBER.to_string(),
                        JsonValue::Array(
                            args.into_iter()
                                .map(Node::into_json)
                                .collect::<Option<_>>()?,
                        ),
                    );
                }
                keyword_object(kwargs, object)?
            }
            Node::Enum(name, value) => {
                let mut object = Map::new();
                object.insert(
                    PYTHON_ENUM_MEMBER.to_string(),
                    JsonValue::String(name.to_string()),
                );
                object.insert(PYTHON_ENUM_VALUE_MEMBER.to_string(), value.into_json()?);
                JsonValue::Object(object)
            }
            Node::Opaque(_) => {
                let mut object = Map::new();
                object.insert(PYTHON_OPAQUE_MEMBER.to_string(), JsonValue::Bool(true));
                JsonValue::Object(object)
            }
        })
    }
}

/// Keyword members into an object that may already hold the engine's own members: a repeated name, or one the
/// engine's tree reserves, is refused.
fn keyword_object(
    fields: Vec<(&str, Node<'_>)>,
    mut object: Map<String, JsonValue>,
) -> Option<JsonValue> {
    for (name, value) in fields {
        if name.starts_with("__python_") {
            return None;
        }
        object
            .insert(name.to_string(), value.into_json()?)
            .is_none()
            .then_some(())?;
    }
    Some(JsonValue::Object(object))
}

/// Where `text` ends, if the source holds exactly it at `at`.
fn literal(source: &str, at: usize, text: &str) -> Option<usize> {
    source
        .get(at..)
        .is_some_and(|rest| rest.starts_with(text))
        .then_some(at + text.len())
}

/// Where the text Python's `repr` prints for `node` ends, if the source holds exactly that text from `at`: `, `
/// between items, `: ` in a dict and an enum, `=` before a keyword's value, `(x,)` for a tuple of one, nothing
/// inside brackets. Compared in place, so a large carrier is read once, not copied up through every ancestor.
fn reprinted(source: &str, node: &Node<'_>, at: usize) -> Option<usize> {
    let items = |at: usize, items: &[Node<'_>]| {
        let mut at = at;
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                at = literal(source, at, ", ")?;
            }
            at = reprinted(source, item, at)?;
        }
        Some(at)
    };
    match node {
        Node::Str(token, _) | Node::Int(token) | Node::Float(token, _) | Node::Opaque(token) => {
            literal(source, at, token)
        }
        Node::True => literal(source, at, "True"),
        Node::False => literal(source, at, "False"),
        Node::None => literal(source, at, "None"),
        Node::List(list) => literal(source, items(literal(source, at, "[")?, list)?, "]"),
        Node::Tuple(tuple) if tuple.len() == 1 => {
            literal(source, items(literal(source, at, "(")?, tuple)?, ",)")
        }
        Node::Tuple(tuple) => literal(source, items(literal(source, at, "(")?, tuple)?, ")"),
        Node::Dict(entries) => {
            let mut at = literal(source, at, "{")?;
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    at = literal(source, at, ", ")?;
                }
                at = reprinted(
                    source,
                    value,
                    literal(source, reprinted(source, key, at)?, ": ")?,
                )?;
            }
            literal(source, at, "}")
        }
        Node::Constructor { name, args, kwargs } => {
            let mut at = items(literal(source, literal(source, at, name)?, "(")?, args)?;
            for (index, (keyword, value)) in kwargs.iter().enumerate() {
                if index > 0 || !args.is_empty() {
                    at = literal(source, at, ", ")?;
                }
                at = reprinted(
                    source,
                    value,
                    literal(source, literal(source, at, keyword)?, "=")?,
                )?;
            }
            literal(source, at, ")")
        }
        Node::Enum(name, value) => {
            let at = literal(
                source,
                literal(source, literal(source, at, "<")?, name)?,
                ": ",
            )?;
            literal(source, reprinted(source, value, at)?, ">")
        }
    }
}

/// How CPython's `repr` spells a finite float: the shortest digits that read back as it, fixed-point where the
/// decimal exponent is from -4 to 15 (`0.0001`, `1000000000000000.0`, always with a fraction), otherwise
/// scientific with a signed two-digit exponent (`1e-05`, `1.5e+300`).
fn python_float(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    if value == 0.0 {
        return Some(
            if value.is_sign_negative() {
                "-0.0"
            } else {
                "0.0"
            }
            .to_string(),
        );
    }
    // How many digits the shortest spelling needs, then those digits rounded from the exact value, ties to
    // even: of the shortest spellings that read back, CPython prints the one nearest the value, which the
    // shortest formatter alone does not always give (`1000000000000000.2`'s value prints `...0.2`, not `...0.3`).
    let shortest = format!("{:e}", value.abs());
    let count = shortest
        .split_once('e')?
        .0
        .chars()
        .filter(char::is_ascii_digit)
        .count();
    // The correctly rounded spelling where it reads back as the value; where it does not - the exact value lies
    // where rounding crosses to a neighbour, as at a power of two - the shortest spelling that does.
    let rounded = format!("{:.*e}", count - 1, value.abs());
    let chosen = if rounded.parse::<f64>().ok()? == value.abs() {
        rounded
    } else {
        shortest
    };
    let (mantissa, exponent) = chosen.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let spelt = if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        if point <= 0 {
            format!("0.{}{digits}", "0".repeat(usize::try_from(-point).ok()?))
        } else {
            let point = usize::try_from(point).ok()?;
            if point >= digits.len() {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        }
    } else {
        let mantissa = if digits.len() == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    };
    Some(if value.is_sign_negative() {
        format!("-{spelt}")
    } else {
        spelt
    })
}

struct Reading<'a> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reading<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            at: 0,
        }
    }

    /// One value. The reading takes the spacing `repr` prints and nothing else, so a text it accepts is already
    /// in canonical form; the reprint check states that, rather than trusting this reading to.
    fn value(&mut self, depth: usize) -> Option<Node<'a>> {
        (depth < PYTHON_REPR_MAX_DEPTH).then_some(())?;
        match self.peek()? {
            b'\'' | b'"' => self.string(),
            b'[' => self.items(b'[', b']', depth).map(Node::List),
            b'(' => self.items(b'(', b')', depth).map(Node::Tuple),
            b'{' => self.dict(depth),
            b'<' => self.angle(depth),
            b'-' | b'0'..=b'9' => self.number(),
            _ => {
                let name = self.identifier(true)?;
                match name {
                    "True" => Some(Node::True),
                    "False" => Some(Node::False),
                    "None" => Some(Node::None),
                    // A bare name is no value `repr` prints: refused rather than read as a string.
                    _ if self.peek() != Some(b'(') => None,
                    _ => self.constructor(name, depth),
                }
            }
        }
    }

    fn items(&mut self, open: u8, close: u8, depth: usize) -> Option<Vec<Node<'a>>> {
        self.consume(open)?;
        let mut items = Vec::new();
        if self.peek()? == close {
            self.at += 1;
            return Some(items);
        }
        loop {
            items.push(self.value(depth + 1)?);
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    if self.peek()? == close {
                        // A trailing comma: `repr` prints one only in a tuple of one.
                        self.at += 1;
                        return (open == b'(' && items.len() == 1).then_some(items);
                    }
                    self.consume(b' ')?;
                }
                byte if byte == close => {
                    self.at += 1;
                    return Some(items);
                }
                _ => return None,
            }
        }
    }

    fn dict(&mut self, depth: usize) -> Option<Node<'a>> {
        self.consume(b'{')?;
        let mut entries = Vec::new();
        if self.peek()? == b'}' {
            self.at += 1;
            return Some(Node::Dict(entries));
        }
        loop {
            let key = self.value(depth + 1)?;
            matches!(key, Node::Str(..)).then_some(())?;
            self.consume(b':')?;
            self.consume(b' ')?;
            entries.push((key, self.value(depth + 1)?));
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.consume(b' ')?;
                }
                b'}' => {
                    self.at += 1;
                    return Some(Node::Dict(entries));
                }
                _ => return None,
            }
        }
    }

    fn constructor(&mut self, name: &'a str, depth: usize) -> Option<Node<'a>> {
        self.consume(b'(')?;
        let mut args = Vec::new();
        let mut kwargs: Vec<(&'a str, Node<'a>)> = Vec::new();
        if self.peek()? == b')' {
            self.at += 1;
            return Some(Node::Constructor { name, args, kwargs });
        }
        loop {
            let start = self.at;
            let keyword = self.identifier(false).filter(|_| self.peek() == Some(b'='));
            match keyword {
                Some(keyword) => {
                    self.at += 1;
                    let value = self.value(depth + 1)?;
                    kwargs.push((keyword, value));
                }
                None => {
                    self.at = start;
                    // A positional argument after a keyword is no call Python prints.
                    kwargs.is_empty().then_some(())?;
                    args.push(self.value(depth + 1)?);
                }
            }
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.consume(b' ')?;
                }
                b')' => {
                    self.at += 1;
                    return Some(Node::Constructor { name, args, kwargs });
                }
                _ => return None,
            }
        }
    }

    /// `<Name.MEMBER: value>`, an enum's repr, wherever the text opens like one - a failure there fails the
    /// text, never becomes an opaque object; otherwise `<...>`, an object `repr` cannot print.
    fn angle(&mut self, depth: usize) -> Option<Node<'a>> {
        if self.enum_header() {
            return self.enumeration(depth);
        }
        self.opaque()
    }

    /// Whether an enum's repr starts here: `<`, a name, then `:` - whatever spacing follows, which `enumeration`
    /// then holds to `repr`'s own. An unprintable object's repr continues its name with a space (`<function f at
    /// 0x...>`), never a colon.
    fn enum_header(&self) -> bool {
        let mut probe = Reading {
            source: self.source,
            bytes: self.bytes,
            at: self.at,
        };
        probe.consume(b'<').is_some()
            && probe.identifier(true).is_some()
            && probe.consume(b':').is_some()
    }

    fn enumeration(&mut self, depth: usize) -> Option<Node<'a>> {
        self.consume(b'<')?;
        let name = self.identifier(true)?;
        self.consume(b':')?;
        self.consume(b' ')?;
        let value = self.value(depth + 1)?;
        self.consume(b'>')?;
        Some(Node::Enum(name, Box::new(value)))
    }

    /// Balanced angle brackets, no quote and no line break inside: what CPython prints for a function, a module,
    /// a context object. A quote would make where it ends a guess.
    fn opaque(&mut self) -> Option<Node<'a>> {
        let start = self.at;
        let mut open = 0usize;
        loop {
            match self.peek()? {
                b'<' => open += 1,
                b'>' => {
                    open -= 1;
                    if open == 0 {
                        self.at += 1;
                        return Some(Node::Opaque(&self.source[start..self.at]));
                    }
                }
                b'\'' | b'"' | b'\n' | b'\r' => return None,
                _ => {}
            }
            self.at += 1;
        }
    }

    /// An integer as `repr` spells one - no leading zero, no `-0` - or a float whose token is exactly `repr`'s
    /// spelling of its value.
    fn number(&mut self) -> Option<Node<'a>> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'+' | b'-'))
        {
            self.at += 1;
        }
        let token = &self.source[start..self.at];
        let unsigned = token.strip_prefix('-').unwrap_or(token);
        if unsigned.bytes().all(|b| b.is_ascii_digit()) {
            let canonical = !unsigned.is_empty()
                && (unsigned == "0" || !unsigned.starts_with('0'))
                && token != "-0";
            return canonical.then_some(Node::Int(token));
        }
        let value: f64 = token.parse().ok()?;
        (python_float(value)? == token).then_some(Node::Float(token, value))
    }

    /// A string as `repr` prints one: either quote, the escapes it writes decoded, a raw control character - which
    /// `repr` always escapes - refused.
    fn string(&mut self) -> Option<Node<'a>> {
        let start = self.at;
        let quote = self.peek()?;
        self.at += 1;
        let mut value = String::new();
        loop {
            let rest = self.source.get(self.at..)?;
            let c = rest.chars().next()?;
            self.at += c.len_utf8();
            match c {
                '\\' => value.push(self.escape()?),
                c if c.is_control() => return None,
                c if c as u32 == u32::from(quote) => {
                    return Some(Node::Str(&self.source[start..self.at], value));
                }
                c => value.push(c),
            }
        }
    }

    /// The escapes `repr` writes in a string: `\\`, either quote, `\n`, `\r`, `\t`, `\xNN`, `\uNNNN` and
    /// `\UNNNNNNNN`, each of hex digits only. Any other - `\N{...}`, `\a`, octal, a signed hex - is not one `repr`
    /// prints, so the text is refused.
    fn escape(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        let hex = |reading: &mut Self, digits: usize| {
            let text = reading.source.get(reading.at..reading.at + digits)?;
            text.bytes().all(|b| b.is_ascii_hexdigit()).then_some(())?;
            reading.at += digits;
            u32::from_str_radix(text, 16).ok().and_then(char::from_u32)
        };
        match c {
            b'\\' => Some('\\'),
            b'\'' => Some('\''),
            b'"' => Some('"'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'x' => hex(self, 2),
            b'u' => hex(self, 4),
            b'U' => hex(self, 8),
            _ => None,
        }
    }

    fn identifier(&mut self, qualified: bool) -> Option<&'a str> {
        let start = self.at;
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        {
            return None;
        }
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || (qualified && b == b'.'))
        {
            self.at += 1;
        }
        Some(&self.source[start..self.at])
    }

    fn consume(&mut self, expected: u8) -> Option<()> {
        (self.peek()? == expected).then(|| self.at += 1)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }
}

#[cfg(test)]
#[path = "python_object_tests.rs"]
mod tests;
