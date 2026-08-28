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

Cross-trace prefix strip (session mode): attribute-input only, per-span reset, role+content subsequence match.
Event-based frameworks (Strands) stay trace-independent; no cross-trace stripping.

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
