# The write protocol for 10,000 spans a second

Status: design, not built. The target (AGENTS.md, re-scoped 2026-10-16) is **10,000 spans/s on one four-core
Ampere A1 with the distributed backend** - ClickHouse, PostgreSQL, an S3-compatible blob store and optionally a
durable queue - with the server inside 2 GB. The embedded backend serves local development and carries no
throughput target; its earlier analysis is Appendix A.

Figures marked *measured* were taken with the distributed ramp harness and the write-path spikes; their logs are kept
outside the repository with the harness, and each will be re-taken with a `scripts/perf/` target before a slice that
relies on it lands. Figures marked
*estimated* say how they are derived. "This host" is an Apple M1 Pro, Docker in an 8-vCPU VM, under a host load of 8
to 110 from the endpoint agents.

## 1. The distributed path: where it stands and what 10,000 needs

### 1.1 Setup, *measured* 2026-10-16

- **Images and pinning.**
  - Server: the bench image at `31a30485`, `--cpuset-cpus 0..N-1 --memory 2g`.
  - ClickHouse 26.4.3.37: cpuset 4-5 (two cores of its own).
  - PostgreSQL 17 and MinIO: cpuset 6.
  - Redpanda 26.2.3, when used: cpuset 6.
- **Generator.** `scripts/perf/ingest-throughput.py`'s open-loop generator, run *inside* the VM on cpuset 7, with
  every step's bodies pre-rendered before its clock starts.
  - The changes are throwaway: `dist_throughput.py`, `fastworker.py`, `run.sh`.
  - Against a do-nothing sink it held 30,000 spans/s at p99 under 2 ms, so it is not the limit.
  - Run from the host instead, through the VM's port forward, the same sink answered at 44 ms p50. The stock
    generator also spent about 0.55 core at 2,000 spans/s rewriting ids per send.
- **The "7.5 exports/s" seen earlier** is the storage-footprint loader. It posts one export at a time, so it measures
  latency (about 133 ms an export), not capacity.
- **Load.** The trace corpus with media: 1,400 exports and 13,684 spans a pass, new ids every pass. Each export is
  moved so its latest instant is a day ago, because ClickHouse refuses spans past its 90-day TTL as unstorable. 8
  projects, 40 s steps. Stored spans are checked equal to accepted ones after each step.
- **Two shapes:**
  - *inline* (`PersistBeforeAck`, no queue), which is what `config/sideseat.example.server.json` configures;
  - *queued* (`DurableQueue` on Redpanda), where the export is answered after the publish and a consumer persists.
    The consumer's rate is stored spans over step plus drain.

### 1.2 The curve, *measured*

**Inline** (`ramp-inline.log`, `ramp-fine.log`, `alt.log`):

| Server cores | sustained spans/s | p50 / p99 at that step | server CPU | ClickHouse CPU (of 2) | PostgreSQL CPU |
| ---: | ---: | --- | ---: | ---: | ---: |
| 1 | 246 (250 step) | 1.2 / 2.6 s | 0.14-0.2 core | median 1.0, p90 1.2 | 0.11-0.18 |
| 2 | 242 (250 step) | 2.6 / 4.3 s | 0.18 | median 1.03, p90 1.19 | 0.11 |
| 4 | 199 (200 step) | 0.9-1.4 / 1.6-3.5 s | 0.22 | median 1.06, p90 1.30 | 0.18 |

- Alternating 4, 1, 4 and 1 cores at 150 and 200 spans/s gave equal results. Core count does not matter.
- The failures of 4 cores at 250 coincided with host loads of 40-95 and one-second PostgreSQL COMMITs, which point
  to WAL sync stalls on the VM's disk.
- **Failure at the knee:** latency climbs (p50 4.7 s at 500 offered on one core) until the generator's 128 in-flight
  requests are all waiting. Nothing is refused, and every stored count matches the accepted one.
- **Memory:** 70 to 190 MB peak. The 2 GB peaks of the profiled runs came from `perf` itself.

**Queued on Redpanda** (`ramp-redpanda.log`):

| Server cores | acknowledgement p50 / p99, up to 2,000 offered | persisted spans/s at 500 / 1,000 / 2,000 / 5,000 offered | server CPU | ClickHouse CPU (of 2) |
| ---: | --- | --- | ---: | ---: |
| 1 | 13-19 / 30-211 ms | 436 / 600 / 542 / 324 | median 0.27 | median 0.76, p90 1.54, max 2.39 |
| 4 | 13-16 / 30-156 ms | 463 / 565 / 485 / see below | median 0.35 | median 0.82, p90 1.39 |

The queue decouples the acknowledgement. The consumer persists 450 to 600 spans/s and degrades as its backlog grows.

**The consumer stalls; it does not lose** (`rp-stall.log`, `out-rp-stall*`). A first run reported 126,881 accepted
spans unstored at 5,000 offered. Its settle check gave up after 10 s of unchanged counts, and the consumer's progress
pauses for longer than that. Three reruns at 4 cores, each requiring 180 s of unchanged counts:

| Offered | stored / accepted | drained after |
| ---: | --- | ---: |
| 250 | 10,087 / 10,087 | 101 s |
| 2,000 | 79,965 / 79,965 | 212 s |
| 5,000 | 199,674 / 199,674 | 320 s |

After each run there were no staged payloads left, no staged-payload anomalies, and a consumer lag of 0 on every
partition.

- Progress advances in steps, with pauses of 4 to 14 s. Two pauses, of 19 s and 56 s, came in a run where
  PostgreSQL COMMITs took 2 to 3 s (VM disk sync stalls under host load).
- A pause is one consumer batch: up to 256 exports taken serially through the per-export round trips below, about
  7 to 10 s for a full batch.
- So the stall is bounded by batch size times store latency. The queue is the backpressure: unacknowledged messages
  stay in Redpanda, and the retention monitor watches the lag.
- The backlog does grow without bound whenever the offered rate exceeds the consumer's 450 to 600 spans/s. That is the
  throughput gap (D1, D2), not a correctness one.

### 1.3 Where the time goes, *measured*

**The serial bound (inline).** One `InlineBatcher` writes one batch at a time (`inline.rs`). Between consecutive
wave logs, each batch spends 27 to 40 ms per export outside the logged wave, which covers the post-write
confirmation and settlement: compare `write-path-at-10k.md` 2.2, steps 7-9. The logged wave itself takes 95 to 173 ms.
So the ceiling is about 25 to 37 exports/s, or 245 to 360 spans/s, which is what the ramp finds.

