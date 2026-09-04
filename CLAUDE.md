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

The **span view** does no cross-trace stripping at all: it loads one span, which is what that view means —
"what this span carried", including the history it re-sent. A span view can therefore hold more messages than
its trace view, and `replay_matching_complete` reports `true` there because nothing was left unmatched, not
because the span holds no repeated history.

The search reports when it is **not exhaustive** (`FeedMetadata::replay_matching_complete`, omitted from
the response when true). The budget is a resource guard, and a guard that silently changes the answer is
what a caller cannot reason about - so either the stripping is complete, or the response says it may repeat
history. Failed states are memoised, which is what makes the search complete for shapes that used to
exhaust it: `(replay position, set of claimed occurrences)`, so nine interchangeable calls do not re-explore
`9!` orderings of the same dead end. Measured envelope: complete to 64 interchangeable two-level branches
(128 blocks) and to 7 three-level branches (21 blocks); beyond that it under-strips and says so.

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

**A repeated *plain* message with no id is one message, deliberately, and the reverse costs more.** A tool
call carries a rank among same-shaped calls of its response (`call_repeat_ordinals`), because a provider's
distinct ids are proof of a genuine repeat; plain text has no such evidence. So a user who twice sends
`retry` in one trace, or a conversation snapshot that lists the same turn twice, collapses to one — and a
snapshot's repeated entry is treated as a re-statement, not a second occurrence. Both were re-examined
against the corpus: ranking plain messages by carrier position turned ADK's `"For context:"` separator
(repeated verbatim within one span) into duplicates, and treating a one-message cross-trace match as a new
turn duplicated a real LangGraph replay that re-sends a single prior prompt. The cautious reading
under-reports a genuine repeat that no framework in the corpus actually produces; the eager reading
over-reports one that several do, which a user sees as duplicates. This is a real limit, not an oversight:
distinguishing a genuine identical repeat from a replay needs durable per-occurrence evidence the telemetry
does not carry.

**Two spans that start at the same instant are ordered by their span ids, and correlation is a forward pass
over that order** — so an id-less tool result whose call lands on the far side of the tie stays uncorrelated,
sorts before the call it answers, and (with two identical results) can collapse into one. Reachable whenever a
tool span and the generation span that called it share a millisecond, which is ordinary.

The obvious repair — a result may also claim a *following* call in a span that starts at the same instant —
was implemented and reverted. Both variants (an equal alternative to the preceding rule, and a fallback used
only when no preceding call exists) changed `adk/tool_use` for the worse: ADK's tool and generation spans do
tie, so the relaxation lets one result claim a call a later result needed, and three results that *had* ids
lost them, with their order moving in front of their calls. Rules 3 and 4 have nothing but document order to
go on, and relaxing them where that order is arbitrary trades a rare mis-order for a common mis-pairing. A
real fix needs causal evidence that is not the span id — the ordering redesign's partial order
(`order_graph`), where call→result is a constraint rather than a position.
`an_idless_result_is_correlated_only_when_span_ids_order_its_call_first` asserts both spellings, so the limit
is measured rather than described.

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

**A token total and the charge beside it must describe the same call.** Providers disagree about whether
cache and reasoning counters sit *inside* `input`/`output` or *beside* them - OpenAI counts cached tokens in
its prompt total, Anthropic bills cache creation on top of an `input_tokens` that excludes it, Gemini is the
mirror image again (cached content inside, thoughts beside). Two independent flags, because one boolean
mis-bills every provider that is not OpenAI-shaped.

The convention is keyed on the **`litellm_provider` of the catalogue entry that priced the call**
(`cache_counters_are_separate_for_provider`), not on a re-parse of `gen_ai.system`. The lookup resolves a
provider from the *model name* as well as the attribute, so `anthropic.claude-3-haiku-...` is priced from a
Bedrock entry even when the system attribute says `AWS` or says nothing - and reading `system` separately
meant such a call was charged at Bedrock's rates and counted under OpenAI's: `input=10, cache_read=1200,
output=5` reported 15 tokens where 1,215 were billed, with the ten ordinary input tokens subtracted away and
billed at nothing. `SpanCostOutput.resolved_provider` carries the answer to `enrich::corrected_total_tokens`,
so the total is derived from the same fact as the charge; the extractor's `system`-based synthesis remains for
spans nothing priced, where there is no better information.

The web adds **nothing** to a side (`lib/token-breakdown.ts`): a trace row aggregates spans that may come
from different providers, so the browser cannot know the convention. It takes the server's total as the
anchor and reports the residual as its own line, naming the counters only when enumerating their subsets
identifies them uniquely - an unlabelled residual is a gap in the explanation, a wrongly labelled one is a
false statement about the bill.

**A "supplied" flag describes the span, not the source that read it.** `cache_read_supplied` recorded only
whether a *flat attribute* existed, so a counter the Logfire `response_data` path filled in was unprotected and
CrewAI's `cached_prompt_tokens` overwrote it — 17 became 100, and the cache charge followed the wrong number.
Every source that supplies a counter now marks it supplied, and the "no counters at all" diagnostic reads all
four flags, which is what keeps the next source maintaining them. The total has a presence flag too
(`total_supplied`): the embedded total is taken through `max`, which against an explicit flat total can only
replace the provider's own statement with the framework's — flat `500/600` with a flat total of 1,100 became
2,000.

**A gate asks about the thing it gates — the same defect, three more times, in message extraction.** Each
was a carrier's read guarded by a question about a *different* carrier, and each cost the answer:

| Gate | What it dropped |
| --- | --- |
| "does this span have any recognised event?" suppressed **every** attribute carrier | A span whose question arrives as `gen_ai.user.message` and whose answer sits only in `output.value` returned the question alone. Removing the gate changed nothing across all 111 captured fixtures — carrier claiming already prevents the duplication it was defending against — so it was protecting nothing and losing answers in shapes nobody had captured |
| Logfire's `if !found` covered `response_data` as well as `request_data` | `events` holding only the question set `found`, so the answer in `response_data` was never read. `request_data` **keeps** its precedence: it duplicates the input side that `events` also carries, and with no captured Logfire fixture nothing can show the two spellings hash alike, so reading both risks a duplicated question. Narrow on purpose |
| `wrap_plain_data` wrapped only objects | `output.value = "the answer"` and `output.value = [{"type":"text",…}]` normalised to a message with no blocks. Scalars and content-block lists are wrapped now; a list of *messages* still is not, since that is expanded upstream |

Each shape is a `_synthetic` fixture, so the answer invariant fires on it with the fix reverted — which is how
each was confirmed. That invariant is what makes this class detectable at all: `assert_has_an_answer` says
"the reply to the final turn is missing rather than merely out of order", and it fired the moment a fixture
carried the shape.

**A state object can hold the conversation *and* the answer** (`extract_langgraph_messages`). Two more of the
same class, both found by probing shapes no fixture had:

- CrewAI running through LangGraph writes the history under `messages` and the final answer under `raw`.
  Reading only the list still **claimed the carrier**, so the extractor that knows `raw` never ran and the
  trace showed the question with no reply. A plain-string `raw` beside the list is now read as the assistant's
  reply — `raw` is this file's own vocabulary for that member, and only a string is taken, since anything
  structured is a state member rather than a reply.
- LangGraph state is a dict its nodes write into, so `{"state": {"messages": […]}}` is ordinary and yielded
  nothing: the search looked at the top level and at direct values only. Bounded recursion now
  (`LANGGRAPH_STATE_DEPTH`), bounded rather than unbounded because the point is to find a state member, not to
  trawl a tool's arguments for anything message-shaped.

Corpus-neutral: none of the nine LangGraph or nine CrewAI fixtures changed. The AutoGen mirror
(`body` and `log.body` carrying one `LLMCall`) was probed in the same pass and is **not** a defect — the
duplicate collapses, which is what content-based identity is for.

**One datum written twice is not two occurrences** (`deduplicate_content_blocks`). A tool result's parts are
collapsed when identical *after* normalisation, which is right for a carrier that writes one result in two
encodings (Vercel sends raw and `{type:"json",value:…}` in one array) and wrong for a tool that genuinely
returned the same part twice. The two are told apart by the **source**: two encodings arrive as different JSON
that normalises to one, a genuine repeat as two identical values. Reachable only from the unit level - no OTLP
path in the corpus reaches that branch - so it is pinned there, with the end-to-end repeat pinned by
`_synthetic/repeated_tool_result_parts`.

**A gate asks about the thing it gates.** CrewAI reports usage twice - flat `gen_ai.usage.*` attributes and an
embedded `token_usage` object in `output.value` - and the block reading the embedded one was gated on a *side*
being missing. That is the wrong question for everything else the block reads: with both flat sides present,
`{prompt: 500, completion: 600, total: 2000, cached: 100}` stored a total of 1,100 and no cache at all, because
the embedded total and the cache counter were unreachable. The outer gate is now just "is this CrewAI"; each
value inside already decides for itself whether it was supplied, which is what the `*_supplied` flags are for.

