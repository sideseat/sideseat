//! What one process may spend: the footprint ceilings, the embedded engine's share of them, and the budgets
//! that hold the ingest path inside them.
//!
//! Re-exported from [`crate::constants`], so every constant keeps the one path it is read by.

// ---------------------------------------------------------------------------
// Footprint ceilings
//
// Enforced by `server/tests/footprint.rs` (the two in-process gates) and
// `scripts/perf/footprint-gates.sh` (the two that need a running server). They live
// here, in one place, because two of the four are read from a shell script and
// a ceiling with two spellings is a ceiling that drifts;
// `the_footprint_script_enforces_the_declared_ceilings` compares the script's
// text against these values.
//
// All four are stated against the *pinned* allocator
// (`runtime/allocation.rs`). An absolute megabyte figure is only comparable
// within one allocator, so a build without it reports the numbers and skips
// the resident gates rather than passing on a figure it cannot interpret.
// ---------------------------------------------------------------------------

/// Resident bytes after startup, quiesced.
pub const FOOTPRINT_IDLE_RSS_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Resident bytes under steady ingest, taken as the median over the sampling window.
pub const FOOTPRINT_INGEST_RSS_MAX_BYTES: u64 = 400 * 1024 * 1024;

/// Spans per second the steady-ingest ceiling above is stated at.
pub const FOOTPRINT_INGEST_SPANS_PER_SECOND: u64 = 5_000;

/// How much *live allocated* memory a long session read may leave behind once its answer and its memo are
/// dropped.
///
/// Live allocations rather than RSS, deliberately: both glibc and jemalloc retain freed pages, so an RSS
/// ceiling here fails correct code and fails it differently depending on timing. See
/// `runtime::allocation` for the whole argument.
pub const FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Turns in the session the ceiling above is stated against.
pub const FOOTPRINT_SESSION_READ_TURNS: usize = 10_000;

/// Live bytes a queued span may occupy, as a multiple of its decoded protobuf size.
///
/// The denominator is **decoded protobuf bytes**, not wire bytes: HTTP accepts gzip and the queue carries an
/// uncompressed encoding, so a wire-relative bound is unachievable for a valid repetitive request.
pub const FOOTPRINT_QUEUED_SPAN_MAX_RATIO: f64 = 3.0;

// ---------------------------------------------------------------------------
// In-process queue admission
//
// The bound is on *bytes*, not on entry count, and it refuses rather than
// trims. `PIPELINE_BATCH_MAX_SIZE` is a drain limit, so budgeting it bounds
// nothing that matters; what has to be bounded is what the queue holds.
// ---------------------------------------------------------------------------

/// Bytes one in-process stream topic may hold in unconsumed entries.
///
/// Sized against the 400 MB steady-ingest ceiling rather than picked: the queue is one contributor to that
/// figure, alongside the decode, the write path and DuckDB's own buffers, so it gets a fraction of it. An
/// exporter that outruns the consumer by more than this is told 503 with `Retry-After` and keeps its data,
/// which is what an OTLP exporter is built to do.
pub const STREAM_MAX_RETAINED_BYTES: u64 = 128 * 1024 * 1024;

/// Charged per entry on top of its payload, so one budget bounds the memory rather than only the payloads.
///
/// A queue of a hundred million one-byte entries costs far more than a hundred megabytes: each occupies a
/// `VecDeque` slot, a heap allocation for its payload, and a pending-map entry per consumer group. A pure
/// payload budget would admit that and the process would die inside a bound it was passing. Deliberately
/// generous, because the failure of underestimating it is an out-of-memory kill and the failure of
/// overestimating it is refusing slightly early.
pub const STREAM_ENTRY_OVERHEAD_BYTES: u64 = 256;

/// Charged per *pending record*, of which each consumer group holds one per delivered-and-unacknowledged entry.
///
/// Separate from the per-entry overhead because the two multiply. Counting only entries made the bound blind to
/// group state: ten thousand entries against a thousand abandoned groups is ten million pending records, and
/// `retained_bytes` reported the queue comfortably inside its budget while it held gigabytes. Smaller than an
/// entry's overhead because a pending record is an id, a consumer name and an instant rather than a payload.
pub const STREAM_PENDING_RECORD_OVERHEAD_BYTES: u64 = 128;

