# CLAUDE.md

## AI Guidance

- Ignore GEMINI.md and GEMINI-\*.md files
- Use subagents for code searches/analysis (give full context background)
- Reflect on tool results quality before proceeding
- Invoke independent operations in parallel
- Verify solutions before finishing
- Do what's asked; nothing more, nothing less
- Never create files unless absolutely necessary
- Prefer editing existing files over creating new ones
- Never create documentation files unless explicitly requested
- When modifying core context files, also update memory bank
- Exclude CLAUDE\*.md from commits; never delete these files
- No emojis in console output
- Cross-platform compatible code (macOS, Linux, Windows)
- Never change .venv files
- Default LLM model: `claude-haiku-4-5-20251001`. Never use Sonnet 3.x or older models.

## Code Standards

**Comments**: Minimal, why not what, no process history, no redundant comments, keep current (delete stale). Doc comments (`///`) for public API only.

**Diagrams**: Use Mermaid syntax in markdown (no ASCII art).

**Rust**: No `pub use` re-exports for compatibility. Use `thiserror`/`anyhow` for errors, `Arc<T>` + `parking_lot` for state. No feature gates (`#[cfg(feature = "...")]`) - all dependencies are always compiled.

**Tailwind CSS v4**: Use native utilities, not arbitrary values. E.g. `max-w-400` not `max-w-[1600px]`, `gap-6` not `gap-[24px]`.

**TypeScript** (`erasableSyntaxOnly: true`): No constructor parameter properties, no enums (use `as const`), no namespaces.

```typescript
// ❌ constructor(private client: ApiClient) {}
// ✅ private client: ApiClient; constructor(client: ApiClient) { this.client = client; }
```

## Tools

```bash
rg "pattern"        # Search content (NOT grep)
fd "name"           # Find files (NOT find)
fd . -t f           # List all files
rg --files          # List files (.gitignore aware)
```

## Plan Mode

- Make the plan extremely concise. Sacrifice grammar for the sake of concision.
- At the end of each plan, give me a list of unresolved questions to answer, if any.

## Project Structure

**SideSeat**: AI/LLM observability toolkit that collects OpenTelemetry traces from AI applications, normalizes multi-framework data into a universal format (SideML), and provides a web UI for debugging.

**Dev server**: Already running (`make dev-server ARGS="--debug --no-auth"`). Use `make dev-server` for auth.

**Databases**: DuckDB/ClickHouse (analytics) + SQLite/PostgreSQL (transactional). Default: DuckDB + SQLite.

### Server (`server/src/`)

```
├── app.rs              # Main orchestrator, startup, command dispatch
├── core/
│   ├── constants.rs    # All constants (env vars, defaults) - ADD NEW CONSTANTS HERE
│   ├── config.rs       # AppConfig loading, validation, StorageBackend enum
│   ├── cli.rs          # Clap argument parsing
│   └── topic.rs        # Pub/sub for inter-component messaging
├── utils/              # PREFER THESE OVER WRITING NEW UTILITIES
│   ├── json.rs         # compute_message_hash() for deduplication
│   ├── string.rs       # truncate_preview(), PREVIEW_MAX_LENGTH
│   ├── otlp.rs         # extract_attributes(), build_attributes_raw()
│   ├── file.rs         # expand_path() for cross-platform paths
│   └── sql.rs          # escape_like_pattern() for safe SQL
├── data/
│   ├── duckdb/         # DuckDB analytics backend (default)
│   ├── clickhouse/     # ClickHouse analytics backend (distributed)
│   ├── sqlite/         # SQLite transactional backend (default)
│   ├── postgres/       # PostgreSQL transactional backend
│   ├── types/          # Shared DTOs across backends
│   ├── traits.rs       # AnalyticsRepository, TransactionalRepository
│   └── mod.rs          # AnalyticsService, TransactionalService enums
├── domain/
│   ├── pricing/        # LLM cost calculation (model lookup, GitHub sync)
│   ├── sideml/         # Universal AI message format
│   │   ├── types.rs    # ChatMessage, ChatRole, ContentBlock, ToolChoice
│   │   ├── pipeline.rs # determine_category(), is_llm_output_event()
│   │   ├── content.rs  # Content block normalization
│   │   └── feed/       # Feed pipeline (dedup, ordering, history detection)
│   │       ├── mod.rs      # Main pipeline: parse → flatten → dedup → sort
│   │       ├── dedup.rs    # Birth time algorithm, identity-based deduplication
│   │       └── types.rs    # BlockEntry, FeedOptions, FeedResult
│   └── traces/
│       ├── extract/
│       │   ├── mod.rs        # keys::* constants (GEN_AI_*, LLM_*, etc.)
│       │   ├── attributes.rs # Framework detection, field extraction
│       │   └── messages.rs   # Multi-framework message extraction
│       └── enrich.rs   # Cost calculation, preview generation
└── api/routes/         # Axum HTTP handlers (direct to repositories)
```

### Web (`web/src/`)