**Metric exemplars are kept in full** (schema v3, `exemplars`). A histogram carries one per bucket - that is
what they are for, getting from a slow bucket to the slow trace - and keeping only the first discarded every
link but one. Both the array and the six indexed flat `exemplar_*` columns derive from **one** storability
filter, so they cannot disagree about the same data point; a bad exemplar clock drops the exemplar, never the
measurement. Attributes are stored through `attrs_to_typed_json`, since an exemplar exists to lead back to
the exact call and the stringifying path made `status_code=200` and `"200"` the same value.

**An SDK reports what it actually did.** `AWSInstrumentor.instrument()` returns whether it patched anything,
because recording success for telemetry that will never be produced also blocked every later retry; the
JS SDK's `forceFlush()` and `shutdown()` both return whether every span was exported, because `diag` is a
no-op until the host installs a logger, so a `void` return reported the loss nowhere at all.

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
| Docs homepage tabs | `docs/src/content/docs/docs/index.mdx` | SDK tab (`install:`) + Direct OTLP tab. A **curated subset** — the homepage shows the common integrations and an even shorter direct-OTLP list, so a new framework belongs here only if it is one of those; the other three are the complete set |
| Telemetry config UI | `web/src/pages/configuration/telemetry-frameworks.ts` | `install`, `altInstall`, `altCode()` per framework entry; `telemetry.tsx` only renders it |
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

**A migration can only append a column, so the fresh schema declares added columns last.** DuckDB's metrics
writer is an `Appender`, which is *positional*: with `datapoint_id` declared second in the fresh schema and
appended last by the migration, an upgraded database had every value written one column across — failing on
type conversion if it was lucky. Fresh and upgraded must agree on the physical order, and the only order a
migration can produce is "at the end". `a_v1_database_upgrades_to_the_same_column_order_as_a_fresh_one`
compares `duckdb_columns()` *ordered by position*, against a v1 database that is populated first — and
writing it found three defects in one migration: the column order, an `UPDATE`-then-`SET NOT NULL` pair
DuckDB refuses in one transaction, and an `ALTER` DuckDB refuses at all while indexes depend on the table
(so it failed on *every* real database it existed to serve). SQLite's writers all name their columns, but
`migration_added_columns_are_declared_last` pins the same property there against the schema text, because
the trap is one mistake away.

**A store holding bytes must be at least as reachable as the rows that name them.** One rule
(`Sharing`, `validate_store_sharing`), checked once at startup, because the same mismatch produces three
unrelated-looking silent failures — and SideSeat splits bytes from their naming record three times over:

| Configuration | What breaks | Why it is silent |
| --- | --- | --- |
| PostgreSQL + `files.storage: filesystem` | Replica A writes the bytes to its own disk and the row to the shared database | B finds the row, cannot serve the content and cannot clean it up; replacing A loses the content with the row still promising it. Reads as a producer that never sent an attachment. |
| PostgreSQL + a keychain/file secrets backend, auth on | An API key created on A hashes under A's pepper | The lookup is *by hash*, so B finds nothing and answers a plain 401 — indistinguishable from a forged key. Authenticated ingestion fails depending on which replica the balancer picked. |
| The same, for the JWT signing key | A session token signed by A | Rejected by B, so a browser is signed out at random. |

The transactional backend is the reference, because that is where the naming rows live: SQLite means the
deployment is one instance by construction and everything matches; PostgreSQL means anything holding bytes
it points at has to be shared too. Refused at startup with the one-line remedy in the message, rather than
warned about — every symptom above sends an operator looking in the wrong place.

**Horizontally scaled, ephemeral instances**: what is and is not shared, because the answer differs per
mechanism and getting it wrong is invisible until there are two instances.

| Mechanism | Shared? | Why that is correct |
| --- | --- | --- |
| Project rows and the project→org mapping | **Not cached at all** | The project row *is* the deletion fence, and a process-local cache cannot be invalidated by another instance: A caches a live project, B tombstones it and clears only B's memory, and A keeps answering from its hit. Re-reading the fence after a fill closes the fill race but not this one, because a hit never reaches the database. The cost is a primary-key lookup. |
| Reconstruction cache (`feed/cache.rs`) | No, process-local | A memo over a *pure function* of the rows. Any instance computes the same answer from the same digest, a cold instance recomputes it, a dying one loses only the saving. N instances change the hit rate, not the answer — `a_cached_reconstruction_equals_a_fresh_one` checks byte equality over the corpus. Sharing it would need a version key to avoid serving the previous build's answers, and a hand-maintained constant is a hole. Filling is **coalesced** (`get_with`), because a cold cache is the normal state here — every new replica starts empty and a deploy replaces them all, so eight readers arriving together at a fresh replica is not an edge case, and as a check-then-compute pair it was eight simultaneous reconstructions of one answer. |
| The pricing catalogue (`domain/pricing`) | Per-instance file, but the **verdict is a function of the catalogue alone** | Cost is computed at ingestion and *persisted*, so if two replicas hold different prices the stored cost depends on which one the balancer picked. Two rules keep them agreeing. Which catalogue loads is decided by **provenance** - a sidecar recording the `embedded_digest` of the build that wrote the file - so a file predating this build is replaced by this build's snapshot, whether it came from a sync or from an older release's embedded copy. A model *count* was the previous rule and is not a statement about freshness: a catalogue bloated with retired models is bigger and more wrong, and it won on size and could not be dislodged by upgrading. Whether a sync is *accepted* is likewise decided only by the payload (a fixed structural floor), never by what this instance happens to have priced - that was tried, and it made replica A refuse a catalogue replica B accepted, plus the observation set was caller-fillable with junk model names. |
| Root secrets: the API-key HMAC pepper and the JWT signing key | **Only with a shared backend** (`env`, `aws`, `vault`) | Auto-detection picks a *per-instance* store (keychain, credential manager, secret service, file), and an API key row holds `HMAC(key, pepper)` and nothing else. So with a per-instance backend, a key created on A is not merely unknown on B — it is unverifiable there, and the lookup is by hash, so B answers a plain 401 indistinguishable from a bogus key. Authenticated ingestion then fails on whichever replica the balancer picked. Both getters also used to answer a read *error* by generating a replacement, which destroyed the only copy: they now refuse to start (`an_unreadable_backend_never_replaces_a_root_secret`). |
| File extraction cache | No, per pipeline | Same shape: a memo keyed by content hash. |
| Trace ingestion topic | Yes, when Redis is configured (`stream_topic` + consumer groups + `claim_stuck_messages`) | At-least-once across instances; ingestion is idempotent by span id, so a redelivery rewrites rather than duplicates. With the **default in-memory backend the queue is skipped entirely** and the request writes before answering - see below. |
**A registration belongs to the socket that made it, not to the client id.** The SDK reuses its `client_id`
across reconnects on purpose — that is how it re-registers — so teardown keyed on the id deleted the *live*
socket's entries when the superseded one finally timed out: the listing went empty and AG-UI answered
`registration_not_found` while the new connection sat there healthy. Reachable on any asymmetric network
failure, where the client notices the break before the server does. The entry now carries
`owner_connection_id`, teardown is `remove_all_for_connection`, and the removal is conditional on still being
the owner, since a reconnect may take an entry over between the index read and the delete.

**A `client_id` identifies a socket only together with its project.** It is the SDK's own value, so two
projects can present the same one, and the control path matched on it alone — so an `agent.invoke` carrying
one project's run input and request id was delivered to whichever socket claimed that id on the instance, and
the reply came back attributed to the invoking project's run. Every `ConnectionControl` variant carries the
project now and `find_local_connections_for_client` requires both. Within *one* project a `Write` key can
still take over any registration by name, which is not a hole but the scope's meaning: the endpoint's own
comment says registering replaces whatever held the name.

**A displaced connection is closed, not asked to leave.** The protocol says a `replaced` socket does not
survive (close 4000) and the server only queued the notice — nothing closed the socket or ended its receive
loop, so the guarantee rested on the client's good manners. The official SDKs disconnect; one that ignored the
frame kept registering and publishing events under a name it no longer owned. `ConnectionHandle::close` fires
after the notice is queued, and the select loop observes it ahead of the inbound arm so a chatty socket cannot
starve its own closure.

