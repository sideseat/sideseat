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
2. FLATTEN     One BlockEntry per ContentBlock (with metadata)
3. CLASSIFY    Determine is_output for each block
4. MARK HISTORY Eight-phase detection (see below)
5. DEDUP       Identity-based, keep highest quality version
6. SORT        (birth_time, message_index, entry_index)
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
- Tool results: hash(content) — tool_use_id ignored
- Regular: hash(trace_id + role + content)

**Quality scoring** (higher = preferred in dedup):

```
+100  Non-history block
+10   Has finish_reason
+5    Enrichment content (thinking)
+2    Event source (vs attribute)
+1    Has model info
```

**Semantic ordering** (for same birth_time):

```
User(0) → System(1) → Assistant+ToolUse(2) → Tool(3) → Assistant+Text(4)
```

### OTel GenAI Conventions

**Status**: Development (unstable). Use fallback chains:

```rust
get_first(attrs, &[keys::GEN_AI_PROVIDER_NAME, keys::GEN_AI_SYSTEM, ...])
```

**Namespaces**: `gen_ai.*` (OTel), `llm.*` (OpenInference), `ai.*` (Vercel), `langsmith.*`/`langgraph.*`

**Frameworks**: StrandsAgents, LangGraph, LangChain, CrewAI, AutoGen, PydanticAI, OpenInference, Logfire, MLflow, LiveKit, AWS Bedrock, Azure OpenAI, Azure AI Foundry, Google ADK, Vertex AI, Vercel AI SDK

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
- `GET /spans`, `/spans/{trace_id}/{span_id}`, `/spans/{id}/messages`
- `GET /sessions`, `/sessions/{id}`, `/sessions/{id}/messages`
- `GET /sse` (real-time)

**Raw span data**: Add `?include_raw_span=true` to span/trace endpoints to get the full OTLP span JSON (attributes, events, links, resource).

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

## Development

**Database**: `./.sideseat/duckdb/sideseat.duckdb` — DuckDB is locked while the server runs. Always check raw data via API, not direct DB access.

**Test data**: `uv run strands <sample> --sideseat` (from `misc/samples/python/`). Samples: tool_use, mcp_tools, structured_output, files, image_gen, agent_core, swarm, rag_local, reasoning, error

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

## Memory Bank

Project uses context files for session continuity. Check these before starting work:

- `CLAUDE-activeContext.md` - Current goals and progress
- `CLAUDE-patterns.md` - Established code patterns
- `CLAUDE-decisions.md` - Architecture decisions and rationale
- `CLAUDE-troubleshooting.md` - Common issues and solutions

Update these files when making significant changes or decisions.
