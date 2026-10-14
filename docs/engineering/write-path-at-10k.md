# What 10,000 spans a second would take

Status: decided, not built. The user's decisions (section 3): the target is 10,000 spans/s on a four-core box, not on
one core; "identical whatever the core count" means identical rows and answers for identical receipts, with
per-table bytes held by the storage gate's deterministic replay; and the new write protocol comes after the other
targets. Until then the write path is tuned, and only by changes with a measured win: fewer statements per export
did not show one (section 1.1). Before more tuning comes a diagnosis of where an export's time goes, because the
rate does not rise with cores (section 1.1). The target was first put as one Ampere A1 core; AGENTS.md now
states it as decided: sustained ingest of at least 10,000 spans a second on one four-core Ampere A1 machine, with
the server and its embedded backend inside 2 GB, and identical rows and answers for identical receipts whatever
the core count.

## 1. What tuning reaches

Measured at `6e33652e` in the pinned aarch64 container (`scripts/deploy/bench`), 2 GB, the corpus averaging 9.8
spans per export:

- the server sustains 715 spans/s on one core, 625 on two and 612 on four, spending 1.35 ms of CPU per span;
- the write path alone (`ingest_write_path`, one thread, staged and settled - no HTTP, no queue) costs 21.2, 8.5 and
  5.4 ms of elapsed time per export at batches of 1, 8 and 64.

A batch is cut into conflict-free waves before it is written (`waves.rs`): at a cap of 64 the corpus makes 86
waves, 16.2 exports each, so "batch 64" commits about 16 exports at a time. Batches 1 and 64 fit `cost per batch =
16.1 ms + 5.15 ms x exports`, and batch 8 sits 1.3 ms an export above that line, so the split is approximate: the
16.1 ms is the overhead a batch pays whatever it holds - statements, DuckDB planning every parameterised read twice
(`compact-storage.md`), commits - and the 5.15 ms an export (0.53 ms a span) includes per-row work - keyed index
fetches, one row at a time at about 30 µs a row (`compact-storage.md`); the appends and their compression; the WAL
write that re-scans the appended row groups; extraction and digests - but also whatever per-wave overhead the waves
spread over fewer exports than the cap. The write path includes staging and settlement, and not HTTP or the queue.

So, without a protocol change:

- removing statements - sharing the second session read (measured in 1.1), merging the remaining reads - shrinks
  only the 16.1 ms;
- larger effective batches spread it, and under load the server's batches are small, which is most of the distance
  between its 1.35 ms and the write path's 0.53 ms a span;
- the per-export 0.53 ms a span remains as far as these measurements show, which extrapolates to about 1,900
  spans/s in the write path, before HTTP and the queue.

Tuning therefore looks to top out between 1,000 and 2,000 spans/s per core: measured at the low end, extrapolated at
the high - not a proven bound.

### 1.1 Batches 256 and 1,024, and the core counts

Single runs, on 2026-10-14, in the same pinned container on the Mac that the rest of this section used, and not on
a quiet machine: with every agent idle, the host's endpoint security agents keep its 1-minute load at 6 to 7. Each
row gives the load when the run started. Image `a` is `4b73ebd3`; `b2` is the same commit with the shared session
read (one statement fewer per wave); `old` is `01982cfc`. The figures are ms of elapsed time per export, for the
write path on one core, staged and settled:

| Image | load at start | batch 64 | batch 256 | batch 1,024 |
| --- | ---: | ---: | ---: | ---: |
| `a` | 7.90 | 6.79 | 6.69 | 6.92 |
| `b2` | 5.37 | 6.45 | 7.10 | 7.30 |
| `old` | 7.76 | 8.38 | 7.49 | 8.89 |

The waves are cut the same way in every image, at 16.2, 43.6 and 73.4 exports per wave at caps of 64, 256 and 1,024.
Larger batches do not lower the cost per export, so the 16.1 ms a batch pays is not what the larger caps spread. The
shared read is 5 % faster at batch 64 and 6 % slower at 256 and 1,024. That is not a measured win, so it is held.

The server, ramped in the same container (`scripts/perf/ingest-throughput.py`, three runs at each core count, the
gate run starting at load 8.61 and ending at 19.50), sustained these spans/s:

| Cores | runs |
| ---: | --- |
| 1 | 622, 710, 632 |
| 2 | 365, 364, 610 |
| 4 | 499, 624, 502 |
| 8 | 502, 502, 230 (the last bound by the load generator) |

One core is the fastest and the runs spread by up to a factor of two. A baseline taken from that spread would
tolerate 67 %, which gates nothing, so the ramp has no baseline yet: it needs a quiet, pinned machine. The finding
that matters is that the rate does not rise with cores while the work it pays for is parallel. So a server CPU core
is probably not the limit. The next measurement is where the time goes at one core and at four:

- the server's CPU per allotted core, from the container's cgroup `cpu.stat`;
- the load generator's headroom, run alone against a sink that does nothing;
- the time split across HTTP decoding, staging and its syncs, the DuckDB appends and the checkpoint, from the
  server's tracing spans;
- whether the refused requests (503) come from the in-flight cap, and what the cap is set to.

That decides whether 10,000 is a harness, a configuration or a design problem.

## 2. The protocol 10,000 would need

10,000 spans/s on a core is 0.10 ms a span, everything included: a fifth of today's per-row cost alone. Three
changes are needed, and none is tuning.

### 2.1 Decisions batched, and still authoritative

Today each batch reads, inside its write, everything it decides from: the stored revisions of its identities (the
winner, kept as `superseded_at` updates in the insert transaction), their content (exact redelivery), the deletion
fences, and the raw arbiter's grant and coverage. Every decision is authoritative because it reads the store; the
cost is per key, one index fetch each.

The change would be the read's shape, not its authority: a group reads each of these facts once for all its keys,
in a form whose cost per key is far below an index fetch. No such form is established. A range scan over a sort key
does not work on its own: successive groups' keys are random identities spread over the whole key space, so the
stored rows are not clustered by them, and a range covering a group's keys covers most of the table - work growing
with stored history, which the resource invariant forbids. It would need a maintained layout - rows kept clustered
by identity, which DuckDB's appends do not provide - with a bound on the rows a group's read scans. This is the
first open question, and if it has no answer the per-key cost stays and 10,000 on one core is out of reach.

Within a group the decisions also depend on each other, which is why waves exist (`waves.rs`): two exports of one
trace, or of one span identity, decide against each other as well as against the store - correction B then
redelivery A against stored A must leave A the winner, two identical exports against an empty store must store one
row, and an export naming a stored file is a barrier. A group keeps those barriers: it is cut into conflict-free
waves exactly as a batch is, and only exports with no shared trace share a wave's reads. The invariants each
decision keeps, unchanged because the decision is still taken from the store, wave by wave, inside the write:

- **winners** - latest `ingested_at`, ties by the existing rule, `superseded_at` set in the same transaction;
- **exact redelivery** - skipped only when the stored winner's content and raw coverage match, as today
  (`fences_late.rs`, `drop_exact_redeliveries`); a redelivery is never appended;
- **deletion fences** - the journal and tombstones read from SQLite immediately before the write, and the
  post-write compensation exactly as today;
- **the raw arbiter** - grant, epoch, cutoff and coverage per record, from the transactional store, refused on a
  contested claim (`RawRecordOwnership.tla`).

What is open is measurement: whether a range scan over clustered keys costs per row what the appends do rather
than 30 µs, at a million spans and at a hundred million.

### 2.2 A deterministic group

A group is the unit every decision above is taken over and committed in. For the bytes to be identical at every
core count, a group's contents and the order of its rows must not depend on what arrived together. So groups are
cut by a rule over the telemetry, not over time: exports are sequenced by their staging registration (`seq`, a
transactional sequence assigned on arrival - deterministic for a given arrival order, not for a given core count),
and that is the weak point: two runs of the same corpus at different core counts register exports in different
orders. Identical bytes therefore need either a sequencing that does not depend on arrival - which an open stream
cannot have without a lateness bound - or a weaker reading of the requirement (decision 3b).

The crash protocol per wave is the one a wave follows today (`batch.rs`, `run_batch_into`), in this order; grouping
changes how many exports share a wave and when they are answered, not the steps:

1. each export is staged before it is written - its blob, then its registry row, durable;
2. files are associated (SQLite) and their bytes finalised, before any row references them;
3. the fences are read again after the file work, which can be slow: projects refusing writes, deleted sessions,
   the deletion journal, deleted traces;