| SDK registrations (`data/registrations`) | **No — and the routing around it assumes yes**, warned about at startup on the same signal the sharing rule uses | The store is process-local (`MemoryRegistrationStore` is the only implementation), while everything built on it is cross-instance: an entry carries `owning_instance_id` and the AG-UI invoke publishes to `connection_control:{instance_id}` over the Redis broadcast. So the control plane spans instances and the *directory* does not: an SDK whose WebSocket landed on A is `registration_not_found` on B, and `GET /registrations` shows each replica its own subset. Presence and AG-UI invoke are therefore single-instance features today; the TTL sweeper's own comment records that a shared store would additionally need leader election to avoid N duplicate `Expired` events. Warned rather than refused, because the channel is optional and many shared-database deployments never touch it — and the invoke route's 404 names the boundary, since "not found" and "registered on another instance" are otherwise indistinguishable. |
| Metrics | Not queued at all — written inside the request | An in-process queue made a 200 mean "buffered", so a crash lost records the exporter had counted as delivered. A failure is a 503 the exporter retries. |
| Metric datapoint identity (`domain/metrics/identity.rs`) | Yes — the id is a digest of the datapoint's own fields | Written little-endian and length-prefixed by hand rather than through `std::hash`, because `Hash for usize` writes native-endian bytes and two replicas on different architectures would then disagree about whether a datapoint is the same one. Disagreeing means a duplicate on one side or a deletion on the other. |
| Logs / SSE topics | No — `topic()` builds an in-process channel whatever the backend | So an instance consumes only what it published: no cross-instance duplication. Nothing stores logs, and the endpoint says so via OTLP `partial_success`. |
| Deletion tombstones (`projects.deleting_at`, `organizations.deleting_at`) | Yes, they are rows | One compare-and-set decides the owner across all instances, and every instance's write path consults it. |
| Deletion sweeps | Run on every instance | Every step is idempotent and the claims are CAS-guarded, so concurrent sweeps duplicate work rather than corrupt state. |
| Migrations | Serialised by `pg_advisory_lock` | Concurrent instances starting together cannot race the schema. |

**Deleting a *trace* leaves a tombstone too** (`deleted_traces`), and it takes **four** steps, because no
one of them is a guarantee on its own. The file fence protects a file from cleanup while something
references it; it cannot stop the reverse. Files and their associations are written **before** the
analytics row that names them - deliberately, so the surviving failure is a reclaimable orphan rather than
a dangling reference - which means a batch already in flight when `delete_traces` runs commits *after* the
deletion reclaimed its bytes, producing a span carrying a `#!B64!#` reference to nothing for a trace the
caller was told 204 for. No elapsed time bounds that: a queued batch can be redelivered minutes later.

| Step | Where | What it closes |
| --- | --- | --- |
| Tombstone **before** the analytics delete | `delete_traces`, `delete_sessions` | No instant exists in which a trace is deleted and not yet tombstoned. A session is deleted *by* deleting its traces, so it records the same tombstones. |
| Check immediately **before** the write | `drop_spans_for_deleted_traces` | The common case: a batch that arrives after the deletion never writes. Applied on **both** paths - the queued batch and `run`, which is the default in-memory ingest, the shutdown drain *and* claimed-message recovery. Having it on only one was a deleted trace re-postable through the ordinary configuration. |
| Re-check immediately **after** the write | `collect_spans_written_for_deleted_traces` | The tombstone is a row in the transactional store and spans go to the analytics store, so nothing makes the pair atomic. A deletion landing inside that window is compensated: the spans are deleted and the batch's associations released, *before* any SSE event announces them. |
| A leased, backed-off **sweep** | `advance_pending_deletions` | The crash between the write and the re-check. Same discipline as the deleted-project records - claimed exclusively (`FOR UPDATE SKIP LOCKED` on PostgreSQL), leased, geometrically backed off to a daily floor, batch-capped - because the records are long-lived and what has to be bounded is the *rate*. "Quiet" requires no error **and** nothing still readable, asked directly rather than inferred from a delete's row count, which means different things per backend. The **files are collected only once the rows are provably gone**: doing it regardless meant a transient delete failure left readable rows pointing at bytes the sweep had taken, and a scheduling flag cannot undo a destructive cleanup. |

**A session gets all four steps too**, not just the pre-write check: its own tombstone, a post-write pass
that tombstones the traces of any session deleted mid-write (handing them to the trace protocol, so there is
one place that knows how a trace is taken away), and its own leased sweep that re-resolves the session's
traces *now* rather than from the deletion's snapshot — which is precisely what a late trace escapes.

**A byte deletion is gated on the claim it read, not on having read one.** The recovery path for an abandoned
claim scans stale rows and then deletes their objects - and between those two steps the row can be released
(by a worker whose own object delete failed) or deleted and recreated by an ingestion that associates the same
content hash. `delete_file_if_unreferenced` re-checks *afterwards*, which is too late: the old code logged
"referenced again after its bytes were removed" at error level, and logging a loss is not preventing it. The
scan now returns the claim's own value and `reclaim_stale_file` is a compare-and-set on it, which also
refreshes the claim so a second worker holding the same reading is refused. The same shape - refresh the
tombstone to lease the work, without lifting the fence - now leases abandoned **project** and
**organization** cleanups (`reclaim_stale_project`, `reclaim_stale_organization`), which were neither claimed
nor capped: every replica resumed every one of them, and a backlog ran ahead of the leased trace and session
sweeps in the same pass, so a stuck association could go unreclaimed indefinitely.

**A session deletion tombstones what it deleted, not what it resolved.** The route resolved the session to
traces, tombstoned those, then called `delete_sessions` - which *re-resolves*, so it also removed any trace
that had joined since. That extra trace lost its rows while keeping its file associations forever (the orphan
sweeper selects on zero references) and had no tombstone, so the trace sweep could not find it and the session
sweep could not either, because it resolves sessions through analytics rows that no longer existed; a later
child-only redelivery then carried no session id, passed both fences, and resurrected it.
`AnalyticsRepository::delete_sessions` returns the trace ids it removed, and the route tombstones and cleans
exactly that set.

**An association carries two facts, not a `provisional` boolean: `pending_writers` and `durable`.** Releasing
on a failure path by first reading whether the trace has rows is not a decision: a second batch can commit
between the read and the delete, and its file loses its protection. A single boolean flag was the first fix
and was not enough either — it could not express *several* batches referencing the same `(project, trace,
hash)` at once, which is the case that matters under concurrency. With a flag, whichever failed first deleted
the row a still-in-flight or just-committed peer depended on, orphaning its file. So each reference (create or
share) increments `pending_writers`; a commit sets `durable` (monotonic, permanently blocking deletion, since
one committed row backs the file); a failure decrements, and the release deletes only a **non-durable row
with no writer left**. A failing batch therefore can never orphan a file another batch committed or is about
to. Every referencing batch — not only the one that created the row — confirms or releases, because sharing
one makes a batch one of its owners. Verified by a parity scenario that commits one batch and releases its
peer and requires the row to survive on both backends. **SSE is published after every drop and compensation**,
and only for spans that survived them (keyed by `(project, trace, span)` — a span id is unique only within a
trace and a trace id only within a project); built beforehand, it announced spans the batch went on to
discard, so a reader saw a span appear and then never find it.

Keyed by `(project, trace)`, because a trace id comes from the client and two projects can present the
same one.

**A session gets its own tombstone** (`deleted_sessions`), because the trace tombstone cannot cover it. A
session is deleted *by* resolving it to trace ids and deleting those — and a trace of the same session that
arrives after that resolution was never in the snapshot, so nothing tombstones it and it recreates the
session the caller was told was gone. The session id is the durable fact; the trace list is one instant's
view of it. Checked on both write paths, before the trace check.

**An association referenced by a batch that does not commit is released** (`created_associations`,
`release_created_associations`). An association holds `ref_count` above zero and the orphan sweeper selects
on *zero*, so a file whose batch failed - or whose trace was tombstoned mid-flight - would occupy the
project's quota with nothing able to reclaim it. Every association the batch **referenced** is released, not
only ones it created: the release decrements `pending_writers` and deletes only a non-durable row at zero, so
releasing a shared one merely undoes this batch's own reference and cannot touch a peer's. `sync_ref_count`
follows, recomputing from the associations that remain rather than subtracting, so a concurrent batch holding
its own keeps the file. The four early-return paths between writing the files and writing the rows each
release, checked structurally (`every_early_return_between_files_and_the_write_releases_its_associations`)
because a new one compiles and passes every behavioural test.

**A crashed writer's increment is reclaimed by the deletion sweep, not by the ingest path.** The counter
cannot be decremented by a writer that is gone, so a stuck `pending_writers` outlives the batch. Deleting the
non-durable row from a *drop* path was tried and reverted: `durable = false` does not prove no analytics row
committed (confirmation runs after the write and can fail), and a concurrent batch that passed the fence
before the tombstone may be committing spans for that trace right now — so deleting the shared row leaves
readable spans pointing at a file nothing holds, which is the dangling reference the write-files-before-rows
ordering exists to prevent. Reclamation belongs where the information is: `advance_pending_deletions` deletes
the trace's rows, confirms by direct read that nothing is readable, and *only then* calls `cleanup_traces`,
which removes every association for the trace regardless of `pending_writers`.

