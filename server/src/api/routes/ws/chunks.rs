//! Reassembler for large `agent.event` payloads chunked over the WS frame
//! cap. See `server/protocol/ws-v1/chunking.md` for the design.
//!
//! Chunks arrive as `agent.event.chunk` frames keyed by `(request_id,
//! group_id)`. We collect them under a per-process `DashMap`, time-out
//! stale partials, and forward the assembled event onto the existing
//! `agent_request:{request_id}` per-request topic as a regular
//! `InvokeReply::Event` so the SSE handler stays oblivious.
//!
//! Concurrency model:
//! - The per-`request_id` byte counter is an `AtomicUsize` updated with
//!   `fetch_add` BEFORE the cap check; a refund (`fetch_sub`) is issued
//!   on every failure path so the counter stays a faithful upper bound
//!   on resident bytes.
//! - The per-group `Mutex<ChunkGroup>` only ever wraps in-shard work;
//!   `evict_stale` uses `try_lock` so a long-held group never stalls
//!   other shards' feeds.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use dashmap::DashMap;
use parking_lot::Mutex;

use crate::core::constants::{
    AGUI_CHUNK_MAX_PER_REQUEST_BYTES, AGUI_CHUNK_REASSEMBLY_TTL_SECS,
};

use super::protocol::AgentEventChunkPayload;

/// Hard ceiling on the number of slots in a chunk group. Anything above
/// this is rejected outright as malformed — even a 1 GiB event chunked
/// into 2.5 MiB slices stays well below 1024 chunks.
const MAX_CHUNK_GROUP_SLOTS: usize = 4096;

/// One in-flight chunk group. Owned by a single `Mutex` per group.
struct ChunkGroup {
    total: usize,
    received: usize,
    /// `slot[i]` decoded bytes for chunk index `i`; `None` until that
    /// index arrives.
    slots: Vec<Option<Vec<u8>>>,
    bytes_held: usize,
    created: Instant,
}

impl ChunkGroup {
    fn new(total: usize) -> Self {
        Self {
            total,
            received: 0,
            slots: (0..total).map(|_| None).collect(),
            bytes_held: 0,
            created: Instant::now(),
        }
    }

    fn ttl_expired(&self) -> bool {
        self.created.elapsed().as_secs() >= AGUI_CHUNK_REASSEMBLY_TTL_SECS
    }
}

type GroupKey = (String, String);
type GroupMap = DashMap<GroupKey, Arc<Mutex<ChunkGroup>>>;
type ByteCounters = DashMap<String, Arc<AtomicUsize>>;

#[derive(Clone, Default)]
pub struct Reassembler {
    groups: Arc<GroupMap>,
    /// Live count of bytes resident across all groups for one
    /// `request_id`, for the per-request memory cap. Keyed by
    /// `request_id`.
    bytes_per_request: Arc<ByteCounters>,
}

/// Outcome of feeding one chunk to the reassembler.
pub enum FeedOutcome {
    /// Group still incomplete; keep buffering.
    Pending,
    /// All chunks present and parsed. Forward this event onto the SSE
    /// pipe.
    Complete(serde_json::Value),
    /// Hard error — emit `agent.error` to the SSE and drop the group.
    Failed(ReassemblyError),
}

#[derive(Debug, thiserror::Error)]
pub enum ReassemblyError {
    #[error("base64 decode failed for chunk {idx}: {source}")]
    Base64 {
        idx: usize,
        #[source]
        source: base64::DecodeError,
    },
    #[error("chunk total invalid (must be 1..={max}, got {total})")]
    BadTotal { total: usize, max: usize },
    #[error("chunk idx {idx} out of bounds for total={total}")]
    IdxOutOfBounds { idx: usize, total: usize },
    #[error("chunk total mismatch: incoming={incoming}, group_total={group}")]
    TotalMismatch { incoming: usize, group: usize },
    #[error("chunk group exceeded per-request byte cap ({held} > {cap})")]
    PerRequestCapExceeded { held: usize, cap: usize },
    #[error("reassembled JSON parse failed: {0}")]
    JsonParse(String),
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop any partial state for a request. Idempotent.
    /// Call when the request finishes (`agent.complete`/`agent.error`)
    /// or its SSE closes.
    pub fn drop_request(&self, request_id: &str) {
        let to_remove: Vec<GroupKey> = self
            .groups
            .iter()
            .filter(|e| e.key().0 == request_id)
            .map(|e| e.key().clone())
            .collect();
        for k in to_remove {
            self.groups.remove(&k);
        }
        self.bytes_per_request.remove(request_id);
    }

