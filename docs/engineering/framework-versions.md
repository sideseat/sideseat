# Framework versions and telemetry formats

SideSeat interprets telemetry that frameworks and instrumentation libraries emit, and those producers change
their formats between releases. This document records how much they changed in the twelve months from
2025-10-05 to 2026-10-05, how the rule language is meant to express the differences (see
[framework-rules-engine.md](framework-rules-engine.md)), and how the version matrix in the example harness
verifies them.

The support target is **every release of the last twelve months** of every supported producer, not only the
latest one. The catalogue below shows why that matters: most producers released weekly, several maintained
two or three major lines at once, and many emit more than one format from a single release depending on an
opt-in switch.

## Method

Two independent sources, the first broad and static, the second narrow and empirical.

1. **Static fingerprint of every release.** For each package the release history was read from the package
   index (`https://pypi.org/pypi/<package>/json`, `npm view <package> time`), and the wheel or tarball of every
   release since 2025-10-05 (plus the last one before it, as a baseline) was downloaded. From the modules that
   emit telemetry, the fingerprint collects the dotted string literals (`gen_ai.input.messages`,
   `llm.token_count.prompt`), semantic-convention identifiers (`GEN_AI_TOOL_DEFINITIONS`) and opt-in values
   (`gen_ai_latest_experimental`). Consecutive releases with an identical set form a **key-set epoch**. An epoch
   boundary is evidence of a vocabulary change; it is not proof of a payload change (a module move shows up as
   keys "removed"), and a payload change that reuses the same keys is invisible to it.