```
├── api/
│   ├── api-client.ts   # Core ApiClient (error handling, SSE, timeouts)
│   ├── otel/
│   │   ├── client.ts   # OtelClient (traces, spans, sessions)
│   │   ├── keys.ts     # Query key factories + extractors
│   │   └── hooks/      # React Query hooks (useTraces, useSpans, etc.)
│   └── types.ts        # TypeScript interfaces matching server DTOs
├── auth/               # AuthProvider, AuthGuard, auth context
├── components/
│   └── ui/             # shadcn/ui ONLY - NEVER MODIFY THESE FILES
├── hooks/              # Custom hooks (use-mobile, etc.)
├── lib/
│   ├── utils.ts        # cn(), deepParseJsonStrings()
│   └── app-context.tsx # AppProvider, useOtelClient()
├── pages/              # Route page components
└── styles/             # CSS theme files
```

## Key Patterns

### Trace Pipeline (`domain/traces/`)

1. **Extract**: OTLP protobuf → `SpanData` + framework detection
2. **Enrich**: Costs (pricing service) + previews
3. **Persist**: Batch write to DuckDB (raw data preserved)

**IMPORTANT**: Message normalization (SideML) happens at **query time**, not ingestion.
This ensures bug fixes apply to historical data without re-ingestion.

### Data Processing Principle

| Phase     | Location             | What to do                                                             |
| --------- | -------------------- | ---------------------------------------------------------------------- |
| Ingestion | `traces/extract/`    | Preserve raw data, add metadata (tool_call_id, exception fields, etc.) |
| Query     | `sideml/pipeline.rs` | Role derivation, normalization, deduplication                          |
| Query     | `sideml/feed/mod.rs` | Error display composed from exception_type/message/stacktrace          |

**Never** transform roles or content during ingestion. All semantic processing in SideML.

### Message Categorization (`sideml/pipeline.rs`)

```
Event: LLM output (gen_ai.choice) → event name
       Special role (tool_call, tools, data) → role
       Other → event name
Attribute: Has role → role, else → user
```

### ChatRole Mapping

| Role      | Aliases                                                  |
| --------- | -------------------------------------------------------- |
| System    | `system`, `developer`                                    |
| User      | `user`, `human`, `data`, `context`                       |
| Assistant | `assistant`, `ai`, `bot`, `model`, `choice`, `tool_call` |
| Tool      | `tool`, `function`, `ipython`                            |

### Feed Pipeline (`sideml/feed/`)

Reconstructs conversation timelines from OTEL spans with history duplication.

**Pipeline stages:**

```
1. PARSE       Raw JSON → SideML messages
2. FLATTEN     One BlockEntry per ContentBlock (with metadata) - never filtered
3. CORRELATE   id-less tool results adopt their call's id (feed/correlate.rs)
4. CLASSIFY    Determine is_output for each block
5. MARK HISTORY Eight-phase detection (see below)
6. DEDUP       Identity-based, keep highest quality version
7. WITHDRAW    Clear a correlated id whose call did not survive dedup
8. SORT        (batch_time, message_index, entry_index, span, after_call, content_hash)
9. ROLE FILTER `?role=` applied to the finished feed, on the derived role
```

**Output classification (is_output field):**

- `gen_ai.choice` events → OUTPUT (protected from history, uses span_end)
- Assistant text/thinking → OUTPUT
- ToolUse from generation spans → OUTPUT
- Everything else → INPUT (can be history, uses event_time)

**Eight-phase history detection:** 0. **Output protection**: gen_ai.choice events are NEVER marked as history

1. **Timestamp-based**: Message timestamp < span start → historical context
2. **Accumulator span input**: Input events from non-execution spans (span/agent/chain)
3. **Session history**: User/system messages in non-agent spans not in agent spans
4. **Generation span history**: Unprotected messages in generation spans with session history
5. **Orphan tool results**: Tool results whose matching tool_use was filtered
6. **Intermediate output**: Non-final assistant text from generation spans
7. **Duplicates**: Later occurrences of same content within trace

Note: gen_ai.assistant.message (history re-send) CAN be history. Only gen_ai.choice (actual LLM output) is protected.

Cross-trace prefix strip (session mode): attribute-input only, per-span reset, and **injective matching
against a partial order**, not a subsequence match against a sequence. A provider writes a conversation
history as a flat list, so a turn's parallel tool calls come back interleaved with their results -
`call, call, result, result` is replayed as `call, result, call, result`. That is a different
linearisation of the same turn, and one forward cursor reads it as a mismatch at the second call, which
ends the prefix and leaks the rest of the span. So each replayed block consumes one *distinct* prior
occurrence (injectivity is what keeps a genuinely repeated question from collapsing onto the first ask -
without it `adk/tool_use` loses one of its two identical questions), and a candidate is refused only when
the evidence says it must precede something already matched.

Matching **searches**, it does not choose greedily. Two unordered branches give `callA → resultA` and
`callB → resultB` with both results carrying the same identity (two tools answering `"ok"`); replayed as
`callB, resultB, callA, resultA` — a valid linear extension — taking the first permitted candidate claims
`resultA` for `resultB`, which then requires `callA` earlier, refuses it, and duplicates the rest of the
turn. Only the order constraints separate interchangeable candidates, so `longest_matching_prefix` does a
bounded depth-first search. Exceeding the budget under-strips (duplicates) rather than over-strips
(deletion). `every_linear_extension_of_the_prior_order_is_fully_stripped` enumerates all 24 orders of
four blocks per shape and requires each linear extension to match in full.