Per export, ClickHouse's `query_log` and `pg_stat_statements` show:

- **Inline:**
  - about 9.5 ClickHouse queries: 6.5 SELECTs at 2.4 to 7 ms each, and 3 INSERTs, one per table per wave, each a
    new part. In order of total time:
    - the `otel_raw FINAL` record lookup;
    - the session `argMin`;
    - the span-to-raw confirmation;
    - the exact-redelivery `count()`;
    - the content-digest read.

    That is about 37 ms of ClickHouse time per export, the serial gap above.
  - 78 PostgreSQL statements in 14.7 transactions.
- **Queued:** 1.25 ClickHouse SELECTs per export, inserts carrying about 12 exports each, and 29 PostgreSQL
  statements in 7.2 transactions. ClickHouse then becomes the bound:
  - its SELECT latency rises to 10-27 ms;
  - its CPU reaches p90 1.4-1.5 and peaks at 2.4 of its 2 cores;
  - it spends about 1.4 ms of CPU per persisted span.
- **PostgreSQL** has one hot row: the storage reservation (`INSERT INTO project_storage_usage ... ON CONFLICT`)
  takes a per-project row lock. In one 40 s step that was 13.3 s of lock time (mean 13 ms, max 847 ms), and the
  wait samples show `transactionid` and `WALWrite`. It is not the main bound today, but its share grows with
  concurrency.

**The server's CPU** (perf at 4 cores, 200 spans/s inline, `out-perf/`) is mostly the machinery of round trips:

- 51 % of samples in the kernel;
- 33 % in wakeups (`__wake_up_sync_key`, `try_to_wake_up`);
- sqlx's PostgreSQL socket writes, 28 % inclusive;
- sqlx's connection ping on every pool acquire, 12.6 %;
- `tenant_transaction`'s `set_config` per transaction, 14.8 %;
- raw-coverage read-back in settlement, 12.3 %.

The server spends 0.5 to 0.7 ms of CPU per span queued, and about 0.7 to 1 ms inline, where a fixed background
share inflates the figure at these rates.

**Found in passing:**

- **Fixed (`6051681b`): every broadcast topic was refused on Redpanda.** That covered `sse_spans:<project>`,
  `presence:`, `agent_request:` and `connection_control:`, with about 300,000 `InvalidTopic` warnings per run. Live
  events, presence and cross-instance invocation never worked there, and each publish cost a failed broker round
  trip. The adapter now encodes the names injectively, and a broadcast parity suite runs on memory, Redis and
  Redpanda.
- **Open: a broadcast subscriber replays history.** It joins a fresh consumer group with
  `auto.offset.reset = earliest`, so a new SSE viewer on Redpanda first receives the topic's whole retained history.
- **Open: topics multiply.** `agent_request:<request id>` creates a Kafka topic per request, and `sse_spans:` one per
  project, so the topic count grows without bound.
- **Open: contradictory error text.** The Redis durability probe refuses `appendfsync everysec`, then its own advice
  says "`everysec` (or `always`)".

### 1.4 The budget

10,000 spans/s is about 1,020 exports/s on this corpus (9.8 spans an export).

- **Server CPU:** 4 cores divided by 10,000 is 0.40 ms per span, everything included. Today it is 0.5 to 0.7 ms
  queued, so it needs a 1.3 to 1.8x cut on this host, before any A1 factor (unverified, Appendix A.1).
- **The serial path:** at 1,020 exports/s, a single serial executor may spend at most about 1 ms per export. With P
  conflict-free batches in flight, P ms per export. Today it is 27 to 40 ms plus the wave.
- **ClickHouse:** about 1.4 ms of CPU per span today means about 14 cores at 10,000 spans/s. The target shape does
  not yet say how many cores ClickHouse gets. At 4 it needs about 0.4 ms per span, a 3.5x cut.
- **Inserts:** at today's 3 per wave, with waves of about one export, the 1,020 exports/s would mean thousands of
  new parts a second. ClickHouse's own guidance is on the order of one insert per second per table. Inserts must be
  per batch, or asynchronous, either way.

**Verdict:** not established either way. Unlike the embedded writer, nothing measured here rules it out, because the
dominant costs are round trips and per-export statements, which the levers remove. But each lever's saving is
estimated until measured, and ClickHouse's core count in the target shape is an open question for the user.

## 2. The distributed levers, in order