2. **Empirical census.** `python -m harness matrix <producer> --census` installs every release of the window in
   its own environment, replays the suite's committed model cassettes offline through the probe scenarios, and
   classifies the captured OTLP by its telemetry shape (see [The version matrix](#the-version-matrix)). This is
   the authority for what a release actually emits.

## Catalogue

Releases are counted from 2025-10-05; "epochs" are static key-set epochs over those releases in version order.

| Producer package | Releases | Epochs | Format changes that matter to rules |
| --- | --- | --- | --- |
| `strands-agents` | 52 | 12 | 1.11: `gen_ai.input.messages` / `gen_ai.output.messages` and a `gen_ai.client.inference.operation.details` event, but only under `OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental`; the default stays per-message span events (`gen_ai.user.message`, `gen_ai.choice`, `gen_ai.tool.message`); `tool.status` becomes `gen_ai.tool.status`. 1.16: `gen_ai.tool.definitions` behind a second opt-in (`gen_ai_tool_definitions`). 1.34: `gen_ai.system_instructions`. 1.48: opt-in `gen_ai_span_attributes_only`. 1.51: `gen_ai.tool.call.arguments` / `.result`. 1.54: `gen_ai.usage.cache_creation.input_tokens` / `cache_read.input_tokens` beside the older `cache_read_input_tokens` / `cache_write_input_tokens`. |
| `@strands-agents/sdk` (TypeScript) | 40 | 9 | 0.5: first semconv telemetry (`gen_ai.*` events and `gen_ai.input.messages`); 0.7: agent metrics, `gen_ai.system_instructions`; 1.0: agent status/input attributes; 1.12: `gen_ai.tool.call.arguments` / `.result`; 1.15: cache token keys. |
| `google-adk` | 63 | 24 | Two major lines released in parallel (1.x to 1.39, 2.x to 2.11) whose telemetry differs at the same date. 1.24: conversation moves to `gen_ai.user.message` / `gen_ai.choice` / `gen_ai.system.message` events; 1.27: `gen_ai_latest_experimental` opt-in writes `gen_ai.input.messages` and `gen_ai.tool_definitions` (underscore); 1.32: renamed `gen_ai.tool.definitions`; 2.x: `gen_ai.workflow.name`, invoke-agent / execute-tool durations, `gcp.vertex.agent.associated_event_ids`; 2.3 and 2.4: reasoning and cache usage keys (`gen_ai.usage.reasoning.output_tokens`) replace `gen_ai.usage.experimental.reasoning_tokens`. OpenTelemetry pins move with the line (`<=1.37`, `<1.40`, `<=1.42.1`). |
| `pydantic-ai-slim` | 199 | 18 | 1.x (to 1.107) and 2.x (from 2.0) in parallel. `InstrumentationSettings(version=N)` selects the format inside one release. 1.42: `gen_ai.provider.name`, `gen_ai.tool.definitions`; 1.75 to 1.88: tool span keys move; 2.0 drops the event form (`gen_ai.*.message` events, `gen_ai.choice`) and keeps only attributes. |
| `agent-framework-core` | 60 | 15 | Weekly betas to 1.0 (2026-04-02), then 1.20. rc2: Traceloop-style `LLM_*` identifiers replaced by `gen_ai.*`; 1.0.0b251216: agent system name becomes provider name; 1.9: provider-specific usage keys (`anthropic.cache_creation_input_tokens`, `openai.reasoning_tokens`) next to semconv cache keys; 1.15: `gen_ai_latest_experimental` opt-in. |
| `logfire` | 47 | 8 | 4.19: semconv `gen_ai.input.messages` / `gen_ai.output.messages` beside Logfire's own `request_data` / `response_data`; 4.31: tool call keys and cache token keys; 5.0: `gen_ai.choice` no longer written; 6.0 betas: instrumentation reorganised (the `logfire.openai` 6.x input-only lifetime span is already a `message_projections` case). Logfire's format reaches the `openai`, `openai-agents`, `anthropic`, `google-genai` and `vertex-ai` suites. |
| `ai` (Vercel AI SDK) | 618 | 7 | Three majors released side by side (5.0.268 and 6.0.299 were still shipping in September 2026 beside 7.x). 6.0.81: `ai.response.reasoning`; 6.0.120: `ai.usage.inputTokenDetails.*` / `outputTokenDetails.*`; 7.0: telemetry moves into `@ai-sdk/otel` and about seventy `ai.*` keys leave `ai` itself (`ai.prompt.messages`, `ai.prompt.tools`, `ai.model.id`). |
| `@ai-sdk/otel` | 127 | 4 | Ships both a legacy `ai.*` integration and a `gen_ai.*` one; 1.0.84: `ai.settings.context.*`; 1.0.126: speech and transcription operations, `gen_ai.request.stream`. |
| `@anthropic-ai/claude-code` (CLI behind both Claude Agent SDKs) | about 300 | 7 (23 sampled) | 2.0.25: `claude_code.interaction` / `llm_request` / `tool` spans; 2.1.97: `span.model_request_*`, `user.*` and `session.*` events; 2.1.117: the `claude_code.*` span and metric names are retired. `claude-agent-sdk` (Python, 108 releases) and the TypeScript SDK (308) only spawn the CLI, so the CLI version is the format version. |
| `ag2` | 33 | 4 | 0.11: first OpenTelemetry integration (`ag2.*`, `gen_ai.input.messages`, `gen_ai.conversation.*`); 1.0 betas: the integration moves modules. |
| `agentscope` | 31 | 3 | 1.0.9: semconv attributes (`gen_ai.input.messages`, `gen_ai.tool.definitions`, `agentscope.function.*`); 2.0: rewritten tracing. |
| `crewai` | 289 | 8 | Nightly `dev` builds; telemetry of interest comes from the OpenInference instrumentor. A2A and task-output keys arrive in 1.15.21 and 1.15.23. |
| `openinference-instrumentation` (+ `-semantic-conventions`) | 33 (21) | 15 (12) | 0.1.51: the shared base starts writing `gen_ai.*` semconv keys beside `llm.*`; provider-host tables change in 0.1.43 to 0.1.61; `llm.choices` in conventions 0.1.22. |
| OpenInference instrumentors (`-agno`, `-autogen-agentchat`, `-bedrock`, `-crewai`, `-langchain`, `-llama-index`, `-openai`, `-smolagents`) | 17 to 37 each | 2 to 9 each | Agno 0.1.17 to 0.1.19 adds then removes `llm.token_count.*`; Bedrock 0.1.41 `output.message.content`; CrewAI 1.0 flow and agent attributes; LangChain and LlamaIndex essentially stable vocabularies with changing payloads. |
| `lmnr` (Laminar, behind Browser Use) | 54 | 24 | Integrations added almost every release; 0.7.42 drops `gen_ai.prompt.N.*` / `gen_ai.choice` events, 0.7.43 adds indexed `gen_ai.completion.N.*`, 0.7.54 writes `langfuse.observation.*`. |
| `opentelemetry-instrumentation-google-genai` | 9 | 5 | 0.4b0: `gen_ai_latest_experimental` messages; 1.0b1: rewritten (`code.function.parameters.*`, interactions API); 1.2b0: streaming step events. |
| `opentelemetry-util-genai` | 6 | 7 | The shared GenAI helper behind the contrib instrumentations; every release changes the vocabulary (1.0b0 drops the experimental opt-in identifier). |
| `opentelemetry-instrumentation-botocore` | 11 | 2 | 0.66b0: semconv cache token keys. |
| `traceloop-sdk`, `opentelemetry-instrumentation-bedrock` | 43, 45 | 4, 5 | Indexed `gen_ai.prompt.N.*` / `gen_ai.completion.N.*` throughout. |
| `langsmith` | 149 | 3 | 0.14: OpenTelemetry export gains `gen_ai.tool.call.id` / `gen_ai.tool.name`, 0.14.1 `gen_ai.tool.definitions`. |
| `langfuse` | 80 | 3 | Stable `langfuse.observation.*` vocabulary; 3.9 experiment keys; 4.7 `langfuse.internal.is_app_root`. |
| `haystack-ai`, `opentelemetry-haystack` | 64, 1 | 5, 1 | 3.0 removes the built-in `haystack.component.*` tracer keys in favour of the separate package. |
| `semantic-kernel` | 17 | 2 | 1.38 drops the `chat.completions` operation spellings. |
| `smolagents`, `autogen-agentchat`, `llama-index-core`, `langchain-core`, `langgraph`, `openai`, `anthropic`, `google-genai`, `openai-agents` | 4 to 91 | n/a | Their telemetry is written by the instrumentors and Logfire above; their own releases change the request payloads the instrumentors copy (for example Anthropic and OpenAI content-block types), which only the census sees. |

### Patterns

- **Opt-in profiles.** One release emits several formats. `OTEL_SEMCONV_STABILITY_OPT_IN` values
  (`gen_ai_latest_experimental`, `gen_ai_tool_definitions`, `gen_ai_span_attributes_only`) in Strands, ADK,
  Agent Framework and the contrib instrumentations; `InstrumentationSettings(version=N)` in Pydantic AI. A
  version alone therefore never identifies a format; a version and a profile does, empirically.
- **Parallel lines.** ADK 1.x/2.x, Pydantic AI 1.x/2.x and the AI SDK 5.x/6.x/7.x ship concurrently, so release
  date and version number disagree about which format is "newer".
- **Carrier moves.** The same conversation moves between span events, span attributes, log records and
  separate packages (`@ai-sdk/otel`, `opentelemetry-haystack`) within the window.
- **Semantic-convention drift.** The GenAI conventions renamed keys during the window (`gen_ai.tool_definitions`
  to `gen_ai.tool.definitions`, cache and reasoning token keys, `gen_ai.system` to `gen_ai.provider.name`), and
  producers adopted each rename at different times, often writing both spellings for a while.

## Census results

Taken with `make matrix-census`; each class is one shape over the probe scenarios, and its variant is
the class's newest release.

**Strands Agents** (52 releases since 2025-10-05, probes `tool_use`, `streaming`, `reasoning`; 14 classes):

| Releases | Default format | `semconv-latest` profile |
| --- | --- | --- |
| 1.11 - 1.12 | per-message span events; tool spans without description, schema or server timing | `gen_ai.input.messages` / `gen_ai.output.messages`, inference-details event |
| 1.13 - 1.33 | tool spans gain `gen_ai.tool.description` and `gen_ai.tool.json_schema`; server timing | the same additions |
| 1.34 - 1.35 | the system prompt is also a `gen_ai.system.message` event | `gen_ai.system_instructions` |
| 1.36 - 1.53 | event-loop spans carry `gen_ai.operation.name` `execute_event_loop_cycle` | the same (1.36 - 1.46); message JSON changes (1.47 - 1.50); `gen_ai.tool.call.arguments` / `.result` (1.51 - 1.53) |
| 1.54 - 1.55.0 | semconv cache token keys beside the older ones | only the semconv cache keys |
| 1.55.1 - 1.57.2 | the `system_prompt` span attribute, which `strands.system_prompt_message` reads, is gone | the same |

1.38.0 and 1.39.0 call Bedrock CountTokens before every model call, so they replay cassettes of their own,
recorded live on Bedrock (`[[recording]]`); both fall in the 1.36 - 1.53 default class and the 1.36 - 1.46
opted-in class. Reconstruction: every default-format class except 1.11 - 1.12 (whose tool results
are the Python `str()` of the result, not JSON) reconstructs the same conversation as the current release, up to the order of parallel tool
results; the `semconv-latest` profile is not yet read correctly in any release (user and assistant messages
arrive as raw `unknown` blocks, reasoning is lost), which the truth ledger records under backlog item 156.

**Google ADK** (59 releases since 2025-10-05 across the 1.x and 2.x lines, probes `tool_use`, `streaming`;
23 classes, no failures). 1.16 - 1.17 and 1.18 - 1.22 differ in `gen_ai.response.finish_reasons` and request
JSON; 1.24 turns model calls into `generate_content <model>` spans; 1.25 - 1.39 is the stable 1.x format,
except 1.27.2, which alone writes `gen_ai.agent.version`. The opt-in takes effect from 1.27.1, first with
`gen_ai.tool_definitions` (underscore), renamed `gen_ai.tool.definitions` in 1.32. The 2.x line changes
almost every minor: 2.0, 2.1 - 2.2, 2.3 - 2.6 (cache read tokens), 2.7.0 (a resource `service.instance.id` and a
provider-valued `gen_ai.system`, both reverted in 2.7.1), 2.8 - 2.9 (cache creation tokens), and 2.10 onwards.

**Logfire** behind the OpenAI suite (10 releases since 2025-10-05 without the 6.0 betas, probes `tool_use`,
`streaming`; 10 classes). 4.12 writes Logfire's own `request_data` / `response_data` and an `events` list;
4.13 drops `response_data`; 4.19 adds semconv `gen_ai.operation.name`, `gen_ai.provider.name` and
`gen_ai.tool.definitions`; 4.21 `gen_ai.response.id`; 4.29 `gen_ai.usage.raw`; 4.33 and 4.37 resource
attributes; 5.0 replaces `events` with `gen_ai.input.messages`, `gen_ai.output.messages` and
`gen_ai.system_instructions`.

**Pydantic AI** (192 releases since 2025-10-05 across 1.x and 2.x, probes `tool_use`, `streaming`; 27
classes). Early 1.x releases import `opentelemetry._events`, which OpenTelemetry 1.45 removed while their
bound stayed open, so OpenTelemetry resolves as of each release's day (`era` in `versions.toml`). Notable
boundaries: 1.42 `gen_ai.provider.name` and `gen_ai.tool.definitions`; 1.68 the `running tools` span goes;
1.74 - 1.75 run ids; 1.76 and 1.86.1 `invoke_agent` / `execute_tool` operation names; 1.89
`gen_ai.conversation.id`; 2.0 span names `invoke_agent agent` / `execute_tool <name>`,
`gen_ai.tool.call.arguments` / `.result` and aggregated usage; most other classes change only the JSON inside
the carriers. 1.28.0 fails every tool call on Bedrock (it imports the optional `anthropic` package) and is
exempt.

**Agent Framework** (27 stable releases from 1.0.0 to 1.20.0, probes `tool_use`, `streaming`; 4 classes, no
failures). 1.0 - 1.0.1, 1.1 - 1.8 (message JSON), 1.9.0 alone (semconv cache token keys) and 1.10 onwards.
The `gen_ai_latest_experimental` opt-in changes nothing in any of them. The provider and orchestration
packages release in lockstep and import the core's internals, so they resolve as of each core release's day.
The weekly pre-1.0 betas are not covered.

**OpenTelemetry's Google GenAI instrumentation** behind the `google-genai` suite (9 beta releases since
2025-10-05, with and without the opt-in; 6 classes). The Google GenAI client resolves as of each release's day
(`era`), because 0.4b0 - 0.7b0 do not instrument the 2.x client the suite locks. Without the opt-in, 0.4b0 -
0.7b1 write the conversation only as GenAI log events, which Logfire's documented setup does not export, so
those captures are withheld; opted in, 0.4b0 - 0.6b0 put `gen_ai.input.messages` / `output.messages` /
`system_instructions` on the model span and 0.7b0 - 0.7b1 add `gen_ai.tool.definitions`; 1.0b1 is rewritten
on `opentelemetry-util-genai` (chat operations, `gen_ai.tool.call.arguments` / `.result`) with or without the
opt-in; 1.1 changes message JSON; 1.2b0 returns to `generate_content` operations. The `vertex-ai` suite shows
the same classes from 0.7b1 on; 0.4b0 - 0.7b0 are exempt there, because the client of their day has no
Vertex AI (`enterprise`) mode.

**OpenTelemetry's botocore instrumentation** behind the `bedrock` suite (11 releases, 2 classes): 0.64b0
onwards differ from 0.59b0 - 0.63b1 only by the SDK's `service.instance.id` resource attribute; the opt-in
changes nothing.

**Semantic Kernel** (17 releases, 2 classes): 1.37.1 names its model spans `chat.completions` /
`chat.streaming_completions`, 1.38 onwards `chat`.

**AG2** (27 releases, 4 classes): 1.0.3 adds `record_usage` spans; 1.0.4 and 1.0.5 change message JSON. The
0.10 - 0.14 releases are exempt: before 1.0 AG2 is imported as `autogen`.

**AgentScope** (31 releases, 2 classes): 2.0.6 writes no `gen_ai.input.messages` on its chat spans, so the
user's turn is only on the agent span and the trace view orders it after the reply; its captures are withheld
until the reconstruction places it. 1.x predates the 2.0 rewrite the suite is written against, and 2.0.0 -
2.0.5 ignore the client the suite assigns and call api.anthropic.com, so both are exempt.

**opentelemetry-haystack** has one release in the window, the suite's own.

**OpenInference's Agno instrumentor** (24 releases, 2 classes): from 0.1.35 the model input's tool messages
carry `llm.input_messages.#.message.tool_call_id`. 0.1.16 - 0.1.30 are exempt: they call wrapt 1.x's
`wrap_function_wrapper(module=...)` without bounding wrapt below 2.

**Logfire behind the Anthropic suite** (37 releases, 4 classes): 4.19 drops the tool loop's own log spans,
4.33 adds `logfire.version` and 4.37 the `host.*` / `os.*` resource attributes. Logfire 4.31.1 - 4.41 import
httpx without declaring it, so the pin adds it. Logfire 4.x reports Anthropic calls as its own
`request_data` / `response_data`, which the asset reads only for OpenAI, so the historical captures are
withheld; the suite records SDK mode only, because native Logfire 5.1.1 abandons the span of a call made
without tools.

**OpenInference's AgentChat instrumentor** behind the `autogen` suite (17 releases, 2 classes): 0.1.5 records
no `llm.output_messages`, model name or token counts, so the reply is in no view and its captures are
withheld; 0.1.6 onwards is the suite's format.

**OpenInference's OpenAI instrumentor** behind `azure-openai` (28 releases, 2 classes): 0.1.46 adds
`llm.finish_reason`. **OpenInference's Bedrock instrumentor** behind `openinference` (26 releases, 4 classes):
0.1.36 adds tool-call names on messages, `llm.provider` and cache token counts, 0.1.43 changes message JSON,
0.1.50 adds `llm.finish_reason`; 0.1.35's tool-calling turn is withheld (an empty text block). Like the
Agno instrumentor, the releases before 0.1.46 (OpenAI) and 0.1.35 (Bedrock) call wrapt 1.x's
`wrap_function_wrapper(module=...)` and are exempt.

**Logfire behind the `logfire` suite** (Chat Completions and an agent loop; 41 releases): the same 10 classes
as behind the OpenAI suite. 4.12's streamed call is withheld, as there.

**The OpenAI Agents SDK** behind `openai-agents` (76 releases, 5 classes from 0.18.1, differing in the JSON of
Logfire's `events` carrier). 0.4.0 - 0.18.0 are exempt: with the current OpenAI SDK they cannot build their
usage record, the current Logfire imports span types they lack, and the OpenAI SDK of their day does not
accept the harness's httpx2 client.

**The TraceLoop SDK** (43 releases, 4 classes): 0.53.4 moves from indexed `gen_ai.prompt.N.*` /
`gen_ai.completion.N.*` `bedrock.converse` spans to `chat` spans in the current conventions, 0.56.1 adds
`gen_ai.tool.name` on tool spans, 0.62.1 drops `traceloop.workflow.name`. 0.47.4 - 0.48.1 import
`opentelemetry._events`, which OpenTelemetry 1.45 removed, and are exempt.

**OpenInference's smolagents instrumentor** (24 releases, 4 classes): 0.1.27 records input messages,
0.1.28 moves their content into `message.contents`, 0.1.33 gives streamed calls their own span; 0.1.19 -
0.1.25 call wrapt 1.x and are exempt. **OpenInference's LlamaIndex instrumentor** (19 releases, 4 classes):
4.3.9 records tool calls in the model input (the 4.3.6 - 4.3.8 captures are withheld for the empty text block
their tool-calling turns leave), and 4.4 and 4.5.3 change message JSON.

**OpenInference's LangChain instrumentor** behind `langchain` and `langgraph` (26 releases): every runnable
release (0.1.63 onwards) emits the suites' own format; 0.1.53 - 0.1.62 call wrapt 1.x and are exempt.

**OpenInference's CrewAI instrumentor** (31 releases, 4 classes, CPython 3.13 since CrewAI supports nothing
newer): 1.1.3 traces the executor's flow nodes, 1.1.4 changes message JSON, 1.1.8 drops the flow-node spans,
1.1.11 adds a `<tool>.run` span per tool run; 0.1.14 - 1.1.2 call wrapt 1.x and are exempt.

**Langfuse** (69 releases, 4 classes from 4.2): 4.7 adds `langfuse.internal.is_app_root`, 4.9.1 and 4.14
change message JSON. 3.6 - 4.1 are exempt: the suite hands Langfuse the application's exporter through the
`span_exporter` argument 4.2 introduced.

**LangSmith's OpenTelemetry export** behind `langsmith` (142 releases, 5 classes): 0.7.36 adds the `gen_ai.*`
prompt, completion, operation and usage attributes, 0.14.0 `gen_ai.tool.name`, 0.14.1 `gen_ai.tool.call.id`
and `gen_ai.tool.definitions`, 0.14.3 changes their JSON. Only 0.14.3 onwards reconstructs: the historical
captures are withheld until the asset reads the earlier JSON.

**Google ADK for Go** (17 releases): only 1.7 onwards runs the suite, in one format; 0.1 - 0.5 predate the
telemetry setup API it configures, and 0.6 - 1.6.1's internal telemetry calls an OpenTelemetry Go log API the
required `otel/log` no longer has. **Genkit for Go** (15 releases, 2 classes): 1.12 still emits otelhttp HTTP
client spans, 1.13 does not; before 1.12 the Google AI plugin has no `BaseURL` to reach the fake Gemini.
**Google ADK for Java** (15 releases, 4 classes): 0.8 - 0.9 write `tool_call` / `tool_response` spans, 1.0
`execute_tool` spans and `call_llm` operations, 1.4 changes message JSON, 1.6 adds usage tokens; 0.4 - 0.7 do
not run the suite.

**Koog** (18 releases): only the suite's 1.3.0 runs it; before 1.0 `SpanAdapter` is internal, and 1.0 - 1.2
have no Claude Sonnet 5 in their Bedrock catalogue. **OpenInference's LangChain4j instrumentor** (5
releases, 2 classes): the finish reason moves from `llm.response.finish_reasons` to `llm.finish_reason` in
0.1.9; 0.1.5 - 0.1.6 lack the AI-service listeners the suite registers. **OpenInference's Spring AI
instrumentor** (3 releases, 2 classes): 0.1.10 adds `llm.finish_reason`. **Strands Agents for TypeScript**
(38 releases): without the opt-in 1.2 onwards is one format; opted in, 1.12 adds
`gen_ai.tool.call.arguments` / `.result`. The 0.x line lacks the `@strands-agents/sdk/telemetry` export the
suite imports; 1.0.0 - 1.1.0 call CountTokens before each model call, replay cassettes of their own recorded live,
and fall in the existing classes.

