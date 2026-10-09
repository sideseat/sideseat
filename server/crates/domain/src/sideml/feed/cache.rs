//! Memoised reconstruction avoids normalising rows twice.
//!
//! Normalisation happens at query time, which is deliberate: a fix to the pipeline applies to history
//! that was ingested before it, with no re-ingestion. The cost is that every read pays for the whole
//! session, and `bench_session_scaling` says what that is - the pipeline is linear in its input at about
//! 27 MB/s, but a framework that re-sends the conversation as its next turn's input makes the *input*
//! quadratic in the turn count. A thousand-turn LangGraph session is 68 MB of telemetry and 2.6 seconds
//! of reconstruction, on every read, for an answer of two thousand blocks.
//!
//! # Why this cannot serve a stale answer
//!
//! The key is a hash of everything the pipeline reads - each row's identity and its payloads - so any
//! change to any row is a different key rather than a stale hit. There is no invalidation to get wrong
//! and no TTL to tune: a re-delivered span rewrites its row, the digest changes, and the old entry is
//! simply never asked for again.
//!
//! Process-local and empty at startup, which is the other half. A cache that outlived the binary would
//! serve reconstructions made by the *previous* version of the pipeline, silently undoing "fixes apply to
//! historical data" - and the alternative, a version constant someone must remember to bump, is a hole
//! rather than a design. A new build is a new process, so it starts from nothing.
//!
//! Keyed by the digest alone, not by the request: the same rows always reconstruct to the same blocks,
//! and callers that narrow the answer afterwards (a trace scoped out of its session, a role filter, a
//! feed page) do that to the cached result.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::types::FeedResult;
use sideseat_core::constants::{RECONSTRUCTION_CACHE_IDLE_SECS, RECONSTRUCTION_CACHE_MAX_BYTES};
use sideseat_ports::types::MessageSpanRow;

/// Which reconstruction the cached answer came from.
///
/// Two callers ask the pipeline different questions of the same rows: `process_spans` builds a
/// chronological trace / session view, while `process_feed` builds the newest-first project feed.
/// Keying only on the rows made the two collide - whichever closure filled the cache first served the
/// other one's request, so a session query could receive feed ordering (one response reversed against
/// the others) or a feed could receive session ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reconstruction {
    /// One span's normalized payload, including replayed input context.
    Span,
    /// `process_spans` output: chronological, forward.
    Spans,
    /// `process_feed` output: responses newest-first, forward within each.
    Feed,
}

/// A memo over the pipeline output, keyed by the content of the rows and the reconstruction mode.
#[derive(Clone)]
pub struct ReconstructionCache {
    state: Arc<Mutex<CacheState>>,
}

type CacheKey = ([u8; 32], Reconstruction);

struct CacheEntry {
    value: Arc<FeedResult>,
    weight: u64,
    last_access: Instant,
    sequence: u64,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<CacheKey, CacheEntry>,
    in_flight: HashMap<CacheKey, Arc<Mutex<()>>>,
    total_weight: u64,
    next_sequence: u64,
}

impl CacheState {
    fn next_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.next_sequence
    }

    fn purge_idle(&mut self, now: Instant) {
        let idle = Duration::from_secs(RECONSTRUCTION_CACHE_IDLE_SECS);
        let expired: Vec<CacheKey> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                now.checked_duration_since(entry.last_access)
                    .is_some_and(|age| age >= idle)
                    .then_some(*key)
            })
            .collect();
        for key in expired {
            if let Some(entry) = self.entries.remove(&key) {
                self.total_weight = self.total_weight.saturating_sub(entry.weight);
            }
        }
    }

    fn get(&mut self, key: &CacheKey, now: Instant) -> Option<Arc<FeedResult>> {
        self.purge_idle(now);
        let sequence = self.next_sequence();
        let entry = self.entries.get_mut(key)?;
        entry.last_access = now;
        entry.sequence = sequence;
        Some(Arc::clone(&entry.value))
    }

    fn insert(&mut self, key: CacheKey, value: Arc<FeedResult>, now: Instant) {
        self.purge_idle(now);
        let weight = u64::from(weight_of(&value));
        if weight > RECONSTRUCTION_CACHE_MAX_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.total_weight = self.total_weight.saturating_sub(previous.weight);
        }
        while self.total_weight.saturating_add(weight) > RECONSTRUCTION_CACHE_MAX_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.total_weight = self.total_weight.saturating_sub(entry.weight);
            }
        }
        let sequence = self.next_sequence();
        self.total_weight = self.total_weight.saturating_add(weight);
        self.entries.insert(
            key,
            CacheEntry {
                value,
                weight,
                last_access: now,
                sequence,
            },
        );
    }
}