| # | Lever | Removes | Expected, *estimated* | Confirming measurement |
| --- | --- | --- | --- | --- |
| D1 | Remove the serial bound. Inline: several conflict-free batches in flight (cut by trace, as waves are). Queued: partition-parallel consumers | the single `InlineBatcher` / consumer loop | throughput rises with in-flight batches until ClickHouse or PostgreSQL CPU binds; inline from ~245 to the queued consumer's ~550-600 at least | the ramp at 1, 2 and 4 cores rising with cores; ClickHouse and PostgreSQL CPU reported per step |
| D2 | Set-based ClickHouse reads per batch: coverage, record lookup, exact redelivery, session stamp, confirmation | 6.5 SELECTs per export inline, 1.25 queued, each 2.4-27 ms | ClickHouse CPU per span from ~1.4 ms towards ≤0.4; the per-export serial gap from 27-40 ms to a few ms per batch | `query_log` families: SELECTs per batch, not per export; ClickHouse cgroup CPU per persisted span |
| D3 | One insert per table per batch, or `async_insert` with `wait_for_async_insert = 1` (already allowed by config) | 3 inserts and 3 new parts per wave | parts per second and merge CPU fall in proportion to the batch size | `part_log` NewPart per second at 10k offered; insert p99 |
| D4 | PostgreSQL round trips | 29-78 statements and 7-15 transactions per export; ping on acquire; `set_config` per transaction; storage reservation per export on a hot row | set-based statements per batch; `test_before_acquire(false)`; one reservation per project per batch. The profile's 28 % + 12.6 % + 14.8 % shrink with the round trips | `pg_stat_statements` calls per export; server perf at 4 cores |
| D5 | Staging when a durable queue already holds the bytes: the S3 PUT and registration per export | a blob PUT (aws_smithy about 7 % of the server's CPU) and one or more transactions per export | candidate only, and not a lever until its durability invariants are checked and modelled: staging serves redrive and settlement, and the acknowledgement contract (`ingestion-architecture.md`) rests on it (`StagingRetirement.tla`, a model of the queue-held variant first) | ack and persist rates with and without |
| D6 | **Done (`6051681b`)**: valid, collision-free Kafka names for every broadcast topic | a failed broker round trip per published span; restores live events, presence and invocation | small CPU; correctness | the broadcast parity suite on Redpanda (`make test-redpanda`); zero `InvalidTopic` in the next queued ramp |

D1 comes first because nothing else shows while the serial loop caps the rate. D2 and D3 follow, because
ClickHouse is the next bound once D1 lifts it (queued: p90 1.4-1.5 of 2 cores at about 550 spans/s).

The token fence (section 4) is a correctness requirement on ClickHouse. Its HIGHs 1, 4 and 6 (section 4.4) are
modelled after this throughput picture is settled. Batching changes (D1-D3) must keep every fence and settlement
invariant they touch.

## Appendix A. The embedded backend (local development, no throughput target)

The analysis below was written for the earlier embedded target, which no longer applies. It is kept because the
measurements stand: the SQLite bridge spike, group commit and the DuckDB figures.

### A.1 Was the embedded target reachable (superseded)

**Not on the current estimates.** The target is 10,000 spans/s on one four-core Ampere A1 (Neoverse N1) with the
server and its embedded backend in 2 GB. That gives 0.40 ms of CPU per span in total, and at most 0.10 ms per span
of wall time in the single DuckDB writer, which is serial.

- The writer. Today persist costs 183 to 250 µs/span on this host (batch 256, `NO_MEDIA`; section 2.1). If all four levers in
  section 2 land at their estimates, it falls to roughly 80 to 235 µs/span here. At an A1 factor of 1.5 to 2x that is
  120 to 470 µs/span on the target, against a bound of 100. The best case misses by about 1.2x and the central case
  by about 3x. These figures understate the writer: the wave log's persist ends (`batch.rs`, `t_persist_done`)
  before compensation, file confirmation, raw-owner confirmation and coverage repair, which are serial too and are
  not yet measured separately.
- The total. Decoding, extraction, the rules and HTTP cost 0.24 ms/span on this host before any store is touched
  (`write-path-at-10k.md` section 2.3). Scaled to A1 that is 0.36 to 0.48 ms, which is the whole 0.40 ms budget
  before staging or persist.
- The A1 factor. The 1.5 to 2x M1 Pro-to-Neoverse-N1 factor is **unverified**: no source is cited, and no run of
  this image on an A1 exists. Its first confirming measurement is to run the same image there
  (`write-path-at-10k.md` section 4).

**What would have to be true to reach it:**

1. The serial writer budget is 50 to 67 µs/span on this host: 100 µs divided by the A1 factor. At 50-export waves
   (about 490 spans), today's per-wave fixed cost of 18 to 31 ms alone is 37 to 63 µs/span. That leaves -13 to
   +30 µs/span for all per-span writer work, against 150 to 226 today. So **both** of these must fall:
   - the per-wave fixed cost, through fewer statements and plans per wave, or waves far larger than 50 exports;
   - and the per-span work, roughly 5 to 10x. That needs:
     - the keyed decision reads to cost at most a few µs per key, against a measured floor of 26 to 30 µs per key
       (`write-path-at-10k.md` 2.1, an open question with no form yet that answers it);
     - and DuckDB's append and WAL work per span (`WriteToWAL` 12.8 %, `UndoBuffer::WriteToWAL` 11 % of the
       profile) to fall by about as much.

   Alternatively, the writer is partitioned, so the serial bound applies per partition. This is not yet justified,
   because persist is not mostly DuckDB CPU.
2. Extraction must get cheaper. With the writer at 0.10 ms of target CPU per span, 0.30 ms remains on A1 for all the
   parallel work: 0.15 to 0.20 ms on this host. Extraction's 0.24 ms needs a 1.2 to 1.6x cut before staging, the
   bridge and everything else take their share. That figure replaces the earlier 4.8x, which carried over the
   one-core allocation of `write-path-at-10k.md` 2.3. A complete CPU allocation per stage is still missing, and it
   decides the real factor.
3. The A1 factor must come in at the low end.
4. Live waves must reach at least 50 exports (lever 4). Without that, the per-wave fixed cost alone is about 460 to
   1,050 µs/span at today's 3 to 4 exports per wave.

So the verdict is that 10,000 spans/s is not shown to be reachable under these estimates. It is not an established
hardware limit.

Levers 1 to 4 are necessary. They are not sufficient: items 1 and 2 above are the open questions.

### A.2 The embedded levers

| # | Lever | Removes, on this host | A1 (x1.5-2, unverified) | Confirming measurement |
| --- | --- | --- | --- | --- |
| 1 | Group commit for staging and settlement | Most of the measured 475-2,273 µs/span (median 1,027) that staging and settlement add, mostly fsync wait (off-CPU wall time). About 8 syncs a wave stay (section 3), plus the blob and SQLite CPU | Disk-bound rather than CPU-bound: set by the volume's sync latency, unmeasured | write path `STAGED=1` under `strace -c -f`: fsyncs per export from ~5.6 to ≤0.2; `bench-http-latency` small export p95 ≤10 ms |
| 2 | Set-based SQLite statements per wave (and a cheaper bridge only where a statement cannot be set-based) | 19-50 µs/span, *estimated*: the arm timings in section 2.2 are measured, then scaled to the real statement mix of 0.875 statements per span | 30-100 µs/span | spike (done); write path again with SQLite statements per span cut from 0.875 to ≤0.03; mixed granted, refused and collecting claims must decide identically |
| 3 | DuckDB plan caching plus one set-based decision read per wave | 45-65 µs/span, *estimated* as profile share times persist cost | 70-130 µs/span | DuckDB spike at 1M spans: "prepare only" (7.6 µs/key) goes, set-based read per key at wave sizes of 100 and 500 keys; write-path persist µs/span |
| 4 | Pipelined prepare and persist, so live waves reach ~50 exports | At the server, the per-wave fixed cost (18-31 ms) goes from 500-900 to 37-63 µs/span; prepare overlaps persist across cores | same order | `ingest-throughput.py` ramp at 1, 2 and 4 cores, with the wave-size histogram from `Pipeline batch completed`; median wave ≥50 exports at 10k offered |
| F | Token fence (section 4): a cost, not a saving | DuckDB: **unmeasured**, since the spike's "18" counts storage-info rows, not bytes. ClickHouse 0.60 B/span: prior figure, artifact not in the parked directory and synthetic monotonic tokens, so it needs re-measuring. One row and one CAS per wave inside the existing transaction, *estimated* <1 µs/span | | read gate: deciding and answering reads with the token filter within 10 %; `make storage-gate` |