/// Consumer groups one in-process stream may have.
///
/// The other half of the pending-record bound, and the half a publish-time check cannot provide: group state
/// grows at *delivery*, which cannot refuse without stalling a consumer, so the only sound bound on it is a
/// bound on the number of groups. Charging pending records at publish stops a backlog from being admitted while
/// group state is already large; it does nothing about a single retained entry delivered to unboundedly many
/// groups.
///
/// Generous, because a legitimate deployment has one group per signal and a handful of consumers inside it: a
/// stream with dozens is a mistake in the calling code rather than a workload, and this is where that mistake
/// becomes a refused subscription with a message instead of a slow memory leak.
pub const STREAM_MAX_CONSUMER_GROUPS: usize = 32;

/// Consumer names one group remembers, for `StreamStats::consumers`.
///
/// The third place group state can grow without bound, and the one the group cap does not reach: a client that
/// reconnects with a fresh name adds an entry per reconnect, so one group with a million reconnects is a million
/// remembered names while the group count stays at one and every entry and pending record is reclaimed.
///
/// Bounded by eviction rather than by refusal, because this map is a *statistic* and not a registry - nothing
/// reads it to decide anything, and refusing a subscription because a stat is full would trade a real capability
/// for a number. The least recently active name goes, which is the one a "how many consumers are on this group"
/// answer cares about least.
pub const STREAM_MAX_REMEMBERED_CONSUMERS: usize = 64;

// ---------------------------------------------------------------------------
// The embedded engine's share of the footprint ceiling
//
// DuckDB's default `memory_limit` is 80% of physical RAM - on a 64 GB host that
// is 51 GB, which makes a 400 MB process ceiling a statement about everything
// except the component most likely to breach it. An embedded engine that
// ignores the budget makes the budget false.
// ---------------------------------------------------------------------------

/// Bytes DuckDB may use, as a share of [`FOOTPRINT_INGEST_RSS_MAX_BYTES`].
///
/// Half, not all of it: the rest of the process - the decode, the pipeline, the queue and the reconstruction
/// cache - has to fit inside the same ceiling, and those are the parts this repository's own benchmarks
/// measure.
///
/// **Most operators spill; not all of them do.** Hash aggregates, sorts and window functions are out-of-core,
/// and `temp_directory` is set beside this so the destination is a directory SideSeat owns. But DuckDB
/// documents complex aggregate states - `list()`, `first()` - as unable to offload, and the trace list builds
/// its tag column with `LIST_DISTINCT(FLATTEN(LIST(...)))`, so a trace with thousands of large tag arrays can
/// raise an out-of-memory error here where the default limit would have completed.
///
/// So the claim is not "a tight limit only costs latency": it can cost the query. The trade is taken because an
/// error names the limit while the default silently makes the process ceiling meaningless, and because which
/// way it should go is a measurement rather than an argument - `make bench-http` plus a large-corpus read. If
/// it proves too tight the fix is a configuration key, not a bigger constant, since the value depends on the
/// corpus.
pub const DUCKDB_MEMORY_LIMIT_BYTES: u64 = FOOTPRINT_INGEST_RSS_MAX_BYTES / 2;

/// Threads DuckDB may run one query on, at most; fewer when the host has fewer cores.
///
/// Each thread scanning a table holds its own decompressed segments of every column it reads, and the long text
/// columns are zstd segments of megabytes, so a query's memory grows with its threads while the limit above
/// does not. Measured on the trace corpus grown to a million spans in one project, the project message feed -
/// twenty rows - completed every time on four threads and failed half its runs on ten, against the same
/// 200 MB. Four is one thread per 50 MB of the limit.
pub const DUCKDB_MAX_THREADS: usize = (DUCKDB_MEMORY_LIMIT_BYTES / (50 * 1024 * 1024)) as usize;

/// Decoded protobuf bytes the CPU phase may have in flight at once.
///
/// Requests are grouped into sequential waves whose summed decoded size stays within this budget. This bounds
/// concurrent expansion independently of CPU count while retaining parallelism for small payloads. Expansion
/// can exceed input size because attachments are decoded and message payloads are parsed.
///
/// A request larger than the budget forms a wave of one because edge admission has already accepted it.
pub const PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES: u64 = 64 * 1024 * 1024;