impl Default for ReconstructionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconstructionCache {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CacheState::default())),
        }
    }

    /// The reconstruction of these rows, computing it only if these exact rows have not been seen.
    ///
    /// `reconstruct` takes the rows by value because the pipeline consumes them; it runs only on a miss,
    /// and only in **one** caller when several arrive together.
    ///
    /// That last part is the difference from a check-then-compute pair. A cold instance is the normal
    /// state under ephemeral scaling - every new replica starts empty, and a deploy replaces them all -
    /// so the concurrent-miss case is not an edge: eight readers arriving at a fresh replica each found
    /// no entry, each reconstructed the same session, and each paid the full cost. On a thousand-turn
    /// session that is eight simultaneous 2.3-second reconstructions of one answer. `get_with` admits
    /// one and hands the rest its result, which turns the worst case from N× the work into 1×.
    pub fn get_or_reconstruct(
        &self,
        mode: Reconstruction,
        rows: Vec<MessageSpanRow>,
        reconstruct: impl FnOnce(Vec<MessageSpanRow>) -> FeedResult,
    ) -> Arc<FeedResult> {
        self.get_or_reconstruct_grouped(mode, rows, &HashMap::new(), reconstruct)
    }

    /// As [`Self::get_or_reconstruct`], with a caller-supplied trace → session grouping.
    ///
    /// The grouping is **in the key**, because the pipeline reads it and the rule here is that the key
    /// covers everything reconstruction reads. It is not redundant with the rows: the grouping comes from
    /// the store and therefore knows about spans the content filter removed, so a contentless root span
    /// whose session changed alters the answer while leaving every row identical. Keyed, that is a
    /// different entry; unkeyed, it would be a stale hit that no invalidation could reach.
    pub fn get_or_reconstruct_grouped(
        &self,
        mode: Reconstruction,
        rows: Vec<MessageSpanRow>,
        session_of_trace: &HashMap<String, String>,
        reconstruct: impl FnOnce(Vec<MessageSpanRow>) -> FeedResult,
    ) -> Arc<FeedResult> {
        let key = (digest_with(&rows, session_of_trace), mode);
        let cached = { self.state.lock().get(&key, Instant::now()) };
        if let Some(value) = cached {
            return value;
        }

        // A per-key lock, not one global reconstruction lock: equal inputs coalesce, while unrelated
        // sessions can still reconstruct in parallel. Rechecking after acquiring it also makes a panic
        // recoverable - the next waiter computes the value instead of waiting forever on an abandoned cell.
        let flight = {
            let mut state = self.state.lock();
            if let Some(value) = state.get(&key, Instant::now()) {
                return value;
            }
            Arc::clone(
                state
                    .in_flight
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _flight_guard = flight.lock();
        let cached = { self.state.lock().get(&key, Instant::now()) };
        if let Some(value) = cached {
            return value;
        }

        let value = Arc::new(reconstruct(rows));
        let mut state = self.state.lock();
        state.insert(key, Arc::clone(&value), Instant::now());
        state.in_flight.remove(&key);
        value
    }

    /// How many reconstructions are held. For tests and diagnostics.
    pub fn len(&self) -> u64 {
        let mut state = self.state.lock();
        state.purge_idle(Instant::now());
        state.entries.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

mod digest;
mod weight;

use digest::digest_with;
use weight::weight_of;

#[cfg(test)]
mod tests;