#### A.2.1 Baselines

All figures below are *measured*, on this host:

- **Staged and unstaged, measured in one run.** The write-path bench at HEAD `93ed4536` (profiling build), batch
  256, `NO_MEDIA`, 2 passes per run. Ten pairs, alternating, with host load 5 to 110 over the run
  (`staged-unstaged.log` and the wave logs, in µs/span):

  | Path | Median | Range |
  | --- | ---: | --- |
  | Staged and settled | 1,439 | 805-2,603 |
  | Unstaged | 382 | 329-636 |
  | Paired difference: staging and settlement | 1,027 | 475-2,273 |

  Staging's cost tracks host I/O load: 475 µs/span in the two quiet pairs (load about 10), above 1,800 in the
  noisiest.
- **Where persist sits inside that.** From the wave log (`Pipeline batch completed`, 4 runs, 64 waves of about 426
  spans each), prepare is 56-60 µs/span and persist 183-203 µs/span, staged and unstaged alike. Staging and
  settlement sit wholly outside the wave. The roughly 90-120 µs/span between the wave total (241-263) and the unstaged
  bench figure is the bench's own per-export work outside `run_waves`.
- **Reconciliation.** The earlier "staged path 1,450-1,600" is this median. The earlier "staging adds ~1,900" is not
  contradicted, but it is a high-load outcome from the top of today's range, not a central figure. The persist base
  for section 1 is 183-250 µs/span, earlier runs included. The earlier fsync figure (about 913 µs/span at about 5.6
  fsyncs per export) has not been re-measured here: `strace` is not in the image.
- A fit gives 18 to 31 ms per wave plus 150 to 226 µs per span (earlier runs).
- The real path issues **0.773 SQLite statements per span, 7.60 per export**: 10,545 statements over 1,387 exports
  and 13,635 spans. Counted by sqlx's statement log on the write-path bench (`STAGED=1 NO_MEDIA=1 BATCHES=256`), in
  `sqlite-statements-staged-256.jsonl.gz`. The bulk:
  - per export: register, the staged-payload `get`, the cutoff probe, and the two settlement deletes;
  - 1.18 `raw_owners` upserts per export;
  - 0.15 per export of file association and confirmation.

  sqlx does not log BEGIN and COMMIT, so these are not counted. The bench stages with `raw_id: None`
  (`batch.rs`), so it omits production's `raw_writers` insert. With it, the real path issues about **0.875
  statements per span, 8.60 per export**.
- Share of the hand-off CPU (`perf-256-self.txt`). The tokio waker path (`eventfd_write`) is 13.9 % of samples:
  - sqlx's channels (flume and futures oneshot) make up about 54 % of it, roughly 7.5 % of all CPU;
  - tokio `spawn_blocking` completions (DuckDB, blob) make up about 31 %;
  - the kernel side (`try_to_wake_up`, 13.9 %) splits the same way.

  So a SQLite-only fix reaches about half of the 28 % hand-off share. The other half is the DuckDB and blob
  `spawn_blocking` hop per call, which lever 3's fewer statements also reduce.

#### A.2.2 The SQLite bridge spike, *measured*

The spike (`bridge_spike_v2_tests.rs`, scratch branch only) runs one wave on the real tables and statements of
`staging::create`, `raw_arbiter::confirm_owners` and `staging::delete`:

- E exports × 10 spans, 2 traces per export: 339 statements at E = 48, 0.706 per span, against 0.773 in the real
  path;
- every arm in one transaction under WAL with `synchronous = FULL`;
- 20,000 exports' owners and 10,000 tombstones pre-loaded;
- arm order rotated every round, round 0 discarded.

The arms:

- **P**: sqlx, one hop per statement (today).
- **A**: rusqlite on its own `spawn_blocking` thread, one hop per wave.
- **B**: sqlx `lock_handle()` raw handle, a prepared FFI loop under `block_in_place`, one hop.
- **C**: ordinary sqlx with set-based statements over `json_each`, 9 statements per wave plus transaction control.
- **T**: today's commit structure, with one commit per registration and per settlement (2E+1 synced commits).

Linking. **rusqlite links the same SQLite: possible.** `rusqlite =0.32.1` requires `libsqlite3-sys 0.30.1`, the
version sqlx 0.8.6 already locks. Cargo's `links = "sqlite3"` forbids a second copy. `cargo tree -i libsqlite3-sys`
shows one node feeding both, and both report SQLite 3.46.0. The cost is two new crates in the lockfile, rusqlite and
hashlink 0.9.1. Arm B needs `libsqlite3-sys` as a direct dependency of the same locked version, so no new crate,
and `unsafe`. Arm C needs neither.

Run 2 (`bridge-spike-v2-run2.log`), 40 rounds after warm-up, in µs per span (median), with P's interquartile range
in µs per wave:

| `--cpus` | E | P (today) | A rusqlite | B raw handle | C set-based | T per-export commits | P IQR / wave |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 1 | 8 | 29.7 | 12.3 | 13.9 | 15.3 | 104.6 | 2,279-3,046 |
| 1 | 48 | 22.1 | 5.9 | 6.5 | 6.7 | 100.1 | 10,025-14,602 |
| 1 | 200 | 33.5 | 4.9 | 8.0 | 7.1 | 99.6 | 62,599-69,776 |
| 4 | 48 | 19.8 | 5.4 | 6.2 | 5.5 | 88.8 | 9,092-13,484 |

Run 1 had 25 rounds at a higher host load. At E = 8, 48 and 200 it gave P = 48.1, 39.7 and 49.0 and B = 18.2, 7.6
and 9.0 µs/span, so the ratios hold while the absolute figures double with load. One hop's overhead, (P − B) per
statement, is 19 to 36 µs in run 2 and up to 57 µs in run 1, against the 10 to 14 µs quoted earlier.

Verdict:

- **B gets 89 to 107 % of A's saving** in every cell of both runs. By the rule set, B wins over A, and A is not
  needed.
- **C, with no unsafe code and no new dependency, gets 83 to 105 % of A's saving**: once statements are set-based,
  the bridge's per-hop cost is spread over about 8 hops a wave and stops mattering.
- **Recommendation:** C everywhere, and B only for a statement that cannot be made set-based.
- B's caveat: `lock_handle` runs SQLite on a tokio worker, so it needs `block_in_place` (multi-thread runtime only)
  or must stay short.
- Per-export commits (T) cost 66 to 170 µs/span more than one wave transaction, or 0.33 to 0.85 ms per additional
  commit. That figure includes the transaction and bridge work as well as the sync, so it is lever 1's local
  confirmation, not an isolated fsync latency.
- C's spike asserts that every cutoff is absent. Before it replaces the per-claim loop, the set-based confirm must
  also be shown to decide mixed granted, refused and collecting claims exactly as the loop does.

### A.3 Embedded group commit against the HTTP SLO

Today's syncs per export:

- the staged blob's file and directory fsyncs;
- the registration commit;
- the settlement commit;
- a share of the wave's two DuckDB WAL-and-directory syncs and the confirm commit.

That comes to about 5.6.

The proposal:

- a wave's registrations in one SQLite transaction;
- its blobs as one staging segment: one file fsync and one directory fsync, a new blob layout inside the existing
  store, which the invariants allow;
- its settlements folded into the next registration commit, with a settlement-only group whenever no registration
  is pending (a terminal flush), so a lone export never waits on a later arrival.

What stays per wave is about **8 syncs**: the two segment syncs, the registration commit, the two DuckDB commits at
two syncs each, and the confirm commit. Fewer would need a further stated change.

**Segment ownership is part of the proposal, not a detail.** Today staging deletes a blob per payload (`staging.rs`).
With a shared segment that would remove the bytes of a member that is still pending or quarantined. A segment
needs:

- members recorded with their offsets;
- deletion only at the last reference, after the registry deletions commit;
- recovery and orphan detection for a failed segment cleanup;
- project accounting per member.

Acknowledgement:

- An export is acknowledged only after the sync of the group that holds its registration.
- Under `PersistBeforeAck` (the embedded backend, inline), the acknowledgement already waits for the wave's persist.
- Under `DurableQueue`, it waits for the publish.

The commit is leader and follower with no timer: an export waits behind at most the group already syncing, then
its own. That bounds how many groups it waits for, **not** how long a sync takes:

- At 0.4 to 1.7 ms per sync, a set of 4 to 8 syncs is 1.6 to 13.6 ms.
- A single fsync of 20 ms appears in the earlier profile.
- So there is no worst-case bound, and against the embedded small-export ceiling (p95 10 ms) group commit could
  breach it.
- Today an export pays its own roughly four serial syncs, so the median should fall. The tail is the risk.
- The distributed ceilings are already missed today (`bench-http-latency.sh` notes), so they give no headroom
  either.

The gate is `bench-http-latency` before and after, on p95.

Model: `StagingRetirement.tla`.

- Add a separate export-acknowledgement event. Today `Ack(c)` acknowledges the queue message, and the export's
  acknowledgement is implicit in `Stage`.
- Add an explicit property: an acknowledged export's registration is durable.
- `Stage(p)` becomes `StageGroup(S)`, which may mix registrations and settlements; `PowerLoss` rolls back a whole
  group transaction, and the old `tail` reset is reworked to match.
- Add segment references, one surviving or quarantined member, and a final settlement-only group.
- The mutant that acknowledges before the group's sync must break the new durability property. It need not break
  `NoSilentLoss`: a lost registration above the high-water mark is reported as an anomaly, which that invariant
  allows.
- The AUTOINCREMENT high-water argument is unchanged, because a group's rows roll back together.

Decided (coordinator, 2026-10-15): staging stays durable before persist under `PersistBeforeAck`. Dropping that
would need its own model, and it is not on the critical path.

## 4. The token fence

Approved direction: non-temporal, by execution tokens.

### 4.1 Primitive

