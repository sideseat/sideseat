# Message-parsing fixtures

Inputs for `server/tests/message_goldens.rs`, which checks that message
**count, content, ordering and absence of duplicates** hold for every framework *that has a
fixture here*, in all four views the API exposes. Coverage is the exact versioned support matrix
below, and not every fixture has a session view - see [What is and is not
covered](#what-is-and-is-not-covered), which is the honest version of this sentence:

| View    | Row set                                                | API endpoint                     |
| ------- | ------------------------------------------------------ | -------------------------------- |
| span    | `WHERE span_id = ?`, no content filter                 | `/spans/{trace}/{span}/messages` |
| trace   | whole session when the trace has one, then scoped back | `/traces/{id}/messages`          |
| session | every row of every trace in the session                | `/sessions/{id}/messages`        |

A trace belongs to **exactly one** session: the one on its earliest span, ordered by
`(timestamp_start, span_id)`. Frameworks do put two session ids on one trace — ADK emits its own
beside the caller's — and 24 of these fixtures once recorded a session view under *both*, holding the
identical messages. The goldens had blessed that duplication as correct, which is the failure mode
they are most prone to.
| feed    | every row, newest response first                       | `/feed/messages`                 |

The first three call `process_spans` and differ only in their row set, so each is built with its
own - using `process_feed` for a session tested ordering no session request can return. The feed
has its own entry point and its own ordering, and is here because while it was left out it was the
only view where a duplicate could surface unchecked. Its pagination is not modelled: that is a
property of the endpoint, not of parsing.

Trace, session and feed row sets apply `MESSAGE_CONTENT_FILTER` and `ORDER BY timestamp_start
ASC`, exactly as the queries do — feeding unfiltered rows made whole sessions come back empty.

A sample's `logs-*` exports are read by the real log extraction and joined to its spans the way the
message queries join `otel_logs`: one record per `(log_digest, ordinal)`, grouped by `(trace, span)`, in
`(timestamp, log_digest, ordinal)` order, attached to nothing when the span is absent. They are not
requests, so the matrix below counts only `req-*`.

## Layout

```
<producer>/<mode>/<scenario>/req-001.pb     captured OTLP payload (protobuf, or .json)
<producer>/<mode>/<scenario>/req-002.pb     one file per exported batch, in capture order
<producer>/<mode>/<scenario>/logs-001.pb    captured OTLP log export, attached to the spans it names
<producer>/<mode>/<scenario>/expected.json  committed expectation
_synthetic/<sample>/                        hand-written shapes no producer emits on its own
```

A producer is a framework or provider (`strands`, `openai-agents`, `bedrock`) or an SDK conformance
program (`python`, `javascript`, `dotnet`, `rust`). The mode says who configured the telemetry:

| Mode | Telemetry configured by |
| --- | --- |
| `_synthetic` | hand-written shapes, no SDK | 20 | 20 |
| `adk/native` | Google ADK 2.11.0 / LiteLLM 1.104.0 (Bedrock Converse) / OpenTelemetry Python 1.42.1 on CPython 3.14.7, ADK's own tracing on a global provider; ADK's trace copy of a request leaves out inline parts, so `files` holds the request text only, and `transfer_to_agent`'s result reaches the next agent only as quoted context | 11 | 18 |
| `adk/sdk` | SideSeat Python 2.0.0 / Google ADK 2.11.0 / LiteLLM 1.104.0 (Bedrock Converse) / OpenTelemetry Python 1.42.1 on CPython 3.14.7; the integration restores the image and PDF ADK leaves out | 11 | 12 |
| `ag2/native` | AG2 1.1.1 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native TelemetryMiddleware; AG2's telemetry records no system prompt, binary input, or reasoning, so `files` and `reasoning` carry their text only | 11 | 17 |
| `ag2/sdk` | SideSeat Python 2.0.0 / AG2 1.1.1 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `agent-framework/native` | Agent Framework 1.19.0 (core) / agent-framework-anthropic 1.0.0b260918 / Anthropic 0.116.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, `enable_sensitive_telemetry()` on a plain provider; no `files` (neither Bedrock client sends documents) | 10 | 13 |
| `agent-framework/sdk` | SideSeat Python 2.0.0 / Agent Framework 1.19.0 (core) / agent-framework-anthropic 1.0.0b260918 / Anthropic 0.116.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 10 |
| `agent_snapshot_reorders_answer/legacy` | a root agent span re-listing a whole turn **answer-first** while its child generation spans emit the calls and the answer separately — the shape the Vercel AI SDK's current integration produces | The **redundant re-listing** rule (`redundant_relistings`, `order_graph.rs`). Its golden records the correct conversation — question, calls, results, answer — and did not until that rule landed: `gen_ai.output.messages` reads as one atomic emission wherever it appears, so the re-listing's stated order was trusted and the answer sorted ahead of the calls that produced it. Two earlier attempts are recorded in `a_relisting_is_discounted_only_on_evidence_from_below_it`: declaring the carrier `accumulated_state` **lost a message** in `agent-framework/tool_use`, and discounting any instance whose messages appear below it fired 4,044 times across the corpus and broke `agent-framework/swarm` and `strands/image_gen`. The rule that works asks two questions instead — is every message witnessed by a *descendant*, and does the instance hold **both** a message the span produced and a result answering it, which no single model response can |
| `agentscope/native` | AgentScope 2.0.9 / Anthropic 1.11.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native `TracingMiddleware` | 11 | 15 |
| `agentscope/sdk` | SideSeat Python 2.0.0 / AgentScope 2.0.9 / Anthropic 1.11.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `agno/native` | Agno 3.1.0 / Anthropic 1.11.0 (Bedrock) / OpenInference Agno instrumentor 1.0.13 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; `files` records no image or PDF because the instrumentor drops media from `llm.input_messages` | 11 | 19 |
| `agno/sdk` | SideSeat Python 2.0.0 / Agno 3.1.0 / Anthropic 1.11.0 (Bedrock) / OpenInference Agno instrumentor 1.0.13 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 12 |
| `anthropic/sdk` | SideSeat Python 2.0.0 / Anthropic 1.11.0 (AnthropicBedrock) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.13.7; SDK only, because native Logfire 5.1.1 abandons the span of every Anthropic 1.11 call made without tools (it JSON-encodes the SDK's `Omit` sentinel), which the SideSeat integration repairs | 9 | 9 |
| `autogen/native` | AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenAI 3.24.0 / OpenInference AutoGen instrumentor 0.1.21 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup, against the harness's deterministic fake-openai endpoint; no `files` (AgentChat messages carry no documents) or `reasoning` (its chat client reads no reasoning from Chat Completions) | 9 | 11 |
| `autogen/sdk` | SideSeat Python 2.0.0 / AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenAI 3.24.0 / OpenInference AutoGen instrumentor 0.1.21 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, against fake-openai | 9 | 12 |
| `azure-openai/native` | OpenAI 3.24.0 (`AzureOpenAI`, Chat Completions on a deployment route, and the Responses API for `reasoning`) / OpenInference OpenAI instrumentor 0.1.63 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup, against the harness's fake OpenAI server because Bedrock credentials cannot reach Azure; the instrumentor writes `__REDACTED__` in place of a base64 image over its 32,000-character default, so `files` carries the image as a placeholder | 9 | 9 |
| `azure-openai/sdk` | SideSeat Python 2.0.0 / OpenAI 3.24.0 (`AzureOpenAI`) / OpenInference OpenAI instrumentor 0.1.63 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, against the harness's fake OpenAI server | 9 | 9 |
| `bedrock/legacy` | boto3 (bedrock runtime); only `invoke_model` remains, because no catalog scenario calls InvokeModel | 1 | 3 |
| `bedrock/sdk` | SideSeat Python 2.0.0 / boto3 1.43.107 Converse and ConverseStream / OpenTelemetry Python 1.45.0 on CPython 3.14.7; no native pair, because OpenTelemetry botocore instrumentation records messages only as log events, which capture does not record | 9 | 11 |
| `claude-agent-sdk-js/legacy` | @anthropic-ai/claude-agent-sdk ^0.3.246 | 8 | 17 |
| `claude-agent-sdk/native` | Claude Agent SDK 0.2.163 (Claude Code CLI telemetry) on Bedrock / OpenTelemetry Python 1.45.0 on CPython 3.13.7; the CLI exports attachments as text placeholders and no thinking | 11 | 26 |
| `claude-agent-sdk/sdk` | SideSeat Python 2.0.0 / Claude Agent SDK 0.2.163 on Bedrock / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 11 | 23 |
| `crewai/native` | CrewAI 1.15.23 / OpenInference CrewAI instrumentor 1.1.20 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, native OTLP setup; no `structured_output`, `files`, or `streaming`: CrewAI's Bedrock provider forces tool choice, refuses media for Claude 5, and does not run a streamed tool call | 8 | 13 |
| `crewai/sdk` | SideSeat Python 2.0.0 / CrewAI 1.15.23 / OpenInference CrewAI instrumentor 1.1.20 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 8 | 8 |
| `cross_span_tie/legacy` | a generation span and its tool span reporting the **identical** instant, with the tool span's id sorting *first* | `adopt_call_positions`. Disable it and this fixture reports the answer at index 1 before its question at index 3; every captured fixture stays green, because none of them ties |
| `log_and_span_event_overlap/legacy` | a dual emitter: the question as a span event **and** a log record, the answer only as a log record | the repeated question collapses to one turn, and the answer appears only because logs are joined (`log_carried_messages_reach_every_view_exactly_once`) |
| `log_events_before_span/legacy` | the OpenTelemetry OpenAI v2 / botocore default: the whole conversation as log records whose body members are the event, exported before the span, one naming its event through the legacy `event.name` attribute | `log_events` recognition by field and by attribute, and the read-time join; without the logs every view is empty |
| `log_inference_details/legacy` | `gen_ai.client.inference.operation.details` as a log record whose **attributes** carry the request and response as structured values | the container readings applied to a log record, with structured `AnyValue`s kept whole |
| `dotnet/native` | OpenTelemetry .NET 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `dotnet/sdk` | SideSeat .NET 1.0.0 / OpenTelemetry 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `google-genai/native` | Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, against the harness's fake Gemini server; reasoning thoughts arrive as text parts (the instrumentation drops Gemini's `thought` flag) and a failed tool call as an error message rather than a tool result | 9 | 9 |
| `google-genai/sdk` | SideSeat Python 2.0.0 / Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, against the harness's fake Gemini server | 9 | 9 |
| `haystack/native` | Haystack 3.3.0 / Amazon Bedrock Haystack 8.3.0 / MCP Haystack 1.5.1 / opentelemetry-haystack 1.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; Haystack's tracer writes a placeholder in place of image and file bytes, so `files` carries the request text only | 11 | 16 |
| `haystack/sdk` | SideSeat Python 2.0.0 / Haystack 3.3.0 / Amazon Bedrock Haystack 8.3.0 / MCP Haystack 1.5.1 / opentelemetry-haystack 1.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `javascript/native` | OpenTelemetry JS 2.11.0 / OTLP exporter 0.222.0 on Node.js 26.9.0 | 1 | 1 |
| `javascript/sdk` | SideSeat JavaScript 2.0.0 / OpenTelemetry JS 2.11.0 on Node.js 26.9.0 | 1 | 1 |
| `langchain/native` | LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; no `multi_agent`, because LangChain's multi-agent patterns run on LangGraph | 10 | 12 |
| `langchain/sdk` | SideSeat Python 2.0.0 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 11 |
| `langgraph/native` | LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup | 11 | 16 |
| `langgraph/sdk` | SideSeat Python 2.0.0 / LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `legacy` | an older capture with no native/SDK pair; removed as each producer is recaptured |
| `llama-index/native` | LlamaIndex Core 0.14.25 / LlamaIndex Bedrock Converse 0.15.3 / LlamaIndex MCP tools 0.6.0 / OpenInference LlamaIndex instrumentor 4.5.4 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; the instrumentor records no document blocks or reasoning and redacts large inline images, and `structured_output` is not captured yet: its reformatting request does not reconstruct | 10 | 21 |
| `llama-index/sdk` | SideSeat Python 2.0.0 / LlamaIndex Core 0.14.25 / LlamaIndex Bedrock Converse 0.15.3 / LlamaIndex MCP tools 0.6.0 / OpenInference LlamaIndex instrumentor 4.5.4 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 11 |
| `logfire/native` | Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0, Logfire's own setup with a scrubbing callback that keeps `session.id` | 11 | 11 |
| `logfire/sdk` | SideSeat Python 2.0.0 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 | 11 | 11 |
| `multi_turn_one_carrier/legacy` | nine turns in **one** carrier, in conversation order | carrier subsequence across many siblings - the ADK shape, where one span holds a whole conversation |
| `native` | the framework's own documented OpenTelemetry setup, or plain OpenTelemetry for a conformance program |
| `openai-agents/legacy` | openai-agents >=0.12.1; two fixtures remain. `image_gen` holds two different exports of the same spans, one scrubbed by Logfire and one not, which makes it the corpus's only fixture whose duplicate copies differ - `which_copy_survives_does_not_change_the_order` needs one - and makes it tie under reversed arrival, so `agent_core` stays the producer's sample for `the_order_spans_arrive_in_does_not_change_the_answer` | 2 | 7 |
| `openai-agents/native` | OpenAI Agents SDK 0.23.1 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 on CPython 3.14.7, Logfire's `instrument_openai_agents` with a scrubbing callback that keeps `session.id`; no `reasoning`, because Bedrock rejects `reasoning.summary` for GPT-6.1-sol, and `files` records the image as its data URL in text and the PDF as its file name, because Logfire's `events` flattens both (the bytes are only in `raw_input`, which no rule reads); in `multi_agent` the handoff's result follows the writer's instructions, because only the writer's request records it | 10 | 18 |
| `openai-agents/sdk` | SideSeat Python 2.0.0 / OpenAI Agents SDK 0.23.1 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 10 | 12 |
| `openai/native` | OpenAI 3.24.0 (Chat Completions, and the Responses API wherever tools are declared, on GPT-6.1-sol) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7, Logfire's documented `instrument_openai` setup with a scrubbing callback that keeps `session.id`; no `reasoning`, because Bedrock's GPT-6.1-sol rejects `reasoning.summary` and returns its reasoning only encrypted | 8 | 13 |
| `openai/sdk` | SideSeat Python 2.0.0 / OpenAI 3.24.0 (Chat Completions and Responses on GPT-6.1-sol) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 8 | 8 |
| `openinference/native` | OpenInference Bedrock instrumentor 0.1.56 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0, native OTLP setup; the instrumentor keeps only the last of a turn's parallel tool results, redacts images by default and drops documents | 12 | 17 |
| `openinference/sdk` | SideSeat Python 2.0.0 / OpenInference Bedrock instrumentor 0.1.56 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0 | 12 | 12 |
| `parallel_tool_calls/legacy` | two distinct calls in one response, then both results | causality *without* adjacency: `call, call, result, result` must be allowed |
| `pydantic-ai/native` | Pydantic AI 2.53.0 / OpenTelemetry Python 1.44.0 on CPython 3.14.7, `Agent.instrument_all()` on a plain provider | 11 | 17 |
| `pydantic-ai/sdk` | SideSeat Python 2.0.0 / Pydantic AI 2.53.0 / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 11 | 11 |
| `python/native` | OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `python/sdk` | SideSeat Python 2.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `resent_history/legacy` | a later span re-sending the earlier turn | the re-send collapses onto the original rather than duplicating it |
| `rust/native` | OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `rust/sdk` | SideSeat Rust 0.2.0 / OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `sdk` | the SideSeat SDK, with the same scenario code |
| `semantic-kernel/native` | Semantic Kernel 1.44.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0, native OTLP setup | 1 | 1 |
| `semantic-kernel/sdk` | SideSeat Python 1.0.8 / Semantic Kernel 1.44.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 | 1 | 1 |
| `smolagents/native` | Smolagents 1.26.0 / LiteLLM 1.103.2 (Bedrock) / OpenInference Smolagents instrumentor 0.1.42 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; no `structured_output` or `files` (unsupported) and no `mcp_tools` (its MCP adapter misreads the server schema) | 8 | 15 |
| `smolagents/sdk` | SideSeat Python 2.0.0 / Smolagents 1.26.0 / LiteLLM 1.103.2 (Bedrock) / OpenInference Smolagents instrumentor 0.1.42 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 8 | 8 |
| `strands-js/legacy` | @strands-agents/sdk ^1.14.0 | 7 | 12 |
| `strands/native` | Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, `StrandsTelemetry` | 11 | 14 |
| `strands/sdk` | SideSeat Python 2.0.0 / Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `tool_use/legacy` | a Strands call/result pair | the baseline hand-written case |
| `traceloop/native` | TraceLoop SDK 0.62.4 / Bedrock instrumentation 0.62.4 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0, native OTLP setup; image and document bytes are exported empty | 12 | 18 |
| `traceloop/sdk` | SideSeat Python 2.0.0 / TraceLoop SDK 0.62.4 / Bedrock instrumentation 0.62.4 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0 | 12 | 13 |
| `vercel-ai-js/legacy` | ai ^7.0.79 | 6 | 13 |
| `vertex-ai/native` | Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, native Logfire setup with the current `enterprise=True` Vertex mode, against the harness's fake Gemini server; reasoning thoughts arrive as text parts (the instrumentation drops Gemini's `thought` flag) and a failed tool call as an error message rather than a tool result | 9 | 9 |
| `vertex-ai/sdk` | SideSeat Python 2.0.0 / Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, current `enterprise=True` Vertex mode, against the harness's fake Gemini server | 9 | 9 |