The relation is `order_graph::causal_precedence`, deliberately **not** the resolver's graph: that one is
over contracted emission *units*, so `call_a → result_a` also asserts `call_b → result_a`, and it
includes generation dataflow, whose input side keeps replayed history and after dedup asserts
`call_b → call_a`. Both are right for presentation and false as statements about causal order. Being
independent of the presentation constraints is also what keeps promoting an ordering class from changing
which messages a session returns.
Event-based frameworks (Strands) stay trace-independent; no cross-trace stripping.

**A carrier's structure says what it is evidence of** (`sideml/carrier.rs`). Four independent facts per carrier, because a conversation snapshot and accumulated framework state are both ordered and both may hold history, and differ only in whether *position* proves multiplicity:

| Fact | Read by |
| --- | --- |
| `position_proves_distinct_occurrence` | the repeat decision below — true for one emission (`gen_ai.choice`), false for accumulated state (`output.value`, which re-lists its own tool calls) |
| `position_provides_sequence_order` | the carrier-subsequence invariant |
| `carrier_is_atomic_emission` | cohesion of one emission's blocks |
| `carrier_may_contain_history_or_state` | whether its observations can be history |

An unclassified carrier takes the cautious reading (snapshot): it may under-report, which the answer invariant catches, never over-report, which a user sees as duplicates.

**A repeat within one response is a repeat.** A tool call's identity ignores the provider's call id — history re-sends regenerate ids — so each call also carries the *rank* of its id among same-shaped calls of the same response (`call_repeat_ordinals`, `feed/dedup.rs`), and a tool result inherits its call's rank. Two identical calls in one response therefore rank 0 and 1 and both survive; a re-send of that pair ranks 0 and 1 again, whatever the ids became, and collapses onto it. A response here is `(trace, span, source)` — **not** message index, because normalisation gives every tool call a message of its own. Without this, `crewai/mcp_tools` showed one MCP call and one error where the model had retried an identical call and then apologised, with nothing in the feed to explain why. Plain messages get no rank: with no id there is no evidence of a genuine repeat, and treating repeated text as two messages would undo the history collapsing.

**Content-based identity** (not ID-based):

- Tool calls: hash(name + input) — call_id ignored (regenerated in history)
- Tool results: tool_use_id when present, else hash(content)
- Regular: hash(trace_id + role + content)
- JSON payloads: members with no value (`null`, `""`, `[]`, `{}`) are dropped before hashing, so a
  schema-filled object and the model's raw one are one answer, not two

**Correlation runs before classification.** Phase 7 keys a tool result by
`(trace, identity-of-the-answered-call, content)`, and the orphan-result phase reads the id — both
need the call reference. Correlating afterwards let two identical-looking results answering two
different calls collapse: `agent-framework/image_gen` returned 1 of its 3 `generate_image` results.
A correlated id is flagged (`tool_use_id_correlated`) because "names no current call" means
"from a past turn" only for a provider's own id.

**Quality scoring** (higher = preferred in dedup):

```
+100  Non-history block
+10   Has finish_reason
+5    Enrichment content (thinking)
+4    Output source (vs an input-source copy)
+3    Tool span (execution, not re-sent context)
+2    Event source (vs attribute)
+1    Has model info
```

**Ordering is a tuple key, not a comparator with cases.** Every term is a value carried on the key, which is what makes it a total order — `sort_by` may panic or return anything without one, and role-based tie-breaking is cyclic (intro text before its call by position, the call before a result by role, the result before the text by role).

```
(batch_time, message_index, entry_index, span, after_call, content_hash)
```

- `batch_time`: one time per **response**, the earliest birth time among its blocks. A response is `(trace, span, timestamp, direction)` — keyed by direction so a span's input and its output are not merged, and not split further by message index, or a turn's intro text would sort after the results it introduces.
- `after_call`: set by `feed_positions` (`feed/dedup.rs`) **before** sorting, and read by both the trace-view sort and the project feed's, so the four views cannot disagree about where a result sits. A message index restarts at zero in every span, so between spans it orders nothing and a tool result could precede its call. Such a result takes its call's position instead — a property of the block and its own call, never of the pair being compared, so the order stays total. Only cross-span ties are adjusted: ADK's `user, call, result, call` and Vercel's parallel `call, call, result, result` share a span and are left alone.

Do **not** re-introduce role ranking here. Per pair it is cyclic, per response it merges ADK's turns, per span it interleaves Vercel's parallel calls; all three are recorded in `dedup.rs`.

### OTel GenAI Conventions

**Status**: Development (unstable). Use fallback chains:

```rust
get_first(attrs, &[keys::GEN_AI_PROVIDER_NAME, keys::GEN_AI_SYSTEM, ...])
```

**Namespaces**: `gen_ai.*` (OTel), `llm.*` (OpenInference), `ai.*` (Vercel), `langsmith.*`/`langgraph.*`

**Frameworks**: StrandsAgents, LangGraph, LangChain, CrewAI, AutoGen, PydanticAI, OpenInference, Logfire, MLflow, LiveKit, AWS Bedrock, Azure OpenAI, Azure AI Foundry, Google ADK, Vertex AI, Vercel AI SDK, Claude Agent SDK

