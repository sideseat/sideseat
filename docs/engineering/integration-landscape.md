# Integration landscape

Which OpenTelemetry instrumentation libraries and agent frameworks SideSeat must read, how much each is
used, and how far each is verified. Demand is last month's downloads (PyPI via pypistats, npm via the
registry API), measured 2026-10-05; it ranks the work, it does not prove telemetry quality. "Verified"
means a captured suite under `server/tests/fixtures/messages/`, read in both native and SDK modes.

## Instrumentation libraries

These write the telemetry. A framework is only as readable as the library that instruments it.

| Library | Language | Monthly downloads | Rule asset | Verified suite |
| --- | --- | ---: | --- | --- |
| OpenTelemetry GenAI semantic conventions (`gen_ai.*`) | all | — | `conventions/semconv.json` | through every suite below |
| OpenInference (Arize) | Python, JS | 2.2M (openai instrumentor) | `openinference.json` | `openinference`, `langchain`, `langgraph`, `crewai`, `autogen`, `agno`, `smolagents`, `llama-index`, `azure-openai` |
| Logfire | Python | 12.8M | `logfire.json` | `logfire`, `openai`, `anthropic`, `openai-agents`, `pydantic-ai`, `google-genai`, `vertex-ai` |
| TraceLoop / OpenLLMetry | Python | 1.7M (`traceloop-sdk`) | `traceloop.json` | `traceloop` |
| Langfuse 4 | Python, JS | 22.9M PyPI, 9.4M npm (`@langfuse/otel`) | `langfuse.json` | `langfuse` (Python) |
| Laminar (`lmnr`) | Python | 5.9M | `browser-use.json` | `browser-use` |
| OpenTelemetry botocore | Python | — | `bedrock.json` | `bedrock` |
| Vercel AI SDK telemetry | JS | 111M (`ai`) | `vercel-ai.json` | `vercel-ai-js` |
| LangSmith OpenTelemetry export | Python, JS | 76M PyPI | `langsmith.json` | `langsmith` (Python, over the LangGraph scenarios) |
| MLflow tracing | Python | 19.7M | `mlflow.json` | none |
| LiteLLM `otel` callback | Python | 91M | none | none |
| OpenTelemetry official GenAI instrumentations (`openai-v2`, `google-genai`, `vertexai`, `util-genai`) | Python | 1.7M-4.8M each | `semconv.json` | none of its own |
| `@opentelemetry/instrumentation-openai` | JS | 31M | `semconv.json` | none |
| OpenLIT | Python | 0.27M | none | none |
| AgentOps | Python | 0.1M | none | none |
| W&B Weave | Python | 0.8M | none | none |

## Agent frameworks

| Framework | Language | Monthly downloads | Verified suite |
| --- | --- | ---: | --- |
| Strands Agents | Python, JS | 36.8M, 2.1M | `strands`, `strands-js` |
| Claude Agent SDK | Python, JS | 29.4M, 48.9M | `claude-agent-sdk`, `claude-agent-sdk-js` |
| OpenAI Agents SDK | Python, JS | 12.2M, 7.9M | `openai-agents` (Python only) |
| Google ADK | Python, JS | 10.0M, 1.0M | `adk` (Python only) |
| Browser Use | Python | 8.0M | `browser-use` |
| LlamaIndex | Python, JS | 6.5M, 0.5M | `llama-index` (Python only) |
| Pydantic AI | Python | 5.3M | `pydantic-ai` |
| CrewAI | Python | 2.4M | `crewai` |
| Agno | Python | 1.7M | `agno` |
| Haystack | Python | 0.6M | `haystack` |
| smolagents | Python | 0.4M | `smolagents` |
| AutoGen / AG2 | Python | 0.4M / 0.2M | `autogen`, `ag2` |
| Semantic Kernel | Python, .NET | 0.3M | `semantic-kernel` (Python) |
| AgentScope | Python | 0.2M | `agentscope` |
| Microsoft Agent Framework | Python, .NET | — | `agent-framework` (Python) |
| LangChain / LangGraph | Python, JS | 76M (via LangSmith); 24M / 14.6M npm | `langchain`, `langgraph` (Python only) |
| Vercel AI SDK | JS | 111M | `vercel-ai-js` |
| Mastra | JS | 7.3M | none |
| LiveKit Agents | Python, JS | 3.5M, 3.1M | none (asset `livekit.json`) |
| DSPy | Python | 5.2M | none |
| Pipecat | Python | 1.0M | none |
| Genkit | JS | 1.0M | none |
| Mem0 | Python | 2.0M | none |
| Langflow | Python | 0.04M | none (asset `langflow.json`) |

