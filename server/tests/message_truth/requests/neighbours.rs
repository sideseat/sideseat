//! The empty turns a client merges into their neighbours, leaving only the newline it joins them with.
//!
//! The evidence is always the span's own payload, never the reconstruction under test - and it is used only
//! where the reconstruction shows one export of the payload's input verbatim, read from that export. An export is
//! either the span's indexed input attributes (`<family>.<n>.message.role` beside its content keys) or the one
//! message list a text attribute decodes to (a JSON or Python rendering whose list members are `{role, content}`
//! messages). It is *aligned* when the span's input blocks are exactly its texts, one block per text, in order,
//! each under the role its message states and each read from that export - an indexed message's own attribute
//! prefix, or the attribute holding the list. Only then is it known which payload message a block shows, and only
//! then does an empty message beside that one evidence a merge's newline. Anything an alignment cannot account
//! for - a part or member that is not text, a list member that is not a readable message, a second message list
//! in one attribute, a block read from elsewhere - leaves no evidence, so the merge is refused.

use std::collections::BTreeMap;

use serde_json::Value;

use super::super::recon::{Block, Recon};
use super::shown_role;

/// Every export of a span's input, by the family or attribute it was read from.
#[derive(Default)]
pub(in super::super) struct EmptyNeighbours {
    exports: BTreeMap<Source, Export>,
    /// The receipt of the span being read.
    receipt: usize,
}

/// Where an export was read from, which is where each of its blocks must have been read from - and in which
/// receipt of the span, since a span delivered twice is two exports that must never be assembled into one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// Indexed attributes `<family>.<n>.message.*`: message `n` is read from `attr:<family>.<n>.message`.
    Indexed(usize, String),
    /// The message list a text attribute decodes to: every message is read from `attr:<attribute>`.
    Decoded(usize, String),
}

/// One export of the input: its messages by index.
#[derive(Default)]
struct Export {
    messages: BTreeMap<u64, Message>,
    /// Whether it cannot say which list a block was read from: its attribute holds a second message list, or more
    /// nesting than the search reads.
    unaccountable: bool,
}

#[derive(Default)]
struct Message {
    /// The role the payload states, where it states one.
    role: Option<String>,
    /// Its non-empty texts by part index, one block each.
    texts: BTreeMap<u64, String>,
    /// Whether it holds anything but text - a part of another kind, a member with a value - which shows as
    /// blocks no text accounts for; or its text twice, as `content` and as `contents`, which need not agree.
    other: bool,
    /// Whether an indexed `content` / `contents.*` member was read.
    has_content: bool,
    has_contents: bool,
}

