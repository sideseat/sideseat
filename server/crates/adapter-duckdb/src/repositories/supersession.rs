//! Which revision follows which: the `superseded_at` a span write sets, on its new rows and on the stored rows
//! they follow (`sideseat_query_sql::winners`).
//!
//! The revisions of an identity are ordered by `ingested_at`, then by row: a stored row before every new one, new
//! rows in batch order, since that is the order the appender gives them row ids. Each revision is superseded at
//! the instant of the one after it, and the last is the winner. A revision that arrives out of order - ingested
//! earlier than one already stored, by another instance's clock or a redelivery - falls between two stored ones:
//! it is superseded at once, and the stored one before it is now superseded at the newcomer's instant instead.

use std::collections::HashMap;

use sideseat_ports::types::NormalizedSpan;

use super::keyed::{SpanIdentity, StoredRevision};

/// What a write sets: `superseded_at` for each new span in batch order, and for each stored row whose value
/// changes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Supersession {
    pub new: Vec<Option<i64>>,
    pub updates: Vec<(i64, Option<i64>)>,
}

/// The ingest instant a span is stored under, in epoch microseconds: the appender writes the same value.
pub(crate) fn instant_of(span: &NormalizedSpan) -> i64 {
    span.ingested_at
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .timestamp_micros()
}

#[derive(Clone, Copy)]
enum Revision {
    Stored(StoredRevision),
    New(usize),
}

/// Plan the `superseded_at` values a write of `spans` sets, given every stored revision of their identities.
pub(crate) fn plan(
    spans: &[NormalizedSpan],
    stored: &HashMap<SpanIdentity, Vec<StoredRevision>>,
) -> Supersession {
    let mut by_identity: HashMap<SpanIdentity, Vec<usize>> = HashMap::new();
    for (index, span) in spans.iter().enumerate() {
        by_identity
            .entry((
                span.project_id.clone().unwrap_or_default(),
                span.trace_id.clone(),
                span.span_id.clone(),
            ))
            .or_default()
            .push(index);
    }
    let mut supersession = Supersession {
        new: vec![None; spans.len()],
        updates: Vec::new(),
    };
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each new span's value is set by its position, and the updates are sorted below"
    )]
    for (identity, indices) in by_identity {
        // (instant, tier, position): a stored row precedes every new one at the same instant, as its row id does.
        let mut revisions: Vec<((i64, u8, i64), Revision)> = stored
            .get(&identity)
            .into_iter()
            .flatten()
            .map(|revision| {
                (
                    (revision.ingested_us, 0, revision.rowid),
                    Revision::Stored(*revision),
                )
            })
            .chain(indices.iter().map(|&index| {
                (
                    (instant_of(&spans[index]), 1, index as i64),
                    Revision::New(index),
                )
            }))
            .collect();
        revisions.sort_unstable_by_key(|(order, _)| *order);
        for (position, (_, revision)) in revisions.iter().enumerate() {
            let following = revisions
                .get(position + 1)
                .map(|((instant, _, _), _)| *instant);
            match revision {
                Revision::New(index) => supersession.new[*index] = following,
                Revision::Stored(stored) if stored.superseded_us != following => {
                    supersession.updates.push((stored.rowid, following));
                }
                Revision::Stored(_) => {}
            }
        }
    }
    supersession.updates.sort_unstable();
    supersession
}

/// Recompute every row's `superseded_at` from the revisions stored - for tests that write rows with plain SQL
/// rather than through the span writer, which keeps it as it writes.
#[cfg(test)]
pub(crate) fn rebuild(conn: &duckdb::Connection) -> Result<(), crate::DuckdbError> {
    conn.execute_batch(
        "UPDATE otel_spans SET superseded_at = following.at FROM (\
           SELECT rowid AS row, lead(ingested_at) OVER (PARTITION BY project_id, trace_id, span_id \
                                                    ORDER BY ingested_at, rowid) AS at \
           FROM otel_spans) following \
         WHERE otel_spans.rowid = following.row \
           AND otel_spans.superseded_at IS DISTINCT FROM following.at",
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "supersession_tests.rs"]
mod tests;
