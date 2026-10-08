# Compact telemetry storage

What it would take for one machine with 2 GB of memory and 50 GB of disk to serve 10,000 users at the
Langfuse Hobby tier, and how close a storage format can get to the information-theoretic limit. This is a
design with measurements, not a description of the current backends.

## The target, in numbers

Langfuse Hobby allows 50,000 units a month per user, where a unit is a trace, an observation or a score,
and keeps 30 days of history. Ten thousand users at the cap are 500 million units a month. Retention is a
rolling 30 days while the quota resets monthly, so a user who spends a whole month's quota at the end of one
month and again at the start of the next has up to **1 billion** units live at once; 500 M is the steady
state, not the worst case.

| Budget                          | Value                                   |
| ------------------------------- | --------------------------------------- |
| Disk usable for data            | ~42 GB (WAL, compaction headroom, OS)   |
| Live units at the cap           | 500 M steady state, up to 1 B worst case |
| Bytes per unit at the cap       | **~84 B** steady, ~42 B worst case, content included |
| Average ingest                  | ~193 units/s                            |
| Burst ingest                    | unbounded by the quota: one user may spend a month's cap in an hour (14k/s); the ingest rate limit, 1,000 requests a minute per user on Hobby, is the real bound |
| Memory                          | 2 GB for process, buffers, page cache   |

A unit bounds neither bytes nor log records nor metric points: one observation may carry a 1 MB tool
output or an image. So no lossless format can promise a fixed number of units per disk without a **byte
quota** as well, and the plan below assumes one. Ingest throughput is not the hard part - a columnar
encoder in Rust handles hundreds of thousands of spans a second per core. **Bytes per unit is**, so the
measurements below are all about it.

## What the corpus says

`scripts/perf/storage-entropy.py` measures the committed capture corpus (`server/tests/fixtures/messages`,
native mode, one framework's captures as one tenant: an application run many times, which is what a hosted
user is), excluding the coding-agent CLIs, which are measured separately below. Numbers are bytes per span, averaged over every span, generations and tool spans alike.

| Layer                                                     | B/span |
| --------------------------------------------------------- | -----: |
| Raw OTLP protobuf as received                             |  5,592 |
| Text leaves, all occurrences                              |  1,343 |
| Text leaves, deduplicated within the tenant               |    726 |
| Text leaves, deduplicated, zstd-19                        |    215 |
| JSON skeletons, deduplicated, zstd-19                     |     16 |
| Content references                                        |     28 |
| Scalar attribute values                                   |     17 |
| span id (random, incompressible)                          |      8 |
| trace id (16 B once per trace)                            |    2.6 |
| start and duration, delta-coded microseconds              |    6.2 |
| Span shape id and shape dictionaries                      |     11 |
| **Total without media**                                   | **303** |
| Media (images, PDFs), decoded binary                      |  2,375 |