4. exact redeliveries are dropped, and the raw arbiter's epoch read;
5. the raw records are written, in their own DuckDB commit;
6. the spans are written - revisions read, `superseded_at` set, rows appended - in a second DuckDB commit, each
   commit's WAL and directory synced (`DuckdbService::write`);
7. deletions that raced the write are compensated, and only then are the surviving associations confirmed;
8. the raw records are claimed through the arbiter and checked to hold the rows;
9. each export is settled: one this first delivery wrote whole, on a backend whose acknowledged writes are durable,
   is retired without reading back (`settle_written`); every other one by reading;
10. each export is answered, only if the store lease was held throughout, in one term.

Recovery is per export, not per wave. A crash before an export's retirement leaves it staged and redriven, every
step idempotent for a redelivery; one after its retirement and before its answer leaves it stored and retired, and
the exporter's retry is an exact redelivery. Folding steps 5 and 6 into one transaction would be a protocol change
with different crash states, and is not proposed. Latency grows by the group's cut, and memory by its size, both
bounded as the inline batcher's are today (32 MiB a batch, 64 MiB queued). How a group is cut is decision 3b's
question: by size alone it can wait without bound, by time its membership depends on timing.

### 2.3 Extraction five times cheaper

Outside the stores a span costs 0.24 ms: OTLP decoding, extraction, the rules, digests, hashing, HTTP. The target
leaves it about 0.05 ms, 4.8 times less. It already runs in parallel across a batch's exports, so on one core it has to get cheaper,
not wider: a profile of `ingest_hot_path` per stage is the next measurement, and its largest stage the first change.
On several cores it could also overlap the single DuckDB writer, but today it does not: a batch is prepared, then
persisted, and the inline batcher awaits each batch before the next; overlapping them is part of the change.

## 3. Decisions for the user

Decided: (a) per four-core box; (b) identical rows and answers for identical receipts, per-table bytes held by the
storage gate's deterministic replay; (c) after the other targets. What each choice meant, as it was put:

**(a) Per core, or per box.** 10,000 spans/s on one A1 core needs all of 2, including a per-key read cost no measured
form provides. 10,000 spans/s on a four-core box divides the parallel work - decoding, extraction, the rules - by
four, but not the writer: one DuckDB writer still has to persist 10,000 spans/s, so its serial cost must fall to
0.1 ms a span on one core either way. The box target is smaller only by the parallel share, about a fifth of
today's cost, and still depends on 2.1's open question.

**(b) What "identical whatever the core count" means under grouping.** Two things are at stake. First, answers: a
span's winner is its latest receipt, ties by insertion order, so the same exports received in a different order -
which more cores produce - can already answer differently today; "identical answers" holds for identical receipts,
and grouping does not change that. Second, bytes: group membership decides where DuckDB's row groups and blocks
break, so per-table stored bytes match across core counts only if groups are cut by a rule over a sequencing
independent of arrival, which an open stream does not have without a lateness bound, and a cut by time makes
membership depend on timing. The choices are byte-identical per-table counts, needing a deterministic sequencing and
cut that may not exist; or identical rows and answers for identical receipts, with per-table bytes held by the
storage gate's deterministic replay, which is how the gate reads checkpoints today. The second is reachable.

**(c) Now, or after the other targets.** 2 is a rewrite of the write path's batching and read shapes across both
backends, with the parity suites, the models (`RawRecordOwnership`, `StagingRetirement`) and the storage gate as
its proof. The storage and memory targets are met or close; this one is at 7 % of its figure.

## 4. What the measurements here are, and are not

The figures above come from a container pinned to cores of an Apple Silicon host under Colima's virtual machine.
That fixes the core count and the memory, not the core: an Apple core is not a Neoverse N1, and a VM adds its own
costs. A figure for the target needs the same image (`docker build -f scripts/deploy/bench/Dockerfile`) and the
same commands (the ramp in `scripts/perf/ingest-throughput.py`, and the write path at batches 64, 256 and 1,024) on an A1 instance - one with
a local NVMe or a block volume of stated IOPS, since the WAL syncs on every commit - with the instance type, kernel
and disk recorded beside the figures.
