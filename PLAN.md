# SideSeat platform foundation — execution plan

**Audience: someone picking this up cold.** It assumes no knowledge of the repository. Read §0–§2, then §9
("Start here").

The *design* lives at `~/.claude/plans/add-claude-agent-sdk-and-anthropic-ai-cl-zany-fern.md` — 13 numbered steps
with the reasoning behind each. That file is the specification; this one is the execution record: what the
architecture is, what has landed and why, what is open, and what has and has not been verified. Where they
disagree the design wins and this file is stale — say so rather than following it.

Written 2026-09-21, updated 2026-09-22. "What has landed" is measured from commit `4a9c30c9`.

---

## Contents

| § | | Read it when |
| --- | --- | --- |
| **0** | [What this project is](#0-what-this-project-is) — glossary, layout, commands, how to run it, the four views, the API | first, always |
| **1** | [The architecture](#1-the-architecture) — crate graph, the ports, **the one constraint** | before designing anything |
| **2** | [The data model](#2-the-data-model) — tables, schema versions, the span row, what step 0 fixed | before touching storage |
| **3** | [The ingest path](#3-the-ingest-path) — write order, the fences, where the footprint gates measure | steps 6, 7, 8 |
| **4** | [The read path](#4-the-read-path) — the cache, the nine feed stages | steps 9, 10 |
| **5** | [What landed, and why](#5-what-landed-and-why) — the 45 code commits, with the reasoning | to avoid re-deciding |
| **6** | [What remains](#6-what-remains) — completion state, acceptance criteria, fixed decisions and known operational bugs | to pick the next thing |
| **7** | [Exact current state](#7-exact-current-state) | to orient |
| **8** | [Verification state](#8-verification-state--read-before-claiming-anything-works) | **before claiming anything works** |
| **9** | [Start here](#9-start-here) | to begin |
| **10** | [Working protocol](#10-working-protocol) — mutation verification, Codex, the conventions that bite | before your first commit |
| **11** | [What this will and will not be](#11-what-this-architecture-will-and-will-not-be) | before trying to "fix" an accepted limit |
| **12** | [Verification matrix](#12-verification-matrix--which-check-covers-which-property) — which check covers which property, and what nothing covers | when you change or add a mechanism |
| **13** | [First day, first week](#13-first-day-first-week) | on arrival |
| **14** | [Risk register](#14-operational-and-follow-on-risk-register) | when planning follow-on work |
| **15** | [Alternatives already rejected](#15-alternatives-already-evaluated-and-rejected) | **before proposing one** |
| **16** | [Keeping this file true](#16-keeping-this-file-true) | after editing it, and when a step lands |

**If you have five minutes:** §9 Start here, then §8 to know what is unverified, then §11 so you do not spend the
day on a limit that is deliberate.

---

## 0. What this project is

**SideSeat** is an observability toolkit for AI/LLM applications. Instrumented agent code exports OpenTelemetry
traces to it; it normalises the many frameworks' incompatible message formats into one internal format
(**SideML**) and serves a web UI for debugging conversations.

Three facts about its shape, because they explain nearly every decision below:

1. **Two storage tiers, two backends each.** *Analytics* (spans, metrics) is DuckDB by default, ClickHouse for
   distributed deployments. *Transactional* (projects, users, files, API keys, tombstones) is SQLite by default,
   PostgreSQL otherwise. **Every read method must return identical rows from both backends of a tier** — enforced
   by parity suites, not by review.

   That is a claim about **answers**, not about capabilities, and the distinction matters: where both engines can
   express a question they must agree, and where one cannot the divergence is *declared* rather than papered over
   (`as_of_us` is honoured by DuckDB and inexpressible on ClickHouse; the rollup contribution table exists only on
   DuckDB). §11 lists every such gap. A parity suite compares the public read, so two backends computing the same
   answer by different means is exactly what it tests.
2. **Normalisation happens at read time, not at ingest.** Spans are stored close to raw; the message pipeline
   reconstructs conversations on every query. Deliberate: a fix to the pipeline applies to history with no
   re-ingestion. The cost is that reads are expensive, which is why memory is an entire plan step and why a
   reconstruction cache exists.
3. **Framework knowledge is data, not code.** Which telemetry attribute holds a conversation, which member is
   the answer, how a content block is shaped — all declared in embedded JSON under `server/assets/rules/` and
   interpreted by generic Rust. Two structural tests fail the build if a production module names a framework or
   one of their telemetry keys. **Do not put a framework name in Rust.**

`CLAUDE.md` at the repository root is the long-form version of all of this: ~1 500 lines, and the single most
useful thing to read before touching anything.

### 0.1 Glossary

You will not get far in the code or in this document without these.

| Term | Meaning |
| --- | --- |
| **SideML** | SideSeat's universal message format. `ChatMessage` with a `Vec<ContentBlock>`; `server/crates/domain/src/sideml/types.rs` |
| **Feed** | the reconstruction pipeline (`server/crates/domain/src/sideml/feed/`) that turns span rows into an ordered, deduplicated conversation |
| **Block / `BlockEntry`** | one `ContentBlock` with its metadata — the unit the feed sorts and deduplicates |
| **Carrier** | the telemetry construct a message arrived in (an OTel event, or a framework's attribute like `output.value`). Its *structure* says what it is evidence of — `sideml/carrier.rs` |
| **History** | a message a framework re-sent as context rather than produced now. Eight detection phases; history blocks are filtered from the answer |
| **Replay / cross-trace strip** | a later trace re-sending an earlier one's conversation. Collapsed by injective matching against a partial order |
| **Tombstone** | a row saying "this was deleted", consulted by the write path so a late batch cannot resurrect it. `deleted_traces`, `deleted_sessions`, `projects.deleting_at` |
| **Deletion journal** | `deletion_journal`: append-only, permanent, what a **restore** replays. A tombstone is removed once the target is quiet; the journal never is. §5.6 |
| **`pending_writers` / `durable`** | the two facts a file association carries. Each referencing batch increments the first; a commit sets the second permanently. A release deletes only a non-durable row at zero |
| **Survivor reconciliation** | after retention deletes some of a trace's spans, recompute which files the *surviving* spans reference and release only the difference |
| **Watermark** | `max_ingested_at_us`, taken from the store rather than the reader's clock, bounding a multi-page traversal to one instant |
| **Parity suite** | a test that writes one dataset into both backends of a tier and requires every read to return identical rows. `clickhouse/parity_tests.rs`, `postgres/parity_tests.rs` |
| **Golden** | `server/tests/fixtures/messages/<suite>/<sample>/expected.json` — the recorded correct answer for a captured OTLP payload, across four views. **121 committed**; a working copy shows 123 because two image-gen fixtures are gitignored for size, so a count from `ls` and a count from `git ls-files` legitimately differ |
| **Mutation-verified** | a fix whose test was proven to fail when that fix alone was reverted |
| **Store** vs **backend** | a *store* is the tier — analytics or transactional. A *backend* is which implementation is serving it — DuckDB or ClickHouse, SQLite or PostgreSQL. Both words appear below and the difference is load-bearing: a property of the *store* holds in either mode, a property of a *backend* is exactly what parity suites exist to compare |

### 0.2 Repository layout

```
server/crates/core/          sideseat-core: constants, config, CLI, storage paths, utils. Names no driver.
server/crates/ports/         sideseat-ports: the traits the domain talks through, plus DTOs. No implementations.
server/crates/domain/        sideseat-domain: ingestion, cleanup, pricing, rules, SideML and trace/metric workflows.
server/crates/api/           sideseat-api: Axum/gRPC/MCP/WebSocket transport boundary.
server/crates/adapter-*/     physical storage, queue, cache, secrets, blob and registration implementations.
server/crates/query-sql/     typed analytics query/DML statements and backend capability lowering.
server/src/
  app.rs, app/        composition root: startup, backend construction, wiring, command dispatch
  runtime/            allocation.rs (the counting allocator), shutdown.rs
  data/, domain/      cfg(test)-only compatibility namespaces for the extracted suites
server/assets/rules/  framework assets: producers/, conventions/, vocabulary/
server/tests/         repository.rs (structural invariants), footprint.rs, fixtures/
scripts/              bench-http-latency.sh, footprint-gates.sh, message-fixtures/capture.sh
```

### 0.3 Commands

```bash
make check                                                # fmt + clippy + every test. No containers.
cargo test --locked -q -p sideseat-server --lib           # composition-root inner loop, ~90s, 88 tests
cargo test --locked -p sideseat-server --test message_goldens # 121 fixtures x 4 views, ~70s — the oracle
cargo test --locked -p sideseat-server --test repository  # 28 structural invariants
make test-postgres                                        # PostgreSQL/SQLite parity, throwaway container
make test-clickhouse                                      # ClickHouse/DuckDB parity, throwaway container
make test-redis                                           # queue durability against a pinned Redis
make bench-http                                           # p95 latency ceilings; non-zero exit on a miss
make footprint                                            # the four memory ceilings; non-zero exit on a miss
```

`cargo clippy` must be warning-free: the workspace sets `all = deny` plus a list of bug classes held at zero.

**`--locked` on every command that resolves dependencies, including in documentation.**
`every_resolving_command_is_locked` reads every tracked file and caught this file's own first draft — a command
that may rewrite `Cargo.lock` makes every `--locked` check downstream a statement about one machine.

**This file is itself under those invariants**, which is worth knowing before editing it. Three of them have
already rejected drafts of it: the `--locked` rule above, `every_module_path_cited_anywhere_resolves` (a
`{a,b}/path` brace shorthand resolves to nothing), and `the_documented_project_structure_matches_the_tree`. Run
`cargo test --locked -p sideseat-server --test repository` after editing.

**`make check` runs no containers**, and the parity suites *skip silently* when `SIDESEAT_TEST_*_URL` is unset. A
green `make check` therefore says nothing about whether the two backends of a tier agree.

### 0.4 Running it, and putting data in

Nothing below needs Docker. The defaults are DuckDB + SQLite + filesystem blobs + an in-process queue, which is a
complete working deployment.

```bash
make dev-server ARGS="--debug --no-auth"    # API on 5388, gRPC OTLP on 4317
make dev-web                                # UI on 5389
make dev                                    # both
```

| Surface | URL |
| --- | --- |
| OTLP ingest (HTTP) | `http://localhost:5388/otel/{project_id}/v1/{traces,metrics,logs}` |
| Query API | `http://localhost:5388/api/v1/project/{project_id}/otel/...` |
| UI | `http://localhost:5389/ui/projects/default/observability/traces` |

The default project id is `default`. **To point any OTel-instrumented app at it**, that is the whole
configuration:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:5388/otel/default
```

**To generate real data**, the repository ships runnable samples per framework under `examples/`:

```bash
uv run --locked --directory examples/python/strands strands tool_use --sideseat
```

Samples: `tool_use`, `mcp_tools`, `structured_output`, `files`, `image_gen`, `reasoning`, `error`, `swarm`,
`rag_local`, `strands_ws`. Other suites: `examples/python/{openai,bedrock,claude-agent-sdk,langgraph,crewai,adk}`
and `examples/javascript`. They read `examples/.env`, which is **not committed** — copy `examples/.env.example`.
Without it `OTEL_EXPORTER_OTLP_ENDPOINT` is unset and a non-`--sideseat` run fails against the OTel default
`localhost:4318` rather than SideSeat's 5388. Most suites need Bedrock credentials; `examples/.env.example`
documents which.

**Configuration layers**, lowest priority first: defaults → `~/.sideseat/` → `./sideseat.json` → CLI args → env
vars. `config/sideseat.schema.json` is the structure and `config/sideseat.example*.json` are worked examples.
Switching to PostgreSQL + ClickHouse + S3 is configuration, not a rebuild — but note the **`Sharing` rule**
refuses incoherent combinations at startup (PostgreSQL with filesystem blobs, or with per-instance secrets while
auth is on), because each such mismatch produces a silent failure an operator would debug in the wrong place.

**Rust floor is 1.94.1**, stated to the patch in the workspace manifest because the AWS SDK crates require
1.94.1 and `1.94` means 1.94.0. Clippy's `incompatible_msrv` enforces it against the source, not just the
lockfile.

### 0.5 The four views

The goldens check four views per fixture, and the difference between them is a frequent source of confusion.
They share one pipeline (`process_spans`) and differ only in their row set:

| View | Rows | Note |
| --- | --- | --- |
| **span** | `WHERE span_id = ?`, no content filter | can hold **more** messages than its trace view: each generation span re-sends the whole history, which trace-level dedup collapses. No cross-trace stripping — it means "what this span carried" |
| **trace** | if the trace has a session, the query loads **the whole session** so cross-trace stripping can run, then narrows to the trace; otherwise `WHERE trace_id = ?` | |
| **session** | every row of every trace in the session, not just rows naming the session | |
| **feed** | a cursor page of the project, newest-first (`process_feed`, a *different* entry point) | pages are chosen by ingestion time while each page is ordered by message time, so concatenating pages is **not** a transcript. `pages_are_globally_ordered` is always false |

**A trace belongs to exactly one session: the one on its earliest span** (`argMin` over
`(timestamp_start, span_id)` — a total order, because a timestamp tie left to the engine gives three surfaces
three answers). Asking "any span named it" was wrong in eight places and is worth reading about in `CLAUDE.md`
before touching session logic.

### 0.6 The API surface

What step 6's "API v1 breaks in place" and step 10's search endpoint are changing.

```
OTLP ingest      POST /otel/{project_id}/v1/{traces,metrics,logs}        HTTP and gRPC
Query            GET  /api/v1/project/{project_id}/otel/traces
                 GET  .../traces/{id}            .../traces/{id}/messages
                 GET  .../spans                  .../traces/{tid}/spans/{sid}[/messages]
                 GET  .../sessions               .../sessions/{id}[/messages]
                 GET  .../sse                                            real-time
Admin            /api/v1/projects, /api/v1/auth/*, /api/v1/health
MCP              /api/v1/projects/{id}/mcp                               for AI coding assistants
SDK channel      GET  /api/v1/project/{id}/ws                            WebSocket: presence + AG-UI invoke
                 GET  /api/v1/project/{id}/registrations
                 POST /api/v1/project/{id}/agents/{name}/runs             AG-UI run, SSE
```

`?include_raw_span=true` on a span or trace route returns the full OTLP JSON.

**Every surface that reads or writes project data is authenticated when auth is on**, and `auth.enabled` defaults
to **true**. Two layers everywhere, because either alone is insufficient: `require_auth` establishes *who* is
asking and passes through untouched when auth is disabled; `verify_project_access` turns a valid credential into
one valid **for this project**, since a key from another organisation is otherwise perfectly valid. A request
arriving with no auth context is *refused*, so a future mounting that forgets the layer fails closed. Three
surfaces once shipped with no auth at all — MCP, the SDK channel, and gRPC OTLP — so this is enforced rather than
reviewed.

---

## 1. The architecture

Four properties, **in priority order**, each of which must end up *checked* rather than intended. The order
decides every trade below.

1. **Layers separated by construction** — a forbidden dependency does not compile.
2. **No framework knowledge in Rust** — already true, unchanged by this work.
3. **Fastest, smallest footprint** — gated numbers, not adjectives.
4. **Not foreclosing the audit layer** — append-only source of truth, stable identities, hold semantics.

### 1.1 Crate graph — target and actual

```mermaid
graph TD
    subgraph enforced["compiler-enforced today"]
        core["sideseat-core"]
        ports["sideseat-ports<br/>traits + DTOs, no impls"]
        domain["sideseat-domain<br/>rules, sideml, extraction, metrics,<br/>files, cleanup, topics"]
        api["sideseat-api<br/>http, grpc, mcp, ws"]
        db["database adapters<br/>duckdb, clickhouse, sqlite, postgres"]
        blobs["sideseat-adapter-blob-storage"]
        cache["sideseat-adapter-cache"]
        reg["sideseat-adapter-registrations-memory"]
        secrets["sideseat-adapter-secrets"]
        topics["sideseat-adapter-topics"]
    end
    subgraph composition["composition root"]
        app["sideseat-server<br/>app wiring only"]
    end

    ports --> core
    domain --> ports
    domain --> core
    db --> ports
    db --> core
    blobs --> ports
    cache --> ports
    reg --> ports
    secrets --> ports
    secrets --> core
    topics --> ports
    topics --> core
    api --> domain
    api --> ports
    app --> api
    app --> db
```

`server/crates/core`, `server/crates/ports`, `server/crates/domain`, `server/crates/api`, all four database adapters, registration storage,
secrets, cache, filesystem/S3 blobs, and the memory/Redis topic backend are real crates. File coordination,
cross-store deletion cleanup, rate limiting and the typed topic runtime live in `sideseat-domain`.
`sideseat-server` is now the composition package: its production modules contain startup/runtime wiring,
backend-selection enums and the provider-SDK implementation, while the former `data` and `domain` namespaces
exist only behind `cfg(test)` for the legacy parity and golden harnesses.

**Why crates and not a lint.** A source-scanning test was considered and rejected: `use` parsing misses
fully-qualified paths, macro-generated code, `#[cfg]` branches, re-exports and inferred types. While everything
compiles as one crate the compiler enforces nothing.

**Each boundary has already paid for itself**, which is the argument for finishing: `core` re-exported every layer
above it; `AppConfig::validate` called into the domain; `DataError` embedded four driver error types *and* had a
`From` impl per adapter; the filter vocabulary lived inside one adapter; the adapters implemented their ports for
`Arc<Service>`, which the orphan rule refuses across crates. None were visible before the split.

### 1.2 The ports

`server/crates/ports/src/traits.rs`, 14 traits:

| Traits | Replaces | Note |
| --- | --- | --- |
| `SpanStore`, `EntityQuery`, `MessageStore`, `AnalyticsMaintenance`, `SurvivorReferences` | `AnalyticsRepository`, 31 methods | the analytics tier, split by cohesion |
| `IdentityStore`, `ProjectStore`, `FileMetaStore`, `ApiKeyStore`, `CredentialStore`, `FavoriteStore` | `TransactionalRepository`, 95 methods | the transactional tier |
| `DeletionJournal` | new | §5.6 |
| `AnalyticsRepository`, `TransactionalRepository` | — | bundles, blanket-implemented, so a caller can still name one bound |

No line numbers here on purpose: an earlier draft cited them and one had already drifted by the end of the same
editing session. Grep the trait name.

`BlobStore`, `Cache`, `Secrets` are ports too (`blobs.rs`, `cache.rs`, `secrets.rs`). **Cache is a decorator over
a port, never a parameter to one** — it used to be `Option<&CacheService>` on 29 transactional methods.

Not yet ports, all from later steps: `MetricStore`, `LogStore`, `SearchIndex`, `RateLimitStore`, `StorageBudget`.
`Clock`, queue backend/error/subscription DTOs, registrations, cache, blobs and secrets are ports now. Typed
`TopicMessage` wrappers remain in the domain, where implementations for foreign OTLP message types are legal.

### 1.3 The one constraint that shapes everything

**Neither analytics backend provides a commit-ordered sequence or a fencing token.** `ingested_at` is
`Utc::now()` on the writing instance, so it does not order two writes; nothing lets a coordinator cancel a write
already issued. Six successive designs in the design document died on this before it was stated explicitly.

```mermaid
graph LR
    subgraph has["Has real ordering"]
        tx["Transactional store<br/>transactions + CAS"]
        kafka["Kafka offsets<br/>total within a partition"]
        duck["DuckDB transactions<br/>local = one instance"]
    end
    subgraph lacks["Has none"]
        ch["ClickHouse"]
        ingested["ingested_at<br/>= Utc::now() per instance"]
    end
    tx -->|"byte counter, pending_writers,<br/>associations, the journal"| ok["safe read-modify-write"]
    kafka -->|"ack + gap tracking"| ok
    duck -->|"replace-on-write"| ok
    ch -->|"confirmation is digest equality,<br/>never 'newer'"| residual["stated residual"]
    ingested --> residual
```

| Wanted | Available | Consequence |
| --- | --- | --- |
| Order two writes to one identity | nothing usable | confirmation is digest equality, never "newer" |
| Prove no further write can land | nothing | the byte budget is a conservative over-approximation |
| A snapshot across a multi-page read | per-query only | search pagination is not snapshot-isolated, and says so |
| Prove a record is in a backup | nothing derivable | restore is a procedure with stated residuals, not a proof |

**The rule: where the fact is unavailable, refuse or over-report rather than delete or under-report, and say so in
the response.** A guard that silently changes the answer is what a caller cannot reason about.

**§11 lists every limit this constraint leaves behind**, with the reason each stays. Read it before trying to
remove one.

§5.6 is what this looks like applied: both rows were in the transactional store, so no ordering trade was
needed — one transaction removed the question three review rounds had been arguing about.

---

## 2. The data model

### 2.1 What lives where

```mermaid
graph TB
    subgraph analytics["Analytics tier — DuckDB (v6) / ClickHouse (v7)"]
        spans["otel_spans<br/>append-only, every revision kept"]
        metrics["otel_metrics<br/>replace by datapoint_id"]
        logs["otel_logs<br/>digest + ordinal identity"]
        terms["span/log search terms<br/>DuckDB relations / ClickHouse arrays"]
    end
    subgraph transactional["Transactional tier — SQLite (v9) / PostgreSQL (v10)"]
        orgs["organizations, users,<br/>organization_members, auth_methods"]
        projects["projects (deleting_at = tombstone),<br/>deleted_projects"]
        files["files, trace_files<br/>(pending_writers, durable)"]
        bodies["content_bodies, span_bodies,<br/>content_body_backfill"]
        staging["staged_payloads<br/>durable before acknowledgement"]
        governance["project_holds, usage,<br/>maintenance leases"]
        tombs["deleted_traces, deleted_sessions,<br/>retention_cleanup"]
        journal["deletion_journal<br/>append-only, permanent"]
        keys["api_keys, credentials,<br/>credential_project_permissions, favorites"]
    end
    subgraph blobs["Blob store — filesystem / S3"]
        content["content-addressed files,<br/>bodies and staged payload bytes"]
    end

    spans -.->|"#!B64!# reference"| content
    files -->|"names"| content
    tombs -.->|"consulted by the write path"| spans
    journal -.->|"replayed by a restore"| spans
```

**`otel_spans` is append-only**, and that is load-bearing: a re-delivered span adds a row rather than replacing
one, and the winner is chosen at read time by `ingested_at` then `rowid`. `as_of_us` reads historical versions.
Metrics, logs and search projections are replace-on-write because nothing reads *their* revision history. Step 8's
contribution table was implemented, benchmarked and removed when it made both measured trace-list fixtures slower.

### 2.2 Schema versions — and the trap

| Store | Version | Notes |
| --- | --- | --- |
| DuckDB | **6** | logs, logical bytes/legal holds, then search term relations |
| ClickHouse | **7** | logs, legal-hold TTLs, search term arrays, then tenant row policies |
| SQLite | **9** | deletion journal, staging, storage governance and content-body ownership |
| PostgreSQL | **10** | the SQLite state plus forced tenant RLS policies and role separation |

**Every schema change needs its own `SCHEMA_VERSION`.** Editing a released version's script reaches fresh installs
and older upgrades and **never** a database already marked at that version. This trap has been hit three times in
this repository; the most recent was putting the `span_id` CHECK into v4 after v4 had shipped. Step 7's
`hold_until` needs v6.

**A DuckDB migration can only append a column, and its metrics writer is a positional `Appender`** — so the fresh
schema must declare added columns *last*, in the same order the migration adds them.
`a_v1_database_upgrades_to_the_same_column_order_as_a_fresh_one` compares `duckdb_columns()` ordered by position.

### 2.3 The file-reference protocol

Four steps of the deletion protocol and three of the remaining plan steps turn on this, and it is hard to follow
in prose. An association is a row in `trace_files` keyed `(project, trace, file_hash)` carrying **two facts, not a
boolean**:

```mermaid
stateDiagram-v2
    [*] --> Referenced: a batch references it<br/>pending_writers += 1
    Referenced --> Referenced: another batch references<br/>the same file (+1)
    Referenced --> Durable: any batch commits its rows<br/>durable = true
    Referenced --> Released: that batch fails<br/>pending_writers -= 1
    Released --> [*]: deleted only if<br/>NOT durable AND pending_writers = 0
    Durable --> Durable: durable is monotonic —<br/>permanently blocks deletion
    Durable --> Reclaimed: the trace is deleted and<br/>provably has no rows
    Reclaimed --> [*]
```

**Why two facts and not a `provisional` flag.** A flag cannot express *several* batches referencing the same
`(project, trace, hash)` at once, which is the case that matters under concurrency: whichever failed first deleted
the row a still-in-flight or just-committed peer depended on, orphaning its file. With a counter, a failing batch
can never orphan a file another batch committed or is about to.

**Why `durable` is monotonic.** One committed row backs the file, so once any batch commits, deletion must be
blocked permanently. It is never unset — which means a **restore** needs a reconciliation path for it that does
not exist today (step 12).

**Every referencing batch confirms or releases**, not only the one that created the row: sharing an association
makes a batch one of its owners. The four early-return paths between writing the files and writing the rows each
release, checked structurally
(`every_early_return_between_files_and_the_write_releases_its_associations`) because a new one compiles and passes
every behavioural test.

**A crashed writer's increment is reclaimed by the deletion sweep, not by the ingest path.** Deleting the
non-durable row from a *drop* path was tried and reverted: `durable = false` does not prove no analytics row
committed (confirmation runs after the write and can fail), and a concurrent batch that passed the fence before
the tombstone may be committing spans right now — so deleting the shared row leaves readable spans pointing at a
file nothing holds, which is the dangling reference the write-files-before-rows ordering exists to prevent.

### 2.4 The span row

`NormalizedSpan` (`server/crates/ports/src/types/normalized.rs`) is the row every step from 7 to 10 has to extend, so its field groups
are worth knowing:

```
Identity        trace_id, span_id, parent_span_id, session_id, user_id
Classification  span_name, span_category, observation_type, framework
Time            timestamp_start, timestamp_end, duration_ms
GenAI core      gen_ai_system, gen_ai_request_model, gen_ai_response_model
GenAI params    gen_ai_temperature, gen_ai_top_p, gen_ai_max_tokens, ...
Tokens          gen_ai_usage_input_tokens, gen_ai_usage_output_tokens      i64, NEVER NULL, default 0
Costs           gen_ai_cost_input, gen_ai_cost_output, gen_ai_cost_total   f64, NEVER NULL, default 0
Error           status_message, exception_type, exception_message, exception_stacktrace
Preview         input_preview, output_preview
Payload         messages (JSON), raw_span (JSON), ingested_at
```

**Tokens and costs are never `Option<T>`.** That is a hard convention, not a default — the read path, the
billing dedup and the web all assume a number.

**Three representations of one span exist today** — extracted columns, the `messages` JSON, and the `raw_span`
JSON. Removing two of them is step 9, and is what makes the "< 3× decoded protobuf" ceiling reachable.

### 2.5 What step 0 fixed, so you do not re-fix it

Marked "Done" in §2's table; these were live data-correctness defects, and seeing the code without this context
invites re-opening them:

| Defect | Fix |
| --- | --- |
| DuckDB retention's batch table was `PRIMARY KEY (trace_id, span_id)` with **no `project_id`** — and a span id is unique only within a trace, a trace id only within a project, both client-supplied. Expiring tenant A's span could delete tenant B's | the project in the key everywhere |
| Retention selected **raw** rows while reads go through the deduplicated relation, so an expired *old* revision selected an identity whose winning correction was recent — and took the correction with it | candidates and counts from the winning relation |
| `max_spans` counted `COUNT(span_id)` over the **whole table**, so the limit was deployment-wide and one noisy tenant spent it for everyone | per project, on the winning relation |
| ClickHouse's span sorting key contained `toDate(timestamp_start)` — a `ReplacingMergeTree` identifies duplicates *by the sorting key*, so a corrected re-delivery crossing midnight UTC returned **both** revisions; across a month boundary it could never collapse, because parts in different partitions never merge | `ORDER BY (project_id, trace_id, span_id)` — identity alone |
| `otel_metrics` was `ReplacingMergeTree()` with **no version column**, so the survivor was insert-block order, while DuckDB's replace was commit-last-wins. Two rules for one question | `ingested_at` as the version on both |

The **cross-month residual is stated and detected, not fixed**: a correction moving a span's `timestamp_start`
across a month boundary puts its revisions in different partitions, and `server/crates/adapter-clickhouse/src/consistency.rs` reports those
identities rather than the read being silently wrong.

---

## 3. The ingest path

```mermaid
flowchart TD
    req["POST /otel/PROJECT/v1/traces<br/>protobuf or JSON, HTTP or gRPC"]
    auth["auth + rate limit<br/>both transports, shared bucket"]
    strip["strip_unstorable_spans<br/>settled at the edge: a 200 must not precede a drop"]
    fence1{"project_accepts_writes?"}
    queue["queue: publish, byte-budgeted<br/>or SKIPPED on the in-memory backend"]
    cpu["CPU phase, byte-bounded waves<br/>extract -> sideml -> enrich -> prepare"]
    fence2{"deleted_traces /<br/>deleted_sessions?"}
    blobs["write blobs"]
    assoc["write associations<br/>pending_writers += 1"]
    rows["write span rows"]
    recheck{"tombstoned<br/>during the write?"}
    confirm["confirm associations<br/>durable = true, pending_writers -= 1"]
    sse["publish SSE<br/>only for spans that survived"]
    ack["acknowledge the queue message"]

    req --> auth --> strip --> fence1
    fence1 -->|no| r404["404 Gone"]
    fence1 -->|yes| queue --> cpu --> fence2
    fence2 -->|tombstoned| drop["drop + release associations"]
    fence2 -->|clear| blobs --> assoc --> rows --> recheck
    recheck -->|yes| compensate["delete the spans,<br/>release the associations"]
    recheck -->|no| confirm --> sse --> ack
```

**Why that order.** Blobs and their associations are written **before** the rows that name them, so the surviving
failure is a reclaimable orphan rather than a dangling reference. Everything after is idempotent by span id.

**Why a re-check after the write.** The tombstone is a row in the transactional store and the spans go to the
analytics store, so nothing makes the pair atomic. A deletion landing inside that window is compensated — and
**SSE is published after every drop and compensation**, because built beforehand it announced spans the batch then
discarded, so a reader saw a span appear and never find it.

**The default in-memory backend skips the queue entirely** and writes inside the request, because a 200 must mean
stored and an in-process queue cannot promise that.

### 3.1 Where the footprint gates measure

```mermaid
flowchart LR
    idle["startup, quiesced"] -->|"gate 1<br/>idle RSS &lt; 100 MB"| g1["RSS"]
    write["steady ingest, 60s"] -->|"gate 2<br/>median RSS &lt; 400 MB @ 5000 spans/s"| g2["RSS"]
    q["queue backlog"] -->|"gate 4<br/>&lt; 3x decoded protobuf"| g4["live bytes"]
    read["10 000-turn session read"] -->|"gate 3<br/>residue &lt; 50 MB"| g3["live bytes"]
```

Gates 1 and 2 are RSS, measured by `scripts/footprint-gates.sh` against a running server. Gates 3 and 4 are **live
allocated bytes**, measured in `server/tests/footprint.rs`. §5.1 says why that split is not arbitrary.

---

## 4. The read path

```mermaid
flowchart TD
    q["GET .../traces/ID/messages"]
    params["MessageQueryParams"]
    rows["span rows, deduplicated at read time<br/>QUALIFY ROW_NUMBER() over (ingested_at, rowid)"]
    cache{"reconstruction cache<br/>key = BLAKE3 of everything read"}
    pipe["the feed pipeline"]
    arc["Arc&lt;FeedResult&gt;"]
    narrow["narrow: ?role=, time window,<br/>scope to trace / page"]
    dto["MessagesResponseDto"]

    q --> params --> rows --> cache
    cache -->|hit| arc
    cache -->|"miss, coalesced"| pipe --> arc
    arc --> narrow --> dto
```

**The cache key is a digest of everything the pipeline reads**, so a changed row is a *different key* rather than a
stale hit — no invalidation to get wrong, no TTL to tune. It is **process-local and empty at startup**, which is
what keeps "a fix applies to history without re-ingestion" true; a persisted cache would serve answers built by
the previous build.

**Narrowing copies only survivors.** With no `?role=` and no window the caller gets the `Arc` itself. Before this
batch every read deep-cloned the whole answer.

### 4.1 The feed pipeline

```mermaid
flowchart TD
    p1["1 PARSE — raw JSON to SideML"]
    p2["2 FLATTEN — one BlockEntry per ContentBlock, never filtered"]
    p3["3 CORRELATE — id-less tool results adopt their call's id"]
    p4["4 CLASSIFY — is_output per block"]
    p5["5 MARK HISTORY — eight phases"]
    p6["6 DEDUP — identity-based, keep highest quality"]
    p7["7 WITHDRAW — clear a correlated id whose call did not survive"]
    p8["8 SORT — a tuple key, never a comparator"]
    p9["9 ROLE FILTER — on the derived role, after everything"]
    p1 --> p2 --> p3 --> p4 --> p5 --> p6 --> p7 --> p8 --> p9
```

Three ordering facts that are not obvious and have each been re-derived the hard way:

- **Correlation runs before classification**, because phase 7 and the orphan-result phase both need the call
  reference. Correlating afterwards let two identical results answering two different calls collapse.
- **Sorting is a tuple key**: `(batch_time, message_index, entry_index, span, after_call, content_hash)`. A
  comparator with role-based cases is *cyclic* — intro text before its call by position, the call before a result
  by role, the result before the text by role — and `sort_by` without a total order may panic.
- **The role filter is last**, on the derived role, because a Gemini or ADK tool result arrives inside a `user`
  message and earlier stages read the blocks it would remove.

---

## 5. What landed, and why

**Forty-three code commits** after `4a9c30c9`; derive the count with §7's command rather than copying a commit
list here. The *reason* is what does not survive summarising, so it is kept.

### 5.1 Step 2 — the memory harness (`5a43a546`)

Four ceilings, declared once in `server/crates/core/src/constants.rs`:

| Ceiling | Constant | Measured on |
| --- | --- | --- |
| Idle RSS < 100 MB | `FOOTPRINT_IDLE_RSS_MAX_BYTES` | resident bytes, quiesced |
| Steady ingest RSS < 400 MB @ 5 000 spans/s | `FOOTPRINT_INGEST_RSS_MAX_BYTES` | resident bytes, window **median** |
| A 10 000-turn read leaves < 50 MB | `FOOTPRINT_SESSION_READ_GROWTH_MAX_BYTES` | **live allocated bytes** |
| A queued span < 3× decoded protobuf | `FOOTPRINT_QUEUED_SPAN_MAX_RATIO` | live allocated bytes |

**Live allocations, not RSS, for the leak gates.** glibc and jemalloc both retain freed pages, so an RSS-based
"returns to baseline" gate fails *correct* code and fails it differently depending on timing and allocator
version — worse than no gate, because it teaches people to rerun until it passes. RSS is reported beside it,
ungated; a large gap between the two is itself informative.

**The count is the program's, not the allocator's.** jemalloc publishes `stats.allocated`, which would have
avoided the `unsafe impl` and would have made the gate a statement about jemalloc rather than about SideSeat.
`runtime/allocation.rs` counts in a wrapper around the pinned backend.

That wrapper holds the **only `allow(unsafe_code)` outside a test module in the workspace**. `GlobalAlloc` has no
safe spelling: the trait is unsafe by definition, because a wrong implementation is memory unsafety.

**`LIVE` is one `AtomicI64`, not `allocated - freed`.** Two `Relaxed` atomics cannot be read consistently: a
reader may observe the newer free beside the older allocation and compute a figure that was never true,
understating by whatever was in flight — the direction that lets a gate pass *during* a regression. Reordering the
writes does not fix it; one counter removes the question. `i64` because a `dealloc` can be observed before its
`alloc`, so the value dips negative transiently and is clamped at zero.

**Pinned, and where it cannot be the gates say so.** `tikv-jemalloc-sys` does not build for Windows, so there the
system allocator stands in, `RESIDENT_CEILINGS_APPLY` is false, and the two RSS ceilings do not apply while the
live-allocation gates still do.

**Measured, first run** (debug, this host): a queued span **1.00×** its decoded protobuf against 3× (151 spans,
1.6 MB, 11 408 bytes/span); a 10 000-turn read leaves **0.0 MB** against 50 MB, peak 25.9 MB for 20 000 blocks.
**The two resident ceilings are unmeasured.**

### 5.2 Step 3 item 1 — the queue refuses instead of discarding (`9b00a0fc`)

The `MAXLEN` defect already removed from Redis was **still live in the default in-memory backend**.

```mermaid
stateDiagram-v2
    [*] --> Retained: publish, if the byte budget allows
    [*] --> Refused: BufferFull -> 503 + Retry-After
    Retained --> Pending: delivered to a group<br/>(the group cursor advances)
    Pending --> Consumed: ack
    Consumed --> [*]: trim_consumed — only below the<br/>oldest id ANY group still needs
```

The old behaviour: trim the deque front to fit a 100 000-entry count bound, **deleting the pending record too**.
Every such entry had been answered 200 by HTTP or gRPC.

Three changes, and the third is what makes the first two definable:

- **Bytes, not entries** (`STREAM_MAX_RETAINED_BYTES`, 128 MB). A count is the wrong unit for entries spanning
  four orders of magnitude. Each is charged its payload plus `STREAM_ENTRY_OVERHEAD_BYTES`, so one budget bounds
  the memory rather than only the payloads.
- **Refusal, not trimming.** Consumed entries reclaimed first, then `BufferFull`.
- **One cursor per group, not per consumer.** With a cursor per consumer "consumed" is undefinable — and it
  **delivered the same entry twice**: consumer A took entry 51 and acked it, so consumer B at cursor 50 found it
  undelivered and processed it again.

Later rounds added the bounds a byte budget cannot reach, because **group state grows at delivery, which cannot
refuse without stalling a consumer**: pending records charged (`STREAM_PENDING_RECORD_OVERHEAD_BYTES`),
`STREAM_MAX_CONSUMER_GROUPS` = 32 (production uses one, `trace_pipeline`), and `STREAM_MAX_REMEMBERED_CONSUMERS`
= 64 with LRU eviction — a client reconnecting under a fresh name otherwise added an entry per reconnect forever.

### 5.3 Step 3 items 7 and 8 — the read path (`79e025dc`)

**The cache held 512 entries whose size nobody bounded.** Its own comment argued an answer is the blocks a reader
sees rather than the megabytes behind them — true on average, false where it matters. Now weighed in bytes
(`RECONSTRUCTION_CACHE_MAX_BYTES`, 64 MB), measured by serialising into a counting sink, plus a flat per-block
charge, **plus the `#[serde(skip)]` fields measured directly** — those are unbounded, and a 1 MiB span name
weighed **911 bytes against an empty block's 911** until they were.
`every_unserialised_block_field_is_accounted_for` fails if a new skipped field is neither fixed-size nor measured.

**Every read deep-cloned the answer it had just found.** Now `Arc<FeedResult>` with `Cow` and projections
downstream — §4.

### 5.4 Step 3 items 9 and 5 — the engine's share, and the fan-out (`491df90b`, `749dd253`)

DuckDB's default `memory_limit` is 80% of physical RAM. **Measured with the setting removed: 25.5 GiB**, against a
declared 200 MB (`DUCKDB_MEMORY_LIMIT_BYTES`, half the ingest ceiling).

Stated rather than sold: `list()` and `first()` **cannot spill**, and the trace list builds its tag column with
`LIST_DISTINCT(FLATTEN(LIST(...)))`, so a trace with thousands of large tag arrays can raise an out-of-memory
error where the default would have completed. The trade is taken because an error names the limit while the
default makes the ceiling meaningless. The release HTTP gates have now run against both backend pairs: search and
the embedded large export pass, while the non-search latency misses recorded in §8 reject any claim that the
current host satisfies every latency ceiling.

The fan-out spawned one worker per core, so peak CPU-phase memory was *cores × the largest request*.
`byte_bounded_waves` groups requests into consecutive waves under `PIPELINE_CPU_PHASE_MAX_INFLIGHT_BYTES` (64 MB).
A request larger than the whole budget forms a wave of one, because refusing it would discard an export the queue
already accepted.

### 5.5 Step 4 — the three superlinear algorithms (`268c9b33`)

Each was a linear scan inside a per-block loop, so each was quadratic in what a long session has a lot of:

| File | Was | Now |
| --- | --- | --- |
| `feed/correlate.rs` | rescanned the pending call list per result *and* per claim | `by_id` + `by_name` index over the same document-ordered list |
| `feed/dedup.rs` | `iter().position(..)` over a growing `Vec<CallKey>` | `HashMap<CallKey, u32>` — the rank *is* the order of first appearance, which is the map's size at insertion |
| `feed/order_graph.rs` | rescanned all edges per unit; `contains` for adjacency; picked next by filtering every remaining member | bucketed edges, `HashSet`, Kahn's with a min-heap |

**The retired ordering implementation is kept under `#[cfg(test)]` as an equivalence oracle** — 512 generated
graphs per property, because a golden covers the shapes the corpus holds and the interesting edge sets here are
ones no framework produces. Mutation-verified: a max-heap fails both properties. This is the established pattern
for a rewrite claimed answer-preserving; 17 retired SQL tables are kept the same way.

### 5.6 Step 6, first half — the deletion journal (`14d99df4` + four review rounds)

Transactional schema **v5**, one append-only table, permanent, exempt from every sweep.

```mermaid
sequenceDiagram
    participant C as Caller
    participant T as Transactional store
    participant A as Analytics store
    participant R as Restore

    C->>T: record_deleted_traces_journalled
    Note over T: ONE TRANSACTION:<br/>tombstone + journal.<br/>Neither can exist without the other.
    C->>A: delete_traces
    A-->>C: 204
    Note over T: tombstone removed once quiet;<br/>the journal entry is permanent
    R->>T: deletions_since(cursor)
    R->>A: re-apply each deletion
    Note over R: a snapshot predating the deletion<br/>predates its tombstone too —<br/>only the journal survives that
```

**Why the tombstones cannot serve.** A tombstone stops a late *writer* and is removed once the target is provably
quiet. The journal is what a **restore** replays: restoring the analytics store further back than the
transactional one — which is what different backup cadences produce — brings back rows the caller was told 204
for. Its second consumer is step 9's staged-payload re-drive sweep, which without it cannot tell a failed write
from a deliberate deletion.

**The ordering question was the wrong question, and it took three rounds to see.** Both orderings are wrong:

| Order | What breaks |
| --- | --- |
| tombstone, then journal | a failed append leaves a deletion the sweeps perform anyway with no record — a restore undoes a deletion the caller was told succeeded |
| journal, then tombstone | a failed tombstone leaves a permanent record for a deletion the request reported as **failed** — a restore replays it and deletes the data |

Both rows live in the transactional store, so the window never had to exist. Four port methods do the pair in one
transaction:

```
record_deleted_traces_journalled            tombstone + journal
record_deleted_sessions_journalled          both tombstones + both journal scopes
claim_project_for_deletion_journalled       CAS claim + journal, only if the claim wins
claim_organization_for_deletion_journalled  the same
```

**The claims journal only when they win.** Journalling before the claim wrote an entry for every *losing*
caller — and an organization cleanup re-runs while its projects' tombstones remain, so one deletion accumulated
permanent, quota-counted records without bound. Conditional-on-winning is only expressible inside the claim's own
transaction.

Load-bearing details:

- `sequence` is the append order and is what a replay resumes from — **not the timestamp**: two entries can share
  a microsecond and a clock is not an order.
- `deletions_since` returns `(entries, highest_sequence_examined)`. A row whose cause or scope this build does not
  understand is skipped rather than guessed at, so a page of newer-version rows would otherwise return empty —
  indistinguishable from end-of-journal — and the replay would stop before the known deletions behind them.
- `CHECK ((scope = 'span') = (span_id IS NOT NULL))`, because the lookup matches on `span_id`: a span row with a
  null id can be appended and then never found, so the sweep recreates the very span the entry explains.
- **Age retention writes nothing.** It is a predicate, so a restored database recomputes the same verdict.
- The v5 migration **normalises** a stray `span_id` on a non-span row rather than deleting the row: that row is
  read correctly today, and deleting it would discard a real deletion record.

**Scope reduction, stated: nothing writes a `Pressure` entry yet.** Per evicted span is faithful and costs ~8 MB
of permanent, quota-counted rows per full `RETENTION_BATCH_SIZE` batch, forever. Per affected *trace* is cheap and
answers the wrong question — it would tell the re-drive sweep a span was deliberately evicted when only a sibling
was, so the sweep would decline to re-drive a payload whose write actually **failed**, which is loss. Omitting it
is the recoverable error: the sweep re-drives, the eviction repeats, and it stops at the cap with a report. The
enum variant and both `CHECK` constraints already admit the vocabulary, because the stored spelling must be
settled before any row is written under it.

### 5.7 Step 1 pieces that landed earlier

| Commit | What |
| --- | --- |
| `c73b6b27` | god-traits split into eleven ports; SQL taken out of `ports` |
| `d60d2bb1` | a partition key on publish, declared per signal. **Spans key on trace id and nothing else** — a session id lives on the span that knows it, so "session else trace" splits one conversation across two partitions mid-conversation |
| `9bfc26dc` | blob store, cache invalidation, secret writing as ports |
| `fedcafc3` | contiguous-offset acknowledgement (`server/crates/adapter-topics/src/ack_window.rs`). Kafka commits **offsets, not ids**: committing offset N asserts everything below N is done |

### 5.8 Steps 1 and 5–12 — the completed foundation (`93dc9a6b` through `cdec7ae0`)

The large storage-foundation commit completed the crate split, clock/project typing, signal lifecycle, durable
staging, logs, quota/holds, body storage, search and the shared typed query layer. The commits after it deliberately
split the externally risky parts into reviewable units:

| Area | Commits | Why it is separate |
| --- | --- | --- |
| Search completion | `b13a20aa` through `2269a448` | live ClickHouse syntax, index lowering, bounded fallback/backfill and the latency gate each have a different failure oracle |
| Durable queue | `334244ce`, `0d9fe719`, `2045f009` | queue selection, broker durability and weighted partition draining are independent contracts |
| Tenant isolation | `b2dec317`, `02f6f79e` through `29d6b8d5` | colliding client ids first; then PostgreSQL owner/runtime roles with forced RLS; then ClickHouse per-query row-policy context |
| Restore repair | `bb84ebed` through `cdec7ae0` | association repair precedes GC; journal and retention drain; quotas converge; missing-project analytics/staging/blob ownership is removed; the second pass is a fixed point |
| Operations | `a69759a1`, `4450d286` | a destructive embedded restore test and executable backup/restore scripts make the runbook falsifiable |
| Supply chain | `344d97a5` | rustls 0.23.45 removes the active TLS advisory without unrelated dependency churn |

The restore ordering is the important part:

1. refuse normal startup while `.restore-pending` exists;
2. replay the permanent deletion journal;
3. run age and quota retention to completion;
4. rebuild or release file/body ownership from surviving analytics rows;
5. remove analytics, staged rows and blob namespaces for projects absent from the transactional recovery point;
6. reconcile quotas and run the repair again to prove the state is stable;
7. remove the marker only after the report has been written successfully.

That procedure does not pretend the backups share a watermark. The three residuals in §11 remain the contract.

### 5.9 Verification closure (`19da9f7f`, `bd004a2c`)

Running the previously unexecuted distributed HTTP gate found two blockers before it could measure latency.
PostgreSQL promotes `SUM(BIGINT)` to `NUMERIC`, so fresh-project governance startup failed while decoding the
held-byte aggregate as `i64`; explicit `::bigint` casts plus a live empty-project regression close that boundary.
The pinned MinIO releases still existed in MinIO's Quay registry but no longer at their Docker Hub names, so the
benchmark now uses the same immutable release tags from Quay. The repaired gate reaches all 200-sample workloads;
its remaining failures are the measured latency gaps in §8, not startup or fixture failures.

---

## 6. What remains

### 6.1 Step dependencies

```mermaid
graph LR
    s1["1 restructure<br/>DONE"] --> s5["5 query layer<br/>DONE"]
    s1 --> s6["6 Signal + logs<br/>DONE"]
    s2["2 harness<br/>DONE"] --> s3["3 footprint<br/>DONE"]
    s4["4 algorithms<br/>DONE"]
    s6a["6a journal<br/>DONE"] --> s7["7 quota + hold<br/>DONE"]
    s6a --> s9["9 bodies + streaming<br/>DONE"]
    s5 --> s8["8 rollups<br/>REJECTED BY GATE"]
    s6 --> s7
    s6 --> s10["10 search<br/>DONE"]
    s9 --> s10
    s3 --> s9
    s7 --> s12["12 tenancy + backup<br/>DONE"]
    s10 --> s11["11 RedPanda<br/>DONE"]
    s6a --> s12
```

Two hard orderings: **step 10 needs step 9** (a ClickHouse text index must be defined over a column that still
exists after bodies move to blobs), and **step 7 needs step 6** (logs must exist before their retention and holds
can).

### 6.2 Step 1, complete

**Re-measure before changing structure.** Numeric inventories drift; the commands are given so you do not trust a
copied count:

```bash
grep -rc 'Utc::now()' server/src server/crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
grep -c 'project_id: &str' server/crates/ports/src/traits.rs
grep -c '?;' server/crates/adapter-topics/src/redis.rs   # the map_err cost of moving TopicError: 54, still exact
```


| Item | Size / where |
| --- | --- |
| `ProjectId` newtype | **DONE:** transactional, analytics, blob and registration ports use `ProjectId`; all tenant-scoped query DTOs own it; `every_tenant_scoped_port_uses_project_id` prevents a return to `String`/`&str`. The transparent serde representation remains a plain string. |
| `Clock` injection | **DONE for application policy and persisted timestamps:** auth, pricing, OTLP ingest/debug stamps, analytics writes/retention/stats, transactional repositories/migrations, secrets, rate limiting and WS registration TTLs use the injected clock. Direct `Utc::now()` sites are test fixtures or the concrete system-clock implementation; RedPanda's broker-lag helper uses wall-clock milliseconds as adapter telemetry rather than domain policy. |
| Extract `sideseat-domain` | **DONE as a Cargo boundary:** the normal tree names no storage/message driver and no `moka`; OTLP generated messages still bring `tonic/axum` transitively and remain the declared wire/domain coupling. |
| Extract `adapter-*`, `api`, `app` crates | **DONE:** every adapter and `sideseat-api` is a real workspace crate. The existing `sideseat-server` package is the app/composition root; production code there only wires ports, adapters, API and runtime concerns. Test-only compatibility namespaces remain under `cfg(test)` so existing parity and golden suites keep exercising the extracted crates. |
| API v1 breaks in place | no v2, no shim — fixed decision |
| Retention versus lag, **continuously** | Kafka retention can delete unacknowledged records during a long outage; a startup check does not cover it |
| Domain's remaining couplings | `TracePipeline` depends on domain `FileService` and typed topics, each backed by ports; no adapter type crosses the boundary. |

### 6.3 Steps 5–12: what each involves, and the smallest first increment

Read the design's own section before starting any of these. "First increment" is the smallest thing that is
reviewable and leaves the tree green.

| Step | What it involves | First increment |
| --- | --- | --- |
| **5** query layer | **DONE:** 46 registered operation groups cover the DuckDB/ClickHouse analytical repository surface, including search and restore project discovery, and the shared migration planner is live in all four DB adapters. Adapters bind, execute and decode; typed plans own statement structure, parameter order and backend capability differences. | Complete; details and gates in §6.4 |
| **6** Signal + logs | **DONE:** all three signals use the shared descriptor/lifecycle, are durably staged before acknowledgement, confirm their own digest, and discard only on a proven deletion or retention predicate. Logs are implemented through ingest, storage, reads, search and retention. | Complete; details in §6.5 |
| **7** quota + hold | **DONE:** unified logical-byte accounting, explicit admission refusal, bounded reclamation, maintenance reserve, project holds, leased convergence and hold-aware TTL/retention are live in both modes. | Complete; details in §6.6 |
| **8** rollups | **DONE as a gated rejection:** the complete DuckDB contribution implementation was built and correctness-tested, then release-benchmarked and reverted because it made the trace-list read slower at both fixture scales. The retained wide query now applies suppression only to winning revisions and has cost-only/re-delivery/deletion regressions. | Rejected by the required measurement; details in §6.14 |
| **9** bodies + streaming | **DONE:** body-level transactional ownership, dual-write/dual-read fallback, resumable backfill, exact cleanup and bounded HTTP JSON streaming are live; unchanged at-least-once deliveries no longer append duplicate analytics revisions. The old columns remain intentionally. | Complete; details and cutover gate in §6.12 |
| **10** search | **DONE:** ClickHouse 26.4.3.37, one domain tokeniser, capped per-field terms, DuckDB relations, ClickHouse text indexes, three-valued lowering, exact phrase verification, chronological API pagination, scan fallback, marker-checkpointed per-project backfill, range-level completeness reporting, and spans/logs live parity are implemented. Recall is 0.962 against the 0.950 floor; embedded HTTP search p95 is 92.5 ms against 100 ms. | Complete; details and gates in §6.15 |
| **11** RedPanda | **DONE:** queue selection is independent from cache selection; the `rdkafka` adapter provides keyed durable publish, consumer-group delivery, contiguous per-partition acknowledgement, broker-lag-versus-retention monitoring, DLQ and stats. Kafka-native rebalance replaces claim and trim is a no-op. `make test-redpanda`, CI, and a complete server-mode Compose stack are present. | Complete; details and gates in §6.15 |
| **12** tenancy + backup | **DONE:** colliding-id isolation, weighted queue draining, PostgreSQL forced RLS with separate runtime/maintenance roles, ClickHouse row policies with per-query context, guarded cross-store restore repair, destructive embedded restore proof, scripts and runbook. | Complete; details in §6.13 |

### 6.4 Step 5 in detail, because its starting point is not what it looks like

**Current result.** `sideseat-query-sql` owns all 46 registered analytical operation groups:
point/list/aggregate span, trace and session reads; feed and filter options; messages and project statistics;
membership, cleanup, row-count and ingestion-watermark reads; session/trace/span/project deletes; span and metric
write targets; and retention. DuckDB and ClickHouse adapters only bind, execute and decode those typed plans. The
registry drives a mutation-verified structural gate over every adapter entry point; operations whose SQL could
hide in helpers (`insert_batch`, statistics and retention) scan their complete production repository files.

The common migration state machine in `sideseat-core::migration` is used by DuckDB, ClickHouse, SQLite and
PostgreSQL, while each adapter retains its own DDL table and transaction mechanics. Live ClickHouse 26.4.3.37 parity
also exercises every migrated read: the shared dialect lowers winner selection to DuckDB windows versus
ClickHouse `FINAL`, preserves nullable result schemas and supplies equality keys for ClickHouse range joins.

Two different measurements, and the difference matters. The design says "~2 760 and ~2 990 lines of hand-written
SQL"; that counts the SQL *content*. The baseline whole-file measurement was 9 101 DuckDB lines against 4 691
ClickHouse lines. The final adapter executor files are 4 943 and 1 029 lines; their stats/message/retention helpers
are 2 293 and 439 lines. Shared typed construction is 4 110 lines of analytics, 721 of DML, 436 of messages and
876 of statistics. Line count is not the completion metric: the 45-entry registry and structural gate are.

The asymmetry is itself informative: DuckDB carries the `as_of_us` bound and the window-function deduplication
that ClickHouse gets from `FINAL`, so the sides are not two spellings of one implementation.

**`data/sql/` is not a partially-built query builder, and mistaking it for one would send you the wrong way.** It
is 756 lines across seven files, and `SqlDialect` is a *token-level* helper:

```rust
fn name(&self) -> &'static str;
fn placeholder(&self, index: usize) -> String;   // "?" vs "$1"
fn array_contains(&self, ...) -> String;         // array_contains(c,?) vs ? = ANY(c)
// ...
```

That is worth keeping and is **not** the seam the step needs. A builder has to own *statement structure* — which
relation, which predicates, which aggregate, which page — because that is where the 13 792 lines of duplication
live. The dialect becomes the thing the builder lowers *through*, not the thing that replaces it.

**Why it has zero consumers today** matters more than that it does: it was built bottom-up, as the facts two
engines differ on, and nothing was ever expressed in terms of it. A builder built the same way will end the same
way. Start from **one real read** and let its needs decide the vocabulary.

**Capabilities, not conditionals.** The differences that must survive as *declared* facts rather than `if
backend == ` branches:

| Capability | DuckDB | ClickHouse |
| --- | --- | --- |
| Deduplicate by version | `QUALIFY ROW_NUMBER()` over `(ingested_at, rowid)` | `FINAL` |
| `as_of_us` (read at a watermark) | **yes** — the bound goes *inside* the deduplication | **no** — `FINAL` has no "as of" form, and a merge may already have removed the earlier version |
| Delete | `DELETE`, transactional | `ALTER … DELETE` with `AWAIT_MUTATION` and `mutations_sync = 2`, on `_local` with `ON CLUSTER` |
| Write target | the table | the `Distributed` table, with `insert_distributed_sync = 1` |
| Correlated subquery | fine | **silently wrong** — correlated `NOT EXISTS` always returns true; use a materialised CTE plus a tuple `NOT IN` |

**Suggested order**, smallest blast radius first: an `EntityQuery` read with no aggregate → a read with a filter →
the trace list with its two-stage aggregate (the hardest, because a trace-list filter selects *traces*, never span
rows) → deletes → upserts. Each group parity-gated before the next.

**The invariant arrives with the step, scoped by the builder's own registry.** `no_adapter_holds_a_sql_literal`
must read which operation groups have been migrated from the builder itself, so it tightens automatically as
groups land. A grandfathering list of exempt files is the failure mode: it can always be appended to, and then
the gate measures nothing.

### 6.5 Step 6 in detail — the six handlers, and the predicate that is easy to get wrong

**Current result.** `server/crates/ingestion/src/signals.rs` declares the common signal descriptor and lifecycle for spans,
metrics and logs; HTTP and gRPC use the same signal-owned extraction, project injection, queue key, partial-success
shape and durability decision. The compiler owns the shape and source tests prevent either transport from growing
a second decision tree.

Every accepted sub-batch is written to staged blob storage before the response can succeed. Confirmation is strict
equality on that delivery's content digest, not identity and not `ingested_at`; a byte-identical retry confirms,
while a correction with the same stable identity does not borrow an older row's success. The default five-attempt
re-drive cap bounds work, and exhaustion marks the registry row unconfirmed while retaining its bytes. A project
fence, deletion journal entry or age predicate makes deliberate absence terminal and releases the payload.

Logs are complete end to end: digest-plus-ordinal identity, DuckDB/ClickHouse storage, filters, API/SDK types,
retention, deletion, staging confirmation, search and parity. Their ordering remains log-shaped:
`(event time, digest, ordinal)`, without requiring a trace id.

### 6.6 Step 7 in detail — the four TTLs that delete held data today

**Current result.** `StorageGovernanceService` is the one inward-facing owner of admission, reconciliation, holds
and the per-project maintenance lease. Logical bytes cover analytics rows, transactional ownership, journal
evidence, staged payloads and bodies. Ordinary writes are explicitly refused at the measured limit; the default is
1 GiB per project. Reclamation is bounded and oldest-first, then usage is replaced from independently measured
stores rather than trusted as an exact counter.

The maintenance reserve is 28.8 MB: one maximum pre-delete batch of journal evidence plus cleanup candidates.
Maintenance may consume that reserve but may not exceed the configured quota, so reaching the ordinary-write limit
does not make the deletion needed to escape it impossible.

A hold is recorded under the project maintenance lease, patched into all three analytics signals, rechecked for
in-flight writers and converged by a leased sweep. DuckDB retention and ClickHouse mutations take the same lease;
ClickHouse TTLs use `greatest(retention expiry, hold_until)`. The accepted boundary remains exact: a hold protects
rows still present when its patch reaches them; a merge or orphaned mutation that removed a row before that point
cannot be undone.

### 6.12 Step 9 in detail — what "three representations" actually means

**Current working-tree result.** `messages`, `tool_definitions`, `tool_names` and `raw_span` are stored as
domain-separated content-addressed objects with project-scoped body ownership in SQLite/PostgreSQL. Ingestion
registers bytes, stages provisional field associations, writes analytics, then confirms only the current winner;
failure keeps the inline analytics columns authoritative. Readers prefer durable bodies, deduplicate object GETs,
parse each distinct message body once, and fall back inline on any registry/blob failure. Retention and explicit
deletion reconcile exact surviving references before body GC; losing-winner churn is reclaimed by a five-minute
grace-period sweeper rather than delete/write thrashing.

Backfill is identity-ordered, page-bounded and resumable per project. The per-project cutover criterion is
`content_body_backfill.complete = true` after the cursor reaches an empty page; any failed dual-write or
confirmation resets that project to incomplete. Dropping the old columns is deliberately a separate migration
and may begin only when every live project satisfies that criterion and a release-level verification finds no
reset/incomplete project. This step does not drop them.

HTTP message/feed responses now stream their top-level arrays item by item with no `Content-Length`; MCP retains
its protocol-required materialised JSON value. The 60-second footprint gate uses a paced, minimal 500-span OTLP
rate fixture and passes at ~4,958 spans/s: idle RSS 91.8 MB, steady median RSS 166.4 MB. The 10,000-turn read leaves
0.0 MB live residue (25.9 MB peak), and queued protobuf retention is 1.00× against the 3.0× ceiling.

Today one span's content exists in three forms, and a reader should check this before assuming:

| Form | Where | Purpose |
| --- | --- | --- |
| Extracted columns | `otel_spans` — model, tokens, costs, previews, status | filtering, aggregation, list rows |
| `messages` JSON | `otel_spans.messages` | what the feed pipeline parses on every read |
| `raw_span` JSON | `otel_spans.raw_span` | served only for `?include_raw_span=true` |

**`raw_span` is the easy win**: it is read by one query parameter and is the largest of the three for an
attribute-heavy span. Moving it to the blob store, fetched only when asked, is a physical-layout change with no
interpretation.

**`messages` is the hard one**, and the reason is in §4.1. Content-addressing it by bytes alone **cannot** collapse
a replay: a re-sent tool call arrives with a *regenerated call id*, so the bytes differ — and today's dedup handles
that deliberately, because a tool call's identity **ignores** the provider id. So a reference encoding only body
equality is not a substitute for the pipeline, and any write-time key the pipeline would depend on freezes a
decision the design keeps at query time so fixes apply to history.

What content-addressing *does* buy: **not re-fetching and re-parsing identical bytes.** For a replaying framework
the same conversation arrives once per turn, so the verbatim bulk is highly duplicated — that is the 68 MB fetch a
1 000-turn session pays today.

**The blob reference convention already exists**: `#!B64!#<mime>::<hash>` inside stored content, with the
association protocol of §2.3 keeping the bytes alive. Bodies shared across traces will need a **body-level**
reference count, because the trace-keyed association cannot express them.

**Do not reduce the cache key to the body-reference list.** It deliberately includes timestamps, status, costs, the
trace→session grouping and the ruleset digest — every one of which changes the answer while leaving bodies
identical. Keying on references alone serves stale answers the goldens cannot catch, because a cache hit is
invisible to them. What *can* be cheapened is hashing the body **hashes** instead of the body bytes.

### 6.13 Step 12 in detail — tenancy starts from zero

**Current result.** Isolation now has all three intended layers:

1. tenant-scoped ports use the `ProjectId` newtype and typed query construction owns parameter order;
2. the SQL registry structurally scans each migrated operation's complete production module;
3. both production stores enforce a fail-closed row-level policy.

PostgreSQL migrations run as the schema/maintenance role, ordinary requests use a non-owner runtime role, and all
protected tables carry `ENABLE` plus `FORCE ROW LEVEL SECURITY`. Tenant operations open a transaction and set the
project with `SET LOCAL`; global retention, restore and migration work uses the explicit maintenance path. The live
parity suite proves both halves: a valid context still finds its own row when the application predicate is absent,
and an unset context returns none.

ClickHouse policies read a custom project setting attached to each query, never a session-persistent `SET`.
Maintenance uses an explicit bypass setting, and replicated schemas install the policy on the local tables where
the rows live. The single-node live suite proves per-query isolation and fail-closed behaviour; the replicated
migration fixture proves policy DDL targets the configured database.

The colliding-id gate seeds two tenants with identical trace ids, session ids and content hashes and exercises
analytics plus blob ownership. Queue workers drain partitions with bounded weighted fairness so one hot tenant
cannot starve all others while preserving in-partition order.

Restore is now an offline command guarded by `.restore-pending`. It replays the journal, drains deterministic
retention, repairs associations before GC, reports missing content, removes analytics/staging/blob state for
projects absent from the transactional recovery point, reconciles quota and reaches a fixed point. The executable
embedded scripts checkpoint and checksum SQLite/DuckDB plus blobs; the published runbook covers PostgreSQL PITR,
ClickHouse S3 backups, object-versioned blobs, RedPanda replication/tiered storage and tenant-selective restore.
The three residuals in §11 are unchanged rather than hidden.

### 6.14 Step 8 in detail — and the honest version of its win

**Current working-tree result: measured and reverted, as this section requires.** The complete DuckDB-only
`trace_contrib` path was implemented first: v5→v6 backfill, source-winner conditional replacement in the span
transaction, hold/quota/deletion integration, narrow trace/session reads, root-metadata/tag/preview preservation,
and regressions for clock-regressed delivery, cost-only child suppression and child deletion reactivating its
parent. An interleaved release benchmark then compared the old wide second stage with the contribution path:

| Fixture | Wide source median | Contribution median | Result |
| --- | ---: | ---: | ---: |
| 400 traces × 5 spans | 10.16 ms | 16.00 ms | contribution was **1.58× slower** |
| 4,000 traces × 5 spans | 14.93 ms | 27.44 ms | contribution was **1.84× slower** |

The optimisation was therefore removed in full: no table, migration, write amplification, quota accounting or
read dependency remains. The investigation did expose and fix a correctness defect worth retaining independently:
DuckDB's billing-suppression subqueries used the raw append-only table, so a superseded billed child could keep
suppressing its parent. They now use the same winning-span relation as the aggregate. Tests pin that corrected
unbilled re-delivery reactivates the parent, a cost-only billed child suppresses it, and deleting the child
reactivates it.

**The win is narrower than it first looks, which is why the step is gated by a measurement rather than an argument.**
`list_traces` already paginates trace ids *before* aggregating, so the win is **not** "stop aggregating over
everything". It is exactly this: the second stage reads a **narrow, purpose-built table** instead of the wide span
table with its JSON columns, for the same set of traces. The relational suppression and the version selection are
**retained, not removed** — they cost what they cost. Against that, one row per span plus the term rows is added
write work and added storage. So the step lands with a before/after measurement on the existing trace-list
benchmark at both fixture scales, and **if the narrow read does not pay for the write amplification the step is
reverted rather than argued for**.

**The aggregate is not a plain `SUM`, because billing dedup is relational.** Today a generation parent is suppressed
by a billed generation child, and a non-generation span by any generation in the trace or by a billed parent
(`gen_totals_sql` in both adapters' `query.rs`). Summing contributions would double-count a parent and child
reporting the same 100 tokens, and deleting the child would have to *reactivate* the parent. So the contribution row
carries `parent_span_id` and `observation_type`, and the rollup applies the **same suppression rule** over
contributions. The existing billing-dedup tests are the acceptance criteria.

**`is_billed` is `tokens > 0 OR cost > 0`**, which is what both adapters already mean by billed. A draft said
"tokens > 0", and the consequence is exact: a **cost-only** child would fail to suppress its parent locally while
ClickHouse continued to suppress it, so the two backends would report different totals for identical data. The
parity corpus has a cost-only trace but **not** a cost-only parent/child suppression case — so nothing would have
caught it. That case is required by this step.

**Replace-on-write is conditional on winning by the *source's* rule, not on committing last.** The surviving span is
chosen by `ingested_at` then `rowid`, so a delivery B that commits *after* A but carries a clock-regressed
`ingested_at` **loses** in the source relation — while a naive replace-on-write would leave the derived table showing
B. The result is a rollup that disagrees with the spans it summarises. So the derived write compares against the
stored version inside its own transaction and replaces only if this delivery would also win in the source relation.
**This is structurally a DuckDB-only concern**: ClickHouse has no `rowid`, its winner is settled by a merge plus
`FINAL`, and it has no derived rows to disagree.

**A trace row needs more than contributions**, so the trace query is two-stage: contributions supply counts, sums
and time bounds; `display_*` and `tags` supply the rest from the same narrow row — `tags` as a per-span array
**unioned** across contributions, because a union genuinely needs every selected span. **Root metadata returns NULL
when a trace has no root**, which is what today's queries return; falling back to the earliest span is arguably
nicer and is a *behaviour change* needing its own goldens.

### 6.15 Steps 10 and 11 — the parts most likely to be got wrong

**Current Step 10 working-tree result.** The implementation uses one Unicode-alphanumeric, lowercase domain
tokeniser and a 512-distinct-term cap per field. DuckDB writes `span_terms` / `log_terms` in the same transaction
as each analytics record; ClickHouse 26.4.3.37 stores the same capped arrays behind native `text(tokenizer=array)`
indexes. The typed query layer carries false/unknown/true as 0/1/2 through nested AND, OR and NOT, while phrases
are verified against domain-reconstructed bodies. The API cursor advances over the last examined candidate,
including an empty page, and arrival detection uses a store-derived traversal watermark.

Live parity covers tied span and log ordering, multi-page and empty-page cursor progression, role-restricted
phrases, nested negation over truncated fields, current corrections, and legacy rows without index markers.
Legacy rows degrade to a bounded source scan rather than disappearing; `search_indexing_complete` is false when
any current row in the requested time range lacks the complete marker, not merely when the current page happens
to examine one. A 16-identity-per-project background page uses the index marker itself as the durable checkpoint
for both signals. Span writes compare the observed `content_digest` and require the row to remain unindexed, so
a correction racing the backfill cannot have its new terms replaced by stale source text.

The release write-amplification fixture measured 532 spans and 62,052 term rows: 116.6 rows/span,
5,927 logical term bytes/span and 5,913 physical bytes/span. Ingest changed from 71.4 ms without terms to
5.370 s with them in this deliberately row-at-a-time measurement. Recall was 0.962 against the fixed 0.950
floor. These are recorded costs, not a throughput claim; the remaining Step 10 gates are the resumable historical
backfill and search latency, closed below.

Both remaining gates are now closed. The search adapter entered the typed-SQL registry as its 45th operation
group, and the registry scans the complete production module so candidate, completeness, arrival, term-maintenance
and backfill SQL cannot hide in helpers. The first HTTP measurement exposed two per-candidate reads and produced
519.3 ms p95. Returning the lightweight record, verification source and index marker in the candidate query
removed that N+1 path; the standard 200-sample embedded run measured **92.5 ms p95 against a 100 ms ceiling**
(78.6 ms p50, 104.1 ms p99).

**Step 10's three-valued logic is the part to get right first.** A truncated `(span, field)` is *unknown*, not false,
and collapsing unknown to false at the leaf is unsound **under nesting**: in `NOT (A OR B)` a capped `B` becomes
false, the disjunction false, and the negation returns the span as a **positive** match it should not be.

| Leaf | Value |
| --- | --- |
| term present in the index | **true**, truncated or not |
| term absent, `(span, field)` complete | **false** |
| term absent, `(span, field)` truncated | **unknown** |
| phrase: conjunction present, body verifies | **true** |
| phrase: conjunction present, body refutes | **false** — the body is complete evidence even when the *index* was capped |
| phrase: body unavailable | **unknown** |

So the relational lowering carries **two relations per clause, not one** — matched and unknown — because
intersection, union and anti-join over a single relation have nowhere to put a third value. A record whose overall
value is unknown is **returned, marked indeterminate**: omitted would read as "does not match", silently included as
"matches".

**And a negated *phrase* cannot use the term anti-join at all.** `NOT "foo bar"` lowered as "anti-join the
conjunction of `foo` and `bar`" removes a span containing `foo x bar` — which does not contain the phrase and should
match. A negated phrase selects candidates by the conjunction and excludes only those the verifier confirms.

**The cursor is the last *examined* position, not the last returned one**, and getting this wrong is pagination
livelock rather than a short page: return A, then hit a bound-sized run of candidates that verification rejects,
then B — with a returned-item cursor every subsequent request resumes after A, re-examines the same rejected run,
exhausts the same bound and returns nothing, so **B is unreachable forever**. The response carries the cursor **even
when the page is empty**.

**Per field, not one combined array.** A single array makes `prompt:foo` and `completion:foo` inspect the same
values, so `prompt:foo AND completion:foo` returns spans where `foo` occurs only in the prompt — a *wrong* answer,
not a missing one, and per-clause lowering cannot repair it.

**Step 11 result.** `QueueBackendType` and `QueueConfig` are independent from the cache while preserving the old
implicit cache→queue choice only when no queue setting is supplied. The RedPanda adapter uses `rdkafka` 0.39.0,
idempotent `acks=all` production and the signal-owned partition key. Received ids encode partition and offset;
`AckWindow` commits only the highest contiguous completed prefix, so a later success cannot acknowledge an earlier
failure. `stream_claim` returns no records because group rebalance reassigns abandoned partitions, and
`stream_trim_consumed` uses the port's no-op default.

Retention is checked continuously rather than only at startup. Every 30 seconds the adapter compares each live
consumer group's committed offsets with broker timestamp offsets at `retention_ms - retention_warning_ms`; local
delivered-but-uncommitted timestamps cover the same interval between broker probes. Approaching the boundary logs
an error and fails queue health. The adapter also exposes broker-derived length/lag statistics and preserves
undecodable records in a dedicated dead-letter topic.

`make test-redpanda` starts pinned `v26.2.3` and proves same-key partition stability, a deliberately out-of-order
ack that does not move the group offset past its gap, gap closure, final zero lag, and Kafka-native claim/trim
semantics. CI runs that target. `deploy/local/docker-compose.yml` now starts SideSeat itself plus PostgreSQL,
ClickHouse, Valkey, RedPanda and Vault; its checked config selects Redis for cache and RedPanda for queue.

### 6.7 Review state

Four Codex rounds have run against this batch: **8, 11, 8 and 6 findings — 33 in total, every one real.** Each
round after the first found defects in the previous round's *fixes*. Round four's six are all fixed (`0dcdd767`);
what they were is worth knowing, because the pattern repeats:

| # | Finding | Fix |
| --- | --- | --- |
| 1 | The v5 migration **deleted valid evidence** — only `scope='span' AND span_id IS NULL` is inert | normalise the column, delete only the inert shape |
| 2 | The late-session sweeper journalled and tombstoned in separate transactions | the combined method |
| 3 | `pipeline.rs` tombstoned without journalling. I argued the session's own entry covers them; **wrong** — a restore holding child spans but not the session-bearing root cannot resolve the trace, so it is resurrected | both sites journalled |
| 4 | PostgreSQL held `ACCESS EXCLUSIVE` through `VALIDATE`, because both ran in the migration's transaction | `NOT VALID` with no `VALIDATE`: the migration's own statements make every row conform |
| 5 | `footprint-gates.sh` cleanup did an unbounded `wait` after SIGTERM | bounded wait, then SIGKILL |
| 6 | The allocation test had **no deterministic floor** — a concurrent free of any size can zero `growth_since` | assert `churn_since`, which reads the monotone `ALLOCATED` alone |

Those four rounds are historical evidence, not the final review count. Later implementation was split into the
commits in §5.8 and reviewed with focused parity, restore and policy gates; a fresh full-range review is still a
useful follow-on, but it is no longer an implementation prerequisite.

### 6.8 How to know a step is done

Baseline for every step: **the 121 committed goldens and both parity suites byte-identical on the far side**,
unless the step is *meant* to change an answer, in which case the changed goldens are reviewed as a diff rather
than regenerated blindly. (`UPDATE_GOLDENS=1` writes the files but still exits non-zero when an invariant was
violated, so known-bad output cannot be committed as reviewed.) On top of that:

| Step | Its own criterion |
| --- | --- |
| 1 | `cargo tree -p sideseat-domain` names no driver |
| 3 | each mechanism measured, not argued; `bench-http` still inside its ceilings |
| 5 | per group: both parity suites identical, and the raw-SQL invariant's scope grows by that group |
| 6 | each signal's confirmation predicate tested across a hold patch **and** a byte-identical retry — they pull in opposite directions |
| 7 | ingest past the quota: refusal explicit, nothing silently dropped, reclaimable space first, held bytes survive, usage converges on `SUM(logical_bytes)`. Plus a {signal} × {removal mechanism} hold-survival matrix, **two cases concurrent** — a writer admitted before the fence, and a retention pass already running — because a sequential matrix passes while held data is still deleted |
| 8 | the before/after trace-list measurement decides it; revert if it does not pay |
| 10 | membership parity **and** ordering/pagination parity (cursors, ties, empty-page advancement, nested negated and truncated clauses), plus write amplification against a recall floor |
| 12 | backup → destroy → restore → repair: every surviving read correct, every unrepairable state *reported*, a restored blob with a missing association rebuilt before the GC could take it, and replaying the journal plus a completed retention pass leaves no resurrected row |

### 6.9 Fixed decisions — do not re-litigate

- **Ports and adapters as Cargo workspace crates.** Not a source-scanning lint (§1.1).
- **No framework knowledge in Rust.** Adding a framework is a new file in `server/assets/rules/producers/`.
- **Footprint ceilings are local numbers**: idle < 100 MB, ingest < 400 MB. Own measurements only.
- **Search is filter-plus-chronological.** No ranking, no new services, no new dependencies. **Score is not in the
  API at all** — not exposed, not filterable, not asserted. Free now, unretrofittable once a score reaches an SDK.
- **API v1 breaks in place.** No v2, no shim.
- **The audit layer is out of scope** (design §6): the Merkle log, COSE receipts, signatures, checkpoints and the
  offline verifier are the *product* on top of this. The foundation must not foreclose it, and does not.
- **The storage quota is best-effort, per project, enforced by refusal.** Eleven review cycles tried to make it
  exact; six mechanisms died on §1.3. It is not exact at any instant and says so.

§11 is the longer form of this list: every accepted limit, with what would have to change to lift it.

### 6.10 Decisions that were open and are now fixed

| Decision | Landed value |
| --- | --- |
| Search tokenisation | one domain Unicode-alphanumeric lowercase tokeniser; both backends store its output |
| Storage quota | 1 GiB per project by default; oldest reclaimable data first; periodic and post-mutation reconciliation; 28.8 MB maintenance reserve |
| Search cap and floor | 512 distinct terms per field; 0.950 minimum recall, measured at 0.962 |
| Staging re-drive | five failed attempts by default; exhaustion reports and holds the payload rather than releasing it |

Four more belong to the audit layer's own plan rather than here: canonicalisation (JCS + COSE, or deterministic
CBOR), signing granularity, whether to ship witness co-signing, and key custody across replicas.

### 6.11 Known-unresolved bugs

Not findings from review — things that are simply not understood yet:

- **`make test-clickhouse-two-shard` has a historically slow fixture path.** Prior runs took ~15 minutes because a
  third distributed-DDL replica entry named `localhost:9000` sat beside the two correct `ch-shardN:9000` entries
  and triggered the hardcoded 90 s `markReplicasActive` wait. The latest 1-test run finished in 3.09 s, so the
  delay did not reproduce; its cause is still not understood well enough to call fixed.
- **Five non-search `make bench-http` ceilings are breached** on the development host. Search and the large export
  pass. Earlier measurements already missed the 2 KB export and concurrent session read before search existed;
  the latest run also misses sequential session messages, trace list and cold session read. What remains
  unresolved is how much is host load versus cumulative regression; settle it on an idle host or with a buildable
  baseline bisect.
- **Three `make bench-http-distributed` ceilings are breached** on the same host: small export p95 86.3 ms against
  80 ms, large export 171.1 ms against 120 ms, and 8-concurrent session messages 2,193.4 ms against 1,500 ms.
  Sequential session messages, trace list and search complete within that target's ceilings.
- **Cross-replica ClickHouse convergence is unverified** — it needs a second replica, which no fixture provides.

---

## 7. Exact current state

```bash
git log --oneline 4a9c30c9..HEAD          # everything since the baseline
git log --oneline 4a9c30c9..HEAD -- ':!PLAN.md'   # the code commits only
```

The list is not reproduced here, because it drifts every time this file is edited and a stale list is worse than
no list. The command currently returns **45 code commits**. §5 names the load-bearing groups and their rationale.

**Working tree:** the implementation and verification-harness repairs are committed through `bd004a2c`.
`CLAUDE.md` is modified by the user and remains deliberately uncommitted. Step 1 and Steps 5–12 are complete;
Step 8 was rejected by its required benchmark and removed. The only remaining work in this file is verification
or an explicitly accepted operational limit, not an unfinished implementation step.

## 8. Verification state — read before claiming anything works

**Current verification evidence:**

| Suite | Result |
| --- | --- |
| `cargo check --locked --workspace --all-targets` | passes with rustls 0.23.45 |
| strict clippy for the server and every changed adapter/domain/query crate | passes with `-D warnings` |
| `cargo test --locked -p sideseat-server --lib` | 89 passed, 4 ignored; most former server tests now live with their extracted crates |
| `cargo test --locked -p sideseat-query-sql` | 65 passed, including the 46-operation typed registry |
| `cargo test --locked -p sideseat-domain restore::tests` | 3 passed: journal replay, association/body GC fixed point, and analytics/metadata/staging-only project removal |
| `cargo test --locked -p sideseat-core` | 267 tests plus doctests pass |
| `cargo test --locked -p sideseat-server --test repository` | 26 of 28 pass; the two failures name only stale paths in the user's uncommitted `CLAUDE.md`, not `PLAN.md` or production code |
| `make footprint` | **all four gates pass:** idle 91.8 MB; steady ingest median 166.4 MB at ~4,958 spans/s; 10k-turn residue 0.0 MB; queued payload 1.00× decoded protobuf |
| `make test-postgres` | **43 passed** — including fresh-project governance aggregate decoding, PostgreSQL/SQLite parity, role separation, forced fail-closed RLS and transactional tenant context |
| `make test-clickhouse` | **26 passed** on ClickHouse 26.4.3.37 — including row policies and span-only, metric-only and log-only restore project discovery |
| `make test-clickhouse-replicated` | **3 passed** on the dedicated one-shard replicated fixture — fresh migration, interrupted migration resume and configured-database tenant policy |
| `make test-clickhouse-two-shard` | **1 passed** — the distributed consistency check finds anomalies and legacy rows across both shards |
| `make test-backup-restore` | destructive checkpoint → independent restore → repair test passes; the second repair is a fixed point |
| release search write-amplification gate | **passes:** 0.962 recall against 0.950; 62,052 rows for 532 spans, 5,913 physical bytes/span |
| embedded HTTP search gate | **passes:** 200 samples, p50 78.6 ms, p95 92.5 ms against 100 ms, p99 104.1 ms |
| `make bench-http-distributed` | completes the 200-sample PostgreSQL + ClickHouse + S3-compatible path; three latency ceilings fail, listed below |
| `make test-redis` | **15 passed** — durable queue refusal, reclaim, acknowledgement and trim cases |
| `make test-redpanda` | **passes live** on pinned RedPanda v26.2.3 — keyed partitioning, contiguous commits, zero final lag, claim/trim semantics |
| backup/restore operational checks | both scripts pass `bash -n` and ShellCheck; a real embedded smoke restore passes; the docs build publishes 45 pages |
| `cargo deny check advisories` | passes after the rustls update |
| web / Python SDK | 93 and 203 passed |

`make check` is not green in the current dirty tree because its repository phase sees those same two stale
`CLAUDE.md` path groups. All production-code phases and the other 26 structural invariants pass independently;
the file is user-owned and was not changed to manufacture a green aggregate result.

All opt-in backend/topology commands listed for the final restore state have now run. The distributed HTTP run
first exposed and then verified fixes for PostgreSQL `SUM(BIGINT)` decoding and dead MinIO Docker Hub pins.
Full `cargo deny check` still reports the pre-existing wildcard path dependency used by the public
`sideseat-ports` crate and two unmatched-license warnings; the advisory check itself is green.

**Run, still failing on non-search latency ceilings:**

```
make bench-http
    trace export 2 KB: p95 16.9 ms against 10 ms
    session messages, sequential: p95 96.2 ms against 40 ms
    session messages, 8 concurrent: p95 473.2 ms against 150 ms
    trace list: p95 44.7 ms against 40 ms
    cold session read: 104.9 ms against 60 ms
    search: p95 92.5 ms against 100 ms — passes
    large export: p95 82.8 ms against 100 ms — passes

make bench-http-distributed
    trace export 2 KB: p95 86.3 ms against 80 ms
    trace export 754 KB: p95 171.1 ms against 120 ms
    session messages, sequential: p95 381.4 ms — passes
    session messages, 8 concurrent: p95 2,193.4 ms against 1,500 ms
    trace list: p95 79.0 ms — passes
    search: p95 57.5 ms — passes
    cold session read: 393.9 ms
```

Two historical caveats remain useful when interpreting this host-sensitive suite: `CLAUDE.md` already documented
breaches for the 2 KB export and 8-concurrent session read before search existed, while this latest run also
missed the sequential, list and cold-read ceilings. Search itself passed its new gate. Separately,
`make test-clickhouse-two-shard` has historically taken ~15 minutes because of an unresolved fixture problem — a
third distributed-DDL replica entry named `localhost:9000` that nobody owns. The latest run did not reproduce the
delay and passed in seconds; that is evidence the target works, not evidence that the intermittent wait is fixed.

---

## 9. Start here

The foundation plan is implemented. There is no next numbered implementation step.

1. Run `git status --short`, preserve the user-owned `CLAUDE.md` edit, then run the checks in §8 appropriate to
   the area being changed.
2. For release confidence, rerun `make footprint`, both HTTP benchmark modes, `make test-clickhouse-replicated`
   and `make test-clickhouse-two-shard`; interpret the known latency and fixture caveats first.
3. Choose a follow-on explicitly: resolve a §6.11 operational issue, improve the opt-in topology fixtures, or
   start a separate audit-layer plan. Do not reopen Step 8 without a new measurement that changes its rejected
   result.

---

## 10. Working protocol

**Every fix is mutation-verified**: write the test, revert *that fix alone*, watch it fail, restore. This caught
four cases in this batch where a test could not have failed — including one where the mutation silently did not
apply, because `cargo fmt` had joined the two lines it matched, and the passing suite was the mutation's absence
rather than the test's strength. **Check that the mutation applied.**

**The recurring defect class here is a gate that passes while seeing less than it claims.** Five of round one's
eight findings were exactly that. Concrete forms seen in this batch:

- a test whose assertion *string* contained the token it searched the source for, so it could never fail;
- a window anchored such that the regression sat outside it;
- a gate that returned `Ok` when it had no input;
- a bound expressed in the wrong unit (entries, where the resource is bytes);
- a test reconstructing prior state by subtracting from the *current* schema.

**Codex review** — the outer loop:

```bash
codex exec --sandbox read-only "$(cat /tmp/prompt.txt)" < /dev/null > /tmp/out.txt 2>&1
```

`codex mcp-server` does not exist in 0.154.0 and `gpt-5.1-codex-max` 404s; the configured model in
`~/.codex/config.toml` is `openai.gpt-5.6-sol` via Bedrock. **stdin must be closed** or it blocks. Write the
prompt to a file — the useful ones are long. What makes a round productive:

1. Name the commits and the files that matter.
2. List this repository's known defect classes (above) and ask it to hunt those specifically.
3. Demand a **concrete failing scenario** per finding — inputs to wrong output — so speculation is separable.
4. Ask explicitly **which of the previous round's findings are now correctly fixed**. That question has found four
   non-fixes.

### 10.1 The measurements that are not in `make check`

All `#[ignore]`d, because each takes tens of seconds and a debug build's numbers describe the debug build:

```bash
cargo test --locked --release -p sideseat-server bench_ingestion -- --ignored --nocapture
    # the real run_batch: extraction, enrichment, file storage, both writes, the project fence.
    # BENCH=<suite>/<sample> selects a fixture, ITERATIONS=n the run count.

cargo test --locked --release -p sideseat-server bench_session_scaling -- --ignored --nocapture
    # how reconstruction scales, in the two shapes that differ: incremental (each span carries its
    # own turn) and replaying (each generation span re-sends the whole conversation).

cargo test --locked --release -p sideseat-server bench_session_membership -- --ignored --nocapture
    # the session-membership formulations INTERLEAVED in one process, so the ratio is meaningful on a
    # contended host. Asserts they select the same trace ids before timing them — a count would have
    # let a faster wrong answer pass.

cargo test --locked --release -p sideseat-server bench_pipeline -- --ignored --nocapture
    # CPU only, stopping before file extraction and every persistence step. A floor, not a request's cost.

cargo test --locked --release -p sideseat-server --test footprint -- --ignored --nocapture
    # the two live-allocation gates. Serialises itself; see §5.1.

cargo test --locked --release -p sideseat-server --test footprint \
    search_term_write_amplification_preserves_the_recall_floor \
    -- --ignored --exact --nocapture --test-threads=1
    # Search term rows/span, logical and physical bytes/span, ingest cost, and recall against the floor.
```

The published numbers for the first two are tables in `CLAUDE.md`. `make bench-http` is different in kind — it
**enforces** its ceilings and exits non-zero on a miss.

### 10.2 A Codex prompt that works

The four elements from above, as a template. The specifics matter: a vague prompt returns style notes, and this
shape has returned 33 real findings across four rounds.

```
Review <commits> with `git show`. Read <design file> sections <n> for the intended contract.

The commits, oldest first:
  <sha>  <one line each>

Files that matter most:
  <explicit list>

Hunt specifically for this repository's recurring defect classes, which have accounted for most real findings:
  1. A gate that passes while seeing less than it claims - a test whose assertion cannot fail, or that would
     pass with the fix reverted, or that reconstructs prior state by subtracting from current state.
  2. A mechanism whose stated bound is not a bound - a limit in the wrong unit, a counter that can drift,
     an "eventually" with no bound.
  3. A refactor claimed answer-preserving that changes an answer on an input the corpus does not contain.
  4. Two definitions of one fact that can disagree.

Concrete questions I want answered, with evidence from the code:
  <one per mechanism, naming the function and the property>

Report ONLY real defects, most severe first. For each: file:line, what is wrong, and a concrete failing
scenario (inputs -> wrong output). If you cannot construct one, say the finding is speculative and why. Do
not report style, naming, or missing tests unless the missing test hides a defect you can name.

State explicitly which of the previous round's findings are now correctly fixed and which are not.
```

That last line is the highest-yield sentence in the prompt: it has found four fixes that did not fix anything,
including one that had never been applied to the file at all.

**Conventions that will bite you:**

- **No feature gates** (`#[cfg(feature)]`). All dependencies always compile. Target-scoped `cfg(target_os)` is
  fine, and is how jemalloc is excluded on Windows.
- **No `pub use` re-exports for compatibility.** Every one that existed was an inversion.
- **A multi-statement SQL script must go through `sqlx::raw_sql`.** `sqlx::query` stops at the first `;`,
  *including one inside a `--` comment*, and the fragment after it is a syntax error nobody looks for. This has
  cost a debugging session in each backend, and in this batch one comment containing a semicolon broke twenty-one
  tests in a file it does not appear in. `no_sql_comment_holds_a_semicolon` guards both schemas.
- **Each schema change needs its own `SCHEMA_VERSION`** — §2.2.
- **Never `git checkout` a file with uncommitted work.** Destroyed work four times this way in this batch. Copy
  to `/tmp` at the moment of mutation.
- **Never commit `CLAUDE*.md`**, never delete them. No secrets in the repository, ever.
- **`cargo fmt --all`**, not `-p sideseat-server` — a partial format let an unformatted signature in
  `server/crates/ports` through until `make check` refused it.
- ClickHouse has a long list of specific traps — correlated subqueries silently wrong, `Decimal64(6)` mapping to
  `i64`, SELECT aliases visible in `WHERE`, aggregate nullability. Enumerated in `CLAUDE.md` under "Common
  Gotchas"; each has cost a debugging session.

**When something in the read path breaks**, `message_goldens` is the oracle: 121 captured OTLP payloads replayed
through the real pipeline, checking message count, content, ordering and duplicate absence across four views. It
is itself mutation-verified — dropping one message, swapping two or duplicating one each fail four of its tests.
If a refactor is meant to be answer-preserving, this is what proves it.

**Adding a parity case** (the other oracle): `server/tests/clickhouse_parity.rs` and
`server/tests/postgres_parity.rs`. Write one dataset, call the same read on both backends, `assert_eq!`
the rows. DuckDB and SQLite are the reference. A case
that passes because neither backend was asked the hard question is the failure mode this repository has been
bitten by twice — make sure the fixture actually contains the shape you are testing for.

---

## 11. What this architecture will and will not be

Read this before trying to improve something below. Each row is a limit **accepted deliberately**, with the reason;
treating one as a defect wastes a cycle, and several have already been re-litigated once.

| Limit, after the plan is complete | Why it stays |
| --- | --- |
| **Confirmation is digest equality, never "newer".** Two genuine corrections racing for one identity are resolved by the store's own choice, and both deliveries are reported as stored | No commit-ordered sequence and no fencing token exists in either analytics backend (§1.3). `ingested_at` is `Utc::now()` per instance, so a clock-regressed correction can carry an earlier value — and since both backends treat that column as the version, an "at least mine" test would let older content falsely confirm a newer delivery |
| **The storage quota is not exact at any instant.** Usage may exceed it by the total size of writes issued but not yet landed | A stalled writer cannot be fenced, an external multi-store scan is not atomic, and no observation of absence says anything about the next instant. Eleven review cycles produced six mechanisms that all died here. What is unconditional is the *contract*: at or above measured usage, writes are refused with a reason |
| **Search pagination is not snapshot-isolated.** Records present and unchanged throughout a traversal are returned exactly once; anything arriving or changing during it may be missed or repeated | Needs a snapshot or a durable commit-ordered change token. The arrival predicate raises a flag for what it *can* see, which is strictly better than nothing and strictly weaker than a guarantee — and it is documented as such, because a completeness flag callers read as a proof is worse than an honest caveat |
| **Restore is a procedure with three named residuals**, not a proof that a record is in a backup | Four designs for proving coverage were tried and each failed: no common watermark exists, truncation cannot undo a destructive operation, and three independently backed-up stores have three boundaries with no fence between them |
| **The two modes are not feature-identical.** `as_of_us` works on DuckDB and cannot be expressed on ClickHouse; the rollup contribution table exists only on DuckDB; there is no ranking anywhere | Each is a property of the engine, not of the code. This does **not** contradict §0's parity rule: parity is about *answers where both can be asked*, and a capability gap is declared instead. The plan states each divergence per mechanism and gates the public read with parity suites, so two backends computing the same answer by different means is what is tested |
| **A long session read stays expensive.** Θ(T²) *as sent* for a framework that re-sends its conversation each turn | That cost is a property of the telemetry, not of the normaliser. The pipeline is linear in its input; what grows quadratically is the input. Making the pipeline faster is not the answer — not paying twice for the same rows is, which is what the memo does. Universal O(n) would be a false claim |
| **The deletion journal is permanent and counted against the quota**, so a project that deletes enough can be refused even after every telemetry byte is reclaimed | Excluding it would make "one limit across all of it" false. Including it is the correct direction: discarding the record of a deletion to admit new writes trades a durable guarantee for throughput. Survivable because entries are ids and instants, and a boot-time check turns it into a configuration error rather than a surprise |
| **Presence and AG-UI invoke are single-instance.** The registration store is process-local while the control plane around it spans instances | Warned about at startup on the same signal the `Sharing` rule uses, and the invoke route's 404 names the boundary. A shared store would additionally need leader election to avoid N duplicate expiry events |
| **The query builder will not cover everything.** Schema DDL is exempt by construction, and the raw-SQL invariant's scope is read from the builder's registry rather than from a target of 100% | A narrow typed builder that must satisfy both engines accumulates escape hatches. Stating the scope as "what has been migrated" is what keeps the gate honest; a global ban would simply be disabled |
| **Revision history is not preserved for held records** | It needs an append-only side table on ClickHouse (`ReplacingMergeTree` merges superseded revisions away and no setting makes that selective), inside the commit sequence, with copy-on-hold, an expiry transition, and reads, exports and confirmation all consulting it. That is a state machine whose incompleteness was found repeatedly, and it belongs to the audit layer, which needs revision history for its own reasons |

**What the plan delivers instead of perfection: limits that are stated and checked rather than discovered.** The
difference is practical — "search pagination is not snapshot-isolated" in a response contract is something a
caller can act on; the same fact undetected is a bug someone debugs in six months.

**What would have to change to go further** — none of it a change to the plan:

- a storage substrate with a commit-ordered sequence, which removes four of the rows above at once;
- **one** mode instead of two, which removes the parity tax and the capability gaps;
- normalisation at write time, which would make reads cheap and break the property the product is built on — that
  a pipeline fix applies to history with no re-ingestion.

---

## 12. Verification matrix — which check covers which property

Two uses. If you change a mechanism, this says what should have caught you. If you add one, it says which column
you owe.

| Property | What checks it | Where |
| --- | --- | --- |
| Layers do not invert | the **compiler** (crate manifests), plus `no_layer_crate_depends_on_a_driver`, `the_driver_gate_reads_a_renamed_dependency`, `no_adapter_imports_a_sibling_adapter`, `the_storage_layer_does_not_import_the_http_layer`, `the_ports_crate_emits_no_sql` | `tests/repository.rs` |
| No framework knowledge in Rust | `no_production_module_names_a_framework`, `no_production_module_carries_a_framework_telemetry_key` — two sweeps, because names alone were not enough: the defect that invalidated the first acceptance was a framework fact spelled as a *value* | lib tests |
| The two analytics backends agree | **ClickHouse parity suite** — one span set into both, every read method must return identical rows, DuckDB is the reference | `clickhouse/parity_tests.rs` |
| The two transactional backends agree | **PostgreSQL parity suite**, 43 cases including populated upgrades, governance aggregate types and RLS role/context behaviour | `postgres/parity_tests.rs` |
| Message reconstruction is correct | **121 goldens × 4 views**: count, content, ordering, duplicate absence — plus invariants that hold *independently* of the goldens, so a blindly regenerated snapshot still fails on a real defect | `message_goldens` |
| A rewrite is answer-preserving | the goldens **plus** an equivalence oracle over generated inputs where the interesting cases are ones no framework produces | `order_within_unit_equivalence`, and 17 retired SQL tables kept under `#[cfg(test)]` |
| Memory ceilings | `make footprint` — two RSS gates against a running server, two live-allocation gates in process | `footprint.rs`, `footprint-gates.sh` |
| Latency ceilings | `make bench-http` and `make bench-http-distributed` — **enforce**, exit non-zero on a miss | `bench-http-latency.sh` |
| The queue loses nothing | six tests, each mutation-verified; `make test-redis` for the durable backend | `server/crates/adapter-topics/src/memory.rs`, `server/crates/adapter-topics/src/redis_stream_tests.rs` |
| Schema upgrades reach every database | populated-upgrade tests per backend, comparing a walked-forward v-old database against a fresh one — including **column order** on DuckDB, because its writer is a positional `Appender` | `migrations.rs`, `parity_tests.rs` |
| Tenant isolation | colliding trace/session/content ids across two projects; PostgreSQL valid-context and unset-context RLS tests; ClickHouse per-query policy and maintenance-bypass tests | domain file tests, PostgreSQL and ClickHouse parity suites |
| Restore repair | destructive embedded backup/destroy/restore; journal replay; retention completion; association rebuild before GC; missing-content report; missing-project analytics/staging/blob removal; second-pass fixed point | `server/tests/backup_restore.rs`, `server/crates/domain/src/restore.rs` |
| Documentation does not rot | `every_module_path_cited_anywhere_resolves`, `every_tree_diagram_names_things_that_exist`, `the_documented_project_structure_matches_the_tree`, `every_resolving_command_is_locked`, `every_relative_schema_reference_resolves` — **this file is subject to all of them** | `tests/repository.rs` |
| Supply chain | `every_action_is_pinned_to_a_commit_and_every_image_to_a_tag`, `the_image_gate_reads_the_shapes_that_defeated_it`, `every_lockfile_carries_its_manifests_engines`, `dependabot_covers_every_manifest_in_the_tree`, `every_workspace_crate_takes_the_one_version` | `tests/repository.rs` |
| A background worker is actually started | `every_detector_is_actually_started_in_production` — a sweep that exists and is never spawned is the failure it prevents | `tests/repository.rs` |

**What remains opt-in or unverified**, and each is a real gap rather than an oversight:

| Gap | Why it is not covered |
| --- | --- |
| Cross-replica ClickHouse convergence | needs a second replica; no fixture provides one |
| The replicated migration path in the default gate | `make test-clickhouse` starts a single server; UUID Keeper paths, `ON CLUSTER`, `Distributed` front-table recreation and interrupted migration require the opt-in `make test-clickhouse-replicated` target |
| A real network hop | every measurement is loopback or a local container |
| A multi-replica deletion backlog at scale | stated as unmeasured in `CLAUDE.md` |
| Production backup services themselves | the repository proves embedded checkpoint/copy/repair and validates the runbook; it does not provision or execute PostgreSQL WAL archives, ClickHouse S3 backup storage, object versioning or RedPanda tiered storage for an operator |

---

## 13. First day, first week

**First hour.** `make check` — it needs no containers and no credentials, takes a few minutes, and a green run
means the tree is sound. Then `make dev-server ARGS="--debug --no-auth"` and
`uv run --locked --directory examples/python/strands strands tool_use --sideseat` if you have Bedrock credentials,
or just open the UI against an empty project if not. Seeing a trace render makes everything below concrete.

**First day**, in this order, because each answers a question the next one raises:

1. `CLAUDE.md` — long, and the only place several of these mechanisms are explained at all. Skim the headings,
   read "Common Gotchas" properly.
2. This file's §1.3 (the constraint), §2 (the data model), §3 and §4 (the two paths).
3. `server/crates/ports/src/traits.rs` — the whole seam in one file. If a method looks odd, its doc comment says why.
4. `server/crates/domain/src/sideml/feed/mod.rs` — the pipeline's nine stages. The comments there record decisions that were made
   and reverted, which is the fastest way to learn what does not work.
5. One parity test and one golden test, run individually, to see what an oracle looks like here.

**First week.** The numbered foundation work is complete. Pick one bounded item from §6.11 or one opt-in gate from
§12, reproduce it before changing code, and run a Codex round on your own change (§10.2). If the goal is audit
evidence rather than platform hardening, start a separate plan instead of extending this one implicitly.

**What to be suspicious of in your own work here**, drawn from what has actually gone wrong:

- a test you have not seen fail;
- a bound in a unit other than the resource it bounds;
- a fix whose commit message is more convincing than its diff;
- "this is behaviour-identical" without the goldens run;
- an argument that a second record is "just a duplicate" — that one has been wrong twice.

## 14. Operational and follow-on risk register

Ordered by the product of likelihood and what it costs to discover late.

| Risk | Why it is plausible | Cheapest mitigation |
| --- | --- | --- |
| **A new tenant table silently bypasses policy** | PostgreSQL ownership and ClickHouse local-table policy are schema properties; a migration can create a table and forget either | Extend the forced-RLS/policy schema assertions in the same migration, then run both live parity suites |
| **The body/search backfills sit incomplete indefinitely** | Both are resumable and bounded, which also means they can remain at 60% without blocking normal reads | Alert on progress/drift and do not drop fallback columns until every live project satisfies the cutover criterion |
| **The restore runbook is trusted without rehearsal** | The embedded proof cannot validate an operator's WAL archive, S3 permissions, ClickHouse backup chain or RedPanda tiered-storage policy | Schedule restore drills and keep `.restore-pending` until the repair report reaches a fixed point |
| **Cross-replica ClickHouse behaviour diverges from the single-node suite** | No two-replica fixture exists, and distributed DDL already has a slow unexplained third participant | Build a true two-replica fixture; keep replicated/two-shard targets opt-in but required before topology changes |
| **Host-sensitive latency regressions are normalised as noise** | Five embedded and three distributed ceilings miss on this host, while footprint and both search gates pass | Re-run on an idle pinned host, then bisect any stable miss rather than widening ceilings |
| **The parity suites pass because neither backend was asked the hard question** | Bitten repeatedly; a fixture can lack the shape its assertion claims to test | Assert fixture preconditions and mutation-verify every new regression |
| **A review round is treated as done because findings were addressed** | Four of eight round-one fixes did not fix anything | Ask the next round explicitly which previous findings are now correctly fixed |

---

## 15. Alternatives already evaluated and rejected

A newcomer will propose several of these within a week. Each was considered at length; the reason is the part worth
keeping, because in most cases the option is *reasonable* and fails on a specific fact.

### 15.1 Search

| Option | Why not |
| --- | --- |
| **tantivy embedded** (MIT, real BM25, phrase, fuzzy, snippets) | Ranking is out of scope, so its main capability is unused — while it reintroduces exactly the cross-store consistency machinery a DuckDB table removes: separate index files, a separate commit, a reconciliation sweep. Adds writer memory against the < 100 MB idle gate, and carries pre-1.0 churn: 63 versions, three yanked, breaking changes at minor versions, and an upgrade can change tokenisation and so ranking, forcing a full reindex. **Revisit if local ranking becomes a requirement** — it is still the strongest option for that |
| **OpenSearch 3.8** | The heaviest thing shippable: a JVM plus `vm.max_map_count=262144` as a *host* sysctl the official Helm chart leaves disabled, `memlock`, `nofile`, `swapoff`, a shipped `securityConfig` the docs say to replace before production, ~1.15–1.3× the indexed text in disk, and delete-by-query that reclaims nothing until merge. The official Rust client is 2.4.0 with a documented ceiling of OpenSearch **2.0** and no 3.x release, so it would have to be plain HTTP. **Revisit when query logs show ranking is the blocker**, and require it to degrade to chronological when absent |
| **SQLite FTS5 projection** | Statically linked already, so it cost no dependency — and that was its only real advantage. It puts search state in the **transactional** store, the one store with no relationship to analytics; no transaction spans DuckDB and SQLite, so it needed a generation marker, a reconciliation sweep and a consistency window in the API; and it had to duplicate every filterable scalar to keep filtering and pagination inside SQLite |
| **DuckDB's FTS extension** | Settled by inspection, not argument: `libduckdb-sys` declares its bundleable set exhaustively and **there is no `fts`**. It is out-of-tree, so bundling means forking the sys crate's build and carrying that fork across DuckDB upgrades; loading at runtime is barred by the single-binary rule. And it would be the wrong mechanism anyway — its index "will not update automatically when the input table changes" and the documented refresh is drop-and-recreate |
| **Index-per-tenant on any Lucene engine** | Hard wall at ≤25 shards per GiB heap and 4 000 per node, so a 16 GiB heap is ~200 tenants with the budget spent on nearly-empty indices. The shared-index alternative shares BM25 statistics across tenants: a relevance problem *and* a leak, since scores reveal term rarity in other tenants' data. No configuration gives both |
| **Quickwit** | The mature form of "tantivy on object storage", and a second data plane with its own PostgreSQL metastore. Only if server-side ranking is required and OpenSearch refused |
| **Our own postings with BM25** | The *scorer* was withdrawn: `df` maintenance, `avgdl`, norms, refcount arithmetic, and two holes in its own argument. With no ranking there is nothing to score, and a distinct-term table with a semi-join is not an information-retrieval implementation |

**Ranked cross-engine parity was never achievable**, and this is recorded so it is not retried. tantivy and Lucene
implement BM25 to the letter — same formula, same `k1`, same `b`, f32 throughout — and their scores still cannot be
equal: Lucene's `docCount` is *documents having the field* while tantivy sums all segments' `max_doc`, and spans are
heterogeneous, so a field present on 40% of documents gives the two different N and different `avgdl`. No
configuration fixes it. Separately, tantivy **drops** tokens of 40 bytes or more while Lucene **splits** at 255
characters — a recall difference for tool arguments, JSON, base64, UUIDs and CJK.

### 15.2 Storage and consistency

| Option | Why not |
| --- | --- |
| **A source-scanning test instead of crate boundaries** | `use` parsing misses fully-qualified paths, macro-generated code, `#[cfg]` branches, re-exports and inferred types |
| **A rollup as a mutable row, merged per delta** | On ClickHouse that is an application-side read-modify-write with no transaction and no compare-and-swap, and the queue partitions by *trace*, not by rollup key — so two workers read the same value, both write a replacement, and one delta vanishes with every operation reporting success |
| **A contribution table on ClickHouse too** | "Cannot diverge by construction" is false there: an incremental materialised view is an insert trigger not atomically visible with its source, can leave partial state after a failure, and reacts to no deletion; replicated tables cannot use the experimental multi-table transactions that would be needed |
| **`PARTITION BY project_id`** | Unbounded partition count, and ClickHouse recommends staying well under ~1 000 distinct values because parts in different partitions never merge. Partition by **time**, order by `(project_id, …)` so a tenant's rows are contiguous, and accept that per-tenant deletion is a mutation |
| **A global truncation watermark for restore coverage** | `max_ingested_at_us` is span-only and its own documentation says a true bound needs a commit-ordered sequence neither backend provides. Four designs died here — see §11 |
| **ClickHouse 26.4's experimental `commit_order` projection** | Not usable for any of the four rows in §1.3, and the reasons are specific rather than dismissive: **experimental** (so its on-disk form may change), **per-partition** rather than global, present on **one** of two backends so nothing built on it could be a shared contract, and an insertion order is not a **fencing token**, which is what the byte budget actually needs. Worth re-examining once stable and if a DuckDB counterpart appears |

### 15.3 Things tried in code and reverted

These are in the tree's comments, and each reverted change looked like an improvement:

- **An id-less tool result claiming a *following* call** in a span that starts at the same instant. Both spellings
  made a real fixture worse: ADK's tool and generation spans do tie, so the relaxation let one result claim a call
  a later result needed, and three results that *had* ids lost them.
- **Ranking plain messages by carrier position.** Turned a separator repeated verbatim within one span into
  duplicates.
- **Skipping the session read for a batch carrying the trace's parentless span.** Recovers about half of 2.7 ms and
  privileges the root span — which is exactly what the canonical-session rule refuses to do, since in a distributed
  trace a child produced on another host can carry an earlier start time than its parent.
- **Parameterising the session-membership subquery by relation.** Put the candidate filter *outside* the window and
  silently gave the optimisation back; an interleaved benchmark is what noticed.
- **Deleting a non-durable association from a drop path** — see §2.3.

---

## 16. Keeping this file true

A document like this fails in one way: it stops matching the tree and nobody notices, and then it is worse than
nothing because it is trusted. Five of its own claims have already drifted during the sessions that wrote it — a
line number within one editing session, the golden count, two `grep` totals, and its own commit count once its own
commits entered the range.

**What is already enforced.** This file is a tracked file, so five structural invariants apply to it, and three have
rejected drafts:

| Invariant | What it catches here |
| --- | --- |
| `every_module_path_cited_anywhere_resolves` | a path that does not exist, including a `{a,b}/x` brace shorthand |
| `every_resolving_command_is_locked` | a documented `cargo` command without `--locked` |
| `the_documented_project_structure_matches_the_tree` | a directory listing that has gone stale |
| `every_tree_diagram_names_things_that_exist` | a diagram naming a module that was renamed |
| `every_relative_schema_reference_resolves` | a config-schema reference that moved |

Run them after editing: `cargo test --locked -p sideseat-server --test repository`.

**What is not enforced, and is therefore on you.** Everything numeric. The mitigation used throughout is to give
the *command* beside the figure so a reader can re-derive rather than trust:

```bash
# counts this file asserts
git log --oneline 4a9c30c9..HEAD -- ':!PLAN.md' | wc -l    # the code commits (45)
git ls-files 'server/tests/fixtures/messages/*/*/expected.json' | wc -l   # committed goldens (121)
grep -c '^pub trait' server/crates/ports/src/traits.rs            # ports (20)
grep -n 'pub const SCHEMA_VERSION' server/crates/adapter-duckdb/src/schema.rs server/crates/adapter-clickhouse/src/schema.rs server/crates/adapter-sqlite/src/schema.rs server/crates/adapter-postgres/src/schema.rs   # 6,7,9,10
cargo test --locked -q -p sideseat-server --lib 2>&1 | tail -2   # 89 passed, 4 ignored
```

**The rule that keeps it honest: no line numbers.** An earlier draft cited them for every port trait and one had
already drifted by the end of the same session. Cite a name, which is greppable and does not move.

**When a step lands**, four things change and all four are easy to forget:

1. §2's status table — the step's row.
2. §5 — a subsection saying what landed and, more importantly, **why**, because the reason is what does not survive
   being summarised later.
3. §8 — what is now verified and what is not.
4. §6's deep dive for that step — delete it, or reduce it to what is still open. A deep dive for finished work
   competes with the sections about unfinished work.

**When a review round runs**, §6.7's count and §11's residuals may both move. A finding that turns out to be an
*accepted limit* rather than a defect belongs in §11 with its reason, not in a fix.

**What to delete from this file eventually.** §5 grows without bound if every step adds a subsection. Once a step's
reasoning is captured in `CLAUDE.md` — which is where the permanent architectural record lives — §5's entry can
shrink to a line and a commit hash. This file is the *execution* record; it should get shorter as the work finishes,
not longer.
