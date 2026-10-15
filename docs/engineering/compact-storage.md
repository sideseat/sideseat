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

### The deterministic gate

A live run cannot be repeated byte for byte: every export is stored at its receipt, the instant it arrived, and a
server's timed checkpoint lands wherever its timer does, so two runs of the same binary on the same corpus have
differed by three blocks. `make storage-gate` replays the pinned corpus in process instead
(`server/tests/storage_gate.rs`), through the path every export takes - `export_signal`, staging, admission, the
trace pipeline and settlement - with each export's receipt fixed by its place in the corpus, every other clock at
one instant, project ids derived from their tenants, and no timed checkpoint. Two replays leave one segment layout
and the same figures, on a RAM disk, on an ordinary disk and from a release build alike
(`make storage-gate-equivalence`).

DuckDB runs on one thread there and in the measurement, where the server sizes its threads from the host. The
requirement that what is stored and answered not depend on the core count is read as each table's byte count and
every answer, not where the engine places a segment in its own file. Measured on the pinned corpus while span terms
were a table of their own (Span search terms): one pass leaves the same 71 blocks and the same per-table bytes at 1,
2, 4 and 8 threads, with span_terms' segments in other blocks above one thread; three passes do not - four threads
leave 185 blocks where one leaves 180, every extra one span_terms', whose 76 segments the checkpoint packs into 45
blocks on four threads and 40 on one. The answers - every read `make bench-reads` makes and each project's trace,
session and span message views, 1,069 reads - are the same bytes at one thread and four. `make
storage-gate-equivalence` replays on four threads and requires the same figures and the same answers as on one;
until the checkpoint runs on one thread whatever the host, the figures do not match and it fails, by design. Its
answers are read by servers on the host's own thread count; reading them at one and four too waits for the engine's
thread count to be a server setting.

The corpus goes through three times, the later passes as new telemetry (ids rewritten, instants a day on), so a
256 KB block is a smaller share of each table. Each table is measured as its segments' bytes and its indexes'. An
index is sized by dropping it on a copy, and the measurement refuses if a drop moved any table's segments, as the
vacuum a table's last index holds back would. DuckDB says where a segment starts but not where it ends, so a
segment runs to the next in its block and the last to the block's end: a block's unused tail is charged to the
segment before it, and where two tables share a block a change to one can move that tail into the other's figure.
What lies in no segment - headers, metadata, free-list blocks - is a residual, held to the baseline but charged to
no signal, and media is reported apart. The figures are held to `scripts/perf/storage-gate-baseline.json` exactly:
any change, a tail moving between tables included, fails until the commit that makes it writes the baseline
(`make storage-gate ARGS=--update`) and says why. `make check`, which the pre-push hook runs, runs the gate.

These are not the live gate's figures and do not compare with them: the live gate spreads the residue over the
signals by rows and gates metrics on the derived million-point load, so its 784 and 815 B/span and 139 B/point were
another measurement. Measured on all 1,758 pinned exports, three passes, excluding media and the residual, on
2026-10-13; the marginal figures below are the ones the targets are judged on:

| Signal | Stored per item | Target | Largest parts |
| --- | ---: | ---: | --- |
| Traces | 558.2 B/span | 480 | `otel_spans` 300.7, its search terms included, and its three ART indexes 89.6, `otel_raw` 102.4 and its index 19.2, `otel_raw_traces` 38.4 |
| Logs | 359.9 B/record | 300 | `otel_logs` 261.7, its search terms included, and its index 32.7, `log_media_refs` 65.4 |
| Metrics | 364 B/point | 150 | `otel_metrics` 364.1, on the 480 captured points: a block's worth, not a rate |

The span indexes - on `span_id`, `trace_id` and the thread key - are the largest part left beside the columns: at
thirty passes the `span_id` index alone is 51 B a span.

### What one more item costs

At three passes a table's partly filled blocks are charged to few items: 256 KB is 6 B a span there but 182 B a
metric point, so the figure above is part rate and part fixed overhead. **The product targets are judged on the
marginal figure** - the bytes one more item adds - and not on the three-pass one. `make storage-gate-marginal`
replays the corpus three times and ten, each into its own store, and reports each signal's bytes per item between
them and, on their own line, each table's fixed blocks: its bytes at three passes less the rate times three, in
blocks, which can be fractional or negative because a table grows by whole blocks. Both are held to
`scripts/perf/storage-gate-marginal-baseline.json` exactly, as the three-pass figures are; `ARGS=--update` writes it
in the commit that moves it. It takes two replays, about thirteen minutes on a RAM disk, so it is opt-in and run
before a release; `make check` runs the three-pass gate alone. It is the regression gate, not the verdict: a target
is called met only on a thirty-pass measurement, recorded here with its date.

The rate is linear in the passes. Measured on 2026-10-13, before span search terms moved onto the row, from three
passes to five, ten and thirty, excluding media:

| Signal | 3 to 5 | 3 to 10 | 3 to 30 | 5's distance from 30's |
| --- | ---: | ---: | ---: | ---: |
| Traces | 677.7 B/span | 661.6 | 671.2 | 0.67 blocks |
| Logs | 245.4 B/record | 210.3 | 199.9 | 0.93 blocks |
| Metrics | 273.1 B/point | 234.1 | 202.3 | 0.26 blocks |