**Claude Agent SDK** is the odd one out: it emits no in-process telemetry. It spawns the Claude Code CLI, which self-instruments and is configured via `CLAUDE_CODE_*`/`OTEL_*` subprocess env vars. Spans are named `claude_code.*` and require `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` (tracing is beta). Never set an `OTEL_*_EXPORTER` to `console` — the CLI writes telemetry to stdout, which is the SDK's message channel.

Message content needs a **second** beta tier: `ENABLE_BETA_TRACING_DETAILED=1` + `BETA_TRACING_ENDPOINT` (base URL, not `/v1/traces`). Only then are `response.model_output` (assistant text), `new_context` (user turns / tool results, tagged `[USER PROMPT]` / `[TOOL RESULT: <id>]`), `user_system_prompt` and `tool_input` emitted. The `try_claude_code` extractor in `messages.rs` maps them; tokens use the CLI's bare `input_tokens`/`output_tokens` names, handled by fallbacks in `attributes.rs`.

### React Query (`api/otel/keys.ts`)

```typescript
otelKeys.traces.list(projectId, params); // ["otel", projectId, "traces", "list", params]
extractListParams<T>(queryKey); // Type-safe param extraction (index 4)
extractTraceListParams<T>(queryKey); // For traceList queries (index 5)
omitPagination(params); // Remove page/limit for filter comparison
```

## API Endpoints

**Default project ID**: `default`

**OTLP Ingestion**: `POST /otel/{project_id}/v1/{traces,metrics,logs}`

**Query API** (`/api/v1/project/{project_id}/otel`):

- `GET /traces`, `/traces/{id}`, `/traces/{id}/messages`
- `GET /spans`, `/traces/{trace_id}/spans/{span_id}`, `/traces/{trace_id}/spans/{span_id}/messages`
- `GET /sessions`, `/sessions/{id}`, `/sessions/{id}/messages`
- `GET /sse` (real-time)

**Raw span data**: Add `?include_raw_span=true` to span/trace endpoints to get the full OTLP span JSON (attributes, events, links, resource).

**SDK runtime channel** (presence + introspection + AG-UI invoke):

- `GET /api/v1/project/{project_id}/ws` (persistent WebSocket; protocol in `server/protocol/ws-v1/`)
- `GET /api/v1/project/{project_id}/registrations` (read-only snapshot)
- `POST /api/v1/project/{project_id}/agents/{name}/runs` (AG-UI run-agent SSE; routes through WS to the SDK that owns the registration)

**Other**: `/api/v1/projects`, `/api/v1/auth/*`, `/api/v1/health`

## Conventions

- **Utils first**: Check `server/src/utils/` and `web/src/lib/utils.ts` before writing new utilities
- **Tokens/costs**: Never NULL, default 0 (`i64`/`f64` in Rust, `number` in TS)
- **Constants**: Define in `core/constants.rs`
- **Logging**: `tracing` macros, prefer `debug!` over `info!`. Set `SIDESEAT_LOG=debug`
- **Config priority**: Defaults → `~/.sideseat/` → `./sideseat.json` → CLI args → env vars
- **Config files**: See `server/sideseat.schema.json` for structure, `server/sideseat.example*.json` for examples
- **shadcn/ui**: Never modify `components/ui/`, wrap or use `className`
- **Imports**: Use `@/` path alias in web (e.g., `@/components/ui/button`)
- **No "use client"**: This is Vite/React, not Next.js
- **Auth**: Context-based with `auth:required` event for 401 handling
- **State**: React Context only (no Redux/Zustand)
- **Testing**: `cargo test` (Rust), `npm test` (web), `cargo clippy` (no warnings allowed)

## Documentation Locations

When updating framework integration docs (e.g., changing a package name, install command, or code snippet), update **all** of these locations:

| Location | Path | Notes |
|----------|------|-------|
| Framework page | `docs/src/content/docs/docs/integrations/frameworks/<framework>.mdx` | Full guide with Quick Start + Without SDK sections |
| Docs homepage tabs | `docs/src/content/docs/docs/index.mdx` | SDK tab (`install:`) + Direct OTLP tab |
| Telemetry config UI | `web/src/pages/configuration/telemetry.tsx` | `install`, `altInstall`, `altCode()` per framework entry |
| MCP setup prompt | `server/src/api/mcp/tools.rs` | `FrameworkSetup`: `no_sdk_extra_pkgs`, `no_sdk_extra_setup` |

The telemetry config UI is served at `/organizations/default/configuration/telemetry`. The MCP `setup_guide` prompt is used by AI coding assistants to generate integration code.

## Development

**Database**: `./.sideseat/duckdb/sideseat.duckdb` — DuckDB is locked while the server runs. Always check raw data via API, not direct DB access.

**Test data**: `uv run --directory misc/samples/python/strands strands <sample> --sideseat`. Samples: tool_use, mcp_tools, structured_output, files, image_gen, agent_core, swarm, rag_local, reasoning, error, strands_ws (WS runtime channel: registers a Strands graph for presence + AG-UI invoke; blocks until Ctrl-C). Provider samples: `uv run --directory misc/samples/python/openai openai-provider <sample> --sideseat` and `uv run --directory misc/samples/python/bedrock bedrock <sample> --sideseat`. Claude Agent SDK: `uv run --directory misc/samples/python/claude-agent-sdk claude-agent-sdk <sample> --sideseat` (samples: tool_use, mcp_tools, structured_output, reasoning, custom_tools, subagents, multi_turn, permissions, error) and `cd misc/samples/js && npm run claude-agent-sdk -- <sample> --sideseat`.