- An execution (a wave's write, a raw rewrite or repair, a collector step) takes a token from one sequence in the
  transactional store: `executions(token, project, kind, state live|retired|revoked, epoch_read)`. The insert goes
  in the wave's existing SQLite transaction, as a set-based statement.
- Every analytics row and raw version the execution writes carries the token.
- **Retire**: a CAS from live to retired, in the transaction that already holds the confirm fence (arbiter grant,
  file confirm).
- **Revoke**: a CAS from live to revoked, by any deciding side, in the transaction where it decides.
- Both CASes serialize on the transactional store's lock:
  - SQLite: the arbiter's write-first lock;
  - PostgreSQL: the token row `FOR UPDATE` in a statement of its own, then the predicate.
- Death is the committed state `revoked`. No clock or lease is involved, so it is irreversible.
- **Deciding reads** see only rows whose token is retired. They read the analytics rows with their tokens, then read
  the unretired tokens (live or revoked, a small set) from the transactional store *after* the scan:
  - a live token is a pending writer, so the decision refuses or defers;
  - a revoked token's rows are invisible.
- **Answering reads** (the API) may see a revoked late landing until a lightweight delete applies, with lag bounded by
  the sweep interval. Any per-row filter they use must stay within 10 % of today's reads.
- Only live and revoked tokens are stored, because absence means retired.
- **Revoked tokens are kept** (coordinator ruling, 2026-10-15). At about 8 B per failed execution they are bounded
  by failures, not by data, and failures are rare. A late landing after the sweep's delete is therefore always
  recognisable, and the sweep repeats over the kept set. Revisit only if a measurement shows the set mattering;
  `make footprint` reports its bytes. The model checks the set's growth is per failure only.

### 4.2 Requirements and how each is met

Rejected time fence, `REJECTED.txt` (five HIGHs):

1. No commit bound is assumed. A landing at any later time is tagged with a revoked token.
2. Outstanding insert attempts do not matter, for the same reason.
3. Replica lag: decisions never take "latest" or "present" from a replica (see below).
4. Death is irreversible, as a committed CAS.
5. Writer admission serializes with collection: the collection decision revokes or refuses the record's live tokens
   in the transaction where it decides.

Rewrite admission by CAS (`compact-storage.md`, pruning order item 2):

- A rewrite or repair CASes the record's admission row (latest version key, holder token) in the transactional
  store, granted only if the latest is still the base it read.
- Retire re-checks, then moves the pointer. One step with the append, across replicas, because the pointer and not
  a replica decides.
- Only records that have ever been rewritten have a row: rare, so bytes about 0 per span.
- Ingest versions stay under the existing epoch and cutoff confirm fence.

Replica-visible raw versions:

- A rewritten-record pass first checks, by token, that the rewrite's version is on every replica the scan reads
  (`clusterAllReplicas`). Otherwise it defers.
- This turns today's replica leak into a deferral.

Atomic queueing:

- The reconciliation entry is written in the retire transaction, so an entry exists exactly when the append landed
  and was retired (`RawRewriteFiles.tla`, `AtomicQueue`).

The pending_writers leak's requirements (`compact-storage.md`; Codex round 1 table). The four HIGH:

- Settlement drops a writer only when its token is retired or revoked. A revoked execution's late span is invisible
  to the survivor scan, so releasing its association is safe and the landing can be collected.
- `DeliberatelyAbsent` mixed verdicts: decided per association by the holding token's state, not by the payload's
  verdict.
- PostgreSQL READ COMMITTED: one lock order, executions then `trace_files` then `files`. The lock is taken in its
  own statement, the predicate evaluated in the next. Parity race tests in the style of
  `an_older_confirm_never_lowers_a_newer_stamp`.
- Enumeration and FK cascade: no cascade. Writer rows go only through retire or revoke resolution, which records the
  debt it owes in the same transaction.

The four MEDIUM:

- Row bound: an attempt is counted at start, quarantined at the cap, and counted toward quota as `raw_writers` is.
- Execution identity: the token is carried through redrive, claimed-message processing, logs and incoming references.
- Invariants: holds, raw references, and the specific execution that holds an association. A tombstone slip is
  re-owed by the deletion sweep.
- Deletion and restore recovery: composed with `DeleteTrace` and `RawRecordRestore` in the model. An association is
  refused on a tombstoned trace in its own transaction, and a confirm makes durable only what its own writer holds.

Restore-repair residual (`MediaIncarnation.tla`, `CollectorsStopped`):

- Collector deletes carry the incarnation they decided on and are applied only by the store's own condition:
  - DuckDB and ClickHouse: `WHERE incarnation <= :decided`;
  - files: incarnation in the blob path, or an S3 conditional delete (the S3 conditional delete is unverified for
    the S3-compatible stores we support).
- Restored content carries a higher incarnation, so a late delete cannot remove it.
- `COLLECTOR_STEP_MARGIN` goes.

### 4.3 What it removes, the invariant each served, and where that invariant lives now

| Removed | Invariant it served | Now held by |
| --- | --- | --- |
| `trace_files.pending_writers` counter | survivor release never takes an association a live writer may still land on | the release predicate "no live token holds it"; revoked landings invisible to the scan |
| `COLLECTOR_STEP_MARGIN`, the `LOST_AFTER`/`RESTORE_SETTLE` budget for deletes | no collector delete lands after a restore repair begins | the incarnation-conditioned delete |
| ClickHouse rewrite's inline best-effort reconciliation | cleanup owed after a rewrite is not lost to a crash between append and queueing | the queue entry in the retire transaction |
| Advisory read-then-write base check on ClickHouse (`BasedOnCheck`) | a rewrite lands only on the version it was prepared from | the CAS admission pointer |
| Per-export staging and settlement commits and blob fsyncs | no acknowledgement before a durable registration | group sync before acknowledgement; per-row `seq` high-water unchanged |
| Per-statement confirm, cutoff and owner loops | (performance only) | set-based statements; same fence, same transaction |
| `raw_writers` (candidate, to be decided by the model) | collection refuses a record with a pending writer | a live token registered against the record |
| Unpruned `raw_collections` (an enablement rather than a removal) | hide versions keyed at or below the mark | the row is pruned once no live token's `epoch_read` is at or below the mark |

Not removed: the deletion journal, tombstones, the post-write compensation, the confirm stamp horizon, and the
process lease's answer gate.

## 4.4 Open after Codex round 1 (2026-10-15)

The review is the first Codex round on this outline. It keeps the token direction and finds that these mechanisms are
asserted, not established. Until each one closes, the corresponding row of 4.3 is not justified.

1. **HIGH: a revoked token does not undo replacement.** ClickHouse `ReplacingMergeTree(version_key)` can merge an
   eligible version A away in favour of a live rewrite B, which is later revoked and swept. A's span then has no raw
   authority. DuckDB has the same problem: a revoked correction may already have set `superseded_at` on the previous
   winner. Token eligibility must govern replacement and supersession themselves, not only visibility.
2. **HIGH: the admission pointer must compare key and bytes**, not the key alone (the existing `KeyOnly` mutant).
   Every path that changes the latest version must take part, ingest and repair included. And a first rewrite must
   not initialise the pointer from a replica that missed a newer ingest version.
3. **HIGH: on PostgreSQL, locking token rows does not serialize changes to the writer set.** An execution
   registered after settlement's enumeration is not blocked, and collection over an empty writer set has nothing to
   lock. Registration, settlement and collection need a common parent lock, such as the existing arbiter and trace
   locks, plus a terminal-payload admission predicate.
4. **HIGH: retirement does not close outstanding attempts.** A timed-out insert that is retried and retired under
   the same token can still land late, after pruning removed the cutoff that would have rejected it. Attempts need
   an identity, and retirement must close over them. Pruning must also keep every recorded prerequisite: purge
   done; no owner, writer or hold; no stored version on any replica.
5. **HIGH: one token's state cannot resolve mixed verdicts.** A wave token holds F1, whose record survives, and F2,
   whose record is deliberately absent. Retirement needs an atomic partition per association into confirm or owe,
   proven against concurrent registration.
6. **HIGH: restore rolls back token death and incarnation allocation.**
   - A backup taken before a revocation brings the token back as absent, which reads as retired.
   - Restore advances incarnations only past the stored objects, so an incarnation that was issued but is no longer
     stored can be issued again, under a pending conditioned delete.
   - Fixing both needs a restore-generation protocol.
7. **HIGH: shared staging segments need reference-based reclamation** (section 3).
8. **MEDIUM:**
   - a settlement-only terminal flush (now in section 3);
   - an explicit acknowledgement-durability property (section 3);
   - the sync count and latency (section 3);
   - the arithmetic in section 1 (corrected above);
   - the token byte evidence (table above);
   - the gaps in the modelling plan (section 5, item 7).

### Existing defects, found by the D1 review (round 1, 2026-10-16)

These are in the code today, not in the proposal. They are reachable across instances now, and D1's concurrency
widens them. The review is the first Codex round on the D1 model.

9. **HIGH, data loss: the session sweep's read-then-tombstone race.** The deleted-session sweep (domain
   `cleanup.rs`, `claim_deleted_sessions_for_check`) resolves a deleted session's traces, then journals and deletes
   them. Nothing orders that against ingestion. Witness: a child of T naming deleted session D is stored; the sweep
   resolves T to D; another instance writes T's earlier root naming live session L, passes its checks and answers
   200; the sweep then tombstones T and deletes the acknowledged root. The serial form loses too: the sweep
   tombstones a child-only T, and L's root arriving later is dropped as a deleted trace's.
   - The per-batch form of this defect is fixed in edbf81b9: the post-write session check now takes only the
     traces a batch began. That fix hands child-only traces to the sweep, so this item is what remains.
   - To be modelled with D1: the session-membership read and the tombstone must be ordered against writers'
     acknowledgement. The working rule for the semantics (a trace is its root's session; a rootless trace's
     deleted-session children are held until a root arrives or an age gate passes) is section 6, decided
     2026-10-17.
