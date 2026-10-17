//! When an observation happened, where its carrier declares it (`dated_by`).
//!
//! A block is dated by its carrier's own time: an event's, or for an attribute its span's start. Some carriers
//! record an observation at another moment than the one it happened at, and say so:
//!
//! - `span_start`: the carrier is written when its span ends but describes what began the span - a tool call
//!   rebuilt from the record of its result happened when the span that ran it started, not when the result came
//!   back. Every observation of such a carrier is dated so.
//! - `span_end`: the carrier holds what the span returned, which exists only once it returns. Only the carrier's
//!   sole observation on its span is dated so, after history marking: a carrier holding several is re-listing a
//!   conversation, whose turns keep their own times. Places are counted, not copies, so a re-delivered span -
//!   which repeats its observations at the same places - is dated as the first delivery was.
//!
//! A span's time is trusted only where it is one: a start the producer omitted arrives as the epoch, and a start
//! after the record's own time, or an end before it, is no moment the observation could have happened at - in
//! each case the block keeps its own time. A date only ever moves a block earlier to its span's start, or later
//! to its end.
//!
//! The date is the block's timestamp and nothing else. Whether a block completes its span (`uses_span_end`) is a
//! question about its shape that dedup reads too, and a date moving it would change which copy survives.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use super::dedup::SpanTimestamps;
use super::types::BlockEntry;
use crate::rules::schema::CarrierDate;

/// The date the block's carrier declares, as resolved for the span that wrote it.
fn declared(block: &BlockEntry) -> CarrierDate {
    crate::sideml::carrier::semantics_for_context(&block.carrier_context()).dated_by
}

/// Dates every observation whose carrier declares `span_start` at its span's start.
pub(super) fn date_at_span_start(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) {
    at_span_start(blocks, span_timestamps, declared);
}

/// Dates the sole observation of each carrier declaring `span_end` at its span's end. Run after history marking.
pub(super) fn date_returns_at_span_end(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) {
    at_span_end(blocks, span_timestamps, declared);
}

fn at_span_start(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
    date: impl Fn(&BlockEntry) -> CarrierDate,
) {
    for block in blocks.iter_mut() {
        if date(block) == CarrierDate::SpanStart
            && let Some(times) = span_timestamps.get(&block.span_id)
            && times.span_start > DateTime::<Utc>::UNIX_EPOCH
            && times.span_start <= block.timestamp
        {
            block.timestamp = times.span_start;
        }
    }
}

/// One carrier on one span: its observations are counted together.
type CarrierKey = (String, String, Option<String>, Option<String>);

fn carrier_of(block: &BlockEntry) -> CarrierKey {
    (
        block.trace_id.clone(),
        block.span_id.clone(),
        block.event_name.clone(),
        block.source_attribute.clone(),
    )
}

fn at_span_end(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
    date: impl Fn(&BlockEntry) -> CarrierDate,
) {
    let mut places: HashMap<CarrierKey, HashSet<(i32, i32)>> = HashMap::new();
    for block in blocks.iter().filter(|b| date(b) == CarrierDate::SpanEnd) {
        places
            .entry(carrier_of(block))
            .or_default()
            .insert((block.message_index, block.entry_index));
    }
    for block in blocks.iter_mut() {
        if date(block) == CarrierDate::SpanEnd
            && places
                .get(&carrier_of(block))
                .is_some_and(|held| held.len() == 1)
            && let Some(end) = span_timestamps.get(&block.span_id).and_then(|t| t.span_end)
            && end >= block.timestamp
        {
            block.timestamp = end;
        }
    }
}

#[cfg(test)]
#[path = "dating_tests.rs"]
mod tests;