**Note**: samples read `misc/.env` (not committed — `cp misc/.env.example misc/.env`). Without it `OTEL_EXPORTER_OTLP_ENDPOINT` is unset and non-`--sideseat` runs fail with connection refused against the OTel default `localhost:4318` instead of SideSeat's 5388.

**Credentials per suite**: Bedrock-only credentials cover every suite except `autogen` — strands, langgraph, crewai, adk, bedrock and claude-agent-sdk (Python and JS) natively, plus the vercel-ai and strands JS suites. `openai`, `openai-agents` and `agent-framework` reach Bedrock through its OpenAI-compatible endpoint and `anthropic` through the Anthropic-compatible one, via `common/bedrock_openai.py` (SigV4-signing httpx client); their `bedrock-*` model aliases are the defaults, so no `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` is needed. Only `autogen` still requires a first-party key (`DEFAULT_MODEL = "anthropic-haiku"`, no Bedrock path in its runner). `strands/agent_core` additionally needs a provisioned AgentCore memory store in `AGENT_CORE_MEMORY_ID`.

**Region**: botocore reads `AWS_DEFAULT_REGION` and ignores `AWS_REGION`, while the JavaScript AWS SDK reads `AWS_REGION` — set both in `misc/.env` or the Python suites silently use the region from `~/.aws/config`. Image generation needs `stability.sd3-5-large-v1:0` in a region that offers it (`us-west-2`); `amazon.titan-image-generator-v2:0` has been retired.

**Ingestion latency, measured end to end**: `cargo test --release -p sideseat-server bench_ingestion --
--ignored --nocapture` runs the real `run_batch` - extraction, enrichment, file storage, both writes and
the project fence - against a temp DuckDB, a temp SQLite and filesystem file storage. `BENCH=<suite>/
<sample>` selects a fixture, `ITERATIONS=n` the run count. Measured on an M-series laptop:

| Fixture | Payload | Spans | Mean |
| --- | --- | --- | --- |
| `langgraph/swarm` | 1.7 MB / 5 requests | 151 | 18.8 ms |
| `openai-agents/files` | 1.0 MB / 2 requests | 6 | 21.9 ms |
| `strands-js/image-gen` | 15.8 MB / 4 requests | 17 | 99.3 ms |

`bench_pipeline`'s `INGEST(cpu only)` number is deliberately narrower - it stops before file extraction
and every persistence step - so it is a floor, not a request's cost.

**Read latency scales with the history the frameworks re-send, not with the answer**:
`cargo test --release -p sideseat-server bench_session_scaling -- --ignored --nocapture` measures
reconstruction over synthetic sessions in the two shapes that differ in the way that matters -
*incremental* (each span carries its own turn, as Strands' per-message events do) and *replaying* (each
generation span re-sends the whole conversation, as ADK, LangGraph and Vercel do):

| Shape | Turns | Input | Blocks | Cold | Warm |
| --- | --- | --- | --- | --- | --- |
| incremental | 100 | 26 KB | 200 | 5.4 ms | 0.2 ms |
| incremental | 1 000 | 269 KB | 2 000 | 20.3 ms | 1.6 ms |
| incremental | 10 000 | 2.7 MB | 20 000 | 224 ms | 15.6 ms |
| replaying | 100 | 697 KB | 200 | 24.9 ms | 0.8 ms |
| replaying | 1 000 | 68 MB | 2 000 | 2.28 s | 47 ms |

The pipeline is **linear in its input** in both shapes (~27 MB/s); what grows quadratically is the input
itself, because a replaying framework emits the whole history once per turn. So the cost of a long
session is a property of the telemetry, not of the normaliser, and making the pipeline faster is not the
answer - not paying it twice for the same rows is.

**Reconstruction is memoised on the rows** (`sideml/feed/cache.rs`). The key is a BLAKE3 digest of
everything the pipeline reads from each row, so a changed row is a *different key* rather than a stale
hit: there is no invalidation to get wrong and no TTL to tune. Hashing 68 MB costs the 47 ms in the warm
column above, against the 2.28 s it replaces. The cache is **process-local and empty at startup**, which
is what keeps "a fix applies to history without re-ingestion" true - a persisted cache would serve
answers built by the previous pipeline, and a version constant someone must remember to bump is a hole
rather than a design. The *unfiltered* reconstruction is cached and `?role=` is applied to a copy, so one
entry serves every role.

**Horizontally scaled, ephemeral instances**: what is and is not shared, because the answer differs per
mechanism and getting it wrong is invisible until there are two instances.