**`@ai-sdk/otel`** behind `vercel-ai-js` (119 releases, 4 classes): 1.0.44 adds `gen_ai.response.model`, 1.0.54
and 1.0.69 change message JSON.

Replay gives the scenario an HTTP proxy that refuses everything but loopback, so a release that ignores the
client it is given (AgentScope 2.0.0 - 2.0.5) fails locally instead of reaching a provider's public API. A suite
that must reach another host declares it with its reason (`allow-hosts`: LiteLLM routes Bedrock models by
the model map it downloads at import).

### Not yet covered by the matrix

- **Live captures needed:** any release whose model traffic differs from the committed cassettes in call
  count or API gets a `[[recording]]` and `--live`, as Strands 1.38.0 and 1.39.0 have.
- **TypeScript suites:** a `versions.toml` beside a suite under `examples/javascript` makes its variants copies
  of the npm project, installed once as of `resolved-before` and cloned copy-on-write per release with only
  the pinned packages moved (`npm install --before`). Not yet covered: the AI SDK's 5.x and 6.x lines, which
  predate the `@ai-sdk/otel` package the suite is written against, and the Claude Agent SDK, whose 300
  releases each bundle a CLI.
- **Go and JVM suites:** a `versions.toml` beside a suite under `examples/go` or `examples/java` makes its
  variants copies of the Go module (`go get <module>@v<version>`) or of the Gradle build with a version catalog
  entry replaced (`pin = "<catalog key>={version}"`); releases come from the go command and Maven Central.
  Agent Framework's pre-1.0 betas are not covered.

