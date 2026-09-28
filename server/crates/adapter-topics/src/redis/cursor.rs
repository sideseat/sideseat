use std::fmt;

use deadpool_redis::redis::{RedisResult, Value as RedisValue};
use sideseat_ports::queue::TopicError;

use super::redis_error;

/// What a recovery pass wants done to its rotating scan cursor, deferred until the claim it enables commits.
///
/// Held separately from the scan so the cursor advances only past entries an `XCLAIM` actually claimed - a
/// claim that errors leaves the cursor put, and the next pass re-scans rather than stepping over unclaimed
/// entries and leaving them stranded under sustained backlog growth.
pub(super) struct CursorAction {
    /// The Redis key holding this group's cursor.
    pub(super) key: String,
    /// What the scan read there, so the write can be conditional on nothing having changed since.
    ///
    /// A plain `SET` can let a delayed tail-page writer overwrite a peer that already wrapped to the front,
    /// skipping unclaimed entries. Compare-and-set turns that stale write into a no-op; one page is rescanned
    /// and nothing is skipped.
    pub(super) expected: Option<String>,
    /// The rotation to store, or `None` to clear it so the next pass starts a fresh one.
    pub(super) next: Option<Rotation>,
}

/// A Redis stream id, ordered as Redis orders it rather than as a string.
///
/// Ids are `<millis>-<sequence>`, so a lexicographic comparison is wrong the moment the millisecond
/// component changes width: `"9-0"` sorts after `"10-0"` as text and before it as a stream id. The trim
/// boundary is a minimum over groups, so getting this backwards would delete entries a group still
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct StreamId {
    pub(super) millis: u64,
    pub(super) sequence: u64,
}

impl StreamId {
    pub(super) fn parse(raw: &str) -> Option<Self> {
        let (millis, sequence) = raw.split_once('-')?;
        Some(Self {
            millis: millis.parse().ok()?,
            sequence: sequence.parse().ok()?,
        })
    }

    /// The next id after this one, which is the oldest entry still needed by a group that has
    /// acknowledged everything it was delivered.
    pub(super) fn next(self) -> Self {
        match self.sequence.checked_add(1) {
            Some(sequence) => Self {
                millis: self.millis,
                sequence,
            },
            None => Self {
                millis: self.millis.saturating_add(1),
                sequence: 0,
            },
        }
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.millis, self.sequence)
    }
}

/// A rotation over the pending list: where to resume, and the id that ended the list when it began.
///
/// The end is what makes the sweep bounded. Wrapping on a short page alone never wraps while the list grows
/// at the tail, and holding the cursor at an unclaimed entry lets a peer that repeatedly claims and abandons
/// it pin rotation forever. A fixed endpoint is immune to both: every entry present at the rotation's start is
/// examined exactly once, and entries that arrive during it belong to the next rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rotation {
    /// `None` means "from the front of the list".
    pub(super) position: Option<StreamId>,
    pub(super) end: StreamId,
}

impl Rotation {
    /// Parse the stored `<position>|<end>` form. `-` is the front.
    pub(super) fn parse(raw: &str) -> Option<Self> {
        let (position, end) = raw.split_once('|')?;
        Some(Self {
            position: if position == "-" {
                None
            } else {
                Some(StreamId::parse(position)?)
            },
            end: StreamId::parse(end)?,
        })
    }
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(position) => write!(f, "{}|{}", position, self.end),
            None => write!(f, "-|{}", self.end),
        }
    }
}