| Mechanism | Shared? | Why that is correct |
| --- | --- | --- |
| Reconstruction cache (`feed/cache.rs`) | No, process-local | A memo over a *pure function* of the rows. Any instance computes the same answer from the same digest, a cold instance recomputes it, a dying one loses only the saving. N instances change the hit rate, not the answer — `a_cached_reconstruction_equals_a_fresh_one` checks byte equality over the corpus. Sharing it would need a version key to avoid serving the previous build's answers, and a hand-maintained constant is a hole. |
| File extraction cache | No, per pipeline | Same shape: a memo keyed by content hash. |
| Trace ingestion topic | Yes, when Redis is configured (`stream_topic` + consumer groups + `claim_stuck_messages`) | At-least-once across instances; ingestion is idempotent by span id, so a redelivery rewrites rather than duplicates. |
| Metrics | Not queued at all — written inside the request | An in-process queue made a 200 mean "buffered", so a crash lost records the exporter had counted as delivered. A failure is a 503 the exporter retries. |
| Logs / SSE topics | No — `topic()` builds an in-process channel whatever the backend | So an instance consumes only what it published: no cross-instance duplication. Nothing stores logs, and the endpoint says so via OTLP `partial_success`. |
| Deletion tombstones (`projects.deleting_at`, `organizations.deleting_at`) | Yes, they are rows | One compare-and-set decides the owner across all instances, and every instance's write path consults it. |
| Deletion sweeps | Run on every instance | Every step is idempotent and the claims are CAS-guarded, so concurrent sweeps duplicate work rather than corrupt state. |
| Migrations | Serialised by `pg_advisory_lock` | Concurrent instances starting together cannot race the schema. |

**Deleting a project or an organization is asynchronous, and the row is a tombstone.** The fence lives in
the transactional store and spans in the analytics store, so no transaction spans them: a writer can read
"live", have the deletion land underneath it, and commit afterwards. **No elapsed time bounds that** - a
blocking insert can outlive its statement timeout, an object store can retry, a container can be paused -
so there is no grace period. What the tombstone gives instead:

1. No *new* writer passes the fence (`project_accepts_writes`: a row exists **and** nothing has claimed
   it, so an absent project is refused as firmly as a claimed one).
2. Cleanup keeps running while the row exists, so a late writer's spans are deleted by the next sweep.
3. The row goes only after `PROJECT_TOMBSTONE_CLEAN_SWEEPS` consecutive sweeps found nothing; one that
   finds data deletes it and starts the count over.

An organization is tombstoned too, because deleting its row cascades its *project* rows away and those
rows are what the projects' cleanups depend on; its row goes once no project rows remain. The residual is
stated rather than hidden: a writer whose first commit lands after that many consecutive quiet sweeps
(ten minutes at the default interval) would leave rows nothing collects.

**Message-parsing goldens**: `cargo test -p sideseat-server message_goldens` verifies message count, content, ordering and absence of duplicates per framework across all three views (span / trace / session). Fixtures are captured OTLP payloads under `server/tests/fixtures/messages/<suite>/<sample>/`; capture with `misc/capture-message-fixtures.sh [suite] [sample]` (needs model credentials), then record with `UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens` and review the diff. Invariants (scope containment, per-trace dedup, tool-id correspondence, no empty thinking, determinism) hold independently of the goldens, so a blindly regenerated snapshot still fails on real defects. `UPDATE_GOLDENS=1` writes the files but still exits non-zero when an invariant was violated, so known-bad output cannot be committed as reviewed. See `server/tests/fixtures/messages/README.md`.

**ClickHouse parity**: `make test-clickhouse` starts a pinned container and runs
`server/src/data/clickhouse/parity_tests.rs`, which inserts one span set into both analytics
backends and requires every read method to return identical rows (DuckDB is the reference). Skips
with a message when `SIDESEAT_TEST_CLICKHOUSE_URL` is unset, so `make check` stays green without
Docker; CI runs it against a service container. Two things it found: `get_trace_tags_options` failed
outright on ClickHouse (non-Nullable `arrayJoin` into `Option<String>`), and the ClickHouse schema
TTLs spans older than 90 days, so a fixture with fixed past timestamps disappears there and
persists in DuckDB.

**Message views all use `process_spans`**: the span, trace and session endpoints differ only in their row set, not their pipeline. `process_feed` (DESC, newest-first) belongs to the **project feed** endpoint (`routes/otel/feed.rs`) — using it for a session view produces ordering no session request can return.

The project feed pages by cursor but does **not** reconstruct page-locally: it takes the page's spans, loads every trace they name in full (`MessageQueryParams::trace_ids`), reconstructs, then keeps the blocks whose span is on the page (`scope_feed_to_page`). Otherwise a trace split across two pages was reconstructed twice from half its spans each time, and the turn each generation span re-sends had nothing to collapse against, so both pages returned it. Page totals come from the page's own rows, because the pipeline now sees more than the page shows. Still page-local by nature: a replay crossing *traces* within a session is recognised only when both traces are on the page, and pages are chosen by ingestion time while each is ordered by message time, so concatenating pages is not globally ordered.

A `start_time`/`end_time` window on the feed selects spans that **overlap** it — lower bound on the span's end, upper bound on its start — and then `apply_time_window` filters the reconstructed messages. Both halves are needed: a completed response carries its span's *end* time, so bounding rows by the start alone dropped a span that finished inside the window, and skipping the message-level filter returned a span that started inside it and finished after. A page whose messages are all filtered out still reports `has_more` and a cursor, because both describe the row page rather than the answer.