10. **HIGH: exact-redelivery skipping makes answers depend on execution order.** `drop_exact_redeliveries`
    compares content and coverage without advancing the matching row's receipt, and staging confirmation accepts
    matching content at an older receipt. Witness: X@1 is stored; B (received at 2) carries Y for the same span and
    stalls; C (received at 3) carries X on another partition, skips its write and answers; B then writes Y@2, which
    wins. Run B first and the answer is X@3. To fix right after the D1 prerequisites.
11. **HIGH: equal-microsecond receipts are not deterministic.** Receipts are truncated to microseconds. DuckDB
    supersession breaks ties by physical insertion order, and ClickHouse's replacement version keeps the last
    insertion among equal versions. Two revisions of one span with the same receipt, written in either order, give
    different winners. The models assume totally ordered receipts, which hides this. To fix right after the D1
    prerequisites, with a tie-breaker every backend applies identically.

## 5. Modelling plan

Every model is checked with `make harden-spec`, with mutants. Section 3 covers lever 1's modelling; each item below
gives the model, what it checks, and what has to break it:

1. **`ExecutionTokens.tla`** (new).
   - Executions, CAS retire and revoke, analytics landings at any time including after revoke, deciding and
     answering reads, a sweeper, and re-landing after the sweep. No clock.
   - Invariants: `DecidingNeverSeesRevoked`, `NoAcknowledgedLoss`, `RevocationFinal`. A liveness property for the
     bounded answering lag, under fairness.
   - Mutants that must break it: a non-CAS retire, token states read before the scan, a missing revoke-or-refuse in
     collection.
2. **`FileWriters` on tokens** (port parked `LandingFence.tla`).
   - Invariants: `NoMediaLost`, `OwedNotLost`, `NoDeadWriter`.
   - A READ COMMITTED variant whose predicate is not atomic.
   - Mutants: a cascade, no owed debt on revoke, a confirm without its own writer.
   - Add the debt-only mutant Codex asked for.
3. **`RawRewriteAdmission`** (port `LandingFenceRaw.tla`).
   - Replicas as separate sets with asynchronous replication; CAS admission; atomic queueing.
   - Invariants: `LatestMediaHeld`, `OwedNotLost`, `NoLeakAfterRipe`.
4. **`MediaIncarnation.tla`**: replace `CollectorsStopped` with the conditioned delete. An unconditioned delete must
   lose restored media.
5. **`RawRecordOwnership` and `RawRecordCutoff`**: pruning by the lowest live `epoch_read`, and `raw_writers`
   replaced by live tokens, if kept in scope.
6. **Lever 4**: no protocol model if prepare is shown to be pure, reading rules only, with every store read inside
   persist. That is a code audit plus a test that prepare holds no store handle. Waves keep `waves.rs`'s barriers
   unchanged.

7. **Additions from Codex round 1**, each with a mutant that must fail:
   - replacement before retirement: ClickHouse merges and `FINAL`, and DuckDB supersession;
   - admission: `KeyOnly`, stale initialisation from a replica, and a path that bypasses the pointer;
   - several attempts per execution, including a successful retry followed by a late landing;
   - writer-set serialization: enumeration, locking, new registration and terminal deletion as separate steps;
   - mixed outcomes under one token, and partially successful waves;
   - restore that rolls back token rows and allocation while requests are in flight;
   - staging segments;
   - progress for abandoned live executions, and the circularity that an execution's own coverage check is a
     deciding read while its token is still live;
   - for lever 4, purity beyond "no store handle": pricing, the extraction cache, byte budgets, configuration
     snapshots, and the combined memory bound.

Order:

1. Models 1 to 3, then a Codex review.
2. Lever 2 (no protocol change).
3. Levers 1 and 3.
4. The fence.
5. Lever 4.

Each lever lands only with its confirming measurement from section 2.

## 6. Session membership and deletion

### 6.0 Decisions (the user, 2026-10-17)

1. The canonical session becomes **the root's session, else the earliest span's**, everywhere: reads, live
   events, the fences and deletion. Section 6.4 below records the alternative that was weighed.
2. `retention.session_deletion_grace_minutes`, default 7 days (10080), configurable: a rootless trace of a deleted
   session waits for its root until the gate passes.
3. Modelled with D1, covering §4.4 item 9 and the serialization point in 6.5.

Status: decided 2026-10-17, not built. The per-batch defect is fixed in
edbf81b9; this covers the session sweep's race (write-protocol outline §4.4, item 9) and the semantics behind it.

### 6.1 The problem

A deleted session's spans must go, and stay gone. A live session's acknowledged spans must never go. These
conflict in one shape: a trace whose stored spans are only children, some naming a deleted session D, whose root
names a live session L and has not arrived yet, or is arriving concurrently.

- Today the session sweep resolves the trace to D from its children, tombstones it, and deletes it.
- A root under L acknowledged in the window between that read and the tombstone is deleted (concurrent loss).
- A root under L arriving after the tombstone is dropped as a deleted trace's (serial loss).