The carrier-overlap defect is documented by `reading_more_carriers_only_adds_messages` rather than by a
fixture: it runs every fixture through both extraction modes and reports what each gains and what
reorders (today: 20 views gain messages, 10 reorder, all named). A hand-written payload for that shape
was tried and dropped - it reproduced the *attributes* but not the behaviour, because the LangGraph
reader claims only on a `langgraph.*` marker and parses a narrower message shape than the one written
by hand, so its exemption would have asserted a cause the fixture did not exhibit.

## Capability exemptions

`PAIRING_EXEMPT` in the test names fixtures whose *source* telemetry cannot satisfy tool
pairing, with the reason. Both `claude-agent-sdk*/subagents` are listed: the Claude Code CLI
emits a subagent's tool executions without the matching `tool_use` block, so the result is
callless upstream. A capability limit of a framework is recorded per fixture rather than
weakening the check for everyone.

`scripts/message-fixtures/review-goldens.py` also classifies the small set of source shapes
that deliberately resemble parser defects. The declarations are per fixture, warning type,
and exact occurrence count; a stale declaration or any additional warning is unresolved and
makes the command fail. The accepted cases are:

- schema-constrained JSON returned as assistant text or a canonical `json` block;
- terminal schema pseudo-tools such as `StructuredOutput` and `Person`, which are outputs rather
  than executions and therefore have no `tool_result`;
