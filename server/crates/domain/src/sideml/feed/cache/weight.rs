//! What one cached answer weighs, which is what bounds the memo by bytes rather than by entries.

use super::super::types::FeedResult;
use sideseat_core::constants::RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES;

/// What one cached answer weighs, in bytes.
///
/// Measured by **serialising into a counter**, not by allocating the JSON: the answer is serialised on its way
/// to a reader anyway, so its serialised size is both a good proxy for what it occupies and the figure that
/// actually means something to a caller. `serde_json::to_writer` into a sink that only counts is O(bytes) and
/// O(1) memory, and it runs once per cache fill - immediately after a reconstruction that cost between
/// milliseconds and seconds, so it is not on any path where it is measurable.
///
/// Serialisation does not see everything, and the two gaps need different treatment. The **fixed** costs - the
/// `BlockEntry` structs, their `Vec` slots, their `span_path` allocations - are covered by a flat
/// `RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES` per block, which also gives the entry count an implicit bound.
/// The **unbounded** ones cannot be: `span_name`, `scope_name`, `scope_version` and `position` are marked
/// `#[serde(skip)]` and are as long as a producer makes them, so a flat charge left two blocks carrying 40 MiB
/// span names weighing about a kilobyte between them while retaining 80 MiB - a ceiling that is not a ceiling.
/// They are measured directly, as is a thinking block's signature, which a view states only as `signed`.
///
/// A serialisation failure weighs the entry at the maximum rather than at nothing. `FeedResult` serialises
/// infallibly today, and a weigher that answered zero on an error would let a value that cannot be measured
/// occupy the cache for free - the same shape as a gate that passes because it saw nothing.
pub(super) fn weight_of(result: &FeedResult) -> u32 {
    /// An `io::Write` that keeps only the length.
    struct CountingSink(u64);

    impl std::io::Write for CountingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len() as u64);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // Part by part rather than through a `Serialize` impl on `FeedResult`: the struct is an internal
    // pipeline result and the wire DTOs are separate, so deriving `Serialize` on it to satisfy a weigher
    // would add a serialisation surface that nothing serialises.
    let mut sink = CountingSink(0);
    let measured = serde_json::to_writer(&mut sink, &result.messages)
        .and_then(|()| serde_json::to_writer(&mut sink, &result.tool_definitions))
        .and_then(|()| serde_json::to_writer(&mut sink, &result.tool_names))
        .and_then(|()| serde_json::to_writer(&mut sink, &result.metadata));
    if measured.is_err() {
        return u32::MAX;
    }

    // The `#[serde(skip)]` fields, measured directly.
    //
    // A flat per-block charge cannot cover these, because they are *unbounded*: `span_name`, `scope_name` and
    // `scope_version` come off the span row and are as long as a producer makes them, and `position` grows with
    // the payload's nesting. Serialisation never sees any of them, so two blocks carrying 40 MiB span names
    // weighed about a kilobyte between them while retaining 80 MiB - which made the byte ceiling not a ceiling,
    // the one property this weigher exists to provide.
    let skipped: u64 = result
        .messages
        .iter()
        .map(|block| {
            let names = block.span_name.as_ref().map_or(0, String::len)
                + block.scope_name.as_ref().map_or(0, String::len)
                + block.scope_version.as_ref().map_or(0, String::len);
            (names + block.position.approximate_bytes() + block.content.unserialised_bytes()) as u64
        })
        .sum();

    let overhead =
        (result.messages.len() as u64).saturating_mul(RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES);
    sink.0
        .saturating_add(skipped)
        .saturating_add(overhead)
        .try_into()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod weight_tests {
    use super::*;
    use sideseat_core::constants::RECONSTRUCTION_CACHE_MAX_BYTES;

    /// The weight grows with the answer, and a bigger answer is charged more than a smaller one.
    ///
    /// The point of the weigher is that 512 large answers can no longer occupy the cache as cheaply as 512
    /// small ones, so what has to hold is the ordering - not a byte-exact figure, which would pin the JSON
    /// representation to this test.
    #[test]
    fn a_larger_answer_weighs_more() {
        let empty = FeedResult::default();
        let empty_weight = weight_of(&empty);
        assert!(
            empty_weight > 0,
            "even an empty answer occupies its own structure"
        );

        let larger = FeedResult {
            tool_names: (0..100)
                .map(|n| format!("tool-{n}-{}", "x".repeat(200)))
                .collect(),
            ..FeedResult::default()
        };
        assert!(
            weight_of(&larger) > empty_weight * 10,
            "an answer carrying 20 KB of names must weigh far more than an empty one: {} vs {}",
            weight_of(&larger),
            empty_weight
        );
    }

    /// A field serialisation cannot see is still charged.
    ///
    /// The exact scenario the weigher missed: `span_name` is `#[serde(skip)]`, so a block carrying a megabyte
    /// of it serialised to nothing and weighed like an empty block. Without this the 64 MB ceiling admits
    /// unbounded memory, and every other test here passes.
    #[test]
    fn a_serde_skipped_field_is_charged() {
        let plain = FeedResult {
            messages: vec![block_with_span_name(None)],
            ..FeedResult::default()
        };
        let heavy = FeedResult {
            messages: vec![block_with_span_name(Some("n".repeat(1024 * 1024)))],
            ..FeedResult::default()
        };

        let plain_weight = weight_of(&plain);
        let heavy_weight = weight_of(&heavy);
        assert!(
            heavy_weight as u64 >= plain_weight as u64 + 1024 * 1024,
            "a 1 MiB span name must be charged: {plain_weight} vs {heavy_weight}"
        );
    }

    /// A thinking block's signature is held for identity and never serialised, so it is measured too.
    #[test]
    fn an_unserialised_signature_is_charged() {
        let thinking = |signature: String| {
            let mut block = block_with_span_name(None);
            block.content = crate::sideml::types::ContentBlock::Thinking {
                text: String::new(),
                signature: Some(signature),
            };
            FeedResult {
                messages: vec![block],
                ..FeedResult::default()
            }
        };
        let short = weight_of(&thinking(String::new())) as u64;
        let long = weight_of(&thinking("s".repeat(1024 * 1024))) as u64;
        assert!(
            long >= short + 1024 * 1024,
            "a 1 MiB signature must be charged: {short} vs {long}"
        );
    }

    /// A block whose only difference is an unserialised span name.
    fn block_with_span_name(span_name: Option<String>) -> crate::sideml::feed::BlockEntry {
        use crate::sideml::provenance::PositionPath;
        use crate::sideml::types::{ChatRole, ContentBlock};
        use chrono::TimeZone;
        let t = chrono::Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid time");
        crate::sideml::feed::BlockEntry {
            position: PositionPath::default(),
            entry_type: "text".to_string(),
            content: ContentBlock::Text {
                text: "hi".to_string(),
                citations: Vec::new(),
            },
            role: ChatRole::User,
            trace_id: "t".to_string(),
            span_id: "s".to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: None,
            span_path: Vec::new(),
            timestamp: t,
            order_time: t,
            occurrence_ordinal: 0,
            span_name,
            scope_name: None,
            scope_version: None,
            observation_type: None,
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "attribute".to_string(),
            event_name: None,
            source_attribute: Some("carrier".to_string()),
            category: sideseat_ports::types::MessageCategory::GenAIUserMessage,
            content_hash: "hi".to_string(),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
            is_cross_trace_history: false,
            tool_use_id_correlated: false,
            promoted_to_span_output: false,
            is_rendering: false,
            declared_direction: None,
        }
    }

    /// Every `BlockEntry` field serialisation cannot see is either fixed-size or measured by `weight_of`.
    ///
    /// The weigher measures a serialised answer, so a `#[serde(skip)]` field is invisible to it. That is fine
    /// for a `bool` or a `DateTime`, whose cost the flat per-block charge covers, and not fine for a `String`
    /// or a `Vec`, which is as large as a producer makes it - a 40 MiB span name weighed nothing at all until
    /// it was measured directly, and the byte ceiling was not a ceiling.
    ///
    /// Read from the source, because the failure is *adding a field* and no behavioural test can notice one
    /// that nothing yet populates. Names the field it does not recognise, so the fix is obvious: either it is
    /// fixed-size and belongs in the list below, or it is unbounded and belongs in `weight_of`.
    #[test]
    fn every_unserialised_block_field_is_accounted_for() {
        let source = include_str!("../types.rs");
        let block_entry = source
            .split_once("pub struct BlockEntry {")
            .expect("BlockEntry is declared here")
            .1;
        let block_entry = &block_entry[..block_entry.find("\n}").expect("its declaration ends")];

        // Fields whose size is bounded by their type, so the flat per-block charge covers them.
        let fixed_size = [
            "order_time",
            "occurrence_ordinal",
            "is_rendering",
            "declared_direction",
        ];
        // Fields `weight_of` measures directly.
        let measured = ["position", "span_name", "scope_name", "scope_version"];

        let mut unaccounted: Vec<&str> = Vec::new();
        let mut skipped_next = false;
        for line in block_entry.lines() {
            let trimmed = line.trim();
            // `skip` and `skip_serializing` both remove the field from what the weigher sees;
            // `skip_serializing_if` does not, since a present value is still written.
            if trimmed == "#[serde(skip)]" || trimmed == "#[serde(skip_serializing)]" {
                skipped_next = true;
                continue;
            }
            if !skipped_next || !trimmed.starts_with("pub ") {
                continue;
            }
            skipped_next = false;
            let (name, ty) = trimmed
                .trim_start_matches("pub ")
                .split_once(':')
                .expect("a field declaration");
            let name = name.trim();
            if fixed_size.contains(&name) || measured.contains(&name) {
                continue;
            }
            // A `bool` needs no measurement whatever it is called; anything else does.
            if ty.trim().trim_end_matches(',') == "bool" {
                continue;
            }
            unaccounted.push(name);
        }

        assert!(
            unaccounted.is_empty(),
            "these BlockEntry fields are hidden from serialisation and unaccounted for in `weight_of`, so the \
             cache's byte ceiling does not bound them: {unaccounted:?}"
        );
    }

    /// The declared ceiling admits a useful number of ordinary answers.
    ///
    /// A weigher whose per-entry floor is set too high turns a 64 MB cache into a dozen entries, which is a
    /// footprint win and a cache that never hits. Stated as a test because it is the trade the flat charge
    /// makes, and it is invisible in the constant.
    #[test]
    fn the_ceiling_admits_a_useful_number_of_ordinary_answers() {
        let ordinary = weight_of(&FeedResult::default()).max(1) as u64
            + RECONSTRUCTION_CACHE_ENTRY_OVERHEAD_BYTES * 200;
        let admitted = RECONSTRUCTION_CACHE_MAX_BYTES / ordinary;
        assert!(
            admitted >= 128,
            "a 200-block answer should fit hundreds of times over in the cache, not {admitted}"
        );
    }
}