## Go

There is no SideSeat SDK for Go; the suites under `examples/go` run plain OpenTelemetry, natively and under
the recipe in `docs/src/content/docs/docs/integrations/frameworks/go.mdx` (`sideseat.framework` on the
resource).

| Framework | Version | Verified suite |
| --- | --- | --- |
| Genkit for Go | 1.13.1 | `genkit-go` (fake Gemini) |
| Google ADK for Go | 1.8.0 | `adk-go` (fake Gemini; conversation in GenAI log events, not yet reconstructed) |
| Eino, LangChainGo | 0.9.21, 0.1.15 | none yet |

## Java and Kotlin

There is no SideSeat SDK for the JVM; the suites under `examples/java` (one Gradle build, wrapper-pinned) run
plain OpenTelemetry, natively and under the recipe in `docs/src/content/docs/docs/integrations/frameworks/java.mdx`.

| Framework | Version | Instrumentation | Verified suite |
| --- | --- | --- | --- |
| Spring AI | 2.0.1 | OpenInference Spring AI 0.1.10 | `spring-ai` |
| LangChain4j | 1.21.0 | OpenInference LangChain4j 0.1.9 | `langchain4j` |
| Koog | 1.3.0 | its own OpenTelemetry feature | `koog` |
| Google ADK for Java, Semantic Kernel for Java | 1.11.0, 1.5.0 | their own | none yet |

## Gaps, in the order to close them

Each gap is a suite on the shared harness (`examples/<language>/<suite>`), a rule asset where the
library has its own dialect, and an SDK integration where the library needs switching on. A suite whose
model requests equal an existing suite's replays that suite's cassettes, as `langfuse` replays `openai`.

1. **LangSmith OpenTelemetry export in JavaScript.** Python is verified by the `langsmith` suite.
2. **MLflow tracing**. Asset exists, no suite. `mlflow.openai.autolog()` with OTLP export.
3. **LiteLLM `otel` callback**. The most downloaded unverified library; no asset yet.
4. **OpenTelemetry's official GenAI instrumentations**, including the newer `util-genai` content modes.
   The reference implementation of the conventions SideSeat claims to read; content arrives as log
   events by default, which the log-event conversation path reads.
5. **JavaScript LangChain / LangGraph**, **OpenAI Agents JS**, **Mastra**, `@opentelemetry/instrumentation-openai`,
   and **Langfuse JS**: the TypeScript harness covers only Strands, Vercel AI and the Claude Agent SDK.
6. **DSPy** (through OpenInference or MLflow), **LiveKit Agents** (asset exists; voice, so its scenarios
   differ), **Pipecat**, **Genkit**.
7. Long tail: OpenLIT, AgentOps, Weave, Mem0.

Keep this table current when a suite lands: the README's support list, the fixtures README's support
matrix and `docs/src/content/docs/docs/reference/production-readiness.mdx` are the user-facing copies.

## Coding-agent CLIs