- `_synthetic/text_split_by_parallel_calls`, whose purpose is to test splitting one response
  around two calls and which intentionally contains no executions.

Run `scripts/message-fixtures/review-goldens.py --suspicious` after regenerating expectations.
Production-ready output is `0 unresolved`; intentional cases remain visible when reviewing their
fixture directly.

## `_synthetic/`

Hand-written, not captured: shapes no captured sample produces, plus a Strands-shaped tool-use
conversation that exercises the harness itself. Event shapes are taken from the assertions in
`server/crates/ingestion/src/traces/extract/messages_tests.rs` rather than invented — an unrealistic fixture
would produce confident but meaningless results.

Real captures are preferred for every framework, and these are not a substitute for one: each exists
because a defect was found in a shape the corpus did not hold, and the fixture is what makes the
answer to that shape reviewable. They are also the only fixtures that survive a checkout with no
credentials, so they keep the harness itself under test.

## Not committed

`crewai/agent_core` is gitignored: CrewAI serialises its entire model config into a span
attribute, so the captured payload contained a live `aws_secret_access_key` and
`aws_session_token`. A secret in a fixture goes straight into git history, where it cannot be
taken back — `scripts/message-fixtures/capture.sh` now discards any fixture whose payload matches that
shape rather than leaving the decision to a later reader.

`strands-js/image-gen` and `vercel-ai-js/image-gen` are gitignored. Those suites inline
generated images as base64 in the OTLP JSON — 7MB and 15MB for a single request — which would
sit in git history permanently for no extra parsing coverage. The Python `image_gen` fixtures
exercise the same path in under 100KB each, because media is rewritten to file URIs before
storage. Capture the JS ones locally when working on image handling; the harness discovers
whatever is present and skips the rest. `scripts/message-fixtures/capture.sh` prints a warning for any
payload over 1MB so the next such case is a decision rather than a surprise.