### 6.2 The working rule

1. **A trace belongs to its root's session.** The root is a span with no parent.
2. **A rootless trace whose children name a deleted session is owed cleanup, not cleaned.** The owed cleanup is
   held until one of two things happens first:
   - **A root arrives.** The root's session decides.
     - If it is deleted, the trace is tombstoned and deleted as today, children included.
     - If it is live, the trace is kept whole, including the children that named D. Those children belong to a live
       session's trace.
   - **The age gate passes**, with still no root. The trace is tombstoned and deleted as today.

### Both behaviours, spelled out

| Situation | Before the gate | After the gate |
|---|---|---|
| Children name D, no root ever arrives | Stored and readable, owed cleanup recorded | Tombstoned, deleted, journalled |
| Children name D, root arrives naming L (live) | Kept; owed cleanup discharged without deletion | Root dropped as a deleted trace's (the gate already took the trace) |
| Children name D, root arrives naming D | Tombstoned and deleted at once | Already deleted; root dropped |
| Root stored naming D, then children | Fenced by the pre-write check, as today | Same |

The second row, after the gate, is the accepted loss: a root later than the gate is treated as late data for a
deleted trace. That is the product choice to put to the user. A longer gate keeps deleted-session data readable
for longer. A shorter one loses more late roots.

### 6.3 The gate's default

**There is no bounded late-arrival window for writers.** That is deliberate: tombstones and
deletion records are kept forever. Several places in the code say why: any retention bounds how late a stalled
writer may commit and still be collected (`constants.rs` on `DELETED_PROJECT_CHECK_*`, `governance.rs`,
`cleanup.rs`).

The bounded windows that do exist, as candidates for the gate's default:

| Window | Value | What it bounds | Fit |
|---|---|---|---|
| Queue retention (`DEFAULT_REDPANDA_RETENTION_MS`, `database.redpanda.retention_ms`) | 7 days | The longest an accepted export can wait in the queue and still be delivered | **The default.** It is the longest the system itself can hold an accepted root back. Past it, the root's export no longer exists. |
| ClickHouse ingest horizon | 90 days | Spans older than this, by start time, are rejected at ingest | It bounds timestamp age, not delivery delay. A root delivered within 7 days is never near it. Too long for deletion. |
| Deleted-trace check backoff (`DELETED_TRACE_CHECK_BASE/MAX_SECS`) | 30 s rising to 1 day | How often a deletion is re-checked | A rate, not an age. |
| Exporter retries (OTel SDK default, outside SideSeat) | About 5 min | An inline export's retries | Not ours to rely on. |

Decided: the gate is a new setting, `retention.session_deletion_grace_minutes`, defaulting to the queue
retention (7 days). It is not tied to it at runtime, so changing the queue does not silently change deletion. The
inline-only embedded backend gets the same default for parity. Documented as the window in which a deleted
session's data in a rootless trace can still be read.

### 6.4 The canonical session, and the alternative weighed

Until this decision the canonical session was not the root's. It was the session on the trace's *earliest* span that carries one (`trace_session_pairs`: `arg_min`
over `(timestamp_start, span_id)`). That choice was deliberate: `stamp_stored_sessions` documented rejecting
root-privileging, because in a distributed trace a child produced on another host can carry an earlier start time
than its parent. Reads, filters, SSE stamping, the pre-write fence and deletion all used it, with parity tests
across DuckDB and ClickHouse.

The two ways to make a trace its root's session were:
- (i) **a change to the canonical session everywhere:** a trace is its root's session, falling back to the earliest
  span's for a rootless trace. That changes reads and SSE, needs parity and golden review, and reverses a
  documented decision. Or:
- (ii) **a rule for deletion only:** deletion waits for a root, but membership stays earliest-span. Then a trace
  whose root names L and whose earlier child names D is D's trace for reads, but not deletable as D's. That is
  inconsistent.

Decided: (i), "root's session, else earliest". Only the root is the trace's own claim, made once. The earlier
concern - a child on another host starting before its parent - no longer moves the session, because the root
decides whatever its start time; it still orders the trace's spans.

### 6.5 Mechanism, sketched

- **Owed record.** In the transactional store, `owed_session_cleanups(project_id, trace_id, session_id,
  first_seen_at)`, written by the post-write check and the sweep when a rootless trace resolves to a deleted
  session. Instead of a tombstone. Idempotent on the key; `first_seen_at` is the earliest sighting.
- **Discharge on root.** A batch that begins a trace (writes its root) and finds an owed record decides by the
  root's session, in the post-write check that edbf81b9 already restricts to begun traces. If the session is
  deleted, it tombstones (journalled, as today). If live, it deletes the owed record.
- **Gate.** The session sweep tombstones owed traces whose `first_seen_at` is older than the gate, journalled as
  today, then deletes their rows.
- **The race this must close.** The gate's "no root yet" read and the tombstone must be ordered against a writer
  that writes the root and acknowledges. Proposed serialization point: the tombstone transaction re-checks the owed
  record under the trace lock that the arbiter already takes on PostgreSQL, and the SQLite writer lock. A root
  writer then marks the owed record discharged in its post-write check, under the same lock, before it
  acknowledges. Either:
  - the root's discharge commits first, and the sweep's re-check sees it and does not tombstone; or
  - the tombstone commits first, and the root writer's post-write trace re-check sees the tombstone and
    compensates. Its export is then answered as data for a deleted trace, which is the accepted after-gate row
    above.
  This is what the model must establish. It is the same shape as the trace-tombstone confirm fence, applied to
  sessions.

### 6.6 For the model (with D1)

- **State:** spans by trace (root or child, session), deleted sessions, owed records, tombstones, acknowledgements.
- **Actions:** write child, write root, delete session, post-write check, sweep with gate, compensation, acknowledge.
- **Invariants:**
  - `NoLiveLoss`: no acknowledged span of a trace whose membership is live is deleted.
  - `DeletedGone`: after the gate, no span of a trace whose membership is deleted is readable.
  - `RootDecides`: once a root is stored, membership is the root's session.
- **Mutants:** no lock on the discharge; sweep without the re-check; gate on `last_seen` instead of
  `first_seen_at`; tombstone the rootless trace at once (today's behaviour). Each must break an invariant.