Researched 2026-10-05 against the installed binaries (Claude Code 2.1.288, Codex CLI 0.160.0, Kiro CLI
2.27.1), their documentation, and live captures through `examples/cli/capture.py`. Captured suites:
`claude-code/native`, `claude-code/logs`, `codex/native`, each holding `tool_use`, `multi_turn`, `error`
and `multi_agent`; assets `producers/claude-code.json` (log events), `producers/claude-agent-sdk.json`
(spans, shared with the Agent SDK, which runs the same CLI) and `producers/codex.json`.

### Claude Code

Configured by environment variables only (`CLAUDE_CODE_ENABLE_TELEMETRY=1` and the `OTEL_*` exporters).
Three signals:

- **Metrics**: `claude_code.session.count`, `cost.usage`, `token.usage`, `active_time.total`,
  `lines_of_code.count`, `commit.count`, `pull_request.count`, `code_edit_tool.decision`. No
  conversation content.
- **Log events**, scope `com.anthropic.claude_code.events`. The body is `claude_code.<name>`; the
  `event.name` attribute repeats the name **without** the prefix, and the record's own `event_name`
  field is unset. Content-bearing: `user_prompt` (`prompt`, and `prompt_text` since 2.1.287; redacted
  unless `OTEL_LOG_USER_PROMPTS`), `assistant_response` (`response`; since 2.1.193, gated by
  `OTEL_LOG_ASSISTANT_RESPONSES`, which falls back to the prompt gate), `tool_result` (`tool_name`,
  `tool_use_id`, `success`, `error`, and `tool_input` with `OTEL_LOG_TOOL_DETAILS` - never the output),
  `system_prompt` (only under the detailed tier), `api_request`/`api_error` (tokens, cost, latency),
  `tool_decision`, and raw bodies (`api_request_body`/`api_response_body`) behind
  `OTEL_LOG_RAW_API_BODIES`. Since 2.1.212 every record emitted during a turn carries the turn's
  `claude_code.interaction` trace and span id - before that, records outside the span's async context had
  none, and before 2.1.214 they could carry the inbound `TRACEPARENT` ids instead.
- **Spans** (beta, `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`): `claude_code.interaction` >
  `llm_request`, `tool` > `tool.blocked_on_user`, `tool.execution`, and `hook`. A sub-agent's requests
  and tools nest under the `Agent` tool's span. Text on spans needs the second tier,
  `ENABLE_BETA_TRACING_DETAILED=1` with `BETA_TRACING_ENDPOINT` (which then receives logs as well, as
  OTLP/JSON): `new_context`, `response.model_output`, `tool_input`, `system_reminders`. Interactive
  sessions get that tier only when the organisation is allowlisted; `-p` and SDK sessions always do.
  `OTEL_LOG_TOOL_CONTENT=1` adds the `tool.output` span event (Read, Bash; Edit and Write with tool
  details; MCP, WebFetch, WebSearch since 2.1.283).

So there are two configurations worth supporting, and both are captured: `native` (detailed tier + logs,
tool content off), where the same text arrives twice and collapses, and `logs` (enhanced tracing without
the detailed tier, tool content on), where the conversation is rebuilt from log events plus `tool.output`.
Both together duplicate every tool result, because `tool.output` names no call; and without the detailed
tier a call's input is only on the `tool_result` record written after the call, so calls show their name
alone (backlog cli-1, cli-2 in the rule-language program). What no configuration
exports: the model's thinking, and a failed Bash call's output (only `error: "Shell command failed"`).
Span-level formats changed in 2.1.268 (`query_source_safe`, `first_content_ms`, `error_class`,
`tool_name_safe`, `bash_*`), 2.1.274 (`effort`) and 2.1.282 (`request_id` from `x-amzn-requestid` on
Bedrock); none of those carry conversation content. The CLI's spans are labelled `ClaudeAgentSDK`,
because nothing in `-p` telemetry distinguishes the CLI from the SDK that runs it.

### Codex CLI