**So every drop path has to leave a tombstone, or the sweep cannot find what it must reclaim.** The
deleted-session fence resolves its sessions to traces and now **records a trace tombstone for each** before
dropping them. Without it those traces were invisible to both sweeps: a writer associates a file for trace T
and crashes before writing any analytics row; the session is deleted, but T was not in the deletion's
snapshot so nothing tombstoned it; the redelivery increments again and is dropped at the fence, decrementing
once — leaving `pending_writers` and `ref_count` at 1 permanently. The session sweep cannot discover T, because
it resolves sessions to traces *through the analytics store* and T has no rows there; the trace sweep cannot
either, because it walks tombstones and T has none. The tombstone hands T to the trace sweep, which is the
one path that can reclaim it. Best effort by design: a failure there is logged, not fatal — the spans must not
be written either way, and failing the batch would only reproduce the state it found.

The detection is not left to inference either: `confirm_associations` compares rows confirmed against
associations owned and reports a shortfall at error level, because that shortfall is precisely "a committed
span references a file nothing holds", and it used to be a silent `Ok(0)`.

**The schema is at version 2, and there is one migration.** Nothing above v1 was ever released, so the
fourteen historical steps that used to live in each backend described upgrades no database could need -
and replaying them meant replaying two table *rebuilds* against tables a v1 database does not have. The
single `v1_to_current` step reaches the destination directly. `a_v1_database_upgrades_to_exactly_the_fresh_schema`
builds a v1 database as "the current schema minus what the migration adds", walks it forward, and compares
`pragma_table_info` for **every** table against a fresh one - so a future migration that alters any table
is covered, not just the one that prompted the test.

A related trap, now removed by construction rather than by care: a multi-statement schema executed through
`sqlx::query` stops at its first `;`, **including one inside a `--` comment**, and the fragment after it is
a syntax error in a place nobody looks. It cost a debugging session in each backend. Every schema and
migration script goes through `raw_sql` now.

**Deleting a project or an organization is asynchronous, and the row is a tombstone.** The fence lives in
the transactional store and spans in the analytics store, so no transaction spans them: a writer can read
"live", have the deletion land underneath it, and commit afterwards. **No elapsed time bounds that** - a
blocking insert can outlive its statement timeout, an object store can retry, a container can be paused -
so there is no grace period. What the tombstone gives instead:

1. No *new* writer passes the fence (`project_accepts_writes`: a row exists **and** nothing has claimed
   it, so an absent project is refused as firmly as a claimed one).
2. Cleanup keeps running while the row exists, so a late writer's spans are deleted by the next sweep.
3. The row goes only after `PROJECT_TOMBSTONE_CLEAN_SWEEPS` consecutive sweeps found nothing; one that
   finds data deletes it and starts the count over. **Counting, deciding and deleting are one statement**
   (`record_project_sweep`) - as two, instance A could decide on the strength of a count that instance B
   reset in between and delete the row anyway, orphaning the row B had just found.