    /// Feed one chunk. Drops expired groups for the same request as a
    /// side effect.
    pub fn feed(&self, payload: AgentEventChunkPayload) -> FeedOutcome {
        // 0. Cheap shape validation BEFORE we allocate or charge bytes.
        if payload.total == 0 || payload.total > MAX_CHUNK_GROUP_SLOTS {
            return FeedOutcome::Failed(ReassemblyError::BadTotal {
                total: payload.total,
                max: MAX_CHUNK_GROUP_SLOTS,
            });
        }
        if payload.idx >= payload.total {
            return FeedOutcome::Failed(ReassemblyError::IdxOutOfBounds {
                idx: payload.idx,
                total: payload.total,
            });
        }

        // 1. GC stale partials for this request, lock-free.
        self.evict_stale_for_request(&payload.request_id);

        // 2. Decode base64 first so we know the chunk's resident size.
        let decoded = match B64.decode(payload.data_b64.as_bytes()) {
            Ok(v) => v,
            Err(e) => {
                return FeedOutcome::Failed(ReassemblyError::Base64 {
                    idx: payload.idx,
                    source: e,
                });
            }
        };

        // 3. Reserve bytes against the per-request cap atomically. If the
        //    reservation pushes us over the limit, refund and bail —
        //    concurrent feeds can no longer race past the cap.
        let counter = self.counter_for(&payload.request_id);
        let added = decoded.len();
        let new_total = counter.fetch_add(added, Ordering::AcqRel) + added;
        if new_total > AGUI_CHUNK_MAX_PER_REQUEST_BYTES {
            counter.fetch_sub(added, Ordering::AcqRel);
            self.drop_request(&payload.request_id);
            return FeedOutcome::Failed(ReassemblyError::PerRequestCapExceeded {
                held: new_total,
                cap: AGUI_CHUNK_MAX_PER_REQUEST_BYTES,
            });
        }

        // 4. Funnel into the per-group buffer.
        let key = (payload.request_id.clone(), payload.group_id.clone());
        let group = self
            .groups
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(ChunkGroup::new(payload.total))))
            .clone();

        let mut g = group.lock();
        if g.total != payload.total {
            // Refund the bytes we just reserved; this group is being torn
            // down so they will never be persisted.
            counter.fetch_sub(added, Ordering::AcqRel);
            let group_total = g.total;
            drop(g);
            self.groups.remove(&key);
            return FeedOutcome::Failed(ReassemblyError::TotalMismatch {
                incoming: payload.total,
                group: group_total,
            });
        }

        // Slot already filled? Treat as idempotent retry: refund the new
        // bytes (they're a duplicate of bytes we already hold) and report
        // Pending so the caller waits for the rest.
        if g.slots[payload.idx].is_some() {
            counter.fetch_sub(added, Ordering::AcqRel);
            return FeedOutcome::Pending;
        }
        g.bytes_held += decoded.len();
        g.slots[payload.idx] = Some(decoded);
        g.received += 1;

        if g.received < g.total {
            return FeedOutcome::Pending;
        }

        // 5. All chunks arrived; concatenate and parse.
        let mut buf = Vec::with_capacity(g.bytes_held);
        for slot in g.slots.iter_mut() {
            if let Some(bytes) = slot.take() {
                buf.extend_from_slice(&bytes);
            }
        }
        let buf_len = buf.len();
        drop(g);
        self.groups.remove(&key);

        // The bytes are leaving the buffer (they now live in `buf`,
        // which the SSE side will own briefly). Refund the resident
        // counter so a subsequent chunked event for the same request
        // gets a fresh budget.
        counter.fetch_sub(buf_len, Ordering::AcqRel);

        match serde_json::from_slice::<serde_json::Value>(&buf) {
            Ok(v) => FeedOutcome::Complete(v),
            Err(e) => FeedOutcome::Failed(ReassemblyError::JsonParse(e.to_string())),
        }
    }

    fn counter_for(&self, request_id: &str) -> Arc<AtomicUsize> {
        // `entry().or_insert_with` is lock-free per-shard. We clone the
        // Arc out so callers don't hold the shard lock while doing work.
        self.bytes_per_request
            .entry(request_id.to_string())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .clone()
    }

    /// Evict every group for `request_id` whose TTL has expired.
    /// Uses `try_lock` so a slow consumer holding a group's mutex doesn't
    /// stall this scan — the worst-case is "we miss this round, retry on
    /// the next feed".
    fn evict_stale_for_request(&self, request_id: &str) {
        let stale_keys: Vec<GroupKey> = self
            .groups
            .iter()
            .filter_map(|e| {
                if e.key().0 != request_id {
                    return None;
                }
                let g = e.value().try_lock()?;
                if g.ttl_expired() {
                    Some(e.key().clone())
                } else {
                    None
                }
            })
            .collect();
        for k in stale_keys {
            // Refund anything held; counter stays accurate.
            if let Some((_, group)) = self.groups.remove(&k)
                && let Some(g) = group.try_lock_for(std::time::Duration::from_millis(50))
                && g.bytes_held > 0
                && let Some(c) = self.bytes_per_request.get(&k.0)
            {
                c.fetch_sub(g.bytes_held, Ordering::AcqRel);
            }
            tracing::warn!(
                request_id = %k.0,
                group_id = %k.1,
                "agui: dropped stale chunk group"
            );
        }
    }

    /// Test-only diagnostic hook: bytes currently reserved for a request.
    #[cfg(test)]
    fn reserved_bytes(&self, request_id: &str) -> usize {
        self.bytes_per_request
            .get(request_id)
            .map(|c| c.load(Ordering::Acquire))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(req: &str, group: &str, idx: usize, total: usize, raw: &[u8]) -> AgentEventChunkPayload {
        AgentEventChunkPayload {
            request_id: req.into(),
            group_id: group.into(),
            idx,
            total,
            data_b64: B64.encode(raw),
        }
    }

    #[test]
    fn happy_path_in_order_zeros_counter() {
        let r = Reassembler::new();
        let raw = b"{\"type\":\"TEST\",\"big\":\"abcdef\"}";
        let half = raw.len() / 2;
        let out0 = r.feed(payload("req-1", "g-1", 0, 2, &raw[..half]));
        assert!(matches!(out0, FeedOutcome::Pending));
        assert!(r.reserved_bytes("req-1") > 0);
        let out1 = r.feed(payload("req-1", "g-1", 1, 2, &raw[half..]));
        match out1 {
            FeedOutcome::Complete(v) => {
                assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("TEST"));
            }
            _ => panic!("expected Complete"),
        }
        // After Complete, reserved bytes for the request must drop back to 0.
        assert_eq!(r.reserved_bytes("req-1"), 0);
    }

    #[test]
    fn out_of_order_works() {
        let r = Reassembler::new();
        let raw = b"{\"v\":1,\"x\":\"hello\"}";
        let third = raw.len() / 3;
        let chunks = [
            (0_usize, &raw[..third]),
            (1, &raw[third..2 * third]),
            (2, &raw[2 * third..]),
        ];
        for &i in &[2_usize, 0, 1] {
            let (idx, slice) = chunks[i];
            let out = r.feed(payload("r", "g", idx, 3, slice));
            if i != 1 {
                assert!(matches!(out, FeedOutcome::Pending));
            } else {
                assert!(matches!(out, FeedOutcome::Complete(_)));
            }
        }
        assert_eq!(r.reserved_bytes("r"), 0);
    }

    #[test]
    fn duplicate_idx_within_group_is_pending_not_complete() {
        let r = Reassembler::new();
        // Group with total=2; send chunk 0 twice — second arrival is a no-op,
        // not a phantom completion.
        let p0_first = payload("r", "g", 0, 2, b"\"hello-");
        let p0_dup = payload("r", "g", 0, 2, b"\"hello-");
        let p1 = payload("r", "g", 1, 2, b"world\"");

        assert!(matches!(r.feed(p0_first), FeedOutcome::Pending));
        let bytes_after_first = r.reserved_bytes("r");
        assert!(bytes_after_first > 0);
        assert!(matches!(r.feed(p0_dup), FeedOutcome::Pending));
        // Duplicate must NOT permanently inflate the counter.
        assert_eq!(r.reserved_bytes("r"), bytes_after_first);
        assert!(matches!(r.feed(p1), FeedOutcome::Complete(_)));
        assert_eq!(r.reserved_bytes("r"), 0);
    }

    #[test]
    fn total_mismatch_drops_group_and_refunds() {
        let r = Reassembler::new();
        let _ = r.feed(payload("r", "g", 0, 2, b"a"));
        let bytes_before = r.reserved_bytes("r");
        let out = r.feed(payload("r", "g", 1, 3, b"b"));
        assert!(matches!(
            out,
            FeedOutcome::Failed(ReassemblyError::TotalMismatch { .. })
        ));
        // Refund: the second-arrival bytes are gone. The original chunk's
        // bytes are still resident (group existed before mismatch).
        assert_eq!(r.reserved_bytes("r"), bytes_before);
    }

    #[test]
    fn idx_out_of_bounds_rejected_pre_alloc() {
        let r = Reassembler::new();
        let out = r.feed(payload("r", "g", 5, 2, b"x"));
        assert!(matches!(
            out,
            FeedOutcome::Failed(ReassemblyError::IdxOutOfBounds { .. })
        ));
        // No bytes were ever charged — rejected before allocation.
        assert_eq!(r.reserved_bytes("r"), 0);
    }

    #[test]
    fn zero_total_rejected() {
        let r = Reassembler::new();
        let out = r.feed(payload("r", "g", 0, 0, b""));
        assert!(matches!(
            out,
            FeedOutcome::Failed(ReassemblyError::BadTotal { .. })
        ));
        assert_eq!(r.reserved_bytes("r"), 0);
    }

    #[test]
    fn oversized_total_rejected() {
        let r = Reassembler::new();
        let out = r.feed(payload("r", "g", 0, MAX_CHUNK_GROUP_SLOTS + 1, b"x"));
        assert!(matches!(
            out,
            FeedOutcome::Failed(ReassemblyError::BadTotal { .. })
        ));
    }

    #[test]
    fn bad_base64_rejected_after_shape_check() {
        let r = Reassembler::new();
        let mut p = payload("r", "g", 0, 1, b"x");
        p.data_b64 = "!!!not base64!!!".into();
        let out = r.feed(p);
        assert!(matches!(out, FeedOutcome::Failed(ReassemblyError::Base64 { .. })));
        // Bytes were never charged: base64 decode failed before reservation.
        assert_eq!(r.reserved_bytes("r"), 0);
    }

    #[test]
    fn drop_request_clears_partials_and_counter() {
        let r = Reassembler::new();
        let _ = r.feed(payload("r-x", "g-1", 0, 5, b"hello"));
        let _ = r.feed(payload("r-x", "g-2", 0, 5, b"world"));
        assert_eq!(r.groups.len(), 2);
        assert!(r.reserved_bytes("r-x") > 0);
        r.drop_request("r-x");
        assert_eq!(r.groups.len(), 0);
        assert_eq!(r.reserved_bytes("r-x"), 0);
        assert!(r.bytes_per_request.get("r-x").is_none());
    }

    #[test]
    fn per_request_cap_rejects_and_refunds() {
        // Use the real cap; we'll feed one giant chunk that pushes us over.
        let r = Reassembler::new();
        // Forge a chunk whose decoded length exceeds the cap. We can't
        // realistically allocate AGUI_CHUNK_MAX_PER_REQUEST_BYTES + 1 in a
        // unit test, so we exercise the path with a small payload but
        // pretend the counter is already near the ceiling.
        // Instead: prime the counter and then send a small chunk.
        r.bytes_per_request
            .entry("r".into())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .fetch_add(AGUI_CHUNK_MAX_PER_REQUEST_BYTES, Ordering::Release);
        let out = r.feed(payload("r", "g", 0, 1, b"will not fit"));
        assert!(matches!(
            out,
            FeedOutcome::Failed(ReassemblyError::PerRequestCapExceeded { .. })
        ));
        // The cap-exceeded path drops the request and resets the counter
        // for the next attempt.
        assert_eq!(r.reserved_bytes("r"), 0);
    }
}
