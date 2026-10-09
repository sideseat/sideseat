//! Java's conventional `toString` of an object graph, read into a JSON tree.
//!
//! The counterpart of the Python constructor `repr`: a JVM framework that records its own message objects
//! with `toString` writes `Name { key = value, ... }`, nested to any depth, with lists as `[a, b]` and maps as
//! `{k=v}`. The grammar is a sealed subset of what such code prints, and anything outside it fails closed,
//! leaving the telemetry to be read as text.

use serde_json::{Map, Value as JsonValue};

/// The member a parsed object keeps its class name under, beside its own members.
const JAVA_CLASS_MEMBER: &str = "__java_class";
const JAVA_TOSTRING_MAX_DEPTH: usize = 64;
/// How many readings of where the text's strings end are tried before it is left as text.
///
/// The format does not escape a quote inside a string, so where one ends is a choice: the reader tries the
/// earliest end the surrounding structure can follow, and on a later failure the next one, depth first. Almost
/// every text is read on the first attempt; the bound keeps a text built to defeat the pruning linear.
const JAVA_TOSTRING_MAX_ATTEMPTS: usize = 64;

/// Parse one object's `toString`, or a list of them, into a JSON tree.
///
/// - An object is `Name { member = value, ... }`; its class name is kept under `__java_class`. Members are
///   separated by `, `, or by a single space where a class leaves the comma out.
/// - A list is `[a, b]`, a map `{k=v, k=v}`; their elements are printed bare, as Java prints them.
/// - A string is a member's value in double quotes, unescaped, and is kept exactly as written.
/// - A bare token is `null`, `true`, `false`, a number that reads back as written, or otherwise its text: an
///   enum constant, a class name.
///
/// **A reading is accepted only where printing it again reproduces the text byte for byte.** Since a quote
/// inside a string is not escaped, a reading that ends a string at the wrong quote can still be well formed;
/// printed again with the format's own spacing it differs from the text unless that text could have been
/// printed from it, and the next reading is tried. Where two readings both reproduce it - the text inside a
/// string is itself a printed member - the text cannot tell them apart, and the earliest end is taken.
///
/// The whole text must be one object or one list. A duplicated member, a string where Java prints none, and a
/// text no reading within the bound reproduces are refused.
pub(crate) fn try_parse_java_tostring(source: &str) -> Option<JsonValue> {
    let mut choices: Vec<usize> = Vec::new();
    for _ in 0..JAVA_TOSTRING_MAX_ATTEMPTS {
        let mut reading = Reading::new(source, &choices);
        if let Some(value) = reading
            .document()
            .filter(|node| reprinted(source, node, 0) == Some(source.len()))
            .and_then(|node| node.into_json())
        {
            return Some(value);
        }
        // Depth first over where each string ends: an attempt is a function of the choices it consulted, so a
        // failure after `consulted` strings fails for every attempt sharing them, and the last consulted string
        // moves on to its next end - or, when it had none left, the one before it.
        let settled = if reading.exhausted {
            reading.consulted.checked_sub(1)?
        } else {
            reading.consulted
        };
        let advance = settled.checked_sub(1)?;
        choices.resize(advance + 1, 0);
        choices[advance] += 1;
    }
    None
}

/// One reading of the text, borrowing every name and string from it.
enum Node<'a> {
    Object {
        class: &'a str,
        members: Vec<(&'a str, Node<'a>)>,
        /// Between each two members: whether the text separates them with a comma or only a space.
        commas: Vec<bool>,
    },
    List(Vec<Node<'a>>),
    Map(Vec<(&'a str, Node<'a>)>),
    /// A member's quoted string, without its quotes.
    Text(&'a str),
    Bare(&'a str),
}

impl Node<'_> {
    fn into_json(self) -> Option<JsonValue> {
        Some(match self {
            Node::Object { class, members, .. } => {
                let mut object = Map::new();
                object.insert(
                    JAVA_CLASS_MEMBER.to_string(),
                    JsonValue::String(class.to_string()),
                );
                for (name, value) in members {
                    object
                        .insert(name.to_string(), value.into_json()?)
                        .is_none()
                        .then_some(())?;
                }
                JsonValue::Object(object)
            }
            Node::List(items) => JsonValue::Array(
                items
                    .into_iter()
                    .map(Node::into_json)
                    .collect::<Option<_>>()?,
            ),
            Node::Map(entries) => {
                let mut map = Map::new();
                for (key, value) in entries {
                    map.insert(key.to_string(), value.into_json()?)
                        .is_none()
                        .then_some(())?;
                }
                JsonValue::Object(map)
            }
            Node::Text(text) => JsonValue::String(text.to_string()),
            Node::Bare(token) => scalar(token),
        })
    }
}

/// Where the text printed from `node` with the format's own spacing ends, if the source holds exactly that text
/// from `at`: the check that a reading is one the text could have been printed from.
fn reprinted(source: &str, node: &Node<'_>, at: usize) -> Option<usize> {
    let literal = |at: usize, text: &str| {
        source
            .get(at..)
            .is_some_and(|rest| rest.starts_with(text))
            .then_some(at + text.len())
    };
    match node {
        Node::Object {
            class,
            members,
            commas,
        } => {
            let mut at = literal(literal(at, class)?, " {")?;
            for (index, (name, value)) in members.iter().enumerate() {
                let separator = match index
                    .checked_sub(1)
                    .and_then(|gap| commas.get(gap).copied())
                {
                    Some(true) => ", ",
                    None | Some(false) => " ",
                };
                at = literal(literal(literal(at, separator)?, name)?, " = ")?;
                at = reprinted(source, value, at)?;
            }
            literal(at, " }")
        }
        Node::List(items) => {
            let mut at = literal(at, "[")?;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    at = literal(at, ", ")?;
                }
                at = reprinted(source, item, at)?;
            }
            literal(at, "]")
        }
        Node::Map(entries) => {
            let mut at = literal(at, "{")?;
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    at = literal(at, ", ")?;
                }
                at = reprinted(source, value, literal(literal(at, key)?, "=")?)?;
            }
            literal(at, "}")
        }
        Node::Text(text) => literal(literal(literal(at, "\"")?, text)?, "\""),
        Node::Bare(token) => literal(at, token),
    }
}