/// Where a rotation resumes after examining a page, or `None` when it is complete.
///
/// Complete means the next pass starts a *fresh* rotation over the list as it then is. Two conditions end one,
/// and both are needed:
///
/// * A **short page** - fewer entries than asked for - means the range held nothing more.
/// * Passing the rotation's **end** means every entry present when it began has been examined. This is the
///   condition that makes the bound real: without it, a list growing at the tail keeps returning full pages
///   and the sweep never wraps, so entries behind the cursor are never revisited.
///
/// The advance is unconditional otherwise - it does not depend on whether an entry was claimable - because a
/// cursor that waits for an entry can be pinned there by a peer that repeatedly claims and abandons it, which
/// starves everything after it.
pub(super) fn advance_rotation(
    rotation: Rotation,
    scanned: usize,
    count: usize,
    last_scanned: Option<StreamId>,
) -> Option<Rotation> {
    if scanned < count {
        return None;
    }
    let last = last_scanned?;
    // Compared *before* incrementing. `StreamId::next` saturates, so the maximum id
    // (`u64::MAX-u64::MAX`) increments to `u64::MAX-0`, which is *earlier* - and a rotation ending there
    // would then rescan its final entry forever, never revisiting the earlier failures behind it.
    // Comparing the id we actually examined has no such edge.
    if last >= rotation.end {
        return None;
    }
    Some(Rotation {
        position: Some(last.next()),
        end: rotation.end,
    })
}

/// The newest pending id for a group, which starts a rotation. `None` when nothing is pending.
///
/// From the `XPENDING key group` summary form, whose third element is the highest pending id.
pub(super) async fn pending_summary_max(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    group: &str,
) -> Result<Option<StreamId>, TopicError> {
    // The error is **propagated**, not swallowed. Reading a failure as "nothing is pending" made an ACL
    // denial, a `NOGROUP`, or any persistent command failure look like a healthy idle queue - so recovery
    // silently stopped running while every pass reported success. A refusal is what surfaces it.
    let summary: RedisValue = deadpool_redis::redis::cmd("XPENDING")
        .arg(key)
        .arg(group)
        .query_async(conn)
        .await
        .map_err(redis_error)?;
    let RedisValue::Array(parts) = summary else {
        return Ok(None);
    };
    // [count, min_id, max_id, consumers]; a zero count leaves the ids nil.
    if let Some(RedisValue::Int(0)) = parts.first() {
        return Ok(None);
    }
    Ok(parts
        .get(2)
        .and_then(redis_string)
        .as_deref()
        .and_then(StreamId::parse))
}

/// One page of a group's pending list, over the inclusive id range `[from, to]`.
///
/// **Not** `IDLE`-filtered. Examining every entry in the range and deciding eligibility locally keeps the
/// sweep's bound independent of idle times; `XCLAIM` still enforces `min_idle_ms` server-side.
pub(super) async fn scan_pending_page(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    group: &str,
    from: &str,
    to: &str,
    limit: usize,
) -> RedisResult<RedisValue> {
    deadpool_redis::redis::cmd("XPENDING")
        .arg(key)
        .arg(group)
        .arg(from)
        .arg(to)
        .arg(limit)
        .query_async(conn)
        .await
}

/// Read `XPENDING` range output into `(id, idle_ms, delivery count)`, in the order Redis returned them.
///
/// The idle time is carried rather than filtered on, so the caller can advance its cursor over everything
/// examined while claiming only what is eligible.
pub(super) fn parse_pending_range(pending: RedisValue) -> Vec<(String, u64, i64)> {
    let RedisValue::Array(entries) = pending else {
        return vec![];
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        // [id, consumer, idle_time, delivery_count]
        let RedisValue::Array(parts) = entry else {
            continue;
        };
        if parts.len() < 3 {
            continue;
        }
        let Some(id) = redis_string(&parts[0]) else {
            continue;
        };
        let RedisValue::Int(idle) = &parts[2] else {
            continue;
        };
        let deliveries = match parts.get(3) {
            Some(RedisValue::Int(n)) => *n,
            _ => 1,
        };
        out.push((id, (*idle).max(0) as u64, deliveries));
    }
    out
}

/// Read a named field out of one `XINFO GROUPS` entry, which is a flat key/value array.
pub(super) fn group_field(group: &RedisValue, field: &str) -> Option<String> {
    let RedisValue::Array(pairs) = group else {
        return None;
    };
    let mut iter = pairs.chunks_exact(2);
    iter.find_map(|pair| {
        let key = redis_string(&pair[0])?;
        (key == field).then(|| redis_string(&pair[1]))?
    })
}

pub(super) fn redis_string(value: &RedisValue) -> Option<String> {
    match value {
        RedisValue::BulkString(bytes) => String::from_utf8(bytes.clone()).ok(),
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::Int(i) => Some(i.to_string()),
        _ => None,
    }
}
