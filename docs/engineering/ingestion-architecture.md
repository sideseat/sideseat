# Ingestion architecture

This document describes the current OTLP ingestion design. It is normative: implementation and
documentation should be changed together. Historical alternatives and review transcripts belong in Git,
not in this file.

## Responsibilities

The backend is a Cargo workspace rooted at the repository root. Every backend library crate lives under
`server/crates/`; the `sideseat-server` crate in `server/` is the executable composition root.

| Layer                  | Responsibility                                                                             |
| ---------------------- | ------------------------------------------------------------------------------------------ |
| `sideseat-core`        | Configuration, constants, storage paths, the schema-version check, and generic utilities   |
| `sideseat-ports`       | Repository, queue, cache, clock, blob, pricing, registration, and secret contracts         |
| `sideseat-domain`      | SideML, rules, files, pricing, search, storage governance, and restore workflows           |
| `sideseat-ingestion`   | OTLP decoding, normalization, identity, staging, durability, and persistence orchestration |
| `sideseat-messaging`   | Typed stream and broadcast messaging over the queue port                                   |
| `sideseat-query-sql`   | Shared SQL query vocabulary and rendering                                                  |
| `sideseat-rule-assets` | Deterministic embedding of framework rule JSON without interpretation                      |
| `sideseat-api`         | HTTP/gRPC decoding, authentication, status mapping, and API schemas                        |
| `sideseat-adapter-*`   | Concrete databases, queues, caches, blob stores, pricing, secrets, and registrations       |
| `sideseat-server`      | Configuration loading, dependency construction, process lifecycle, and background tasks    |

Dependency direction is inward:

```text
server -> api/ingestion/domain/messaging/adapters -> ports/core
api -> ingestion/domain/messaging/ports/core
ingestion -> domain/messaging/ports/core
messaging -> ports
domain -> rule-assets/ports/core
rule-assets -> external embedding library only
adapters -> ports/core
```

Adapters do not import sibling adapters. API and ingestion depend on the typed messaging facade, not on a
queue implementation. The API crate does not select infrastructure. The composition root is the only place
that chooses concrete implementations.

## Entry points

OTLP enters through HTTP or gRPC in `sideseat-api`.

The transport layer owns:

- decoding and content encoding;
- authentication and project access checks;
- request-size and concurrency limits;
- conversion of lifecycle outcomes into protocol responses.

After decoding, both transports call the same signal implementation in
`sideseat_ingestion::signals::export_signal`. HTTP and gRPC must not implement separate ingestion
decisions.

The registered signals are traces, metrics, and logs. Each declares:

- its queue topic;
- its durability strategy;
- its stable identity;
- its producer-content confirmation predicate;
- how project identity is injected;
- how unstorable records are removed;
- how partial success is represented.

## Shared signal lifecycle

```text
decoded request
    |
    v
inject project id
    |
    v
optional debug capture
    |
    v
remove unstorable records
    |
    v
storage-governance admission
    |
    v
stage payload and record identities
    |
    +---------------------------+
    |                           |
    v                           v
persist before ACK         durable queue
    |                           |
    v                           v
confirm stored content     consumer persists
    |                           |
    +-------------+-------------+
                  |
                  v
       release staged payload
                  |
                  v
         OTLP success response
```

The staging blob is written before its registry row. A staged payload has no time-to-live: it remains
discoverable until every record is either confirmed in storage or explained by deletion and retention
rules.

Confirmation compares stable producer-owned identity and content. System metadata such as ingestion time is
not part of the content digest.

A trace export's rows and raw record, and a metrics export's datapoints, are stored at its receipt - the
instant the request arrived, which staging keeps with the payload - not at the instant a write runs. Nothing
orders two workers' writes: a consumer, the inline requester and redrive each load a payload, write it and
settle it, and one holding an earlier export can write it after another wrote a later revision of the same
span or a later correction of the same datapoint. Stored at its receipt, that late copy is superseded at once
rather than taking the span or datapoint back. Settling it finds the later revision the winner, received no
earlier than the export, and settles it as superseded. A span settles that way only once the export's own
record holds it, since raw records are the authority and an export never written has none. A datapoint keeps
only its winning revision, so a superseded one settles on the later correction alone: writing it would store
nothing. `StagingRetirement.tla` and `RawRecordOwnership.tla` check that a later receipt keeps winning
(`LaterReceiptWins`) and that only a written export is retired; `StagingCorrections.tla` checks the datapoint
rule, that an export retired unwritten is always answered by a later revision that is stored. Readers that pin
a traversal or a window by `ingested_at` see such a late write as stored at its receipt: it can enter a
traversal already under way, and the ClickHouse cross-partition check can miss a correction written long after
it was received.

## Acknowledgement contract

An OTLP success means one of two things:

1. the signal was committed to its store in the request; or
2. the staged payload was accepted by a durable queue whose configured durability requirement succeeded.

An in-memory queue is not durable. Trace ingestion therefore persists synchronously when no durable stream
backend is configured.

Every store commits durably before the response: SQLite runs WAL with `synchronous = FULL`, and on Apple
platforms, where `fsync` stops at the drive's write cache, SQLite, DuckDB and the blob store all sync with
`F_FULLFSYNC`. DuckDB does that only when built with `HAVE_FULLFSYNC`, which its own build never defines;
the repository's `.cargo/config.toml` defines it for Apple targets.

A queue reference can outlive its registry row legitimately: redrive retires a row whose message is still
queued, a consumer can crash between retiring and acknowledging, a claimed message is processed twice, and a
project deletion removes its rows. A registration lost after the response looks the same unless something
says otherwise, so each registration takes the next value of a durable sequence, committed with its row, and
the reference carries it. A missing row whose sequence is above the registry's high-water mark, or held by a
different payload once reused, was lost: the consumer records it in `staged_payload_anomalies`, logs it as
an error, and acknowledges only once that record is durable. Any other missing row was finished.
`server/specs/StagingRetirement.tla` models the protocol, including the one double fault it cannot see: a
lost sequence reused by a payload that is itself retired before the old reference is read, which SQLite's
`AUTOINCREMENT` permits and PostgreSQL's sequences do not.

Metrics and logs use `PersistBeforeAck`. Traces can use `DurableQueue` because their processing path is
larger and supports asynchronous batching.

Project deletion is fenced twice:

- the API performs an early cached check so exporters receive useful feedback;
- the write path performs the authoritative check immediately before persistence.

The second check is mandatory because queued traces may be processed after the project state changes.

## Trace ingestion

`TracePipeline` consumes staged trace references or runs synchronously in the request. Queue partitions are
scheduled with bounded weighted round-robin so one hot tenant cannot occupy every batch.

The CPU phase is:

```text
ExportTraceServiceRequest
    |
    v
extract attributes and classify spans
    |
    v
extract message, tool, and carrier observations
    |
    v
normalize messages to SideML
    |
    v
enrich costs and previews
    |
    v
prepare file references and NormalizedSpan rows
```

Requests are processed in bounded waves. Both request count and encoded byte size limit the amount of
in-flight CPU work.

Persistence order is deliberate:

1. reject spans fenced by project or trace deletion;
2. persist extracted files and pending associations;
3. write analytics rows;
4. compensate for deletions that raced the write;
5. confirm surviving associations;
6. publish SSE events only for surviving spans;
7. confirm and release the staged payload.

Files precede rows that reference them. If the second write fails, retention can reclaim an orphaned file;
the reverse order would leave a row promising bytes that do not exist.

A failed write does not release its associations on its own account. A ClickHouse insert can store its rows
and still report failure, and one whose answer was lost can land after the caller gave up. So a project whose
write failed has its associations settled by the references its stored rows and raw records hold: confirmed
where they are named, released where they are not. A write whose outcome is in doubt (`DataError::InDoubt`: a
lost connection, a timeout, an async insert whose wait expired) releases nothing, because no read can yet prove
its rows absent; the retry or redrive that follows resolves the trace. An exact redelivery settles the
associations it re-created the same way, which is how an association lost to an earlier failure is restored.

## Metrics and logs

Metrics and logs share the signal lifecycle but have smaller synchronous persistence paths.

Metric identity is a digest of the OTLP datapoint shape, including protobuf value variants. A corrected
redelivery therefore replaces the same logical datapoint without collapsing distinct series.

Log identity is a semantic digest plus its ordinal within the export. The ordinal preserves equal log records
that were intentionally emitted more than once in one request.

Both paths:

- inject the project id before normalization;
- apply storage-governance admission;
- persist before returning success;
- use the same staging and strict confirmation model as traces.

### Log-carried messages

Some instrumentations emit the conversation as log records linked to a span instead of as span events
(OpenTelemetry's OpenAI v2 and botocore instrumentations by default, and the
`gen_ai.client.inference.operation.details` event). A record that carries a trace and span id and that a
declared `log_events` entry recognises is read at log ingest by the same event reader span events use, with
no span context, and the resulting raw messages are stored in the derived `otel_logs.messages` column. The
column is not producer content: it is outside `log_digest` and outside the confirmation identity.

The messages are never written into the span's row. Span rows are latest-ingest-wins and may arrive after
the log, and the log is durable before acknowledgement while the span may still be queued. Instead, every
message read (`get_messages` and `get_project_messages`) left-joins one aggregate per `(trace, span)` of the
log store's winners in the same statement:

- one row per `(log_digest, ordinal)` - DuckDB keeps one row per identity, ClickHouse reads `FINAL`;
- the traversal watermark applies to that winner on both backends, so a re-delivered record is as old as
  its newest delivery (the only version DuckDB keeps);
- arrays are concatenated in `(timestamp, log_digest, ordinal)` order, identical bytes on both backends;
- the aggregate is narrowed by the span selector, through `GLOBAL IN` for a session on ClickHouse;
- the content filter admits a span whose only messages are log-carried.

At read time the log messages are appended to the span's raw messages before SideML projection, so roles,
history and deduplication treat them as span events of the same name; a producer emitting a turn both ways
is collapsed by content deduplication.

Known limits:

- the project feed pages by the span's `ingested_at`, so a log record arriving after its span was paged
  does not bring that span back into the traversal; the next traversal shows it;
- the tool-span enrichment applied to span events at ingest (copying the span's tool name and call id onto
  its tool messages) is not applied to log-carried messages, because the message row carries no tool
  attributes - a log event on a tool execution span keeps only what the record states.

## Framework knowledge

Framework-specific knowledge is data under `server/assets/rules/` and is embedded by
`sideseat-rule-assets`. `sideseat-domain` compiles those assets into typed plans.

Ingestion-time plans classify spans and extract carriers. Query-time plans normalize stored carriers,
classify history, deduplicate messages, and build ordering constraints.

The ruleset digest participates in the reconstruction cache key. Changing a rule therefore invalidates
cached projections without manually flushing the cache.

Rust code should describe generic operations and semantics. Adding support for a producer normally means
adding or changing rule data and fixtures, not branching on a framework name in production code.

## Query-time reconstruction

Analytics rows store normalized span facts and raw message carriers. User-facing message views are rebuilt at
query time by `sideml::feed`.

```text
MessageSpanRow set
    |
    v
parse stored carriers
    |
    v
flatten to content blocks
    |
    v
correlate calls and results
    |
    v
classify output and history
    |
    v
deduplicate logical occurrences
    |
    v
resolve ordering constraints
    |
    v
apply view and role projection
```

Span, trace, session, and feed views intentionally begin with different row sets. A span view reports what
that span carried, including replayed context. Wider views can collapse equivalent copies and strip
cross-trace replay.

Ordering is a partial-order problem. Atomic emission order, call-to-result relations, generation dataflow,
carrier sequence, and request framing become graph edges. A deterministic resolver produces the final order.
Contradictions are surfaced and pinned by fixtures; they are not silently treated as proof that either edge
was correct.

## Storage and retention

Transactional storage owns projects, identity, file metadata, deletion fences, staging records, and cleanup
journals. Analytics storage owns traces, spans, metrics, logs, message rows, and search records.

Deletion and retention are multi-step operations:

- mark intent in transactional storage;
- delete analytics rows;
- reconcile surviving file references;
- remove unreferenced blobs and metadata;
- clear the journal only after cleanup is complete.

This ordering makes interrupted cleanup resumable. A process restart must not turn a partially completed
deletion into either resurrected telemetry or leaked files.

## Failure behavior

The ingestion path fails closed where an acknowledgement would otherwise overstate durability:

- staging failure rejects the request;
- durable queue refusal rejects the request;
- synchronous persistence failure rejects the request;
- strict confirmation failure leaves staging pending for redrive;
- malformed individual records may produce OTLP partial success when the protocol permits it.

Background consumers retry subscriptions and claim abandoned queue entries. A malformed entry is isolated;
it must not terminate ingestion for the lifetime of the process.

## Verification

The main verification layers are:

- unit tests for extraction, normalization, identity, staging, and governance;
- `server/tests/message_goldens.rs` for end-to-end message projections over captured OTLP fixtures;
- `server/tests/clickhouse_parity.rs` for ClickHouse/DuckDB behavior;
- `server/tests/postgres_parity.rs` for PostgreSQL/SQLite behavior;
- `server/tests/repository.rs` for dependency and file-layout invariants;
- property tests for determinism, idempotence, ordering, and duplicate suppression;
- mutation controls for rules and reconstruction behavior.

Golden files are observations, not independent truth. Source-program tests construct both telemetry and an
external occurrence oracle, which distinguishes a new emission from a replay even when their content is
identical.

## Extension rules

When adding a signal or producer:

1. use the shared transport-neutral signal lifecycle;
2. define stable identity and strict confirmation before choosing queue behavior;
3. keep protocol decoding in `sideseat-api`;
4. keep repository contracts in `sideseat-ports`;
5. keep concrete infrastructure in an adapter crate;
6. represent producer knowledge as rules when the behavior is data-driven;
7. add captured fixtures or a source-program oracle;
8. extend both HTTP and gRPC registration checks;
9. document any acknowledgement or retention change here.