That ratio compares unlike quantities: the raw figure includes the base64 media (79% of the trace corpus' bytes)
while the stored figure excludes it. Against raw bytes without media the same layers are about 4x; with media stored
once per project at its floor, traces cannot be stored losslessly at better than ~4.8x (see
[Measured in the real backends](#measured-in-the-real-backends)). Per unit (spans plus traces) the estimate is about
**250-260 B** for framework applications.

The table is the framework applications (every native suite except the coding-agent CLIs), whose
generations carry whole conversations. The workloads differ by an order of magnitude, so the script reports
them separately (`--only`, `--exclude`):

| Workload (native captures)                 | Raw OTLP | Stored | Per unit (spans + traces) |
| ------------------------------------------ | -------: | -----: | ------------------------: |
| Framework applications, spans              | 5,592 B  | 303 B  | ~260 B                    |
| Codex CLI, spans (many small spans)        |   376 B  |  18 B  | ~18 B                     |
| GenAI log events with content (Bedrock, SK) | 1,776 B | 174 B  | per record                |
| Codex CLI log events                       |   444 B  |   7 B  | per record                |
| Metric points                              |      -   | ~1.4 B | estimate; no metric corpus yet |

Media is extra in every row: 2,375 B per framework span on average, all of it from the `files` scenarios.
The metric figure is Gorilla's published 1.37 B per point for regular series, not a measurement here; a
metrics capture should replace it. Claude Code's log exports are JSON rather than protobuf in the corpus and
are not yet measured.

Two findings shape the design:

- **Media dominates when present and does not compress.** Base64 attachments are 90% of the bytes of a
  `files` scenario. They belong in a content-addressed blob store with its own quota, decoded to binary
  (25% smaller than base64), and never in the column store. Langfuse also stores media separately.
- **General-purpose compressors have converged.** On the deduplicated text, zstd-19, zstd-22 with long
  range, xz -9e and brotli-11 land between 1.27 and 1.36 bits per byte. The remaining gains are
  structural, not a better entropy coder: deduplicating the conversation every call re-sends, separating
  JSON structure from text, and dictionaries for small tenants.

A shared zstd dictionary trained on *other* frameworks' captures cut small tenants' content by 31%
(1,414 to 974 B/span on held-out frameworks), which matters because most Hobby tenants are small and have
no history of their own to compress against. A dictionary must be trained on public or synthetic data,
never on another tenant's telemetry.

## Measured in the real backends

`scripts/perf/storage-footprint.py` (`make footprint-storage`, and `-distributed`) loads the whole corpus into a real
server, one project per producer, and reads the backends' own accounting. Its gate is **stored bytes per item
excluding media** - media is stored once per project at its floor (the unique decoded bytes) and reported beside
it - against the ruled ceilings of 480 B per span, 300 B per log record and 150 B per metric point, plus a regression
ceiling at the last measured figure.

**How it is measured.** The database is built from scratch by the run, then settled - `FORCE CHECKPOINT` and
`VACUUM` - so nothing is left in the write-ahead log, and the bytes charged per item are exactly DuckDB's
`used_blocks`: the per-column attribution from the segment map plus the residue (indexes, headers, the unused tail
of each last block). Free blocks are reported beside the figure and never charged, because they are capacity the
file keeps for reuse rather than bytes the corpus stores, and counting them made the same corpus measure
differently depending on the churn of the run. ClickHouse is read the same way through its own accounting, per
part.

Embedded backend, on the pinned corpus with the derived metric load - what the gate measures - 2026-10-07:

| Signal  |    Items | Raw OTLP | Stored, excluding media | Per item | Measured alone |
| ------- | -------: | -------: | ----------------------: | -------: | -------------: |
| traces  |    13,384 | 37.3 MB | 22.2 MB                 |  1,658 B |        1,814 B |
| logs    |     1,392 | 1.42 MB | 5.67 MB                 |  4,070 B |        5,085 B |
| metrics | 1,001,300 | 378 MB  | 163 MB                  |    163 B |          159 B |

The last column is `--signal`, which loads one signal's corpus alone. The two differ because DuckDB's residue -
its indexes and metadata - cannot be attributed to a table, so it is spread over the signals by rows: a signal
measured alone carries all of it, and a signal sharing the file with a million metric points carries less. The
gate measures the pinned mix, which is reproducible because both the fixture manifest and the load's parameters
are pinned; `--signal` is the figure to read when asking what one signal costs. Before the raw store landed,
traces cost 8,240 B/span on the same mix.

The measurement itself is exact - the same database measures identically every time - but the figure still moves
by about half a percent between runs, because which spans end up in one write batch depends on timing, and the
block residue follows. The regression ceilings sit about 2 % above the measured figures for that reason. The run
takes about ten minutes: it generates the million-point load, loads 3,582 exports through a real server, and
honours the server's back-pressure (a 503 is retried, as a collector would).

Where a trace span's bytes are, measured alone: DuckDB's indexes and block residue 1,106 B/span, search terms
285, `otel_spans` 274, the raw record 117.5 and its trace index 19.6, the file registry 12.3, media 296.5
reported separately. Two rounds of removing duplication got here from 8,240. The `raw_span` JSON column was the
largest column *and* the largest content-body object, so each span's OTLP JSON was stored twice while the raw
record already held the whole export - 3,628 B/span. The content-body store was then redundant in full: with it
gone the inline columns did not grow by a byte (`otel_spans` stayed at 274.2 B/span), which is the measurement
that shows its 2,556 B/span was pure duplication of what the columns and the record already held.

What is left, in order of size, is the DuckDB residue - almost all of it index pages, and 588 B/span of that is
the ART index on `span_terms(term)`, which the search does not drive from (its lookups are keyed by span
identity and measure no faster with the index than without) - then the term postings themselves, one row per
term per span.

**An index exists only where a read provably uses it.** DuckDB reads through an ART index only for a scan whose
one filter is an equality or IN list on the column of a single-column index; it never scans through a compound
index, and a second predicate in the same scan reads the whole table. The compound indexes on spans, logs,
metrics and raw records were therefore never read - on a million spans every per-identity read scanned every row
with them and without - while they held a quarter of the file and every insert maintained them. They are gone;
what remains is one single-column index per key a read looks rows up by (`otel_spans.span_id` and `trace_id`,
`otel_raw.raw_id`, `otel_raw_traces.raw_id`), plus the unique index that makes a second row for a log identity an
error. Every read keyed on one of them is written so the key is its scan's only filter
(`sideseat_query_sql::keyed`), binds at most 512 keys, and stays on the index however many rows its keys find
(DuckDB's `index_scan_max_count`, raised on the connection), so a long trace or a span id a client reuses costs the
rows it asks for rather than the table; deletes go by the row ids a keyed read found. Log records and datapoints
carry their own instant in their identity, so their reads take no index - one on `log_digest` measured 68 B per
record and one on `datapoint_id` 85 B per point - but a list of the records' exact instants, which the row groups'
zone maps answer one value at a time: a range from the earliest to the latest instant reads every row group
between them, 983,040 rows against 245,760 for two instants a day apart on a million rows. A span's search terms
are its winner's - the revision a read answers with, latest by `ingested_at` - and carry that instant, so the
winner's instant, which the span rows already hold, is the pointer to them: a correction that wins deletes exactly
the previous winner's terms, a revision older than the stored winner writes none, and a span with no stored
revision deletes nothing. A store at version 2 whose tables, columns, column order or indexes differ from the ones
this build creates is refused at startup with the reset instruction, as a store at another version is.
`keyed_scan_tests` holds each lookup to a small multiple of its keys. Measured by the storage gate on the corpus:
1,398 to 805 B per span, 3,499 to 2,586 B per log record and 154 to 138 B per metric point; on the corpus
replicated to a million spans, the ingest-time lookups went from every row of the table to the rows they name
(`read_paths` in the DuckDB adapter).

**A winner is a condition on its row, not a window over the table.** A span identity keeps every revision it
was delivered as, and a read answers with the latest. DuckDB computed that with `ROW_NUMBER() OVER (PARTITION BY
identity ...)` over the whole of `otel_spans`, holding whole rows to do it, before any condition of the read could
apply; on the corpus replicated to a million spans six reads - list traces, get trace, list spans, the span feed,
the project message feed and span search - ran out of the 200 MB memory limit, and list sessions and project stats
took 2.2 and 2.6 seconds. Each row now carries `superseded_at`, the `ingested_at` of the revision that follows it,
`NULL` while it is the winner, set by the write that stores a revision on its new rows and on the stored row they
follow (`sideseat_query_sql::winners`). It answers "the winner as of a watermark" as well, which a traversal pins,
and it costs no measurable bytes: the column is `NULL` on all but the corrected rows. Measured at a million spans
(`read_paths` in the DuckDB adapter): list traces 228 ms, get trace 57 ms, list spans 71 ms, the feed 16 ms, the
project message feed 100 ms, list sessions 137 ms and project stats 346 ms, each within the memory limit; span
search still exceeded it, on its term lookups rather than its winners.

**Metrics are measured on a derived load, and the gate uses it.** 480 captured points cannot measure a store whose block is 256 KB:
most of the figure is one partly-filled block per column. `scripts/perf/metrics-load.py` derives a deterministic
load of about a million points from the captured *shapes* - every series keeps its resource, scope, instrument
kind, unit, temporality and attribute set, from a fleet of replicas distinguished by `service.instance.id`,
cumulative every interval, with values that only grow - and `--metrics-load` measures that instead. The parameters
are committed (`metrics-load.json`), not the data, so the same parameters and the same corpus give the same exports
byte for byte. On that load a point costs 162 B, against 581 B on the captured corpus, where almost all of the figure was one
partly-filled block per column; the captured corpus stays as the correctness fixture.

**Arithmetic for media.** Media cannot be compressed losslessly: six generated PNGs, a JPEG, a PDF and encrypted
reasoning signatures, 15.3 MB unique per project out of 74.6 MB. However well everything else is stored, traces
including media are bounded at about 4.8x and logs at about 3.7x, which is why the gate counts media separately.

### The raw record

Raw telemetry is the single authority; everything else is a cache that a re-derivation rebuilds from it. The raw form
is the **SSR1 record** (`sideseat_domain::raw_payload`): the received body byte for byte, with every base64 run of 256
or more characters that re-encodes exactly cut out and stored once per project under the file store's own address -
the BLAKE3 of the base64 text - and `(gap, length, hash)` recorded in its place, so an image a span's extraction
already stored is the same object. Decoding splices the text back, so non-canonical protobuf, OTLP/JSON
and runs that touch framing bytes all round-trip; `server/tests/raw_round_trip.rs` proves it for all 1,549 exports
(76.9 MB received, 18.5 MB of records, 63 media objects of 11.7 MB). Compressed with zstd in 256 KB segments - what a
DuckDB column in storage format v1.5 does - the trace records are about 146 B per span.

**Both transports store the producer's bytes.** Over HTTP that is the request body after content encoding. Over
gRPC it took a codec: tonic's generated service hands over a decoded message, so the record used to be prost's
re-encoding of it - the same bytes for the canonical encoders exporters use, and not the same for a non-minimal
varint, an unusual field order, or any field prost does not know, which is exactly the telemetry a parsing defect
would later need re-reading. `RawCodec` (`api::routes::otlp_collector::grpc_raw`) decodes into the message *and*
the frame it came from, and the export services store the frame. It is the generated dispatch with the codec
swapped, so compression, message-size limits and routing behave as before.

### The record's life

A record is written before the rows derived from it and lives as long as a row names it. Three tables carry that:
`otel_raw` holds the versions of each record, `otel_raw_traces` says which records hold spans of which trace, and
`otel_raw_pending` is the reconciliation queue. Every version records its **origin** - `received` (the body as it
arrived), `fenced` (the received export re-encoded without spans the ingest's deletion fences refused) or `deleted`
(rewritten after a deletion took some of what it held) - so "byte for byte" is a claim only about the first, and a
re-derivation can tell the two apart.

Keeping the record and its rows agreeing is a protocol, not an invariant one statement can hold, because nothing is
atomic across the record, the rows and the tombstones:

- every deletion and expiry of span rows enqueues the records holding those traces, in the same transaction as the
  delete, and finds them through the trace index rather than through the rows - a record whose rows already expired,
  or whose only naming row was a superseded revision a ClickHouse merge removed, is named by nothing and must still
  be reached;
- the reconciler then rewrites each queued record without the spans the deletion fences refuse, or deletes it when no
  row names it, and looks again afterwards, re-enqueueing anything still unsettled, so whichever reconciler acts last
  sees its own effect;
- an ingest whose rows the latest record does not hold appends the union of the two, at least two versions above what
  it read, so a concurrent reconciler's rewrite of an older version cannot win over it whatever the writers' clocks
  say, and supersedes a latest version that no longer decodes as if it held nothing;
- an ingest whose span write fails enqueues the records it stored before the rows: no row may name them, and one
  fenced before a deletion that landed meanwhile holds content the deletion removed, which only the reconciler
  takes out;
- a legal hold stops both: the records and the index carry `hold_until` exactly as the span rows do, retention and
  deletion skip a held row, and the reconciler leaves a held record queued.

Media follows the same ownership as an extracted file (`trace_files`, `pending_writers`, `durable`), owned by every
trace whose span text carries it; survivor reconciliation therefore keeps what a surviving record references, not
only what its derived columns do. Media the file store refused - a project over its quota - stays inline, so a record
is decodable whatever happened to the files.

`server/specs/RawRecordOwnership.tla` models the protocol and `RawRecordReconcilers.cfg` the races between two
reconcilers; four invariants are checked over every interleaving - every row's content is in the latest record, a
settled record holds nothing deleted, a record no row names is collected, and a hold loses nothing.

### Deferred: content-defined chunking of re-sent history

Every model call re-sends the conversation so far. Storing repeated text once per project by content-defined
chunking (FastCDC over the media-stripped records, a per-project chunk store, references in the record) was measured
on the trace corpus: 145.5 B/span without it, 126.1 / 123.8 / 123.6 B/span with 1, 2 and 4 KB average chunks. zstd
within a segment already removes most of the repetition, so chunking saves about 22 B/span (15%) - at the cost of a
store whose objects are shared across exports and therefore need reference counting under retention, deletion, legal
hold and restore. It is kept as a measured lever, to be used only if the per-span budget needs it.

## How far from the limit

| Component            | Now (measured) | Floor and why                                                   |
| -------------------- | -------------: | --------------------------------------------------------------- |
| Identifiers          |      10.6 B/span | ~10.4: client-chosen random 64/128-bit ids cannot be compressed |
| Timing               |       6.1 B/span | ~4: entropy of microsecond offsets and durations               |
| Structure and scalars |       44 B/span | ~15: local leaf ids instead of 5-byte references; shapes       |
| Novel text           |      215 B/span | ~140 at ~0.9 bits/byte with context mixing on cold data; the   |
|                      |                | Shannon estimate for English is 0.6-1.3 bits per character      |

The lossless floor for this corpus is therefore about **170-200 B/span, ~150-170 B/unit**. The current
design reaches ~250 B/unit, and a cold tier with context-mixing compression plus local references would
reach ~200.

**The honest conclusion:** lossless storage of full content cannot fit 500 M units in 42 GB, let alone the
1 B worst case. The design as
measured (~250 B/unit) holds ~168 M live units, 34% of the aggregate cap; with the cold-tier coder and local
references (~200 B/unit) it holds ~210 M live units, which serves 10,000 Hobby users at an average of **42% of their cap**
with full fidelity and 30-day retention, before the operational indexes below (another 20-40 B/unit) and
before the measurement corrections in [Measurement limits](#measurement-limits). An adversarial review put
the realistic floor at 130-190 B/unit before indexes. The bound depends on the workload: a tenant whose units are coding-agent CLI spans (~18 B/unit) fits 100%
of its cap with room to spare, while a tenant of framework applications re-sending long conversations does
not. Free tiers are also heavy-tailed - most users are far below the cap - so
that is very likely enough in practice, but it is a statement about the user distribution, not a
guarantee. Serving every user at 100% of the cap needs one of:

1. A cold tier on object storage (S3-class storage costs about a tenth of block storage per GB), with
   the local 50 GB holding recent days and indexes.
2. A per-tenant byte quota alongside the unit quota, which is what the physics actually limits.
3. A lossy policy for oversized payloads (for example, tool outputs beyond 64 KB kept as a prefix plus a
   digest). This conflicts with the invariant that raw telemetry is preserved, so it would have to be an
   explicit, visible per-tenant setting, never a default.

## The format

A single-node log-structured store, partitioned by tenant inside shared files.

**Durability and ingest.** One shared write-ahead log with group commit; an ingest request is acknowledged
after its WAL frame is fsynced, which keeps the durability-boundary invariant. A bounded shared memtable
(128 MB) is flushed as segments sorted by `(tenant, trace start, trace id, span start)`. Per-tenant files
would be 10,000 open handles and millions of small segments, so a segment holds many tenants, each as one
contiguous run with a directory entry.

**Span columns**, per tenant run:

- span id raw (8 B); trace id once per trace; parent as an index into the trace's span list;
- start as a delta from the trace start and duration, both microsecond varints;
- one **shape id** per span: name, kind, status, scope and the attribute key set, from a per-tenant shape
  dictionary. An application emits a few dozen shapes, so keys cost almost nothing;
- typed scalar columns: integers zigzag-delta, floats Gorilla-XOR, low-cardinality strings dictionary-coded;
- large values in the content store.

**Content store**, per tenant. JSON nested in strings is parsed recursively into a skeleton and its string
leaves. Leaves of 24 bytes or more are content-addressed (BLAKE3, 64-bit key, collision-checked) and stored
once; skeletons are deduplicated the same way; base64 media is extracted into the blob store. A value is
reconstructed byte-exactly: if re-serialising the parsed form does not reproduce the original string, the
value is stored verbatim instead, so the store stays lossless and raw telemetry is preserved. Content
blocks are ~64 KB for random access, compressed with the tenant's dictionary or, for small tenants, a public
dictionary.

**Tiers.** Hot segments are zstd-3, written in minutes. A daily compaction rewrites each shard's day at
zstd-19 with the tenant dictionary and a larger window; an optional cold pass recompresses older days with
a context-mixing coder. Each tenant's day is an independently reclaimable extent inside the shared segment, tracked by a
copy-on-write manifest, so retention, deletion and legal hold act on one tenant without rewriting the
others; the space is reused, and a segment file is unlinked once all its extents are free. Compaction
rewrites one shard's day at a time (a few hundred MB), never a whole day of every tenant, so its temporary
space stays inside the headroom.

**Logs.** Structured GenAI log events go through the same skeleton and leaf store. Free-text log bodies use
template extraction in the style of CLP: the template is stored once, the variables in typed columns,
searchable without decompressing.

**Metrics.** Gorilla-style series: delta-of-delta timestamps and XOR-coded values, about 1-2 bytes per
sample, with a series dictionary per tenant.

**Indexes live on disk, not in memory.** Per segment: the tenant directory, time zone maps, and binary fuse
filters (~9 bits per key) for trace, session and user ids, memory-mapped. A trace-to-offset index for ~70 M
live traces is ~840 MB on disk; per-tenant hourly rollups (counts, tokens, cost, a DDSketch of latency) are
~720 MB at 100 B per tenant-hour. Both are paged in on demand, never resident, and both are counted in the
20-40 B/unit index budget. Filtered, sorted pagination - Langfuse's trace list - needs sorted per-tenant
secondary columns (timestamp, name, user, session, cost, latency) per day, not only rollups.

| Memory                         | Budget  |
| ------------------------------ | ------- |
| Kernel, OS and page tables     | 250 MB  |
| Process, runtime, connections  | 150 MB  |
| Memtable, double-buffered      | 192 MB  |
| Compaction and query buffers   | 192 MB  |
| Dictionary and directory cache (LRU; dictionaries are not all resident) | 128 MB |
| Page cache for hot segments and indexes | ~1 GB |

Ten thousand 64-112 KB dictionaries would be 0.6-1.1 GB if all were resident, so dictionaries are loaded on
demand into a bounded cache; most Hobby tenants use the shared public dictionary.

At the cap a day is about 4 GB compressed, so the page cache holds roughly the last six hours; older
data is read from disk, which an SSD serves in single-digit milliseconds per run.

**Search.** Langfuse-style filters (time, name, user, session, tags, model, status, cost and latency
ranges) are served from the span columns and rollups. Full-text search over content uses a per-tenant,
per-day token posting list over the deduplicated leaves (each leaf is indexed once, however often it was
re-sent). A useful text index costs 10-25% of the compressed text for token postings and 20-50% for exact
substring search; a 2-byte-per-leaf filter cannot work (50 trigrams at a 1% false-positive rate need
~60 B per leaf). Full-text search is therefore a per-tenant option with its own byte cost, not free.

**Query path.** Listing a tenant's traces reads the directory, then only that tenant's runs in the day
segments the time range covers, decoding only the requested columns. A trace detail decodes one run of a
few kilobytes. Daily recompaction at the cap is about 10 GB of deduplicated text, which zstd-19 handles in about 40
minutes of one core in isolation; under concurrent ingest and queries it must be rate-limited, and that
interaction is unmeasured.

## Measurement limits

`storage-entropy.py` estimates, it does not implement the format, and it errs in both directions:

- **Omitted**, so it understates: resource and scope attributes, schema URLs, parent ids, links, event
  timestamps, flags, trace state, status messages, dropped counts, frame headers, hashes, reference counts,
  manifests and the WAL. Scalars are written as untyped text, and parsed JSON is not charged for the
  byte-exact fallback.
- **Flattering**: span metadata is compressed across tenants and trace ids deduplicated globally, though
  identity is per project; text is compressed as one stream per tenant rather than in 64 KB random-access
  blocks; the fixtures re-use canned prompts and responses, which helps deduplication more than real
  traffic would.
- **The media regex** neither validates base64 nor charges blob references.

The corrected numbers belong to the implementation, measured by `make footprint` on the real engine; this
script only shows which layers matter.

## How it would enter SideSeat

It is an adapter behind the existing storage ports, next to DuckDB and ClickHouse, and must give the same
public answers - the parity suites decide that. The current embedded schema stores `messages` and the
whole `raw_span` as JSON beside each other and hex identifiers as strings, so the content store pays off
even before a new engine exists. In order:

1. Make `scripts/perf/storage-entropy.py` and `make footprint` report bytes per span for the real
   backends, so every step is measured.
2. Content store and media extraction inside the current embedded backend: leaves and skeletons by
   reference, media as blobs, identifiers as binary.
3. The segment engine as a new embedded adapter, behind the same ports, with parity coverage.
4. Daily compaction with tenant and public dictionaries; then the optional cold tier.
5. Logs with template extraction, metrics with Gorilla coding.
6. Structural gains the review identified, each measured before it is kept: trace-local span ordinals and
   resource/scope dictionaries with bit-packing (~20-35 B/unit); a persistent per-session conversation DAG
   or FastCDC chunking of re-sent histories (could remove 20-60% of the text); context-mixing on the cold
   tier (25-35% of the remaining text, at 10-100x the CPU).
7. A latency benchmark against Langfuse's own queries - trace list with filters, trace detail, session
   view, dashboard - under concurrent ingest, which is the only evidence for "equal or faster".

## Verdict

| Question                                              | Answer |
| ----------------------------------------------------- | ------ |
| 10,000 Hobby users at their full 50k-unit cap, lossless, on 50 GB | **No.** It needs ~84 B/unit steady (42 B worst case); the design measures ~250 and can plausibly reach 130-190, plus 20-40 for indexes. |
| The same users at a realistic Hobby distribution      | **Likely**, at ~35-45% of the aggregate cap, but it rests on the distribution; it must be measured on real traffic. |
| 100% of cap guaranteed                                | Needs a per-tenant byte quota, plus either an object-storage cold tier (local disk for recent days and indexes) or an explicit, visible lossy policy for oversized payloads. |
| 2 GB of memory                                        | Fits with on-disk indexes, an LRU dictionary cache and a bounded memtable; tight, and to be proven under load. |
| Equal or faster than Langfuse                         | Plausible for trace detail and filtered lists (one tenant run per day, columns only); unproven until step 7. |
| Coding-agent CLI tenants (many small spans)           | Fit with room to spare at ~18 B/unit. |

## Risks

- **Byte-exactness.** Re-serialisation must round-trip or fall back to verbatim; a property test over the
  corpus guards it.
- **Deduplication across tenants** would save more and is ruled out: it leaks whether another tenant sent
  a text, and complicates deletion.
- **Hash collisions** at 64 bits are checked against stored bytes, never assumed away.
- **Deletion and legal hold** need reference counts on leaves and blobs, the same ownership model the
  current file store uses.
- **Small corpus.** The fixtures are short scenarios. Real tenants re-send longer histories and repeat
  their system prompts thousands of times, which helps deduplication; they also produce longer model
  outputs, which is novel text and does not. The measurement should be repeated on a real tenant's export.

## References

Gorilla (Pelkonen et al., VLDB 2015); BtrBlocks (Kuschewski et al., SIGMOD 2023); FSST (Boncz et al., VLDB
2020); CLP (Rodrigues et al., OSDI 2021); FastCDC (Xia et al., USENIX ATC 2016); binary fuse filters (Graf
and Lemire, 2022); DDSketch (Masson et al., VLDB 2019); Grafana Tempo's Parquet block format; Loki's
label-only index; VictoriaMetrics' storage format; zstd dictionary training; Shannon, "Prediction and
Entropy of Printed English" (1951); LLM-based compressors (LLMZip, ts_zip) as the practical lower bound.