Configured by the `[otel]` table of `~/.codex/config.toml` (ignored in a project's `.codex/`):
`exporter` (logs), `trace_exporter`, `metrics_exporter`, each `otlp-http` or `otlp-grpc`;
`log_user_prompt`; and, undocumented in 0.160, `log_agent_responses`. `service.name` is the client:
`codex_exec`, `codex_cli_rs`, `codex_vscode`.

- **Log events**, scope `codex_otel.log_only`, named by the `event.name` attribute:
  `codex.conversation_starts`, `codex.user_prompt` (`prompt`), `codex.tool_decision`,
  `codex.tool_result` (`tool_name`, `call_id`, `arguments`, `output`, `success`, `agent_name`),
  `codex.agent_response` (`response`, `agent.type` main/subagent, `parent.conversation.id`),
  `codex.api_request`, `codex.sse_event` (token counts on `response.completed`), `codex.turn_ttft`,
  `codex.websocket_*`. Every record carries `conversation.id`. Sub-agents (`spawn_agent`, `wait_agent`,
  `close_agent`) log under their own `conversation.id` in the parent's trace.
- **Spans**: Codex's internal `tracing` spans - several hundred per turn (`turn/start`,
  `session_task.turn`, `run_sampling_request`, `handle_responses`, `exec_command`, `fs.read_file`...),
  none in the GenAI conventions and none carrying text; token usage sits on `handle_responses` and
  `session_task.turn`.
- **Metrics**: `codex.api_request`, `codex.sse_event`, `codex.tool.call` (each with `.duration_ms`),
  `codex.turn.*`, and many more.

The conversation is therefore entirely in log events, and a record names a span **only while the trace
exporter runs**: with `trace_exporter = "none"` (Codex's default, and what most users set) every record is
unattached, and SideSeat reads no messages from it. What Codex never exports: the assistant's text
between tool calls (only the turn's final reply is `codex.agent_response`), reasoning, and the system
prompt. No span carries the conversation id under a key of its own (`session_task.turn` has it as
`thread.id`, which elsewhere is the OS thread), so a resumed session's turns are separate traces with no
session. Before `log_agent_responses` the assistant's reply was not exported at all.

Model access for capture: Codex's built-in `amazon-bedrock` provider calls `bedrock-mantle`, so the
driver declares a custom Responses provider at the harness proxy, which signs for `bedrock-runtime`'s
`/openai/v1` endpoint. A provider named "Amazon Bedrock" is taken for the built-in one and signs its own
requests.

### Kiro CLI

No OpenTelemetry export of the conversation. Kiro CLI 2.27.1 (`kiro-cli-chat`) contains a metrics-only
OTLP/HTTP exporter for its own product telemetry (`kiro_cli_model_request_duration_seconds`,
`kiro_cli_user_turn_duration_seconds`, `kiro_cli_startup_duration_seconds` and TUI latencies), switched
by the undocumented `KIRO_TELEMETRY_OTEL` / `KIRO_TELEMETRY_OTLP_ENDPOINT` /
`KIRO_TELEMETRY_EXPORT_INTERVAL_MS` and defaulting to `telemetry.<region>.kiro.dev`. It carries no prompt,
response or tool content and was not exercised here, since running it needs a Kiro account and Kiro's own
model service. The documented monitoring - the Kiro console dashboard, per-user activity reports, prompt
logs, CloudTrail and CloudWatch - is administrative and not OTLP. Support would need Kiro to emit spans or
log events; hooks could forward events, but that would be a SideSeat-specific integration, not Kiro's
telemetry.

### Gaps

- Codex logs without the trace exporter, and Claude Code logs without a traces exporter, name no exported
  span, so they reach no conversation. Reading them needs a session-scoped log conversation keyed by
  `conversation.id` / `session.id` (a server change, recorded in the rule-language backlog).
- Codex sessions: a span field source conditional on the span (or the instrumentation scope) would let
  `thread.id` on `session_task.turn` name the session without reading every OS thread id as one.
- Claude Code's `-p` telemetry cannot be told apart from the Agent SDK's, so its label stays
  `ClaudeAgentSDK`.