/// What a value sits inside, and so what may follow it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// `Name { ... }`: members, closed by `}`.
    Object,
    /// `[ ... ]`: bare elements, closed by `]`.
    List,
    /// `{ ... }`: `key=value` entries, closed by `}`.
    Map,
}

/// One attempt at reading the text, with each string ending where `choices` says.
struct Reading<'a, 'c> {
    source: &'a str,
    bytes: &'a [u8],
    at: usize,
    stack: Vec<Frame>,
    /// For the n-th string read, which of its admissible ends to take: 0 is the earliest.
    choices: &'c [usize],
    /// How many strings this attempt has read, the one it failed on included.
    consulted: usize,
    /// The last string read had no admissible end left at its choice.
    exhausted: bool,
}

impl<'a, 'c> Reading<'a, 'c> {
    fn new(source: &'a str, choices: &'c [usize]) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            at: 0,
            stack: Vec::new(),
            choices,
            consulted: 0,
            exhausted: false,
        }
    }

    fn document(&mut self) -> Option<Node<'a>> {
        let node = match self.peek()? {
            b'[' => self.list()?,
            _ => {
                let class = self.identifier(true)?;
                self.skip_whitespace();
                self.object(class)?
            }
        };
        (self.at == self.bytes.len()).then_some(node)
    }

    /// A value where Java prints one bare: a list element, a map value, or a member that is not a string.
    fn value(&mut self) -> Option<Node<'a>> {
        match self.peek()? {
            b'[' => self.list(),
            b'{' => self.map(),
            b'"' | b',' | b']' | b'}' | b'=' => None,
            _ => {
                let start = self.at;
                let token = self.bare();
                self.skip_whitespace();
                if self.peek() == Some(b'{') && is_qualified_identifier(token) {
                    return self.object(token);
                }
                // A bare token does not swallow the whitespace after it: the caller reads what follows.
                self.at = start + token.len();
                (!token.is_empty()).then_some(Node::Bare(token))
            }
        }
    }

    fn object(&mut self, class: &'a str) -> Option<Node<'a>> {
        self.enter(Frame::Object)?;
        self.consume(b'{')?;
        let mut members = Vec::new();
        let mut commas = Vec::new();
        self.skip_whitespace();
        while self.peek()? != b'}' {
            let name = self.identifier(false)?;
            self.skip_whitespace();
            self.consume(b'=')?;
            self.skip_whitespace();
            let value = if self.peek()? == b'"' {
                Node::Text(self.string()?)
            } else {
                self.value()?
            };
            if name == JAVA_CLASS_MEMBER {
                return None;
            }
            members.push((name, value));
            self.skip_whitespace();
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.skip_whitespace();
                    commas.push(true);
                }
                b'}' => {}
                // A member written after the last with no comma between them.
                _ => {
                    self.member_at(self.at).then_some(())?;
                    commas.push(false);
                }
            }
        }
        self.consume(b'}')?;
        self.stack.pop();
        Some(Node::Object {
            class,
            members,
            commas,
        })
    }

    fn list(&mut self) -> Option<Node<'a>> {
        self.enter(Frame::List)?;
        self.consume(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        while self.peek()? != b']' {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.skip_whitespace();
                }
                b']' => {}
                _ => return None,
            }
        }
        self.consume(b']')?;
        self.stack.pop();
        Some(Node::List(items))
    }

    fn map(&mut self) -> Option<Node<'a>> {
        self.enter(Frame::Map)?;
        self.consume(b'{')?;
        let mut entries = Vec::new();
        self.skip_whitespace();
        while self.peek()? != b'}' {
            let key = self.bare();
            if key.is_empty() {
                return None;
            }
            self.consume(b'=')?;
            entries.push((key, self.value()?));
            self.skip_whitespace();
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.skip_whitespace();
                }
                b'}' => {}
                _ => return None,
            }
        }
        self.consume(b'}')?;
        self.stack.pop();
        Some(Node::Map(entries))
    }

    /// A member's string, ending at the admissible closing quote this attempt's choice names.
    fn string(&mut self) -> Option<&'a str> {
        let index = self.consulted;
        self.consulted += 1;
        let choice = self.choices.get(index).copied().unwrap_or(0);
        self.consume(b'"')?;
        let start = self.at;
        let mut seen = 0;
        let mut quote = start;
        loop {
            let Some(offset) = self.bytes[quote..].iter().position(|&b| b == b'"') else {
                self.exhausted = true;
                return None;
            };
            quote += offset;
            if self.closes_here(quote) {
                if seen == choice {
                    break;
                }
                seen += 1;
            }
            quote += 1;
        }
        self.at = quote + 1;
        Some(&self.source[start..quote])
    }

    /// Whether the structure around a member's string can follow a closing quote at `quote`: the frames it
    /// closes match the ones open, and then what comes next is what the innermost remaining frame allows. Only
    /// a filter - the full reading and its reprint decide - but one exact enough that a quote inside the text
    /// is rarely taken for the end.
    fn closes_here(&self, quote: usize) -> bool {
        let mut at = self.whitespace_from(quote + 1);
        let mut open = self.stack.len();
        loop {
            let Some(&next) = self.bytes.get(at) else {
                return open == 0;
            };
            let Some(frame) = open.checked_sub(1).map(|top| self.stack[top]) else {
                return false;
            };
            match (frame, next) {
                (Frame::Object | Frame::Map, b'}') | (Frame::List, b']') => {
                    open -= 1;
                    at = self.whitespace_from(at + 1);
                }
                (_, b'}' | b']') => return false,
                (Frame::Object, b',') => return self.member_at(self.whitespace_from(at + 1)),
                (Frame::Object, _) => return self.member_at(at),
                (Frame::List | Frame::Map, b',') => {
                    let value = self.whitespace_from(at + 1);
                    return self
                        .bytes
                        .get(value)
                        .is_some_and(|b| !matches!(b, b'"' | b',' | b']' | b'}' | b'='));
                }
                _ => return false,
            }
        }
    }

    /// Whether a member - `name =` - starts at `at`.
    fn member_at(&self, at: usize) -> bool {
        let name = self
            .bytes
            .get(at..)
            .unwrap_or_default()
            .iter()
            .take_while(|&&b| is_identifier_byte(b, false))
            .count();
        name > 0
            && !self.bytes[at].is_ascii_digit()
            && self.bytes.get(self.whitespace_from(at + name)) == Some(&b'=')
    }

    fn enter(&mut self, frame: Frame) -> Option<()> {
        (self.stack.len() < JAVA_TOSTRING_MAX_DEPTH).then_some(())?;
        self.stack.push(frame);
        Some(())
    }

    /// A run of bytes up to whitespace or structure.
    fn bare(&mut self) -> &'a str {
        let start = self.at;
        while self.peek().is_some_and(|b| {
            !b.is_ascii_whitespace() && !matches!(b, b',' | b'}' | b']' | b'{' | b'[' | b'=' | b'"')
        }) {
            self.at += 1;
        }
        &self.source[start..self.at]
    }

    fn identifier(&mut self, qualified: bool) -> Option<&'a str> {
        let start = self.at;
        if !self
            .peek()
            .is_some_and(|b| is_identifier_byte(b, false) && !b.is_ascii_digit())
        {
            return None;
        }
        while self
            .peek()
            .is_some_and(|b| is_identifier_byte(b, qualified))
        {
            self.at += 1;
        }
        Some(&self.source[start..self.at])
    }

    fn whitespace_from(&self, mut at: usize) -> usize {
        while self.bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    }

    fn skip_whitespace(&mut self) {
        self.at = self.whitespace_from(self.at);
    }

    fn consume(&mut self, expected: u8) -> Option<()> {
        (self.peek()? == expected).then(|| self.at += 1)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }
}

/// An identifier byte: ASCII letters, digits, `_` and `$`, and `.` in a qualified class name.
fn is_identifier_byte(b: u8, qualified: bool) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || (qualified && b == b'.')
}

fn is_qualified_identifier(token: &str) -> bool {
    token
        .bytes()
        .next()
        .is_some_and(|b| is_identifier_byte(b, false) && !b.is_ascii_digit())
        && token.bytes().all(|b| is_identifier_byte(b, true))
}

/// A bare token as the value it prints: the three literals, a number only where it reads back exactly as
/// written - `0.50` stays text, since as a number it would print `0.5` - and its text otherwise.
fn scalar(token: &str) -> JsonValue {
    match token {
        "null" => JsonValue::Null,
        "true" => JsonValue::Bool(true),
        "false" => JsonValue::Bool(false),
        _ => token
            .parse::<serde_json::Number>()
            .ok()
            .filter(|number| number.to_string() == token)
            .map_or_else(|| JsonValue::String(token.to_string()), JsonValue::Number),
    }
}

#[cfg(test)]
#[path = "java_tostring_tests.rs"]
mod tests;