**Message-view totals come from the entity, not the pipeline.** The trace endpoint reports the trace's tokens and cost and the session endpoint the session's. A message query only returns rows carrying messages, tools or an error, so summing what the pipeline saw made a span billed with nothing to show count as free, and skipped the parent/child billing dedup that keeps a nested generation from counting twice.

- span: `WHERE span_id = ?`, no content filter
- trace: if the trace has a session id, the query loads **the whole session** so cross-trace prefix stripping can run, then `scope_feed_to_trace` narrows the result; otherwise `WHERE trace_id = ?`
- session: `trace_id IN (SELECT trace_id WHERE session_id = ?)` — every row of every trace in the session, not just rows carrying the session id

Trace and session queries apply `MESSAGE_CONTENT_FILTER`, so rows with no messages, tools or error never reach the pipeline. A span view can hold **more** messages than its trace view: each generation span re-sends the whole history, which trace-level dedup collapses.

**A trace-list filter selects traces, never span rows.** Each entry of the `filters` array becomes its own per-trace predicate, in both backends (`trace_aggregate_expression` / `ch_trace_aggregate_expression`):

| Column kind | Predicate | Why |
| --- | --- | --- |
| Aggregate (`trace_name`, `duration_ms`, `start_time`, `end_time`, all tokens and costs) | condition on the displayed aggregate, in one `GROUP BY … HAVING` subquery | tokens and cost are sums over a trace's spans; a row-level comparison hid a trace displaying 3000 tokens from `> 2500` and returned it for `< 1500` |
| Displayed-but-per-span (`session_id`, `user_id`, `environment`) | condition on `trace_display_first(col)` — the earliest span that has a value, which is what the row shows | these live on the spans that know them, usually the root alone, so `session IS NULL` was true of every such trace and returned traces displayed *under* a session |
| Span attribute the row does not show (model, provider, framework, tags, trace id) | `trace_id IN (SELECT … WHERE <cond>)`, one subquery each; a **negated** operator becomes `trace_id NOT IN (<positive form>)` | ANDed on one row, two filters demanded a single span carrying both values — a session id on the root plus tokens on its child matched nothing. And "none of X" asked as written returned a trace that used X once and something else next, and dropped rows with no value at all (`NULL NOT IN (…)` is NULL) |

Negation uses `Filter::positive_twin()` (`NoneOf`→`AnyOf`, `IsNull`→`IsNotNull`, `Ne`→`Eq`) and negates the *subquery*: for an entity made of many spans, "not this" means **no** span, not "some span was something else".

Token and cost expressions come from the same `gen_totals` SQL the list projects, so the filter cannot drift from the number on screen. Being trace-level, the conditions live in the shared WHERE and so apply identically to the count, the totals CTE and the page. The `from`/`to` time window is *not* part of this: it still selects span rows, which is what a time-bounded list means — so the filter's own totals carry the same window.

Cost of the aggregate path, measured on a temp DuckDB (4k traces / 20k spans, then 20k / 100k): an aggregate filter roughly doubles the unfiltered list query (219→446 ms, 680→1447 ms) — linear, the ratio steady, no cliff; an attribute filter is *cheaper* than no filter because it prunes. On ClickHouse the clause is embedded twice per statement, so each aggregate filter's scan can run twice there. Hoisting it into a named CTE would fix that and means reworking the bind scheme (`bind_to_n` binds the whole set N times), which is the part that must not break.

**A session-list filter selects sessions, then the list aggregates each whole.** `matching_sessions` picks the sessions (from the rows that name one, exactly as the count query counts them); `session_traces` then takes *every* trace of those sessions. One predicate for both questions returned a partial session: two traces in different environments, filtered to one, listed with that trace's times, counts and cost while opening the session showed both.

**URLs**:

- API: `http://127.0.0.1:5388/api/v1/project/default/otel/traces/`
- UI: `http://localhost:5389/ui/projects/default/observability/traces`

## Quality Standards

- **Universal solutions**: Not fragile, works for all frameworks
- **Review checklist**: Gaps? Duplications? Architecture? Best practices?
- **Before implementation**: Find subtle issues, think outside the box, test for surprising issues
- **After implementation**: Is plan fully implemented? Any gaps?
- **Production ready**: Consistent code, remove unused code (except migrations)

**Frontend**: Avoid generic "AI slop" aesthetic. Make creative, distinctive, context-specific designs. Vary themes.

## Key Types

### NormalizedSpan (`data/duckdb/models.rs`)

Core database entity for OTEL spans:

```
Identity:       trace_id, span_id, parent_span_id, session_id, user_id
Classification: span_name, span_category, observation_type, framework
Time:           timestamp_start, timestamp_end, duration_ms
GenAI Core:     gen_ai_system, gen_ai_request_model, gen_ai_response_model
GenAI Params:   gen_ai_temperature, gen_ai_top_p, gen_ai_max_tokens, ...
Tokens:         gen_ai_usage_input_tokens, gen_ai_usage_output_tokens (i64, never NULL)
Costs:          gen_ai_cost_input, gen_ai_cost_output, gen_ai_cost_total (f64, never NULL)
Error:          status_message, exception_type, exception_message, exception_stacktrace
Preview:        input_preview, output_preview (truncated text for list display)
```