Ten passes is the second count. Five was chosen first, its rate within one block of thirty's for every signal over
the items between the two counts, but one block over the 27,314 spans two passes add is 9.6 B a span, and the first
change judged on it - a span content digest cut from 34.6 to 17.9 B a span, measured column by column - read as a
rise from 476.12 to 485.72, the blocks other tables' indexes happened to fill outweighing it. Over the 95,599 spans
seven passes add, a block is 2.7 B a span, 14 B a log record and 78 B a metric point; the rate sits within a few
blocks of thirty's, traces 3.5 blocks low on this measurement, which is why thirty passes give the verdict. From
three passes to thirty, where a block is 0.7 B a span, 3.6 B a record and 20 B a point, the traces stood at 671
B/span against 480, logs at 200 B/record against 300 and metrics at 202 B/point against 150. With the span terms on
the row the ten-pass marginal trace figure is 464.19 B/span (`storage-gate-marginal-baseline.json`), 2026-10-13.

**The verdict, 2026-10-14**, thirty passes at 384d2019 against the three-pass gate, excluding media:

| Signal | Marginal, three to thirty passes | Target | A block there | Met |
| --- | ---: | ---: | ---: | --- |
| Traces | 455.78 B/span | 480 | 0.7 B | yes |
| Logs | 199.92 B/record | 300 | 3.6 B | yes |
| Metrics | 242.72 B/point | 150 | 20 B | not a rate |

The metric figure moved from 202.27 on 2026-10-13 with no change to metrics: its table took 14 blocks at thirty
passes where it took 12, none of them shared with another table, for the same rows. At 12,960 points a block is 20 B
a point, so the metric target is judged on the derived load instead (Metrics are judged on a derived load): 128 B a
point there, met.

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

What was left then, in order of size, was the DuckDB residue - almost all of it index pages, and 588 B/span of that
the ART index on the term table's `term`, which the search did not drive from (its lookups were keyed by span
identity and measured no faster with the index than without) - then the term postings themselves, one row per term
per span, since moved onto the span row (Span search terms).