## Version variants in the rule language

The rule language keeps one principle from the engine design: behaviour is selected by observable evidence,
and producer identity never selects a parser. Version support follows from it:

- **Shape is the authority.** A version difference is expressed as another ordered alternative, `first_of`
  branch or prioritised clause whose predicate (`where`) matches what the payload looks like. There is no
  separate `variants` construct: it would be a second spelling of ordered alternatives.
- **Versions are evidence, not switches.** A clause may carry a behaviour-free `observed_in` annotation
  (`[{"package": "strands-agents", "since": "1.11.0", "before": "1.51.0", "profile": "semconv-latest"}]`,
  half-open ranges) recording which releases it exists for. It feeds documentation, coverage (each annotated
  clause must fire on a matrix fixture inside each range) and ageing (a clause whose every range ended before
  the support window is reported for removal).
- **A version gate is a last resort.** Only when two releases write the same carrier with the same observable
  shape but different meaning may a clause test a version, through a typed `version` predicate atom over a
  locally observable source (`scope.version`, a named attribute or resource attribute), with an explicit
  scheme (PEP 440 or SemVer). An absent or malformed version is unknown, never "latest". Each gate states why
  shape cannot decide and is backed by fixtures below, at and above its boundary.

The engine work this needs is tracked in the rule-language iteration program; until it lands, the matrix
fixtures are checked by the ordinary goldens and invariants.