4. The count measures **windows, not sweeps** (`last_sweep_at`, on the *database's* clock). Every instance
   sweeps, so a bare increment let N instances reach the required number inside one interval - the barrier
   getting weaker the more instances you run. Per-instance clocks would also mean every instance read a
   different notion of now from the same row.
5. Project creation is fenced **by the insert** (`INSERT ... SELECT ... WHERE deleting_at IS NULL`). As a
   separate check it is a lost race: a project created just after its organization was tombstoned is live,
   returns 201, and accepts writes - and a client creating projects in a loop could keep the organization's
   deletion from ever finishing.

6. Removing a tombstone **records the id in `deleted_projects`**, in the same transaction. Finite evidence
   still loses to an arbitrarily delayed writer - it can commit after the row is gone, and then nothing
   knows the project existed - so the sweep keeps collecting rows *and files* that appear for a remembered
   id, and the records are kept **permanently**: any retention would be a bound on how late a stalled
   writer may commit. Discovery is bounded three ways, because permanent records re-checked at a fixed
   rate are unbounded lifetime work: **leased** (the claim pushes `next_check_at` out before returning, so
   a batch of storage listings that outruns a sweep interval is not re-claimed while it runs), **claimed
   exclusively** (`FOR UPDATE SKIP LOCKED` on the inner select - `WHERE id IN (SELECT … LIMIT n)` alone
   lets a blocked replica resume with a stale subquery snapshot and return the same id, the same mechanism
   as the file claim's), **backed off** (`next_check_at` materialised from `quiet_checks`, geometric to a
   daily floor - otherwise 100k historical deletions meant 100k storage listings per sweep, forever) and
   **batched**. Indexed on `next_check_at`, the due time *itself*: an index on an input to the eligibility
   expression bounds rows returned, not rows examined. "Quiet" requires no files **and** no rows **and** no
   error - deciding it from the analytics count alone counted a sweep that had just deleted a late writer's
   files, or one whose storage delete failed, as evidence the project had gone quiet. Nothing sweeps at
   startup: inline it made every new instance wait for work that is not urgent.
7. **Membership mutations** refuse for a tombstoned organization, as renaming and project creation do -
   writing into one is writing into something no read can see and the cascade is about to remove. The
   check is *inside* the mutation's transaction (`FOR UPDATE` on PostgreSQL): on its own connection it was
   a read-then-write a committing deletion defeats.
8. An **incomplete replay match reaches the client** (`replay_matching_complete` on both message and feed
   metadata, absent when true). The flag existed internally first and the DTOs dropped it, which made it
   worthless - the point is that the caller can act on it.
9. **`resource` is optional in OTLP**, so the project-id injector *creates* one. Skipping resource-less
   groups meant they were never attributed, and persistence substitutes `default` - telemetry
   acknowledged for project P and stored where P cannot see it.

An organization is tombstoned too, because deleting its row cascades its *project* rows away and those
rows are what the projects' cleanups depend on; its row goes once no project rows remain. Creating a
project under a tombstoned organization is refused with a **409** naming the reason.

**Every surface that reads or writes project data is authenticated when auth is on.** `auth.enabled` defaults
to **true** and `mcp.enabled` does too, yet three surfaces were mounted with no auth at all — so the shipped
configuration served one organisation's data to anyone who could reach the port:

| Surface | What was reachable | Fix |
| --- | --- | --- |
| MCP (`/api/v1/projects/{id}/mcp`) | spans, prompts, raw attributes, sessions, statistics for any project in the URL | `require_auth` + `verify_project_access` |
| SDK channel (`/ws`, `/registrations`, `/presence`, AG-UI `/runs`) | agent manifests **including system prompts**; registering an agent name its owner holds; invoking one | same two layers, per handler |
| gRPC OTLP | writes into any project named by an untrusted `x-sideseat-project-id` header, because `otel.auth.required` gated HTTP only | `GrpcIngestAuth` on all three services |

Two layers everywhere, because either alone is insufficient: `require_auth` establishes *who* is asking (and
passes through untouched when auth is disabled, so `--no-auth` development and the SDK samples are
unaffected), while `verify_project_access` turns a valid credential into one valid **for this project** — a
key from another organisation is otherwise perfectly valid. A request arriving with no auth context is
*refused*, so a future mounting that forgets the layer fails closed rather than open. On gRPC each service
owns its own `auth` field, so deleting a gate fails the build.

**An answer must not misdescribe what happened to the data.** Six shapes of this, all fixed:

- A **200 preceded a silent drop**: the durable queue acknowledges on publish, then the consumer discarded a
  span whose timestamp no backend can store — and nothing downstream can report back to a request that has
  already returned. Storability depends on the payload alone, so it is settled at the edge
  (`strip_unstorable_spans`) and reported; only what can be stored is queued.
- Every drop claimed **"unknown project"**. For a live project with an unstorable payload that is a lie, and
  404 tells an exporter to retry *elsewhere* — so it retried the same doomed span forever while its operator
  hunted a healthy project. `DropReason` is named by the answer it implies: `Gone` → 404, `Unstorable` → a
  success reporting `rejected_spans`.
- A **missing entity answered an empty 200** on all three message routes, while the detail routes beside them
  already 404. So a stale URL after a deletion, and any well-formed id that names nothing, rendered as "this
  trace exists and said nothing" - and for the span route there was no existence check at all, which that
  query makes free: it applies no content filter, so no rows *is* no span.
- A **correlated tool-call id was presented as the provider's** (`tool_use_id_correlated`). The pipeline
  computes it, `history.rs` depends on it, and serialization skipped it - so the UI printed an inferred
  reference under the same `tool_call_id` label as an observed one. It is now sent (absent when false) and
  shown as `inferred`, because a flag nobody can read is not a disclosure. Same shape as
  `replay_matching_complete`, which had already been fixed once for exactly this reason.
- The **realtime page turned a failed refresh into "no new data"**: every per-trace request was
  `.catch(() => ({ messages: [] }))` and an all-empty result returned early, so a 404 for a deleted project
  looked like a quiet one, and after a first success the page kept showing stale content indefinitely. It now
  counts what could not be served and says so, and it surfaces `replay_matching_complete` from the responses
  it was already fetching and discarding.
- **A stale detail response overrode fresh message totals.** The header prefers the token/cost breakdown (from
  the entity query) over the message response's own totals, so a detail request that failed after new spans
  arrived left fresh messages beside stale numbers with nothing saying so. The breakdown is withheld while
  that query is in error, which falls the header back to the totals that came *with* these messages.
- The UI **dropped the server's own completeness metadata**: `replay_matching_complete` was absent from the
  web contract, so a thread that may repeat history rendered as canonical — duplicated turns being
  indistinguishable from a model that repeated itself. A session over one page was truncated silently, which
  for a debugging tool is worse than an explicit gap.

**Acknowledge only what is durable.** A 200 on an OTLP endpoint has to mean the data is stored, and which
mechanism delivers that depends on the topic backend (`TopicBackend::is_durable`):

| Signal | Redis backend | Default (in-memory) backend |
| --- | --- | --- |
| Traces | Published to the stream, acknowledged after the write, unacknowledged messages reclaimed | Queue skipped: `TracePipeline::ingest_now` writes inside the request |
| Metrics | Written inside the request | Written inside the request |
| Logs | Not stored; `partial_success` with every record rejected | Same |

Both transports, and both apply it: gRPC used to publish and answer success regardless. And a record that
is **dropped** rather than stored - the project stopped accepting writes between the admission check and
the write - is reported as such: 404 for traces, `partial_success` with `rejected_data_points` for metrics.
A bare `true` made "stored" and "discarded" the same answer (`IngestOutcome` now distinguishes them), which
is the failure this whole path exists to remove. ClickHouse's `wait_for_async_insert` defaults to true, in
the code *and* in the shipped example configs, and `async_insert` **with** the wait off is now refused at
startup rather than warned about: in that combination an `INSERT` returns once ClickHouse has the rows in
memory, so a 200 - and a queue acknowledgement - rests on a buffer a restart discards. It was configurable,
and no amount of care elsewhere survives one boolean that makes the acknowledgement a lie.

**The queue is bounded by consumer progress, never by length.** Publishing used `XADD ... MAXLEN ~ 100000`.
Redis trims by *length*, with no notion of whether an entry has been read, so any backlog past the bound
deleted the oldest payloads - each already answered 200 by HTTP or gRPC. A queue that discards accepted
work is worse than none, because the loss is silent and the exporter has already moved on.

Now: no `MAXLEN`; entries go only through `stream_trim_consumed`, whose boundary is the oldest entry *any*
group still needs - its oldest pending entry if it has one, else one past its last delivered id - so
nothing unread and nothing unacknowledged is removable. Stream ids are compared numerically (`StreamId`),
because `"9-0" > "10-0"` as text and a string minimum would pick a boundary past what a group still owes.
A backlog that outgrows the bound turns into `BufferFull` → 503 with `Retry-After`, leaving the data with
the exporter that still has it; the length comes back in the same pipeline as the `XADD`, so the threshold
costs no extra round trip and overshoots by at most the publishes in flight. A stream with no consumer
group is never trimmed: nobody has read it, so everything is still needed.

**A retry counter is evidence about the system, not about the payload.** Recovery used to acknowledge an
entry once its delivery count reached ten, to stop it starving the entries behind it. That is the same loss
by another route: ten failures is what a minute of analytics downtime looks like, the payload was already
answered 200, and the counter says nothing about the payload itself. The starvation is real, though —
claiming resets idle time, so a chronically-failing entry stays eligible and refills a window that starts at
the oldest pending entry, and an abandoned entry past the window was never recovered at all. Two mechanisms
replace the deletion, each doing a job the other cannot:

| Mechanism | What it fixes | Why the other cannot |
| --- | --- | --- |
| **Fewest deliveries first**, within one scan | A chronic failure stops consuming the window while ordinary entries wait | Rotation alone hands the window straight back to it every time its turn comes round |
| **A rotating scan start** (`pending_scan_cursor`), resuming past what was examined and wrapping at the end | Every pending entry is reached in a bounded number of passes | Sorting only orders what was *scanned*; a full window of equally-failing entries makes the one behind them invisible, not merely late |

So a chronic entry is retried more slowly and reported loudly, and kept. The one case that *is* evidence
about the entry is a payload that cannot be read at all: nothing can ever process it and leaving it pending
holds the trim boundary forever, so it moves to `<stream>:dead` and is acknowledged only **after** the bytes
are safely there — an interruption between the two leaves a copy on both streams rather than none, and
ingestion is idempotent by span id. The dead-letter stream is deliberately uncapped: a length bound there
would delete the very evidence it exists to keep. Failing to preserve it means *not* acknowledging it.

`make test-redis` runs this against a pinned Redis, because none of it had ever run against one — the unit
tests covered key prefixes and URL redaction. Restoring `MAXLEN` fails the suite.

**A limit enforced on one of two transports is not a limit — twice now.** `otel.auth.required` applied to
HTTP only until the gRPC interceptor was written; `rate_limit.ingestion_rpm` then did the same, because the
gRPC server carried only the auth-failure limiter. Both gates now travel together in `GrpcIngestGuards`, so a
third is a field rather than another parameter nobody threads through, and
`every_grpc_export_authorizes_before_reading_its_payload` requires *both* between a handler's
`extract_project_id` and its `into_inner()`. The ingestion bucket is **shared** with HTTP, unlike the
auth-failure buckets: those are keyed by a spoofable address, where one transport must not exhaust the other's
counters, while this is keyed by the project being written to — separate buckets would hand a client twice the
quota for splitting its traffic.

**A rejection names the cause that applied** (`Stored::rejection`). Metrics reported every dropped datapoint as
"the project is unknown or is being deleted", including one dropped for an unstorable timestamp — the same
defect the trace path fixed with `DropReason`, still live on the metrics path. The two imply different actions:
stop sending there, versus your clock is wrong and a retry cannot help. A request that hits both says so.

**A non-finite exemplar value is unstorable**, for the same reason an unstorable instant is: the flat
`exemplar_value_double` column keeps the NaN or infinity while the JSON array goes through `serde_json`, which
has no non-finite numbers and writes `null` — so the indexed column and the full array described different
values for one exemplar. Dropped in the one filter both derive from, leaving the measurement untouched.

**The project id is *set*, not appended** (`set_project_id`). A client may already carry
`sideseat.project_id`, and appending left the key twice — harmless for storage, which collapses duplicate keys,
and not harmless for metric identity, which digests the raw resource attribute list: the same datapoint at the
same instant hashed differently depending on whether the client had sent the attribute, so one series became
two rows.

**Distributed ClickHouse with no quorum is warned about**, as Redis with unacknowledged replicas is. In
distributed mode the tables are `Replicated*` by construction, so an insert that `insert_distributed_sync`
carried to a shard still lives on one replica until replication catches up — after the exporter was answered
200. Warned rather than refused, unlike `insert_quorum = 1`: a cluster with one replica per shard is a
legitimate deployment where a quorum of two would block every insert forever.

**A durable Redis is one that is checked, to a stated bound.** `is_durable()` returned true for anything
that answered `PING`, so a cache-tier instance with AOF off and a keyspace-wide LRU licensed acknowledging
before the write. `probe_redis_durability` now reads `appendonly`, `appendfsync` and `maxmemory-policy` at
startup and refuses AOF-off, `appendfsync no`, and any policy that could evict an unread stream entry; a
server that refuses `CONFIG GET` is refused too, rather than assumed. The shipped compose file's
`allkeys-lru` failed this and is now `noeviction`.

**Replication is acknowledged, not assumed — and the acknowledgement is `WAITAOF`, not `WAIT`.**
`appendfsync always` covers the loss of *that host*; a failover promoting a replica that never *durably had*
the entry is a separate window. `database.redis.min_replica_acks` makes each publish wait for it, and a
shortfall is a 503 the exporter retries — with a startup warning when the server *has* replicas and nothing
requires their acknowledgement, which is the configuration where the gap exists and is invisible. The command
is `WAITAOF 1 <n>`, not `WAIT <n>`: `WAIT` blocks until N replicas hold the entry *in memory*, so a replica
that received but had not yet fsynced it, then promoted after a crash, still loses it — the exact failover
this guards. `WAITAOF` blocks on the append-only file being fsynced locally (the `1`, confirmed rather than
assumed from `appendfsync always`, so a misconfiguration is a refusal) and on `<n>` replicas. It needs
`appendonly yes`, which the startup probe already enforces. ClickHouse has `insert_quorum` (with
`insert_quorum_parallel = 0`) for the same reason at shard level, since `insert_distributed_sync` reaches a
shard rather than its replicas. A quorum of **1** is refused at startup: it is satisfied by the initiating
replica alone, which is what happens with no quorum, plus latency and a false sense of safety.

`appendfsync always` is **required**, not merely preferred. `everysec` was accepted for a while on the
grounds that it is what production Redis runs — but it lets a 200 precede the fsync, so a host failure loses
up to a second of exports this server has already reported as stored, and documenting that window makes the
loss honest without making the data durable. An operator who wants that throughput has the default
in-memory backend, which writes inside the request instead of acknowledging early. The failover window —
a replica promoted without the entry — is closed by `min_replica_acks` above, at the cost of a `WAITAOF`
round trip per publish.

**Recovery cannot be starved, and a poison payload cannot stop ingestion.** `stream_claim` asked for the
first N pending entries and discarded the ones that were not idle enough, so a backlog of freshly delivered
entries hid every abandoned one behind it; the window is filtered with `XPENDING ... IDLE` now, so it holds
only eligible entries and the oldest abandoned come first. And a payload that cannot be decoded used to
reach the consumer loop as a generic error and *break* it — one malformed message stopped all trace
ingestion for the life of the process, and since it was never acknowledged a restart met the same message.
It is now `TopicError::Undecodable`, carrying the id so the consumer acknowledges it and continues: nothing
can store a payload it cannot parse, and the alternative is blocking every message behind it.

**A metric datapoint has an identity, because a `ReplacingMergeTree` needs one.** `otel_metrics` was sorted
by `(project_id, metric_name, toDate(timestamp), timestamp)`, and a replacing engine treats rows with an
equal sorting key as versions of one row. Attributes were not in the key, so `requests{status=200}` and
`requests{status=500}` from a single export — the ordinary shape of a labelled metric — collapsed into one
row at the next merge, after the ingest had returned 200. DuckDB, append-only with no key, had the mirror
failure: the same export delivered twice was stored twice.

`domain/metrics/identity.rs` defines it once for both: the resource, the scope, the metric name, the type,
the temporality, the **unit**, the **monotonicity**, the attribute set and both timestamps — everything OTel
says names a series and an instant. The unit and the monotonic flag are in it because they are part of what
the instrument measures rather than commentary on it: one name reported in `ms` and in `s`, or a monotonic
sum against a non-monotonic one, are different streams. The *description* is excluded, and so is the
*measurement* — so a re-delivery carrying a corrected value replaces its row rather than sitting beside it,
which is the rule spans already follow.

Attributes are hashed with their **OTLP types preserved** (`attrs_to_typed_json`). The ordinary extraction
path stringifies every value, which is right for display and wrong for identity: `code=200` (int) and
`code="200"` (string) became the same series, as did OTLP bytes `DE AD` and the string `"dead"` until the
byte form was tagged.

The attribute sets are hashed **from the protobuf**, tagged by OTLP variant, not from any JSON rendering.
Three encodings were tried and two were forgeable: bare hex collided with the string `"dead"`, a `__bytes:`
prefix collided with `"__bytes:dead"`, and a tagged array `["__otlp:bytes", "dead"]` collided with an OTLP
array of exactly those two strings. JSON has no bytes type and no non-finite numbers, so *every* encoding of
them is expressible as some other OTLP value — only the variant itself is unforgeable. Doubles are hashed by
bit pattern, so a NaN is a stable distinct value. Timestamps come from the raw nanosecond counts too:
`timestamp_nanos_opt` answers `None` past 2262 and the fallback was zero, so every datapoint of every later
year shared an instant. The scope's attributes and both schema URLs are in the identity as well, and were
being discarded from telemetry entirely.

An **empty** `datapoint_id` means "no identity known" and never collapses: legacy rows carry it, and so
does anything written without passing through the extractor that stamps one. Treating it as an identity
made every such datapoint a single row.

ClickHouse carries the id in its sorting key so a `ReplacingMergeTree` merge keeps distinct series;
**DuckDB deletes the ids it is about to write, in the same transaction as the append**. Counting distinct
ids at read time was not enough — it hid the physical duplicates rather than preventing them, leaving two
rows for one instant with nothing to say which measurement was current, and a retrying exporter's write
amplification unbounded. Rows written before the identity existed keep `''` and retain the old behaviour
among themselves; datapoints already merged away are gone.

Measured by `make bench-http`, which **enforces** these numbers rather than printing them: the run exits
non-zero when an operation misses its ceiling. A benchmark nobody compares against a target is a report,
and the tables below could otherwise drift arbitrarily far from the promise while every run passed.

Four things the harness is careful about, each because getting it wrong produces a number that looks like
evidence and is not:

- **The read workload is the whole fixture.** Every request of `langgraph/swarm` is posted once, and the
  script prints the span count the session list reports. Re-posting one request many times does not build a
  session — ingestion is idempotent by span id, so it stays one request's worth however often it is sent.
- **A failed request is not a sample.** Every measured call checks its status; a fast 404 would otherwise
  be recorded as excellent latency.
- **Samples are paced** (`BENCH_GAP_MS`, 25 ms). Back to back, a 754 KB export measures *queueing delay* —
  each request waiting for the previous write — and the whole distribution above the median moved run to
  run: p95 of 43, 53, 56 and 135 ms across four runs. An OTLP exporter batches on a schedule and never
  behaves that way. With a gap the same figure sits at 51–55 ms across runs. The 8-concurrent read keeps no
  gap, because concurrency is what it is measuring.
- **The gate is p95, and p99 is reported ungated.** At 200 samples the p99 *is* the second-worst request,
  so it is set by whatever else the host was doing: the large export's p99 ranged 125–667 ms across runs
  with its p50 steady at 20 ms. Isolating that tail (same payload, `files.enabled` off) reproduced it, so
  it is DuckDB's own write amortisation rather than file extraction — inherent to an embedded store taking
  750 KB writes with no gap, and not a shape an exporter produces.

**Open, and measured rather than guessed**: on the development host these figures currently sit ~35% above
the table below - concurrent session-read p50 ~165 ms and p95 ~188 ms against a documented 121/138 - across
four consecutive runs, so it is not run-to-run noise. Two things are established about it. It is **not** the
session-membership work: reverting that subquery to its previous form leaves the numbers unchanged (p50 27.6
vs 27.4 ms, p95 191 vs 188 ms), measured directly at the benchmark's own scale. And it is not the narrowing
optimisation either, which is justified on its own interleaved measurement (`bench_session_membership`: 1.77x
the loose predicate deduplicating the whole project, 1.29x deduplicating candidates only, at 4,000 traces).

What is unresolved is whether the remaining gap is this host or a cumulative regression elsewhere. Every
measurement today was taken with 15-20 other processes competing (load 8-35 all day), and the table below was
taken on an idle machine. Settling it needs an idle host, or a bisect - and note that `c00fe46d` does not
build in a `git worktree`, which is worth understanding before attempting one.

**DuckDB + SQLite**, release build, loopback, 200 samples (100 for the large export):

| Operation | p50 | p95 | p99 (ungated) | p95 ceiling |
| --- | --- | --- | --- | --- |
| trace export, 2 KB | 3.9 ms | 4.6 ms | 5.7 ms | 10 ms |
| trace export, 754 KB | 19.2 ms | 40.4 ms | 96.7 ms | 100 ms |
| session messages (136 spans), sequential | 23.5 ms | 26.9 ms | 32.8 ms | 40 ms |
| session messages, 8 concurrent | 121.3 ms | 137.6 ms | 155.7 ms | 150 ms |
| trace list, 50 | 26.8 ms | 30.1 ms | 40.1 ms | 40 ms |
| **cold read** — first reader, empty reconstruction cache | 22.5 ms | | | |

**PostgreSQL 17 + ClickHouse 25.8 + MinIO**, same harness (`make bench-http-distributed`), all in local
containers so there is no network hop; the ClickHouse image is amd64 under emulation on this arm64 host,
which inflates it:

| Operation | p50 | p95 | p99 (ungated) | p95 ceiling |
| --- | --- | --- | --- | --- |
| trace export, 2 KB | 50.8 ms | 58.5 ms | 81.3 ms | 80 ms |
| trace export, 754 KB | 74.3 ms | 82.5 ms | 157.7 ms | 120 ms |
| session messages (136 spans), sequential | 357.8 ms | 463.9 ms | 489.3 ms | 600 ms |
| session messages, 8 concurrent | 526.2 ms | 665.9 ms | 804.3 ms | 1500 ms |
| trace list, 50 | 400.3 ms | 474.2 ms | 703.9 ms | 700 ms |
| **cold read** — first reader, empty reconstruction cache | 522.8 ms | | | |

The distributed run uses **MinIO**, not local files, because the shared-store rule refuses PostgreSQL with
filesystem storage — so the benchmark has to describe a coherent distributed deployment, which is what
makes its file numbers the right ones.

The ClickHouse column is deliberately an order of magnitude looser, and it is a *measured* target rather
than an aspiration: there the row fetch dominates, so no amount of work on the normaliser changes it.
Anyone who needs interactive reads at DuckDB latencies with ClickHouse durability should expect to add
caching in front of the read path, not to find it here.

**The cold read is now measured, because it is the ephemeral-scaling case rather than a curiosity.** Every
new replica starts with an empty reconstruction cache and a deploy replaces them all, so the first reader
of any session pays it on every instance. It is taken before any warm-up, and it is close to the warm
figure for a 136-span session — the cache earns its keep on long *replaying* sessions, where the pipeline's
input is quadratic in the turn count: 2.28 s cold against 47 ms warm at 1 000 turns. Concurrent cold
readers are also coalesced now (`get_with`), so eight arriving at a fresh replica reconstruct once between
them rather than eight times.

Two more things the tables say out loud. DuckDB **serialises** reads, so eight at once cost about five
times one — read concurrency there buys throughput, not latency — while ClickHouse's cost 1.3×.

The read figures rose ~5 ms when DuckDB's deduplication became a `QUALIFY ROW_NUMBER()` window rather than a
join on `MAX(ingested_at)`. That was not a trade for speed: the join returned **every** row tied at the
maximum, so two deliveries of one span in the same stored microsecond both survived and the span appeared
twice. A duplicate is the one thing the feed must never produce, so the window function is the correct shape
and its cost is the price of correctness — still inside every ceiling.

**S3 file storage, against a real S3 API** (MinIO in a container): a file-carrying export takes 18-55 ms
including the object writes; deleting a project removes exactly its own objects and leaves other projects'
untouched (verified by object count before and after); and the `ListObjectsV2` that an *empty* project's
cleanup performs - the call the deletion sweep pays per remembered id - costs ~6 ms. So a batch of
`DELETED_PROJECT_CHECK_BATCH` costs about 0.3 s per sweep here, and even at 100 ms per listing against real
AWS it is ~5 s inside a 120 s interval and a 600 s lease. That is the number the backoff and batch cap exist
to bound.

**Multi-replica**, two instances against one PostgreSQL and one Redis: a project created on A is visible on
B, ingestion through B lands, and after deleting it on A, B refuses both ingestion and reads *immediately* -
the cross-instance fence, working because the project row is not cached anywhere. Note that with the default
DuckDB analytics backend each replica has **its own** database, so horizontal scaling requires ClickHouse: a
replica cannot read another's spans.

Still unmeasured, deliberately stated: a real network hop (everything above is loopback or a local
container), and a multi-replica deletion backlog at scale.

**What "all frameworks" means, exactly.** The claim is bounded by this corpus, and it is checked rather than
described: `the_corpus_matches_the_support_matrix` fails if a suite is added or removed without updating it.

See **`server/tests/fixtures/messages/README.md`** for the table: which suites, at which SDK versions, and
how many samples and captured requests each contributes. It lives there rather than here because
`the_corpus_matches_the_support_matrix` parses it. This file is *tracked*, but project convention keeps it out
of routine commits, so its committed content lags the working copy by however much has been written since -
and a test reading it would assert against whatever state a given checkout happens to carry. The README is
maintained beside the fixtures it describes, which is what makes it a document a test can hold to account.


Each captured request is real OTLP from a real run of that SDK, replayed through the real ingestion path.
What the corpus verifies per fixture: message count, content, ordering and absence of duplicates across all
four views, plus the invariants that hold independently of the goldens. What it cannot verify is a framework
or version nobody has captured - that is open-world, and the matrix is what makes the boundary legible
rather than implied.

**Deleting on ClickHouse waits for its mutation** (`AWAIT_MUTATION`, `mutations_sync = 2`). `ALTER ...
DELETE` is asynchronous by default, so the trace-deletion route removed the files those spans referenced
and answered 204 while the spans were still readable — and a failed mutation left them that way. A
structural test (`every_clickhouse_delete_waits_for_its_mutation`) reads the source, because a new delete
site written without the setting compiles and passes.

**Distributed ClickHouse writes go through the `Distributed` table, not `_local`.** The distributed tables
shard on `sipHash64(project_id)`, and a write aimed at `_local` lands on whichever node the connection
reached instead. Behind a load balancer one span delivered twice could land on two shards, where `FINAL`
deduplicates within a shard only — so the read returned it twice and every count was wrong; behind a fixed
endpoint the whole cluster's data went to one node. `insert_distributed_sync = 1` goes with it, or the
insert returns once the rows are spooled on the initiating node's disk, which is the same lie as
`wait_for_async_insert = 0` by another route. Reads (distributed table) and deletes (`_local` with `ON
CLUSTER`, where the parts are) were already right; only the insert was wrong.

**Message-parsing goldens**: `cargo test -p sideseat-server message_goldens` verifies message count, content,
ordering and absence of duplicates per framework across all **four** views (span / trace / session / feed).
Every one of the 111 samples has an `expected.json` beside its captured requests - checked structurally, none
missing - and each records, per view, the message count, the role sequence, and for every message its index,
role, entry type, content, content digest, tool name, finish reason and observation type. So the four
dimensions the goldens exist to protect are each pinned by a distinct field rather than inferred from a
summary. Mutation-verified: dropping one message, swapping two, or duplicating one each fail four of the
suite's tests. Fixtures are captured OTLP payloads under `server/tests/fixtures/messages/<suite>/<sample>/`; capture with `misc/capture-message-fixtures.sh [suite] [sample]` (needs model credentials), then record with `UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens` and review the diff. Invariants (scope containment, per-trace dedup, tool-id correspondence, no empty thinking, determinism) hold independently of the goldens, so a blindly regenerated snapshot still fails on real defects. `UPDATE_GOLDENS=1` writes the files but still exits non-zero when an invariant was violated, so known-bad output cannot be committed as reviewed. See `server/tests/fixtures/messages/README.md`.

**ClickHouse parity**: `make test-clickhouse` starts a pinned container and runs
`server/src/data/clickhouse/parity_tests.rs`, which inserts one span set into both analytics
backends and requires every read method to return identical rows (DuckDB is the reference). Skips
with a message when `SIDESEAT_TEST_CLICKHOUSE_URL` is unset, so `make check` stays green without
Docker; CI runs it against a service container. Two things it found: `get_trace_tags_options` failed
outright on ClickHouse (non-Nullable `arrayJoin` into `Option<String>`), and the ClickHouse schema
TTLs spans older than 90 days, so a fixture with fixed past timestamps disappears there and
persists in DuckDB.

**A trace belongs to exactly one session: the one on its earliest span.** `arg_min` / `argMin` over
`(timestamp_start, span_id)` - a *total* order, because a timestamp tie left to the engine gives three
surfaces three answers about one trace. Frameworks really do put two session ids on one trace: ADK emits its
own alongside the caller's, and 24 of the 111 fixtures had a trace with a session view under **both**,
holding the identical messages. The goldens had recorded that duplication as correct.

Asking "any span named it" instead was wrong in eight places, and the consequences differed enough that
fixing them one at a time missed several rounds running:

| Surface | What "any span" did |
| --- | --- |
| Message reads (`traces_of_session`, both backends) | Both sessions returned the whole trace, so one session's view showed content the UI displays under another |
| `delete_sessions` (both backends) | Deleting session B deleted **every span** of a trace canonically in A. Data loss, and ClickHouse kept doing it after DuckDB was fixed because the regression test covered one backend |
| The ingestion fence (`canonical_session_of_traces`) | A redelivery of that trace was tombstoned and dropped in full, and the tombstone then had the sweep delete the rows already stored under A |
| Trace / span / session list *filters* | A row matched a filter for a session it is not in |
| The session **list** and project **statistics** | Two sessions, each claiming the trace's full spans, tokens and cost; opening the second showed nothing |
| Session-filtered **SSE** | Child spans carry no session at all, so their events were discarded by the page they belonged to; a stray-session span went to a page the trace does not appear on |

The *advanced filter* path is included: a `session_id` filter on the span or session list becomes a
trace-level predicate against the canonical relation rather than a comparison on the raw column, so the two
ways of asking the same question - that filter and the dedicated `session_id` parameter, in the same query -
cannot disagree. A negated operator wraps the *positive* form in `NOT IN`, for the same reason the trace list
does: "not this session" means "its session is not this one", and the negation as written also drops traces
with no session (`NULL NOT IN (…)` is NULL).

And in the UI, "View in session" navigates by the **trace's** session, *only* — no fallback. A session id sits
on the span that knew it, so gating that button on `span.session_id` hid it on every child span of a trace that
does have a session, and showed it on a span naming one the trace is not in, where it led to a page showing
nothing. Falling back to the span's own value *while the trace request is in flight* is the same defect on a
timer: whether the user got the wrong session depended on how fast the request returned. Until the trace
answers the destination is unknown, which the button says by staying disabled, with a tooltip that separates
"resolving" from "no session".

**And the live stream is stamped from the store, not from the batch, whole** (`stamp_stored_sessions`). A subscriber
filtered by session compares `event.session_id`, so the value has to be the one a subsequent read returns —
and the canonical session is the one on the trace's *earliest* span, which may have arrived in a previous
batch. A streaming exporter's later flush therefore announced a span under session B while every read placed
its trace under A: the live stream and the page disagreeing about one trace, which for a debugging tool reads
as a message that appears and then cannot be found. Asked after the write, on both ingest paths, so the store's
answer includes this batch — and taken *whole*: a trace the store reports no session for **has** no session,
which is a fact and not a gap. Overwriting only the traces it named left the batch's value standing, so a batch
that corrected a span by removing its session still announced the old one.

A **failed** read clears the session rather than falling back to the batch's view. Both lose something and
they are not symmetric: an event with no session reaches every unfiltered and trace-filtered subscriber and no
session page, while a wrongly stamped one is delivered to a page the trace does not appear on *and* withheld
from the page it does — a false positive and a false negative from one guess.

The read is **unconditional**, and costs ~2.7 ms per batch (22.4 → 25.1 ms on the `langgraph/swarm` ingestion
benchmark). Skipping it for a trace whose parentless span the batch carries was tried and reverted: it recovers
about half of that and privileges the root span, which is precisely what the canonical rule refuses to do — in
a distributed trace a child produced on another host can carry an earlier start time than its parent, so a
batch holding the root still does not know the answer, and the case it gets wrong is the misrouting this
exists to fix.

One definition per backend, enforced on the source text
(`the_session_membership_subquery_is_defined_once_per_backend`), because a second copy compiles and passes
every behavioural test - and both message reads had already grown one within a day. Its four placeholders come
from `traces_of_session_binds`, since a wrong order there is a silently wrong answer rather than an error.

**The deduplication is narrowed to candidate traces, not applied to the project.** A window function cannot
use an index, so what matters is how many rows reach it: 1.85x the (incorrect) loose predicate deduplicating
everything, 1.28x deduplicating only traces that ever named the session, measured by
`bench_session_membership` - which runs the formulations **interleaved in one process**, so the ratio is
meaningful on a host this benchmark cannot have to itself, and asserts they select the same **trace ids**
before timing them. A count would have let a faster wrong answer pass. Parameterising the subquery by
relation once put the candidate filter *outside* the window and silently gave the optimisation back; that
benchmark is what noticed.

**A session's membership is a fact from the store, resolved on the deduplicated rows, at the traversal's
own instant.** Three separate defects in one small area, each producing duplicates:

- **Deduplicated, not raw.** `otel_spans` is append-only, so a span re-delivered with a different
  `session_id` leaves its old row behind. DuckDB resolved membership from the raw table while reading the
  messages from the deduplicated view, so the *old* session still found the trace and returned its *current*
  content - a session reporting messages that no longer belong to it, and one trace under two sessions.
  ClickHouse read the subquery with `FINAL` and was already right, so this was a silent backend disagreement.
- **From the store, not from the rows.** The feed groups traces into conversations so a cross-trace replay can
  be collapsed. Those rows have been through `MESSAGE_CONTENT_FILTER`, and a framework records the session on
  the span that knows it - usually a root carrying no content, which the filter removes. With every such row
  gone, each trace became its own conversation and the second trace's re-sent history came back as duplicated
  turns while the response still said `session_scoped`. The route passes `FeedOptions::session_of_trace`
  (`get_trace_session_pairs`), and it is **in the reconstruction cache key**: it knows about spans the filter
  removed, so a contentless root whose session changed alters the answer while leaving every row identical.
  `process_feed_cached` also had to stop reconstructing with a bare `FeedOptions::new()`, which discarded the
  caller's options on every production read.
- **As of the watermark.** Membership reads take `as_of_us`; the feed passes its traversal watermark. Against
  current data a trace re-delivered into another session mid-traversal is read with its old content but
  expanded under its new session, so the session it actually replays is never loaded. DuckDB honours the bound;
  ClickHouse cannot express it, the same limit already stated for the page and context queries.

The session is the one on the trace's **earliest** span (`argMin` over `(timestamp_start, span_id)`), matching
what the trace and session views display. `MIN(session_id)` was deterministic but picked the lexicographically
smallest, so a trace could be shown under one session and grouped under another. And the grouping key is a
typed enum, not `format!("trace:{id}")`: session ids come from the client, so a session literally named
`trace:B` grouped with the sessionless trace B - two unrelated conversations reconstructed as one.

**Message views all use `process_spans`**: the span, trace and session endpoints differ only in their row set, not their pipeline. `process_feed` (DESC, newest-first) belongs to the **project feed** endpoint (`routes/otel/feed.rs`) — using it for a session view produces ordering no session request can return.

**A feed traversal is a view of one instant** (`ingested_before_us`). Pages are chosen by ingestion time,
and the reconstruction context loaded around each page used to be unbounded in that dimension - so a span
ingested *during* a traversal could enter an earlier page's context, win deduplication against a span still
to be paged, and then be scoped off the page it was not selected for. The older copy was suppressed as a
duplicate and the newer one never returned: a message absent from every page of that traversal. The cursor
now carries a **watermark**, established on the first page, and it bounds both the page query and the
context load. A cursor issued before the watermark existed carries none and keeps the old behaviour, so a
traversal in flight across an upgrade completes rather than failing on its next page.

The bound goes **inside the deduplication**, not around it (`dedup_spans_as_of_watermark`), and in **both**
queries the endpoint issues — the page selection and the reconstruction context beside it. Applied outside,
it is worse than no bound for a *re-delivered* span: dedup picks the newest row over the whole table, the
bound rejects it, and the older row was never selected — so the span disappears. Applied to only one of the
two queries, the page could select a span whose context held no version of it, and reconstruction saw a
fragment of a trace it was told to treat as whole. ClickHouse cannot do this: `FINAL` has no "as of" form and
a merge may already have removed the earlier version, so there is nothing to select. That limit is per
backend, like the latency ceilings, and stated rather than implied.

The watermark itself comes from the **store**, not from `Utc::now()` (`max_ingested_at_us`). A reader's clock
is a statement about the reader: ahead of the store's it excluded rows already committed, behind it admitted
rows the next page would read again. The residual is stated on that method — a write stamped before the read
but committing after it is below the watermark and appears on a later page — and it is the duration of one
write rather than an arbitrary clock difference. Closing it fully needs a commit-ordered sequence neither
analytics backend provides.

**The feed loads whole sessions, not whole traces**, so a replay crossing traces *within* a session is
recognised wherever the page boundary fell — previously both pages returned the turn. What remains is stated
in the response rather than left to assumption: `session_scoped` says whether every contributing span carried
a session id, and `pages_are_globally_ordered` is always **false**, because pages are selected by *ingestion*
time (the only key giving a stable total cursor) while each page's messages are ordered by *message* time,
which the pipeline computes and SQL cannot page by. A page is a correct window on activity; a concatenation
of pages is not a transcript, and the trace and session endpoints are where a conversation is read in order.

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
| Displayed-but-per-span (`user_id`, `environment`) | condition on `trace_display_first(col)` — the earliest span that has a value, which is what the row shows | these live on the spans that know them, usually the root alone, so `session IS NULL` was true of every such trace and returned traces displayed *under* a session |
| `session_id` | the canonical relation (`canonical_session_filter` / `push_canonical_session_filter`), the same subquery the span and session lists use | one question must not have two answers: the trace list matched the displayed aggregate while the span list negated the subquery, so a sessionless trace was absent from "session is not A" on one page and present on the other |
| Span attribute the row does not show (model, provider, framework, tags, trace id) | `trace_id IN (SELECT … WHERE <cond>)`, one subquery each; a **negated** operator becomes `trace_id NOT IN (<positive form>)` | ANDed on one row, two filters demanded a single span carrying both values — a session id on the root plus tokens on its child matched nothing. And "none of X" asked as written returned a trace that used X once and something else next, and dropped rows with no value at all (`NULL NOT IN (…)` is NULL) |

Negation uses `Filter::positive_twin()` (`NoneOf`→`AnyOf`, `IsNull`→`IsNotNull`, `Ne`→`Eq`) and negates the *subquery*: for an entity made of many spans, "not this" means **no** span, not "some span was something else".

**That applies to an aggregate too, and it is not a stylistic choice — it is what NULL costs.** A negation
rendered against the aggregate is `FIRST(user_id ORDER BY …) NOT IN ('x')`, which is NULL for a trace with no
user id anywhere, and NULL is not true — so "none of x" dropped exactly the traces that are not x. Each
negated aggregate filter therefore becomes its own `trace_id NOT IN (SELECT … HAVING <positive>)`, in both
backends, since the positive ones share one `HAVING`. The parity case meant to pin this named a user the
fixture does not have, so every trace matched and it could not tell a correct answer from a filter that had
been dropped entirely; it now names one the fixture has.

**One expression per displayed value, or the list, the detail view and the filter give three answers.**
`trace_display_name` was used only by the *filter* while four projections (three DuckDB, one ClickHouse)
hand-wrote their own — three of them with no `ORDER BY` at all. A trace with two roots at the same start
instant was listed under one name, shown under another when opened, and matched by a filter for a third,
and which one you got depended on the plan. Every site takes the helper now, it carries the
`(timestamp_start, span_id)` tie-break that makes the order total, and
`the_displayed_trace_name_is_defined_once_per_backend` reads the source — a fifth copy compiles and passes
every behavioural test. The preview and metadata aggregates got the same tie-break, for the same reason at
a smaller stake.

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
