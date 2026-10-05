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

1.38.0 and 1.39.0 call Bedrock CountTokens before every model call, which the cassettes predate; they are
exempt until captured live. Reconstruction: every default-format class except 1.11 - 1.12 (whose tool results
are the Python `str()` of the result, not JSON) reconstructs the same conversation as the current release, up to the order of parallel tool
results; the `semconv-latest` profile is not yet read correctly in any release (user and assistant messages
arrive as raw `unknown` blocks, reasoning is lost), which the truth ledger records under backlog item 156.

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
