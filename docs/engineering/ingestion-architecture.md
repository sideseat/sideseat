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

An acknowledged export's raw record is kept while its traces own it. The ingest writes its rows, claims the
records they name through the raw arbiter's confirm fence in the transactional store, checks that each record holds
the rows, writing it again from its own bytes if not, and only then reports the export written, which is what
settles its staged payload and, without a durable queue, what the response waits for. A claim the fence refuses was
overtaken by a collection, whose purge may have removed the ingest's rows, so the batch fails and is delivered
again. A record is collected only by a transactional decision that finds no owner, no registered writer and no
hold; its purge takes only versions keyed at or below its epoch, while a grant rests only on an epoch above every
decision, and no collection removes a row - so no reconciler, dying, slow, or taken over, can take the record or
the rows of an acknowledged export. Rows naming a collected record are an unacknowledged write's, and go with their
trace's retention. The retention release reads which rows remain with a read that follows every acknowledged
insert, or releases nothing (`docs/engineering/compact-storage.md`, `RawRecordOwnership.tla`).

Metrics and logs use `PersistBeforeAck`. Traces can use `DurableQueue` because their processing path is
larger and supports asynchronous batching.

Project deletion is fenced twice:

- the API performs an early cached check so exporters receive useful feedback;
- the write path performs the authoritative check immediately before persistence.

The second check is mandatory because queued traces may be processed after the project state changes.

Deletion removes the project's staged payloads too, registry rows before their blobs, which live in the
project's file store: a queued reference to one is then finished, as a retired one is, and no row is left
naming a blob that is gone.

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

Logs follow the same protocol through the same module (`ingestion/src/media.rs`). A log record's media is
its bytes values large enough to be an attachment and of a media type the content names; they are cut out
before anything is derived from them, since the derived body renders bytes as hex. The derived columns hold
the `#!B64!#` reference as a string, and the raw record keeps its `bytesValue` member holding the reference,
which is not base64, so the raw form restores the record exactly from the stored objects. The address is
BLAKE3 of the base64 text, the file store's and the raw store's own. A file is owned by `(project, trace,
hash)` whichever signal names it, and released only when neither a span nor a log of the trace names it: the
survivor reconciliation reads the trace's log references too - DuckDB from `log_media_refs`, a row per
media-bearing log written and deleted with it, ClickHouse from the trace's log rows - and the log age pass
records a media-bearing log's trace for release before it deletes, at the bound the span pass uses.

A file's bytes are stored under `{hash}.{incarnation}`, the incarnation being its row's id, a sequence neither
SQLite's `AUTOINCREMENT` nor PostgreSQL's `BIGSERIAL` reuses; readers resolve the name through the row. The
collector of an unreferenced file claims the row, deletes the bytes, then the row, and a claim that looks
abandoned is taken over by another collector. Nothing stops the first from waking afterwards and deleting
what it was about to delete: under the hash alone that removed the bytes of the same content ingested again,
and acknowledged, since. Under the incarnation it names a name that is gone, and its row release and delete are
conditional on the row's id and claim time. `server/specs/MediaIncarnation.tla` models it. There is one row
per hash at a time, so the content is still stored once; a restore adopts the bytes it finds under any
incarnation of the hash into the restored row's.

A restore takes the sequence back to the backup's, while the blob store, restored to its own point, can hold
objects named by incarnations issued after it. So the repair's first step, before it creates any row, lists the
blob store's highest incarnation and moves the sequence past it and past every row (`file_incarnations_advanced_past`
in the report); the restored sequence then never issues a lost timeline's name again. The procedure stops every
process first, and that is load-bearing rather than something naming could provide: a collector paused before
the restore deletes, when it wakes, the bytes of the row the restore brought back, under the very incarnation the
backup holds. The model shows both: `Advance` FALSE breaks `StoredNamesBelowNext`, `CollectorsStopped` FALSE breaks
`AckedReadable`. So the procedure is enforced: each server holds a lease in `process_leases` while it runs, the repair
refuses while one is live and looks again after a renewal, and no server starts while the repair holds its own
(`domain::process_leases`). A server whose renewals stop landing does not exit, which would take every instance down on
one store failover. Instead it closes its `CollectionFence`. The fence holds a deadline 12 s past the last renewal
that landed and compares it with the clock itself. Retention, the claim and zero-reference sweeps, file reclaim,
quota eviction, the raw reconciler and staged-payload release check it before each pass, claim or release. Every blob
deletion goes through `FencedStorage`, which refuses while the fence is closed and abandons a delete at the deadline.
The fence opens again once the server has taken the lease back, which it cannot do while a repair holds one, and
then only once every pass entered before the loss has left (`CollectionFence::enter`, the drain): retention, the
sweeps, eviction, settlement, the trace wave and the export each run inside a pass and check the fence immediately
before each statement that deletes, so a decision taken before a restore never acts after it. The model holds this
claim: `MediaIncarnation.tla` with `Lease` and `Drain` keeps `AckedReadable`, and breaks it with `Drain` FALSE. What
no fence closes is one statement already in flight, and the boundary is drawn at the port: every destructive port
call is preceded by a check with no await in between, and what happens inside one call is that call's statement.
ClickHouse's `delete_spans`, which enqueues the spans' raw records and then submits the delete, is one such call: the
two statements are not checked apart. In the same class are a process paused rather than exited, and a request a
remote store received before its client gave up. A pass that hangs stalls collection on that instance, which the
log and `collection.waiting_for` in the project storage report show. The
fence closes before the repair's second look 20 s after its first, so at that look every server either holds a lease
the repair sees or has stopped starting deletes - though a delete sent before the fence closed can still be applied
afterwards, the restore residual `compact-storage.md` records as open. Exports are not acknowledged while the fence is closed (`export_signal`).
An export written across the restore could confirm the association the restore and a later export recreated under
another incarnation. A reused name alone is harmless, because `{hash}.{id}` holds the same bytes whatever timeline
issued it; the model's `AckedReadable` holds with `Advance` FALSE.