impl EmptyNeighbours {
    pub(super) fn of_span(recon: &Recon, trace: &str, span: &str) -> Self {
        use opentelemetry_proto::tonic::common::v1::any_value::Value as Any;
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut out = EmptyNeighbours::default();
        for path in &recon.paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if !name.is_some_and(|n| n.starts_with("req-")) {
                continue;
            }
            let request = crate::decode_request(path);
            let spans = request
                .resource_spans
                .iter()
                .flat_map(|r| r.scope_spans.iter())
                .flat_map(|s| s.spans.iter())
                // The span is the (trace, span) pair: span ids repeat across traces.
                .filter(|s| hex(&s.span_id) == span && hex(&s.trace_id) == trace);
            for found in spans {
                out.begin_receipt();
                for attribute in &found.attributes {
                    match attribute.value.as_ref().and_then(|v| v.value.as_ref()) {
                        Some(Any::StringValue(text)) => out.record(&attribute.key, text.clone()),
                        // A member that is not text has a value no text accounts for.
                        Some(_) => out.record_other(&attribute.key),
                        None => {}
                    }
                }
            }
        }
        out
    }

    /// Start reading another receipt of the span: what follows is a separate export.
    pub(super) fn begin_receipt(&mut self) {
        self.receipt += 1;
    }

    /// The message an indexed attribute key names, and the member of it the key is.
    fn indexed<'k>(&mut self, key: &'k str) -> Option<(&mut Message, &'k str)> {
        let (head, member) = key.split_once(".message.")?;
        let (family, index) = head.rsplit_once('.')?;
        let index = index.parse::<u64>().ok()?;
        let message = self
            .exports
            .entry(Source::Indexed(self.receipt, family.to_string()))
            .or_default()
            .messages
            .entry(index)
            .or_default();
        Some((message, member))
    }

    /// One of the span's text attributes: an indexed input member, or a text that may decode to a message list.
    pub(super) fn record(&mut self, key: &str, value: String) {
        let Some((message, member)) = self.indexed(key) else {
            self.record_decoded(key, &value);
            return;
        };
        if member == "role" {
            message.role = Some(value);
        } else if member == "content" {
            // Beside `contents.*`, a second representation of the same message, which may disagree with it.
            message.other |= message.has_contents;
            message.has_content = true;
            if !value.is_empty() {
                message.texts.insert(0, value);
            }
        } else if let Some((part, field)) = member
            .strip_prefix("contents.")
            .and_then(|rest| rest.split_once('.'))
            .and_then(|(part, field)| Some((part.parse::<u64>().ok()?, field)))
        {
            message.other |= message.has_content;
            message.has_contents = true;
            match field {
                "message_content.text" => {
                    if !value.is_empty() {
                        message.texts.insert(part, value);
                    }
                }
                "message_content.type" if value == "text" => {}
                _ => message.other = true,
            }
        } else {
            message.other = true;
        }
    }

    /// One indexed input attribute whose value is not text.
    fn record_other(&mut self, key: &str) {
        if let Some((message, _)) = self.indexed(key) {
            message.other = true;
        }
    }

    /// A text attribute that is not an indexed member: the message list it decodes to, if it holds one.
    ///
    /// Every list in the decoded value that has a member reading as a message is a message list, and each of its
    /// members is enumerated: a member that does not read as a message stays one stating no role, which no
    /// alignment accepts. An attribute holding more than one message list cannot say which one a block was read
    /// from, so it is unaccountable.
    fn record_decoded(&mut self, attribute: &str, text: &str) {
        let Decoded::Value(decoded) = decode(text) else {
            return;
        };
        let mut lists: Vec<Vec<Option<Value>>> = Vec::new();
        let searched = collect_lists(&decoded, &mut lists, 0);
        if lists.is_empty() && searched {
            return;
        }
        let export = self
            .exports
            .entry(Source::Decoded(self.receipt, attribute.to_string()))
            .or_default();
        export.unaccountable |= lists.len() != 1 || !searched;
        let Some(list) = lists.first() else {
            return;
        };
        for (index, member) in (0u64..).zip(list) {
            // A member that is no object stays a message stating no role, which no alignment accepts.
            let message = export.messages.entry(index).or_default();
            if let Some(object) = member.as_ref().and_then(Value::as_object) {
                read_message(object, message);
            }
        }
    }

    /// Whether some aligned export holds, beside the message the block at `place` shows, an empty message of
    /// `role` at `offset` - before it (`-1`) where the block is that message's first text, after it (`1`) where
    /// it is its last.
    fn evidenced(&self, blocks: &[&Block], place: usize, role: &str, offset: i64) -> bool {
        self.exports.iter().any(|(source, export)| {
            let Some(shown) = export.aligned(source, blocks) else {
                return false;
            };
            let Some(&(index, part)) = shown.get(place) else {
                return false;
            };
            let message = &export.messages[&index];
            let at_edge = if offset < 0 {
                message.texts.keys().next() == Some(&part)
            } else {
                message.texts.keys().next_back() == Some(&part)
            };
            // An aligned export holds no `other`, so empty is having no text.
            at_edge
                && index
                    .checked_add_signed(offset)
                    .and_then(|other| export.messages.get(&other))
                    .is_some_and(|other| {
                        other.texts.is_empty()
                            && other
                                .role
                                .as_deref()
                                .is_some_and(|stated| shown_role(stated, &Value::Null) == role)
                    })
        })
    }

    pub(super) fn empty_before(&self, blocks: &[&Block], place: usize, role: &str) -> bool {
        self.evidenced(blocks, place, role, -1)
    }

    pub(super) fn empty_after(&self, blocks: &[&Block], place: usize, role: &str) -> bool {
        self.evidenced(blocks, place, role, 1)
    }
}

impl Export {
    /// The (message, part) each block shows, where the blocks are exactly this export's texts in order, each
    /// under its message's role and read from this export; `None` where anything is left over on either side.
    fn aligned(&self, source: &Source, blocks: &[&Block]) -> Option<Vec<(u64, u64)>> {
        // Every member from the first index on, each a message stating a role and holding only text.
        let contiguous = (0u64..).zip(self.messages.keys()).all(|(i, &k)| i == k);
        if self.unaccountable
            || !contiguous
            || self
                .messages
                .values()
                .any(|message| message.other || message.role.is_none())
        {
            return None;
        }
        let texts: Vec<(u64, u64, &str, &str)> = self
            .messages
            .iter()
            .flat_map(|(&index, message)| {
                let role = message.role.as_deref().unwrap_or_default();
                message
                    .texts
                    .iter()
                    .map(move |(&part, text)| (index, part, role, text.as_str()))
            })
            .collect();
        if texts.len() != blocks.len() {
            return None;
        }
        texts
            .iter()
            .zip(blocks)
            .all(|(&(index, _, stated, text), block)| {
                let read_from = match source {
                    Source::Indexed(_, family) => format!("attr:{family}.{index}.message"),
                    Source::Decoded(_, attribute) => format!("attr:{attribute}"),
                };
                // A role spelling the rubric does not know - a producer's own pseudo-role, which the
                // reconstruction shows under a canonical one - may be the block's.
                let shown = shown_role(stated, &Value::Null);
                block.carrier == read_from
                    && block.kind == "text"
                    && block.text() == Some(text)
                    && (shown == block.role || shown == "unknown")
            })
            .then(|| {
                texts
                    .iter()
                    .map(|&(index, part, _, _)| (index, part))
                    .collect()
            })
    }
}

