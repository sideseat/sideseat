//! Which copy survives a dedup tie, as a switch the tests can flip.

// Test-only: prefer the *later* of two tied copies instead of the earlier.
//
// Which copy survives a quality tie is decided by arrival order, and it is not something a caller can
// vary - reversing the row order does not reach it, because rows are re-sorted by timestamp before
// dedup ever sees them. So the property "the *order* of the answer does not depend on which copy
// survived" had no test, which is the central claim of the ordering redesign.
//
// Flipping this changes which copy survives, and content differs between copies - so the test asserts
// the shape is unchanged *and* that the content moved somewhere, since a perturbation that reaches
// nothing proves nothing.
//
// Thread-local, not a global: the suite runs in parallel, and a process-wide flag changed what every
// other test was measuring at the same time. The read path runs on its caller's thread, so a
// thread-local reaches exactly the pipeline under test.
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    #[doc(hidden)]
    pub static PREFER_LATER_ON_TIE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn prefer_later_on_tie() -> bool {
    PREFER_LATER_ON_TIE.with(|flag| flag.get())
}

#[cfg(not(any(test, feature = "test-support")))]
pub(super) fn prefer_later_on_tie() -> bool {
    false
}

use std::cmp::Ordering;

use crate::sideml::ContentBlock;
use crate::sideml::feed::types::BlockEntry;

/// Whether `block` replaces `existing`, two copies of one identity whose quality ties: the copy carrying the
/// value wins over its producer's redaction marker (`feed::redaction`), then the more original copy, and only then
/// arrival order - an order by those three in turn, so no third copy can undo what two decided. The marker
/// preference holds only where every copy of the identity names one tool (`one_tool`): copies naming different
/// tools are a contradiction the identity carries, and which of them a marker is decides nothing about it, so
/// such an identity is decided as it was before markers. A marker against a marker, or a value against a value,
/// is decided as it always was.
pub(super) fn wins_on_equal_quality(
    block: &BlockEntry,
    existing: &BlockEntry,
    one_tool: bool,
) -> bool {
    use super::super::redaction::is_marker;
    let information = if one_tool {
        is_marker(existing).cmp(&is_marker(block))
    } else {
        Ordering::Equal
    };
    match information.then_with(|| existing.origin_rank().cmp(&block.origin_rank())) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => prefer_later_on_tie(),
    }
}

/// The identities whose copies name more than one tool between them (`redaction::tool_named`).
pub(super) fn naming_several_tools<'a>(
    copies: impl Iterator<Item = (&'a super::DedupKey, &'a BlockEntry)>,
) -> std::collections::HashSet<super::DedupKey> {
    let mut named: std::collections::HashMap<&super::DedupKey, &str> =
        std::collections::HashMap::new();
    let mut several = std::collections::HashSet::new();
    for (key, copy) in copies {
        if let Some(tool) = super::super::redaction::tool_named(copy)
            && *named.entry(key).or_insert(tool) != tool
        {
            several.insert(key.clone());
        }
    }
    several
}

/// A block's content as the text the order compares: its canonical JSON, members in key order. Computed once,
/// on the first tie that needs it, so a sort pays for one rendering of each block it has to look inside and none
/// for the rest. The order of what blocks say, in place of a content hash - whose values change whenever what is
/// hashed, or how, does.
#[derive(Default)]
pub(super) struct ContentText(std::cell::OnceCell<String>);

impl ContentText {
    pub(super) fn of(&self, content: &ContentBlock) -> &str {
        self.0.get_or_init(|| {
            serde_json::to_value(content)
                .map(|value| canonical_text(&value))
                .unwrap_or_default()
        })
    }
}

/// A JSON value as text with every object's members in key order.
fn canonical_text(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(members) => {
                let mut sorted: Vec<_> = members.iter().collect();
                sorted.sort_unstable_by(|a, b| a.0.cmp(b.0));
                out.push('{');
                for (at, (key, member)) in sorted.into_iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(key.clone()).to_string());
                    out.push(':');
                    write(member, out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            scalar => out.push_str(&scalar.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// The survivors in one canonical order before the order graph sees them: a pure function of their content, so
/// two identical requests return one message list.
pub(super) fn canonical_order(
    result: Vec<(BlockEntry, super::DedupKey)>,
) -> Vec<(BlockEntry, super::DedupKey)> {
    let texts: Vec<ContentText> = result.iter().map(|_| ContentText::default()).collect();
    let mut order: Vec<usize> = (0..result.len()).collect();

    // HashMap iteration order is arbitrary, and the feed sort downstream has ties it
    // cannot break (same span, same timestamp, same indices). Leaving those ties to hash
    // order lets two identical requests return the message list in different orders.
    // Anchor it here so the pipeline is a pure function of its input.
    order.sort_by(|&x, &y| {
        let ((a, a_key), (b, b_key)) = (&result[x], &result[y]);
        a.trace_id
            .cmp(&b.trace_id)
            .then_with(|| a.span_id.cmp(&b.span_id))
            .then_with(|| a.message_index.cmp(&b.message_index))
            .then_with(|| a.entry_index.cmp(&b.entry_index))
            .then_with(|| a.entry_type.cmp(&b.entry_type))
            // Role next: two blocks can share span, indices and entry_type and still be
            // distinct messages, and without this the tie falls back to hash order.
            .then_with(|| a.role.as_str().cmp(b.role.as_str()))
            // Content last, so the comparator is TOTAL. Everything above can be equal for two
            // blocks that differ only in content - a span that emits several parts under one
            // message/entry index - and those were left in HashMap order, so two identical
            // requests returned the same messages in a different order. Observed directly:
            // repeated runs over one fixture disagreed on 3 then 4 views.
            .then_with(|| texts[x].of(&a.content).cmp(texts[y].of(&b.content)))
            // Two calls of one shape a span's two carriers place at one position can tie on all of the above,
            // and their rank and id are what tell them apart.
            .then_with(|| a_key.1.cmp(&b_key.1))
            .then_with(|| a.tool_use_id.cmp(&b.tool_use_id))
            // Last, for copies whose every shown member agrees - a thinking block's signature is not shown -
            // the hash of all of it.
            .then_with(|| a.content_hash.cmp(&b.content_hash))
    });
    let mut slots: Vec<Option<(BlockEntry, super::DedupKey)>> =
        result.into_iter().map(Some).collect();
    let result: Vec<(BlockEntry, super::DedupKey)> = order
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect();
    result
}