## The version matrix

`examples/python/<suite>/versions.toml` describes a suite's support window and its variants:

- `[matrix]`: the `package` whose release history defines the window, the `pin` template installed for a
  release, `since` (first day of the window), `resolved-before` (every environment resolves as of this instant
  with `uv --exclude-newer`, so a rebuilt environment holds the same dependencies without a lockfile per
  variant), the `probes` the census runs, the `scenarios` a variant records, and named `profiles` (environment
  settings such as `OTEL_SEMCONV_STABILITY_OPT_IN`).
- `[[variant]]`: one release under one profile, recorded as fixture mode `native@<version>[+<profile>]`.
- `[[exempt]]`: a release the census cannot run, with the reviewed reason and a revisit date.
- `[[recording]]`: a release whose model traffic the suite's cassettes cannot answer (another API, an extra
  call), with the reason. It replays `<suite>/cassettes@<version>/`, recorded live once with
  `python -m harness matrix <producer> --live [<version>]` through the recording proxy on the ambient AWS
  credentials; the census and the variants then replay it offline like any other.

The window ends at `resolved-before`: a release uploaded after it cannot be installed in an environment
resolved as of that instant, and waits for the next census, which moves both dates.

Commands, run from `examples/python/harness` (or through `make`):

| Command | Network | What it does |
| --- | --- | --- |
| `make matrix P=<producer> [V=<variant>]` | package index only, for a missing environment | Builds each variant's environment under `$SIDESEAT_MATRIX_CACHE` (default: the user cache directory), replays every variant scenario offline from the suite's cassettes, writes `server/tests/fixtures/messages/<producer>/native@<variant>/<scenario>/` and `<producer>/versions.json` (provenance), then deletes the environments. |
| `make matrix-census P=<producer>` | yes | Classifies every release of the window under every profile and writes `<suite>/versions.census.json`. |
| `make matrix-check` | no | Every classified release has a variant with its shape, no variant is redundant or aged out, every unclassified release is exempt, and each variant's committed probe fixtures still have the shape the census recorded. Also run by the harness tests. |

Replay matches an identical request to its recorded answer and otherwise falls back to arrival order on the
same method and path, because an older release serialises its requests differently. The fallback is sound
only while the conversation takes the same course, so a matrix run fails if any request has no answer or if
any recorded answer is left unasked. Fake-model suites need no cassette. Where a release changes the model
traffic itself (a different API, an extra call), replay fails and the release needs a live capture.

The **shape** of a capture keeps what rules can match: resource attribute keys; per span the scope name, span
name, attribute keys (indexed segments folded) and event names with their keys; per log record the event name,
keys and body structure; the structure of every JSON-valued string; and identifier-like values such as
`gen_ai.operation.name` values, which predicates compare against. Ids, timestamps, free text and versions are
dropped. Equal shapes form an empirical telemetry class: strong evidence of one format, not a proof, because
array order and multiplicity are not kept and only the probe scenarios are compared.

The matrix is opt-in: it is not part of `make quick` or `make test`. The fixtures it writes are ordinary
golden fixtures, so `message_goldens` and the truth checks cover them like any other.