/// What a text decodes to.
enum Decoded {
    /// JSON, else a Python rendering.
    Value(Value),
    /// Neither, and it does not look like one: text.
    Text,
    /// Neither, though it looks like one - nested deeper than a reader goes, or malformed - so what it holds is
    /// unknown.
    Unknown,
}

fn decode(text: &str) -> Decoded {
    let trimmed = text.trim();
    if let Some(value) = serde_json::from_str::<Value>(trimmed)
        .ok()
        .or_else(|| sideseat_domain::sideml::test_support::parse_python_rendering(trimmed))
    {
        return Decoded::Value(value);
    }
    // A container (a tuple too), a quoted string or a constructor call: what a rendering starts as.
    let constructor = trimmed.split_once('(').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    });
    if trimmed.starts_with(['{', '[', '(', '"', '\'']) || constructor {
        Decoded::Unknown
    } else {
        Decoded::Text
    }
}

/// How deep renderings may nest inside one attribute before the search gives up. Giving up leaves the attribute
/// unaccountable, never a list unseen.
const MAX_DEPTH: usize = 8;

/// Every list in `value` with a member that reads as a message, each member decoded where it is a rendering
/// (`None` where it does not decode to anything) - searched everywhere outside a message list, inside every
/// decoded string too, so a list cannot hide beside the one found. `false` where the search gave up: nesting past
/// `MAX_DEPTH`, or a string that looks like a rendering no reader decodes.
fn collect_lists(value: &Value, out: &mut Vec<Vec<Option<Value>>>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match value {
        Value::String(text) => match decode(text) {
            Decoded::Value(inner) => collect_lists(&inner, out, depth + 1),
            Decoded::Text => true,
            Decoded::Unknown => false,
        },
        Value::Array(items) => {
            let members: Vec<Option<Value>> = items
                .iter()
                .map(|item| match item {
                    Value::String(text) => match decode(text) {
                        Decoded::Value(value) => Some(value),
                        Decoded::Text | Decoded::Unknown => None,
                    },
                    other => Some(other.clone()),
                })
                .collect();
            if members.iter().flatten().any(is_message) {
                // Its members are messages: any value they hold beyond their texts already makes them
                // unalignable, so nothing inside one can lend evidence.
                out.push(members);
                true
            } else {
                items.iter().all(|item| collect_lists(item, out, depth + 1))
            }
        }
        Value::Object(members) => members
            .values()
            .all(|item| collect_lists(item, out, depth + 1)),
        _ => true,
    }
}

fn is_message(value: &Value) -> bool {
    value.get("role").and_then(stated_role).is_some()
}

/// One decoded message: its role, its texts, and whether it holds anything else.
///
/// Empty means nothing at all besides its role: no content, or content whose every part is a text part with no
/// text, and every other member absent, null or empty.
fn read_message(object: &serde_json::Map<String, Value>, message: &mut Message) {
    let Some(role) = object.get("role").and_then(stated_role) else {
        message.other = true;
        return;
    };
    message.role = Some(role.to_string());
    message.other |= object.iter().any(|(member, value)| {
        member != "role"
            && member != "content"
            && member != "__python_constructor"
            && !nothing(value)
    });
    match object.get("content") {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) => {
            if !text.is_empty() {
                message.texts.insert(0, text.clone());
            }
        }
        Some(Value::Array(parts)) => {
            for (part, value) in (0u64..).zip(parts) {
                match text_part(value) {
                    Some("") => {}
                    Some(text) => {
                        message.texts.insert(part, text.to_string());
                    }
                    None => message.other = true,
                }
            }
        }
        Some(_) => message.other = true,
    }
}

/// A role as a payload states it: a string, or an enumeration member as the rubric's Python reader gives one.
fn stated_role(role: &Value) -> Option<&str> {
    role.as_str()
        .or_else(|| role.get("__python_value").and_then(Value::as_str))
}

/// The text of a part that is a text part and nothing else, empty included.
fn text_part(part: &Value) -> Option<&str> {
    let object = part.as_object()?;
    if object.get("type").is_some_and(|kind| kind != "text") {
        return None;
    }
    let text = object.get("text")?.as_str()?;
    object
        .iter()
        .all(|(member, value)| member == "type" || member == "text" || nothing(value))
        .then_some(text)
}

fn nothing(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}