**An index exists only where a read provably uses it.** DuckDB reads through an ART index only for a scan whose one
filter is an equality or IN list on the column of a single-column index; it never scans through a compound index,
and a second predicate in the same scan reads the whole table. The compound indexes on spans, logs, metrics and raw
records were therefore never read - on a million spans every per-identity read scanned every row with them and
without - while they held a quarter of the file and every insert maintained them. They are gone; what remains is one
single-column index per key a read looks rows up by (`otel_spans.span_id` and `trace_id`, `otel_raw.raw_id`,
`otel_raw_traces.raw_id`). A log identity's one row is the write's, as a metric point's is: it replaces the stored
row with the batch's winner, the latest `ingested_at` as ClickHouse keeps, under the store's single writer; the
unique index that once refused a second row cost 131 B a record on the corpus (556.14 to 425.28 B per log record).
Every read keyed on one of them is written so the key is its scan's only filter (`sideseat_query_sql::keyed`), binds
at most 512 keys, and stays on the index however many rows its keys find (DuckDB's `index_scan_max_count`, raised on
the connection), so a long trace or a span id a client reuses costs the rows it asks for rather than the table;
deletes go by the row ids a keyed read found. The keys are bound one placeholder each: a list bound as one value -
split from a string, or cast to `VARCHAR[]` for `= ANY` or `unnest`, the only list forms the Rust driver can bind -
is not read through the index at all, and scanned every row of a million to find 64 on 1.5.6. No read depends on
that form. Writing the keys into the statement as literals instead of binding them was measured too and saved
nothing: the same cost at 1, 64 and 512 keys over a million spans, and per-export write-path and server ingest
figures within noise in the pinned 1-core container. Why neither form can help is in DuckDB itself:
`ClientContext::PrepareInternal` (1.5.6, `src/main/client_context.cpp`, lines 428-433) asks
`PreparedStatement::CanCachePlan` (`src/main/prepared_statement.cpp`, line 137), which refuses any plan holding a
table scan, and for a statement with parameters then sets `always_require_rebind` and returns before optimising; the
execution binds, plans and optimises it again with the values. Every parameterised read so plans twice - 15.5 % of
the server's CPU in the prepare and 20.3 % in the rebind, in the pinned 1-core ingest profile - and typed
parameters or cached statements change nothing. Only fewer statements per export can lower it, and removing one
(the second session read) showed no measured win (`write-path-at-10k.md`). The raw identities
stay hex text too, measured against 32-byte
binary over a million spans by rebuilding each raw table both ways: the columns compress to the same size
(`otel_raw` 98.83 MB hex against 99.09 MB binary, `otel_spans.raw_id` 4.46 against 4.19 MB), and only the two ART
indexes gain - `idx_raw_id` 7.34 to 4.72 MB, `idx_raw_traces_raw` 7.60 to 5.24 MB, about 5 B a span in all, deferred
as not worth a schema change for 0.7 %. Dropping `signal_until` and `hold_until` from DuckDB's `otel_raw_traces`,
which repeat the record's own, would save 0.79 MB, 0.8 B a span, and is not done. What an index scan holds of those
rows sits outside `memory_limit`: DuckDB 1.5.6 gathers the key's row ids in a `std::set` while the scan initialises,
about 50 B a row, then keeps them as an array of 8 B a row, neither through the buffer manager; and it fetches the
rows one at a time, roughly 30 µs a row. Both grow with the rows a key finds, which the read has to answer anyway -
a million rows under one key is about 50 MB of heap for a moment and 30 seconds of fetches
(`docs/engineering/request-context.md` has the frames read's measurements). Log records and datapoints carry their
own instant in their identity, so their reads take no index - one on `log_digest` measured 68 B per record and one
on `datapoint_id` 85 B per point - but a list of the records' exact instants, which the row groups' zone maps answer
one value at a time: a range from the earliest to the latest instant reads every row group between them, 983,040
rows against 245,760 for two instants a day apart on a million rows. A span's search terms are on each revision's
own row, and a search reads winners, so a correction deletes none and a deleted row takes its terms with it (Span
search terms). A store at version 2 whose tables, columns, column order or indexes differ from the ones this build
creates is refused at startup with the reset instruction, as a store at another version is. `keyed_scan_tests` holds
each lookup to a small multiple of its keys. Measured by the storage gate on the corpus: 1,398 to 805 B per span,
3,499 to 2,586 B per log record and 154 to 138 B per metric point; on the corpus replicated to a million spans, the
ingest-time lookups went from every row of the table to the rows they name (`read_paths` in the DuckDB adapter).

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
project message feed 100 ms, list sessions 137 ms and project stats 346 ms, each within the memory limit.

**A search reads a record's field state from the record.** Each of a span's six fields was answered by three
correlated subqueries over `span_terms` - the term present, the field truncated, the field indexed at all - and
DuckDB carried every candidate's identity into each, which at a million spans exceeded the memory limit; an empty
field wrote an empty term only so the third could find it, 12% of the term rows. The record now carries
`search_fields` and `search_truncated`, a bit per field, written with it and by the backfill, and the terms a search
names are one aggregate over the term rows that hold them, joined to the candidates. Measured: span search on the
corpus 223 to 20 ms, and at a million spans 81 ms within the limit; `span_terms` 281 to 255 B/span, and the trace
signal 829 to 784 B/span on the storage gate, the two columns adding nothing measurable to `otel_spans`. The terms
have since moved onto the row as well (Span search terms).

**Metrics are judged on a derived load.** 480 captured points cannot measure a store whose block is 256 KB: most of
the figure is one partly-filled block per column, and even at thirty passes of the storage gate a block is 20 B a
point - the gate's metric line is a regression check, not a rate. `scripts/perf/metrics-load.py` derives a
deterministic load of about a million points from the captures, and `--metrics-load` measures that instead. Every
series any captured export carried is kept - its resource, scope, metric and attribute set, 148 of them - and copied
to a fleet of 50 replicas, each resource's captured `service.instance.id` giving way to the replica's, batched into
one export per producer and interval as a collector sends them. Nothing a real fleet varies is repeated: every value
is drawn anew at each point from seeded increments (counters and histogram buckets never fall, gauges wander), each
replica started at its own instant up to a day before the load, which its cumulative points carry, and collects at
its own phase of the interval, late by up to 50 ms each time, so no two replicas' timestamps agree and no interval
repeats exactly. Each point carries one of the exemplar sets its series' captured points carried, chosen at random,
each exemplar with a fresh trace, span, instant and value: 0.55 exemplars a point, where the captures' 480 points
carry 0.59 between them because their series do not carry them equally. The parameters are committed
(`metrics-load.json`), not the data, so the same parameters and the same corpus give the same exports byte for byte;
`"exemplars": "densest"` gives every point its series' largest captured set instead, 1.15 a point, as a second
figure. An earlier load copied only each metric's last export - 38 series of the 148, no exemplar, one clock for
every replica - and measured 140 B a point where the same server stores this one at 185. Measured 2026-10-14 on the
embedded backend:

| Store | Sampled exemplars, 0.55 a point | Densest, 1.15 a point |
| --- | ---: | ---: |
| Before: flat exemplar columns and a JSON array, 64-hex content digest | 185 B/point | 222 B/point |
| Exemplars once, in one list (Metric exemplars); content digest 128 bits | 128 | 149 |

The exemplars take 59.4 B a point before and 21.6 after at the sampled rate, and the content digest 34.6 and 18.3.
Ten times the replicas - ten times the series, a tenth of the points each - measured within a point of the same figure
on the earlier load, so the figure does not rest on how often a series repeats within a row group. The metric target
is met on this load at both rates.

**Arithmetic for media.** Media cannot be compressed losslessly: six generated PNGs, a JPEG, a PDF and encrypted
reasoning signatures, 15.3 MB unique per project out of 74.6 MB. However well everything else is stored, traces
including media are bounded at about 4.8x and logs at about 3.7x, which is why the gate counts media separately.

**Memory is the figure the OS charges, and an enforced limit is the final word.** The two process ceilings - 100 MB
idle, 400 MB at 5,000 spans/s - were judged on RSS, which on macOS also counts pages an allocator has freed and
marked reusable: under steady ingest RSS climbed to 560 MB while `phys_footprint`, what macOS charges the process,
stayed between 215 and 325 MB, and `vmmap` put the difference in the system allocator's zones, which DuckDB and the
other C libraries use (317 MB reclaimable at one sample, against about 70 MB held by jemalloc). The gates
(`scripts/perf/footprint-gates.sh`) now measure the charged figure: `phys_footprint` on macOS, and on Linux the
`memory.current` of a cgroup that holds the server alone, minus nothing - page cache and kernel memory included.
RSS is reported beside it, ungated, so a regression in retained pages stays visible. On Linux a page jemalloc has
freed and not purged is charged to the cgroup, so the binary states its purge policy instead of leaving retained
pages for the kernel to find (`runtime/allocation.rs`: purged within a second, no lazy `MADV_FREE` stage, and
jemalloc's own background thread on Linux), and `the_allocator_purges_on_the_policy_it_states` reads it back from
the allocator. On macOS, at a matched 2,000 spans/s, the policy moved neither the charged median (261-288 MB,
against 262-275 MB on jemalloc's defaults, three runs each) nor CPU per span beyond the noise. A charged figure
still describes a process that was never refused memory, so `footprint-gates.sh container` runs the Linux image
under enforced limits - the 400 MB ceiling with every core at 5,000 spans/s, and the target host, one core and 2 GB
for the server and its embedded backend, at 10,000 spans/s - and fails if the kernel kills the server or reclaim
slows it below 90% of the rate.

**Token totals read only the traces a read is about.** Every list, detail and statistic applies one rule to token
and cost usage: a span counts when it carries usage and is a generation no direct generation child of which carries
any, or is not a generation, its trace holds no generation with usage and its parent carries none. The trace and
session message reads state those totals; a span view states its one span's own, and a feed page none. DuckDB checked
it with three anti-joins over every winning span of the project, whichever traces the read was about, and the
trace and session lists computed it for every trace and session in scope before choosing the fifty they show. The
rule only ever compares spans that carry usage, so it is now evaluated over those of the traces the read selects
(`query-sql/src/analytics/tokens.rs`); the lists read totals for their page alone unless sorted by them; a page's
spans are bounded by its traces' start times, which lets DuckDB skip row groups by their zone maps; the session
list's canonical sessions are computed for its project rather than the store, and the statistics' for the traces
its window touches. Measured at a million spans in one project (`read_paths`, the fastest of runs on a loaded
host): list traces 347 to 106 ms, list sessions 401 to 139 ms, project statistics 240 to 184 ms, get session 122 to
57 ms, the traces of a session 125 to 70 ms, get trace 34 to 21 ms, every answer equal to ClickHouse's.

**Open: what would bound the whole-project reads.** Three reads still read every span of the project: the
unfiltered trace and session lists sort every trace or session by its first start, which takes an aggregate over
all of their spans, and the statistics read the token rule's peers and the canonical sessions of their window's
traces from a scan of the table, since nothing indexes a trace's spans by anything a scan can skip on. At two
million spans, on the same host, the three took 177, 459 and 744 ms - against 103, 137 and 225 ms at one million,
faster than linear for the two that aggregate sessions, and the session list and the statistics already past the
ceilings `make bench-reads` holds them to at one million (`scripts/perf/read-ceilings.json`). At ten million a linear
projection from two puts them at 0.9, 2.3 and 3.7 seconds, and the measured trend is worse than linear. The design
to take up when a measured read approaches its ceiling:

- *Token totals from a narrow table.* `token_facts` would hold one row per winning revision that carries usage -
  its identity, parent, generation flag, start, request model and the twelve usage values - kept by the span write
  as `superseded_at` is. The rule would run over it at read time, where today it runs over the spans. A maintained
  per-trace partial sum cannot replace it cheaply: a span's contribution depends on its parent, its children and
  its whole trace, exporters deliver children before their parents, and deciding a parent that arrives last needs
  its stored children, which only a `parent_span_id` index or a scan of the trace can find.
- *Per-trace rollups for the lists.* Partial rows a write appends for the traces it touches - span, observation and
  GenAI-span counts as signed sums, a superseding revision's correction subtracting its predecessor - and the
  first start, last end, canonical session, first user and environment as (instant, span, value) candidates.
  A candidate cannot be retracted exactly when the runner-up of the same write is lost, so a revision that does not
  dominate the one it replaces, and a retention batch that removes part of a trace, mark the trace for a bounded
  compaction that recomputes it from its spans; reads take marked traces from the spans until then.
- *What the statistics would need on top.* They are windowed by each span's own start, with trend buckets of any
  width, so per-trace rows are not enough: partials per trace and hour, the two edge hours read from the spans,
  distinct users as per-(trace, hour, user) counts so that a correction can subtract one, and the average trace
  duration over the in-window spans of each trace.

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
re-derivation can tell the two apart. Every version keeps the record's `received_at`, a repair included, so a record
replays at one instant. ClickHouse keeps the records in one partition: it resolves `FINAL` a partition at a time, and
partitioned by receipt month, a record's versions in two months were two latest versions.

A version is keyed `(epoch << 32) | n` in `version_key`: the raw arbiter's per-project epoch the writer read before
it wrote, then its place within that epoch (`sideseat_ports::types::raw_version`). Versions order by epoch first,
and a collection decision is recorded at an epoch and bumps it, so its purge - bounded to the versions keyed at or
below that epoch - can never reach a version keyed above it, however late or often it runs. A version keyed at or
below it is not protected by the key alone: a writer that read the epoch before the decision lands within the
purge's bound, and the protocol's confirm fence must refuse that version ownership and have it keyed again above
the decision (`server/specs/RawRecordOwnership.tla`, `Confirm` and `Rekey`; `server/specs/RawRecordCutoff.tla`). A store whose
versions are the receipt instants of earlier builds - a `version` column - is refused at startup with the reset
named.

Who owns a record is decided in the transactional store - `raw_owners` (record x trace), `raw_writers` (a staged
payload's pending-writer twin, created and settled with its registration), `raw_arbiter` (the project epoch and
grant sequence) and `raw_collections` (each collected record's pending decision and high-water mark) - never from
the analytics rows:

- an ingest, once its rows commit, claims each record its rows name through the **confirm fence**: the claim rests
  on the epoch the ingest read before storing its record and writing its rows, and is granted - its traces become
  owners, except traces a journalled deletion took - only when that epoch is above every decision of its record
  and no decision is pending. A record with no live version is keyed afresh before the claim; a claim refused, or
  answered with a pending decision, fails the batch, its records are queued, and its redelivery claims above the
  decision. Only after the grant is the record checked against the rows (repaired if it does not hold them), and only
  then may the payload settle: an ingest whose claim fails is refused and delivered again. Settlement - and the
  check whether a redelivery's rows are already stored - counts a span stored only when its record is live and
  owned through the span's trace;
- every deletion and expiry of span rows enqueues the records holding those traces, finding them through the trace
  index rather than the rows. A journalled deletion takes its traces' ownership in its own transaction; the
  retention pass takes it once the rows are gone, releasing only grants stamped at or below the grant sequence it
  read before it looked at the rows - so an ingest whose rows that look missed keeps its owner - and then queues
  every record the trace index holds under those traces, so a retry after a failure between the two still queues
  what the first attempt released. The look at the rows must see every acknowledged insert, so on a cluster it
  reads every replica of every shard (`clusterAllReplicas`, with `skip_unavailable_shards = 0`): an acknowledged
  insert is committed on at least one replica of its shard, with or without `insert_quorum`, so a replica lagging
  behind cannot hide a row. A replica that cannot be reached fails the read; nothing is released, the cleanup stays
  owed and is retried by the next pass. A replica that stays down therefore stalls retention, and the stall is
  visible rather than looking like growth: `GET /api/v1/projects/{id}/storage` reports `retention_cleanup` - the
  cleanups owed, the age of the oldest, and the most passes any one has been claimed by without completing - read
  from the transactional store, so every instance answers alike. ClickHouse's age pass records a cleanup for every
  trace it expires, read across every replica, before its delete, as DuckDB's does; a row inserted with an expired
  timestamp between that read and the delete is deleted without one, and its record waits for its time-to-live;
- the reconciler asks the arbiter, for each queued record it finds stored, for a **collection decision**: one guarded statement that
  grants it only when nothing owns or writes the record and no legal hold is active, recording the project epoch and
  bumping it. Granted, it purges the versions keyed at or below that epoch and folds the decision into the record's
  high-water mark; it never reads who owns what, and a purge that runs arbitrarily late - after a takeover purged
  and dropped the decision and a redelivery claimed the record again - cannot reach a version an owner rests on.
  **No collection removes a row**, on either backend: rows naming a collected record are those of writes the
  client was never told succeeded - a batch that failed or was abandoned - which at-least-once delivery admits;
  they go with their trace's retention or deletion, alike on both backends, and nothing reads them as stored,
  since nothing owns them. (Removing them was the first design; a removal bounded by row epochs is sound on DuckDB,
  but ClickHouse merges keep one revision of a span by receipt order, and receipts are not monotonic across a
  record's incarnations, so a late removal could take the only row an owner rested on.) On DuckDB the record's
  trace-index rows go with its last version, in the purge's transaction; ClickHouse writes them before the
  version, so a stored version always has its index, and leaves them to their time-to-live. A record with no live
  version is not decided on: the pass purges what its cutoff covers. Refused, it rewrites the record without the
  spans the deletion fences refuse, or - holding nothing deleted - purges the versions below the latest it read,
  and, at the latest's own key, every version but the last stored: two rewrites can key their versions alike, and
  the one readers do not take may hold deleted content the other removed. ClickHouse cannot tell equal-keyed
  versions apart in a delete; its merge keeps the last stored, so there that version goes at the next merge.
  It then looks again and re-queues anything unsettled. A decision a collector left unpurged - it died, or is slow -
  is purged again after five minutes; repeating a purge is safe, since it reaches only versions at or below its
  epoch;
- readers that decide ownership questions - the ingest's claim and check, settlement, the reconciler - see a record
  only if its latest version is keyed above the record's cutoff (the greater of its high-water mark and a pending
  decision). A version under a pending decision is simply doomed; one at or below the high-water mark landed
  after its purge completed - a landing that beat the landing horizon - and each distinct one found is counted
  (`late_raw_landings`) and logged once with how many epochs late it is, so an operator learns the analytics
  store's clocks or execution bounds were exceeded;
- an ingest whose span write fails enqueues the records it stored before the rows, which nothing then owns.

A reconciler reads a queue entry only once the **landing horizon** (30 s) has passed since it was enqueued, so the
version its writer was landing has landed by the read. On DuckDB the version and its entry commit in one
transaction. On ClickHouse a repair is appended and then queued, and the **landing fence is pending**: nothing stops
a submitted insert committing after the horizon, past a client timeout or its process's death. What the model
proves for `FencedLanding = FALSE` is what holds without it: a late landing is never claimed or counted as stored,
and never costs an acknowledged export its record, but its bytes - or a derived row no trace owns - persist until the
record is next touched or its time-to-live expires, not within one pass. The fence (an `INSERT ... SELECT FROM
input()` gated by a deadline on the server's clock, under its execution-time cap) is its own slice. A second
merge-bounded residue sits beside it: on ClickHouse, a version two rewrites keyed alike and readers do not take
lingers - holding deleted content the other removed - until the next merge keeps the last stored; a delete cannot
tell equal-keyed versions apart, and no reader takes it meanwhile.

**Still being built:** the user-facing raw readers (download, export, re-derivation, restore) do not yet apply the
cutoff or live ownership, so until they do a late landing's bytes, or deleted content awaiting its rewrite, can be
read through them; and a rewrite is appended directly rather than staged as a payload of its own. **A known open
defect:** the reconciler looks for a legal hold before a rewrite or supersede purge, in separate calls, so a hold
placed in between does not stop one it has already decided on; the collection decision checks the hold in its own
transaction and is not affected.

Media follows the same ownership as an extracted file (`trace_files`, `pending_writers`, `durable`), owned by every
trace whose span text carries it; survivor reconciliation therefore keeps what a surviving record references, not
only what its derived columns do. Media the file store refused - a project over its quota - stays inline, so a record
is decodable whatever happened to the files.

`server/specs/RawRecordOwnership.tla` models the protocol, with four checked instances - one delivery redelivered
against a crashing reconciler, duplicate executions against two crashing reconcilers (the takeover corridor), three
collectors, and backup and restore under a hold - and `server/specs/RawRecordCutoff.tla` the mapping from its one
record to many. Checked over every interleaving within the models' bounds: no acknowledged export loses its record,
no purge removes the latest version owned rows rest on, every live row an acknowledged ingest wrote stays owned (so
what a collection leaves is only unacknowledged writes' rows), a hold keeps its live content, deleted content
leaves every version once settled, and no reader gated by live ownership sees deleted content. Without the epoch
bounding purges, without the confirm fence, or with one project-wide cutoff, TLC breaks them.

**Deferred to the retention slice:** `raw_collections` keeps a row for every record ever collected, and nothing
prunes it yet - bounded live telemetry leaves unbounded collection state behind, counted in the project's held
transactional bytes. And owner rows are released only by deletion and SideSeat's own retention passes: ClickHouse's
90-day time-to-live can reap rows while SideSeat is not running (the age pass runs a day ahead of it, hourly, but
not while stopped), leaving their owners unreleased. The same holds for the file associations of the logs the TTL
reaps while SideSeat is stopped: the log age pass records a media-bearing log's trace for release at the same bound
as the span pass, so only a stopped server lets the TTL remove a log unrecorded, and what it leaves is a held
association - a file kept too long, never a reference to bytes that are gone. A record that arrives already older
than the TTL can likewise be reaped by a merge before the hourly pass sees it, spans and logs alike: the same held
association, and the same remedy in that slice - recording the intent at ingest for a record already past the bound. Both are arbiter-state retention and
are pruned there. The
project's held transactional bytes are also computed there, as a sum over the arbiter's rows beside the deletion
journal, on every storage read and quota admission - work in proportion to what is stored; that slice replaces the
sum with a per-project counter kept in the transactions that touch those rows, if admission runs it per batch.

**Pruning `raw_collections`, decided order.** After its purge a row's only job is its high-water mark, which hides
the record's versions keyed at or below it - landed late, or still to land from an execution that read its epoch
before the decision - and makes the confirm fence refuse them. A missing row reads as never collected, so a row
may go only once no such version can land or be seen: the purge is done, nothing owns or writes the record, no
hold is active, no version is stored on any replica, and no execution that read an epoch at or below the mark can
still land. The last condition is the one that needs a mechanism, and the order is:

1. a time-to-live read predicate on ClickHouse - a raw version past `greatest(signal_until + 90 days, hold_until)`
   reads as absent and the confirm fence refuses it - which also fixes a record that lands already past its TTL:
   visible until a merge, or removed before the age pass records it;
2. the landing fence (`INSERT ... SELECT FROM input()`), designed in TLA+ first. It must also make a raw rewrite's
   admission - it lands only while the version it was prepared from, key and bytes, is still the record's latest
   - one step with its append on ClickHouse, and visible across replicas, as it is in DuckDB's transaction.
   ClickHouse reads and then writes, on one replica, so a version landing in between, or committed on a replica
   the read missed, is not seen and its check is only advisory (`server/specs/RawRewriteFiles.tla`);
3. pruning on top of it, by one rule on both backends: the lowest epoch any execution that can still land has
   read.

Rejected: a project-wide floor in place of the rows, which hides an old record never collected and keyed below it
- the `PerRecord = FALSE` counterexample of `RawRecordCutoff.tla`; and, before the landing fence exists, a
process-local watermark on the embedded backend alone, sound there because DuckDB admits one process but a second
scheme for one invariant, and an exception to process-local state not worth spending for 124 B a collected record.

**A raw rewrite's file cleanup: derived, not held.** A rewrite that removes deleted spans from a raw record owes
their traces a file reconciliation once its version lands, because the record stops referencing the removed spans'
media. Today a failed reconciliation is only logged, and once the reconciler's entry settles nothing finds those
traces again. The decided fix makes the owed cleanup derivable: the sweep recomputes from the stores which
associations no live row and no latest raw version references, rather than trusting a recorded intent. It is
designed in TLA+ first, and may share the landing fence's re-derivation cursor.

Rejected after five review rounds: a recorded intent that a rewrite **holds** from the retention sweep until its
append lands. Each round found a way to lose the cleanup, and each fix needed a new fence:

1. completing the intent inline cancelled unrelated cleanup debt of the trace; and the sweep could complete the
   intent before the append, against the content the rewrite was removing;
2. one rewrite's release ended another's hold; and a recorder completing its own work inline deleted a held
   intent on its current token;
3. a token handed out while the hold stood completed the intent after the release; and expired holds piled up
   behind live ones;
4. a hold that lapsed by time took with it the only record that the cleanup was owed - a rewrite stalled past
   its hold landed after the sweep had completed, then died;
5. with holds bound to the record and released by the reconciler on finding no deleted content: a **stale
   observation** - a pass saw no deleted content, stalled, and then ended the hold of a rewrite of a later
   deletion still in flight; and a **stranded hold** - a hold committed late, after another reconciler had
   settled the record and cleared every entry, so nothing read the record again.

The fresh-token rule survived: tokens come from one sequence, so no row carries a token an earlier one did. The
parked hold code and model are kept outside the tree, so nobody rebuilds them.

**Open: an ingest batch's pending writer outlives a crash.** A file association counts its in-flight writers in
`trace_files.pending_writers`, and survivor reconciliation releases only an association with none. An ingest
batch increments it when it associates a file, and decrements it when it confirms on success or releases on
failure. A batch that crashes between the association and the confirm or release leaves its increment behind.
The payload's redrive then associates again (+1) and confirms (-1), so the count stays at one: survivor
reconciliation never releases the association while the trace lives, and its file is kept until the whole trace,
or its project, is deleted, whose cleanup removes associations whatever they count. That is a file kept too long,
not data loss, and nothing today resets a dead writer's count. A fix needs a writer's identity behind each increment, so that a
dead one can be told apart from one in flight. A lease would bring back the stalled-writer hazard, so the fix is
still to be designed.

**Deferred to the trace-index slice, right after it:** the retention release is bounded per call by traces (64 at
a time), not within a trace. It reads every owner of those traces and every record their rows still name, so one
long-lived trace costs memory and work in proportion to its own rows. On DuckDB the presence of a trace's rows is
read through the span table's `trace_id` index - which the production connection keeps whatever a key finds
(`index_scan_max_count = 2^40`, `adapter-duckdb/src/session.rs`), so it never falls back to a table scan - but the
keyed read materialises the trace's rows; and the release's lookup of the records indexed under the traces filters
`otel_raw_traces` by a `trace_id` it has no index on, which scans that table. Paging owners alone would not bound
it; an index answering (trace, record) presence does, and that slice measures it.

### Log views

A log record's `body`, `body_text` and `attributes` are functions of its raw record (`otel_logs.raw_log`) and of the
references ingest found unbacked in the project's store (`noted_references`), so they are not stored: ingest renders
them from the raw record exactly as a read will, compares the result with what it derived from the decoded record,
and stores the three columns only where they differ (`domain::log_views`). On the corpus that is no record; it is a
record holding a double that OTLP/JSON cannot hold or whose text does not parse back to the same value, or an
attribute holding bytes, which the typed attributes tag and the rebuild does not. A read - the log list, a single log,
search's candidates and its term backfill - answers the stored columns where there are any and the rebuilt ones
otherwise, through the domain, so every reader sees the same views whichever way they were reached. On ClickHouse,
which keeps no reference table, the release reads the raw record's values unescaped, since a verified producer
reference now lives only there.

### Log messages

A log record's messages are a function of its raw record too, read by one derivation (`domain::log_messages`)
that ingest runs on the record and a read on the stored raw record. They are stored only where the raw record does
not rebuild them exactly, where they hold a file reference the release reads them for, or where the raw record is
more than twice their length and 1 KiB besides; `messages_bytes` (4 bytes, zero on a record without messages)
replaces `messages != '[]'` and the frame budget's `strlen(messages)`. On the corpus about one carrier in eight
keeps its messages for size and a handful for references; the column fell from 65.4 B/record to below a block.
ClickHouse also keeps `messages_hash` (1.8 B/record on the scale bench) for its two-statement feed read.

### Log search terms

A log record's exact search terms are on its row, as ClickHouse keeps them: one column per field logs search
(`search_body`, `search_event_name`, `search_severity`, `search_attributes`), each term between single spaces, so a
term is present exactly where ` term ` is - a term is a run of alphanumerics and never holds a space. DuckDB's
`log_terms` table repeated the record's 64-character digest, its project and the field name on each of its
fifty-odd terms, and its term text was 818 B/record; the row columns cost two blocks on the corpus. Search reads the
terms from the candidate rows themselves (`search::duckdb_term_column`), with the field state from the same
row's `search_fields` and `search_truncated`, and the backfill writes them with those bits; a term goes with its
record by construction, so no delete has terms to chase.

### Span search terms

A span revision's exact search terms are on its row, as a log record's are and as ClickHouse keeps them: one column
per field spans search (`search_prompt`, `search_completion`, `search_tool_name`, `search_tool_args`,
`search_error`, `search_span_name`), each term between single spaces, read by `search::duckdb_term_column`'s ` term
` match with the field state from the same row's bits. The `span_terms` table repeated the span's project, trace,
span, field and instant on each of about 32 terms; measured at thirty passes of the pinned corpus it held 237.4 B a
span, its term text 70.6 of them. The prompt's column is the long one, 423 B of terms on average: with the default
compression DuckDB leaves it uncompressed, 161.7 B a span, so it and the completion's are zstd and the four short
fields keep the default; all six cost 37.75 B a span, measured by dropping the table. Each revision carries its own
terms and every search path reads winners only, so a correction writes no delete - the bookkeeping that chose which
revision's terms to delete is gone - and a deletion of any kind takes the terms with the row;
`a_span_s_search_terms_go_with_its_row_on_both_backends` holds both backends to that through a correction, a reused
identity and the five deletions. On the storage gate the trace signal went from 750.19 to 558.24 B a span at three
passes, and from 677.66 to 476.12 B a span marginal from three passes to five; 464.19 from three to ten. A search
pays for it in reading: every candidate's term text is scanned for its ` term `, so at a million spans DuckDB's span
search went from 134.0 to 170.2 ms through the API and from 116.5 to 156.1 ms in the distributed bench, scanning 2.0
million rows where it scanned 34.6 million, against ClickHouse's 333.5 and 325.5 ms; log search did not move. Split
by column at thirty passes on one thread, the prompt's `contains` takes 128.6 ms and the completion's 98.7, the four
short fields 39.2 together, and decompressing the prompt alone - its lengths summed - 27.7: the cost is the
substring scan of the long text, not its zstd. Testing the short fields first, a pure reordering of the expression,
measured 200.0 ms against 210.2, within the run's noise, since every field is evaluated for every row either way; it
is not done. Ingest appends about 32 fewer rows a span, which the throughput profile has yet to measure.

### Span content digest

A span revision's content digest is the first 128 bits of BLAKE3 over its producer-owned resource, scope and span,
as 32 hex characters, where it was all 256 as 64. No API answer, export or cursor holds it; a backup copies it as
the store's own state. It tells one revision of a span identity from another - a staged payload's confirmation, the
search backfill's check that the winner is unchanged, a search source's revision - and nothing outside the identity
is compared with it. Two revisions of one identity agreeing by chance is 2^-128 a pair; a producer choosing both
revisions to agree needs about 2^64 BLAKE3 evaluations, one matching an already stored revision about 2^128, and
either gains only that its own span's earlier revision is taken for its later one. New rows' logical bytes, which
storage accounting sums, are 32 B smaller with it. On the storage gate the trace signal went from 564.64 to 545.45 B
a span at three passes, and from 464.19 to 450.48 marginal from three passes to ten, five blocks where a block is
2.7 B a span. Measured on 409,710 random digests, the column costs 34.55 B a span as 64 hex characters, 17.92 as 32
and 18.56 as a 16-byte BLOB, so it stays text. A metric point's content digest, its own function, is cut to 128 bits on the same reasoning; its `datapoint_id`, which the API addresses a point by, keeps all 256.

### Metric exemplars

A metric data point's exemplars are stored once each, in one list on its row - DuckDB's `STRUCT(trace_id VARCHAR,
span_id VARCHAR, time_unix_nano UBIGINT, value_int BIGINT, value_double DOUBLE, attributes VARCHAR)[]`, ClickHouse's
`Array(Tuple(...))` of the same fields, unnamed because the driver parses no named tuple element. Six flat columns
held the first exemplar for the trace-correlation filter and a JSON array held all of them, the first again, with
its ids as hex strings, its instant as RFC 3339 text and its keys spelled out on every entry. The answers are
rebuilt from the list: the first exemplar's trace, span and microsecond instant, and the array, byte for byte as the
server wrote it - `the_exemplar_array_is_answered_exactly_as_it_was_stored_before` holds the rebuild to the old
writer, and `captured_exemplars_are_answered_as_before_on_both_backends` holds every captured exemplar on both
backends to it. The `exemplar_trace_id` filter still matches the first exemplar's trace only. The ids stay hex
inside the list: compressed, 32 hex characters cost 17.92 B where 16 bytes as a BLOB cost 18.56, on 409,710 random
ids, and text keeps an id of any length. DuckDB's positional appender binds no list, and its text form of one does
not cast back every string - an attribute with quotes and backslashes in some orders fails to - so a batch is
appended to a temporary staging table holding each row's exemplars as JSON, which `serde_json` writes and
`from_json` reads exactly, and moved into the table by one `INSERT ... SELECT` that converts them;
`awkward_attribute_text_round_trips_exactly` round-trips a thousand seeded cases of quotes, backslashes, brackets,
`NULL` as text, control characters and unicode. On the derived load the exemplars went from 59.4 to 21.6 B a point
at the sampled rate, and the metric figure from 185 to 128 B a point with the 128-bit content digest, 222 to 149 at
the densest.

### ClickHouse column codecs

ClickHouse compresses every column with LZ4 unless its definition names a codec. The large text columns and the
digests that are not sort keys now take ZSTD(3): the spans' messages, tool definitions and names, previews, metadata,
exception text, usage details, statements, content digest, raw id and search columns; the metrics' attributes,
histogram, quantile and digest columns; and the logs' search columns. The sort-key ids keep LZ4, which a key lookup
decompresses most. The spans' `messages` also compress in 4 MiB blocks: in the default 1 MiB ones they cost 187.2 B a
span where DuckDB's cost 96, and in 4 MiB ones 95.9. Row order made no difference: on one pass of the corpus,
insertion, trace and time orders all compress to 183-184 B a span in default blocks. On the distributed footprint
(wide parts, one pass), traces went from 1,087 to 525 B a span, logs from 467 to 368 B a record and metrics from 154 to
127 B a point, and the load's CPU fell from 68.3 to 61.9 s in the server and 399.2 to 372.6 s in ClickHouse. The reads
were timed with `SIDESEAT_READ_PARITY_CODECS` on the million-span scale test: one store as it was, A, and one
with the codecs, B, with the same rows and part layout, alternated five rounds a read. At a quiet run's spread of 1.035 the
median B/A was 1.024 and the worst read 1.201 (messages of a session, through the domain), the searches 1.13-1.19. A
codec or block size changes no answer, so a store created with others is accepted; ClickHouse refuses only layouts
it would read wrongly.

### Not done: a numeric span key

The `span_id` index is the largest left beside the columns, 49.9 B a span at 409,710 spans. An index on an 8-byte
integer key costs 37.75 there and one on the same 8 bytes as a BLOB 46.07, but every answer still needs the span id
as text, so the key would be a column beside it - about 8 B a span of random bits - for a net 4 B. Taking the key
from a hash of the id would need no fallback for ids that are not 16 hex characters, and saves the same 4 B.
Dropping the text column for the key saves about 15 B but needs a second stored form for the ids a client sends that
are not hex, and a branch in every read that renders one. DuckDB 1.5.2 indexes an expression - `hash(span_id)`,
46.07 B a row - but never scans through it. None is worth its column, index or second path, so none is done.

### Deferred: log records into the raw store

Log records are their own raw form, `otel_logs.raw_log`: the OTLP/JSON of the record, media held by reference and
producer strings escaped (`file_uri::escape_producer_text`), so it restores exactly from its stored objects. The
cleaner form is the one traces have: the export as received, an SSR1 record in `otel_raw` with media by hash, from
which `raw_log` is derived like every other log column. It moves raw ownership for logs under the arbiter, so it is
the storage slice's, after its retention work.

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