### ChatMessage (`domain/sideml/types.rs`)

Universal message format (SideML):

```
role:         ChatRole (System, User, Assistant, Tool)
content:      Vec<ContentBlock> (Text, Image, ToolUse, ToolResult, Thinking, ...)
tools:        Option<Vec<JsonValue>> (tool definitions)
tool_use_id:  Option<String> (links tool result to tool use)
finish_reason: Option<FinishReason> (Stop, Length, ToolUse, ContentFilter)
```

### Key Enums

- **ObservationType**: Generation, Embedding, Agent, Tool, Chain, Retriever, Span
- **SpanCategory**: LLM, Tool, Agent, Chain, DB, HTTP, Storage, Other
- **ContentBlock**: Text, Image, Audio, Document, ToolUse, ToolResult, Thinking, Context, ...

## Common Gotchas

1. **Tokens/costs are never NULL** - Always `i64`/`f64` with default 0, not `Option<T>`
2. **shadcn/ui is read-only** - Never edit files in `components/ui/`, wrap or use className
3. **No constructor parameter properties** - TypeScript `erasableSyntaxOnly` forbids them
4. **Query key structure matters** - Use `extractListParams<T>()` not magic indices
5. **Role normalization is lossy** - Unknown roles default to User, check `try_from_str` first
6. **gen_ai.system is deprecated** - Use fallback chain: `gen_ai.provider.name` → `gen_ai.system`
7. **Messages come from two sources** - Events (OTEL events) and Attributes (framework-specific)
8. **Framework detection is ordered** - First match wins, see `attributes.rs` detection order
9. **Birth time ≠ event time** - Messages use birth_time for sorting (earliest occurrence)
10. **No service layer** - Routes call repositories directly, keep business logic in domain/
11. **Config validation** - Ports must be >0, S3 requires bucket, port collision checked at startup
12. **ClickHouse timestamps** - Use `fromUnixTimestamp64Micro(?)` with `.bind(ts.timestamp_micros())`, never format strings
13. **ClickHouse distributed DELETE** - Use local table + `ON CLUSTER` clause, not distributed table
14. **ClickHouse no correlated subqueries** - ClickHouse silently breaks correlated `NOT EXISTS` on the same table (always returns true for EXISTS), and rejects correlated `NOT IN` (Code 48). Use materialized CTE + tuple `NOT IN` (e.g. `(trace_id, span_id) NOT IN (SELECT trace_id, parent_span_id FROM cte)`) to scope by trace without correlation. See `build_dedup_lookup_cte()`/`TOKEN_DEDUP_CONDITION` in `query.rs`
15. **ClickHouse Nullable propagation** - Expressions on `Nullable` columns stay Nullable: `nullable_col = 'val'` → `Nullable(UInt8)`, `max(Nullable(UInt8))` → `Nullable(UInt8)`. Wrap with `coalesce(..., default)` or `assumeNotNull(...)` to strip
16. **ClickHouse Decimal64(6) maps to i64** - The `clickhouse` crate deserializes `Decimal64(6)` as `i64` (value × 10^6), not `f64`. Use `to_decimal64()` from `utils/clickhouse.rs` for writes, `toFloat64(col)` in SELECT for reads
17. **ClickHouse SELECT aliases visible in WHERE** - Unlike standard SQL, ClickHouse SELECT aliases shadow original column names in WHERE. Alias `toInt64(...) as timestamp_start` then `WHERE timestamp_start >= ...` compares Int64 vs DateTime64 → overflow. Use distinct alias names (`start_time`, `end_time`)
18. **ClickHouse aggregate nullability** - `avg()`, `sum()`, `count()`, `dateDiff()` return non-nullable types. Don't use `Option<T>` for these in Rust structs. But `max(nullable_expr)` stays Nullable
19. **ClickHouse JOIN requires equality** - JOIN ON must have at least one equality condition. Range-only joins (`ON a >= b AND a < c`) are rejected. Use cross join + WHERE for bucketing, then LEFT JOIN on equality
20. **Identity rules differ by block type** - Tool calls dedupe by (name+input), tool results by tool_use_id when present (else content)
21. **History-only messages are filtered** - If a message appears only in history (no current-turn equivalent), it's excluded from feed
22. **A span id is unique only within a trace** - 8 bytes, so two traces reuse ids freely. Anything matching spans by id must carry `trace_id` too: the feed cursor needs it in its key (a page boundary between two same-id spans skipped one for good), and DuckDB's token dedup needs it in both parent/child `NOT EXISTS` clauses (a generation in one trace suppressed the tokens of a same-id span in another)
23. **"GenAI only" is `genai_span_predicate`** - an observation type *or* any GenAI attribute, one definition shared by both backends. Testing `observation_type != 'span'` hides transport-level instrumentation (which records `gen_ai.*` on a plain span) and usage-only spans, which is what the span list, feed spans and filter options each did while the trace list did not

## Memory Bank

Project uses context files for session continuity. Check these before starting work:

- `CLAUDE-activeContext.md` - Current goals and progress
- `CLAUDE-patterns.md` - Established code patterns
- `CLAUDE-decisions.md` - Architecture decisions and rationale
- `CLAUDE-troubleshooting.md` - Common issues and solutions

Update these files when making significant changes or decisions.