A record without a trace keeps its media inline: there is no trace to own the file under. No captured
record does this. The planned extension is the log's own identity in the trace position of the owner key.

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

A log's body, its text and its attributes are not stored where its raw record rebuilds them, as it does for every
captured record: ingest checks the rebuild against what it derived, and a read answers the rebuilt views through the
domain (`domain::log_views`; see `compact-storage.md`, Log views).

Both paths:

- inject the project id before normalization;
- apply storage-governance admission;
- persist before returning success;
- use the same staging and strict confirmation model as traces.

### Log-carried messages

Some instrumentations emit the conversation as log records linked to a span instead of as span events
(OpenTelemetry's OpenAI v2 and botocore instrumentations by default, and the
`gen_ai.client.inference.operation.details` event). A record that carries a trace and span id and that a
declared `log_events` entry recognises is read by the same event reader span events use, with no span context
(`domain::message_events`), through one derivation over the record's decoded content (`domain::log_messages`):
ingest reads it from the record, a read from the stored raw record. The messages are stored in
`otel_logs.messages` only where the raw record does not rebuild them exactly, where they hold a file reference a
release reads them for, or where the raw record is far larger than they are (more than twice their length and
1 KiB besides, so a read of them transfers within a constant of what it budgets); everywhere else the column is
NULL and the domain rebuilds them. `messages_bytes`, their length, says which records carry messages - what every
read filters by - and is what the frame budget sums. The messages are not producer content: they are outside
`log_digest` and outside the confirmation identity.

The messages are never written into the span's row. Span rows are latest-ingest-wins and may arrive after
the log, and the log is durable before acknowledgement while the span may still be queued. Instead, every
message read (`get_messages` and `get_project_messages`) left-joins one aggregate per `(trace, span)` of the
log store's winners in the same statement:

- one row per `(log_digest, ordinal)` - DuckDB keeps one row per identity, ClickHouse reads `FINAL`;
- the traversal watermark applies to that winner on both backends, so a re-delivered record is as old as
  its newest delivery (the only version DuckDB keeps);
- arrays are concatenated in `(timestamp, log_digest, ordinal)` order, identical bytes on both backends;
- the aggregate is narrowed by the span selector, through `GLOBAL IN` for a session on ClickHouse;
- the content filter admits a span whose only messages are log-carried (`has_log_messages`).

The store answers each record's messages as a source - the stored array, or the raw record and what the
derivation reads beside it - and the domain resolves them before any reader sees the row. ClickHouse's project
feed is read in two statements: the page first, each span with a fixed-size token of its winning carrier
revisions (count, and a sum of hashes of identity, instant and `messages_hash`, from narrow columns), then the
page's spans' sources with the same token; a page whose tokens disagree is read again, and after four attempts the
one statement that joins every candidate span answers. One statement scoped by `GLOBAL IN` over the page measured
42% slower on the feed, since ClickHouse evaluates the page subquery again. ClickHouse reads a log aggregate's
scope as `PREWHERE`, which it does not choose under `FINAL`; the scope is the record's trace and span, the same in
every delivery, so the winners are unchanged and a view reads only its own records' raw forms.

At read time the log messages are appended to the span's raw messages before SideML projection, so roles,
history and deduplication treat them as span events of the same name; a producer emitting a turn both ways
is collapsed by content deduplication.

Known limits:

- admission - which spans have log messages - reads the project's carrier keys on both backends (DuckDB's
  carriers relation, ClickHouse's token aggregate), a per-project cost on narrow columns; only the messages and raw
  records a page shows are read whole;
- a ClickHouse view of a session whose carriers are mostly rebuilt reads their raw records in place of their
  messages, up to about twice the bytes: measured +14% on the scale bench's busiest such session;

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
