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

## Layout

```
<producer>/<mode>/<scenario>/req-001.pb     captured OTLP payload (protobuf, or .json)
<producer>/<mode>/<scenario>/req-002.pb     one file per exported batch, in capture order
<producer>/<mode>/<scenario>/expected.json  committed expectation
_synthetic/<sample>/                        hand-written shapes no producer emits on its own
```

A producer is a framework or provider (`strands`, `openai-agents`, `bedrock`) or an SDK conformance
program (`python`, `javascript`, `dotnet`, `rust`). The mode says who configured the telemetry:

| Mode | Telemetry configured by |
| --- | --- |
| `native` | the framework's own documented OpenTelemetry setup, or plain OpenTelemetry for a conformance program |
| `sdk` | the SideSeat SDK, with the same scenario code |
| `legacy` | an older capture with no native/SDK pair; removed as each producer is recaptured |

A scenario captured in both `native` and `sdk` is a parity pair, and the goldens require the two to
produce the same conversations.

The fixture is the **raw OTLP payload the framework actually sent**, not database rows. That
is the only input the server really receives, so a fixture cannot drift from reality. The test
replays it through the real ingestion path (`extract_attributes_batch`,
`extract_messages_batch`, SideML conversion, enrichment) before comparing.

## Support matrix

The boundary of "correct for all frameworks": exactly the suites below, at the versions they were captured
against. `the_corpus_matches_the_support_matrix` fails if a suite is added or removed without updating this
table - so the claim stays checked rather than described.

It lives here, beside the fixtures it describes, and not in `CLAUDE.md`. That file is tracked, but project
convention keeps it out of routine commits, so its committed content lags the working copy by however much has
been written since - a test reading it would compare the corpus against whatever state a given checkout
happens to carry, which passes or fails on how recently someone committed a document rather than on whether
the corpus matches it.

| Suite | Version captured against | Samples | Captured requests |
| --- | --- | --- | --- |
| `_synthetic` | hand-written shapes, no SDK | 17 | 17 |
| `adk/legacy` | google-adk >=1.27.0 | 8 | 18 |
| `adk/native` | google-adk >=1.27.0, native OTLP setup | 10 | 11 |
| `adk/sdk` | SideSeat Python 1.0.8 / google-adk >=1.27.0 | 10 | 11 |
| `ag2/native` | AG2 1.1.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, native OTLP setup | 1 | 1 |
| `ag2/sdk` | SideSeat Python 1.0.8 / AG2 1.1.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 1 | 1 |
| `agent-framework/legacy` | agent-framework-core >=1.0.0b0 | 10 | 17 |
| `agent-framework/native` | agent-framework-core >=1.0.0b0, native OTLP setup | 10 | 10 |
| `agent-framework/sdk` | SideSeat Python 1.0.8 / agent-framework-core >=1.0.0b0 | 10 | 10 |
| `agentscope/native` | AgentScope 2.0.9 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, native `TracingMiddleware` | 1 | 1 |
| `agentscope/sdk` | SideSeat Python 1.0.8 / AgentScope 2.0.9 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 1 | 1 |
| `agno/native` | Agno 3.0.11 / OpenAI 3.22.1 / OpenInference Agno instrumentor 1.0.12 / OpenTelemetry Python 1.45.0 on CPython 3.13.7, native OTLP setup | 1 | 1 |
| `agno/sdk` | SideSeat Python 1.0.8 / Agno 3.0.11 / OpenAI 3.22.1 / OpenInference Agno instrumentor 1.0.12 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `anthropic/legacy` | anthropic >=0.84.0 | 7 | 18 |
| `anthropic/native` | Anthropic 1.8.0 / Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.13.7 | 1 | 1 |
| `anthropic/sdk` | SideSeat Python 1.0.8 / Anthropic 1.8.0 / Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.13.7 | 1 | 1 |
| `autogen/native` | AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenInference AutoGen instrumentor 0.1.18 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `autogen/sdk` | SideSeat Python 1.0.8 / AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenInference AutoGen instrumentor 0.1.18 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `azure-openai/native` | OpenAI 3.22.1 / OpenInference OpenAI instrumentor 0.1.62 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup | 1 | 1 |
| `azure-openai/sdk` | SideSeat Python 1.0.8 / OpenAI 3.22.1 / OpenInference OpenAI instrumentor 0.1.62 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `bedrock/legacy` | boto3 (bedrock runtime) | 6 | 14 |
| `claude-agent-sdk/legacy` | claude-agent-sdk >=0.2.0 | 8 | 17 |
| `claude-agent-sdk-js/legacy` | @anthropic-ai/claude-agent-sdk ^0.3.246 | 8 | 17 |
| `crewai/legacy` | crewai >=1.10.1 | 9 | 33 |
| `dotnet/native` | OpenTelemetry .NET 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `dotnet/sdk` | SideSeat .NET 1.0.0 / OpenTelemetry 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `google-genai/native` | Google GenAI 2.26.0 / Logfire 6.0.0b7 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.12.8 | 1 | 1 |
| `google-genai/sdk` | SideSeat Python 1.0.8 / Google GenAI 2.26.0 / Logfire 6.0.0b7 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.12.8 | 1 | 1 |
| `haystack/native` | Haystack 3.3.0 / opentelemetry-haystack 1.0.0 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0, native OTLP setup | 1 | 1 |
| `haystack/sdk` | SideSeat Python 1.0.8 / Haystack 3.3.0 / opentelemetry-haystack 1.0.0 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 | 1 | 1 |
| `javascript/native` | OpenTelemetry JS 2.11.0 / OTLP exporter 0.222.0 on Node.js 26.9.0 | 1 | 1 |
| `javascript/sdk` | SideSeat JavaScript 2.0.0 / OpenTelemetry JS 2.11.0 on Node.js 26.9.0 | 1 | 1 |
| `langchain/native` | LangChain 1.4.3 / LangChain Core 1.6.6 / LangChain OpenAI 1.6.7 / OpenInference LangChain instrumentor 0.1.76 / OpenTelemetry Python 1.45.0 on CPython 3.13.7, native OTLP setup | 1 | 1 |
| `langchain/sdk` | SideSeat Python 1.0.8 / LangChain 1.4.3 / LangChain Core 1.6.6 / LangChain OpenAI 1.6.7 / OpenInference LangChain instrumentor 0.1.76 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `langgraph/native` | LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup | 11 | 16 |
| `langgraph/sdk` | SideSeat Python 2.0.0 / LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `llama-index/native` | LlamaIndex Core 0.14.25 / LlamaIndex OpenAI 0.8.2 / OpenAI 2.54.0 (latest allowed by the adapter's `<3` constraint) / OpenInference LlamaIndex instrumentor 4.5.3 / OpenTelemetry Python 1.45.0 on CPython 3.13.7, native OTLP setup | 1 | 1 |
| `llama-index/sdk` | SideSeat Python 1.0.8 / LlamaIndex Core 0.14.25 / LlamaIndex OpenAI 0.8.2 / OpenAI 2.54.0 (latest allowed by the adapter's `<3` constraint) / OpenInference LlamaIndex instrumentor 4.5.3 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `logfire/native` | Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.12.8, native Logfire setup | 1 | 1 |
| `logfire/sdk` | SideSeat Python 1.0.8 / Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.12.8 | 1 | 1 |
| `openai/legacy` | openai >=1.80.0 | 6 | 8 |
| `openai-agents/legacy` | openai-agents >=0.12.1 | 10 | 37 |
| `openai/native` | OpenAI 3.19.2 / Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.13.7 | 1 | 1 |
| `openai/sdk` | SideSeat Python 1.0.8 / OpenAI 3.19.2 / Logfire 6.0.0b7 / OpenTelemetry Python 1.44.0 on CPython 3.13.7 | 1 | 1 |
| `openinference/native` | OpenInference instrumentation 0.1.67 / semantic conventions 0.1.40 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup | 1 | 1 |
| `openinference/sdk` | SideSeat Python 1.0.8 / OpenInference instrumentation 0.1.67 / semantic conventions 0.1.40 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `pydantic-ai/native` | Pydantic AI 2.53.0 / OpenTelemetry Python 1.44.0 on CPython 3.14.7, `Agent.instrument_all()` on a plain provider | 11 | 17 |
| `pydantic-ai/sdk` | SideSeat Python 2.0.0 / Pydantic AI 2.53.0 / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 11 | 11 |
| `python/native` | OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `python/sdk` | SideSeat Python 2.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `rust/native` | OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `rust/sdk` | SideSeat Rust 0.2.0 / OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `semantic-kernel/native` | Semantic Kernel 1.44.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0, native OTLP setup | 1 | 1 |
| `semantic-kernel/sdk` | SideSeat Python 1.0.8 / Semantic Kernel 1.44.1 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 | 1 | 1 |
| `smolagents/native` | Smolagents 1.26.0 / OpenAI 3.22.1 / OpenInference Smolagents instrumentor 0.1.41 / OpenTelemetry Python 1.45.0 on CPython 3.13.7, native OTLP setup | 1 | 1 |
| `smolagents/sdk` | SideSeat Python 1.0.8 / Smolagents 1.26.0 / OpenAI 3.22.1 / OpenInference Smolagents instrumentor 0.1.41 / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 1 | 1 |
| `strands-js/legacy` | @strands-agents/sdk ^1.14.0 | 7 | 12 |
| `strands/native` | Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, `StrandsTelemetry` | 11 | 14 |
| `strands/sdk` | SideSeat Python 2.0.0 / Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `traceloop/native` | TraceLoop SDK 0.62.4 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, native OTLP setup | 1 | 1 |
| `traceloop/sdk` | SideSeat Python 1.0.8 / TraceLoop SDK 0.62.4 / OpenAI 3.22.1 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 1 | 1 |
| `vercel-ai-js/legacy` | ai ^7.0.79 | 6 | 13 |
| `vertex-ai/native` | Google GenAI 2.26.0 / Logfire 6.0.0b7 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.12.8, native Logfire setup with the current `enterprise=True` Vertex mode | 1 | 1 |
| `vertex-ai/sdk` | SideSeat Python 1.0.8 / Google GenAI 2.26.0 / Logfire 6.0.0b7 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.12.8, current `enterprise=True` Vertex mode | 1 | 1 |
| **64 suites** | | **250** | **385** |

Two further samples exist but are **not in the repository**: `strands-js/image-gen` and
`vercel-ai-js/image-gen`, whose payloads are 15 MB and 7 MB of inlined base64 image data (the Python
`image_gen` fixtures cover the same path in under 100 KB, because media is rewritten to file URIs). They are
gitignored and captured locally when working on image handling, so the counts above are what a checkout has.
`local_only_samples_are_actually_gitignored` stops that exemption from excusing a sample somebody merely
forgot to commit.

## Production message rubric

Each captured sample receives one binary score for every applicable criterion below. A sample is
100% only when every criterion passes; averaging cannot hide a missing message behind unrelated
passing checks. Any upstream capability exemption is named per fixture with a reason.

| Criterion | Passing evidence |
| --- | --- |
| Wire fidelity | Fixture is the exact OTLP request emitted by the pinned SDK/framework version |
| Framework identity | Every native framework suite reaches its declared framework label without SDK fallback |
| Span view | Every span returns only its own messages, in source order |
| Trace view | Full conversation is complete, ordered, and scoped to one trace |
| Session view | Traces partition one session exactly; no trace belongs to two sessions |
| Project feed | Same messages appear once in documented feed order |
| Content | Full canonical content digests match, not only previews or counts |
| Tool causality | Every identified result follows one matching call and is answered once |
| No duplicates | Re-sent history, redundant carriers, and repeated delivery add no copy |
| Determinism | Re-run, reverse arrival order, and cache hit produce identical output |
| SDK parity | Language SDK/OTel and framework SDK/native pairs match requests, span projections, trace topology, sessions, and feed |

The rubric is enforced by `message_goldens`, its invariant tests,
`sdk_and_plain_otel_conformance_are_identical`, and
`framework_sdk_and_native_conversations_are_identical`. A support-matrix row is not considered SDK
parity coverage until both paired suites are committed.

Logfire 6 emits a successful streaming request as an input-only span and the completed response
as a separate log. Its default context makes that log an unrelated root trace, which the server cannot
safely reconnect after ingestion. Both the native control pipeline and SideSeat install the streaming
reparenter before their OTLP exporter. Framework parity therefore requires the same request count, span
count, complete trace topology, session content, and project feed.

The credential-free SDK pairs are reproduced with:

```bash
make capture-sdk-conformance-dotnet
make capture-sdk-conformance-javascript
make capture-sdk-conformance-python
make capture-sdk-conformance-rust
```

## Capturing a suite

Needs working model credentials, since the samples call a real model.

```bash
scripts/message-fixtures/capture.sh                         # every suite, native + SDK
scripts/message-fixtures/capture.sh strands                 # one suite, native + SDK
scripts/message-fixtures/capture.sh strands tool_use native # one native sample
scripts/message-fixtures/capture.sh strands tool_use sdk    # the matching SDK sample
```

The latest direct OpenAI, Azure OpenAI, Anthropic, Google GenAI, Pydantic AI, AutoGen, AG2,
AgentScope, LangChain, Agno, Semantic Kernel, Smolagents, LlamaIndex, TraceLoop, and generic
Logfire pairs are credential-free and deterministic:

```bash
scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh openai chat_completions both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell; the sample maps its Azure-shaped hostname to this local fixture.
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh azure-openai canonical both

scripts/message-fixtures/fake-anthropic.py --port 5402
# In another shell:
ANTHROPIC_API_KEY=x \
ANTHROPIC_BASE_URL=http://127.0.0.1:5402 \
CAPTURE_MODEL=claude-sonnet-4-6 \
  scripts/message-fixtures/capture.sh anthropic messages both

make capture P=pydantic-ai

scripts/message-fixtures/capture.sh logfire canonical both

scripts/message-fixtures/fake-google-genai.py --port 5404
# In another shell:
GOOGLE_GENAI_BASE_URL=http://127.0.0.1:5404 \
  scripts/message-fixtures/capture.sh google-genai generate_content both

scripts/message-fixtures/fake-google-genai.py --port 5404
# In another shell:
VERTEX_AI_BASE_URL=http://127.0.0.1:5404 \
  scripts/message-fixtures/capture.sh vertex-ai canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
AUTOGEN_OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
AUTOGEN_API_KEY=x \
CAPTURE_MODEL=gpt-4o-mini \
  scripts/message-fixtures/capture.sh autogen agent both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh ag2 canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh langchain canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh haystack canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh agno canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh semantic-kernel canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh smolagents canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh llama-index canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh agentscope canonical both

scripts/message-fixtures/fake-openai.py --port 5401
# In another shell:
OPENAI_API_KEY=x \
OPENAI_BASE_URL=http://127.0.0.1:5401/v1 \
CAPTURE_MODEL=gpt-5-nano-2025-08-07 \
  scripts/message-fixtures/capture.sh traceloop canonical both
```

`CAPTURE_MODEL` is validated before being appended to the sample command. The fake endpoints cover
ordinary completion, SSE streaming, and a two-call tool roundtrip; they are wire-contract fixture servers,
not model-quality substitutes. Pydantic AI uses its deterministic in-process `FunctionModel`, so it needs
neither a fake HTTP endpoint nor provider credentials. The Google endpoint exercises ordinary generation,
SSE streaming, and automatic Python function calling against the real Google GenAI client and instrumentor
in both Gemini Developer API and Vertex AI modes.

New captures use `<suite>-native/<sample>` and `<suite>-sdk/<sample>` so the support
matrix can prove framework-level parity instead of mixing instrumentation modes under one
name. The historical unsuffixed corpus remains immutable evidence for the versions listed
above; pass `legacy` explicitly only when reproducing one of those old captures.

Then record the expectations, **read them**, and only then let them gate:

```bash
UPDATE_GOLDENS=1 cargo test --locked -p sideseat-server --test message_goldens   # write expectations
scripts/message-fixtures/review-goldens.py                                   # read them: counts, roles, content
scripts/message-fixtures/review-goldens.py --suspicious                      # only fixtures with warnings
scripts/message-fixtures/review-goldens.py strands/tool_use                  # one sample, full detail
git diff server/tests/fixtures/messages
cargo test --locked -p sideseat-server --test message_goldens           # from now on it gates
```

`review-goldens.py` exists because `git diff` on this much JSON is unreadable. It
renders each view's message count, role sequence and content, and flags patterns that usually
mean a parsing defect (a conversation with no assistant message, unbalanced tool calls, raw
JSON in a text position). Those are heuristics for a human to judge — the hard guarantees are
in the test.

Recording is a separate, explicit step on purpose: a golden written straight from current
output enshrines whatever bugs exist today. `UPDATE_GOLDENS=1` writes the files but still exits
non-zero if an invariant was violated, so known-bad output cannot be committed as reviewed.

The invariants hold regardless of what a golden says, which is what makes a blindly regenerated
snapshot still fail on a real defect:

- every returned block belongs to the scope requested, by exact id (a span view never leaks a
  sibling span; a trace view never survives `scope_feed_to_trace` with another trace's block)
- every native framework suite produces the framework label its SDK slug declares somewhere in
  that suite, without relying on `sideseat.framework` to fill the gap. Nested producers keep their
  own labels, so this is suite-level evidence rather than a demand that every child span have one name
- a session's trace views partition its session view exactly — summing them must equal it. This is
  now asserted for **every** session, and the reason it once could not be is worth keeping: ADK emits
  its own session id alongside the sample's, so a trace named two sessions and appeared under both,
  which made the partition meaningless for exactly the fixtures where it mattered. A trace belongs to
  the session on its earliest span, so that cannot happen — and the check that a session claims no
  foreign trace is an assertion rather than a skip
- no duplicate (role, kind, full-content digest) within one trace. This is also a deliberate
  product limit: a genuine repeat of the same tool call or message inside one trace is collapsed,
  because it is indistinguishable from a history re-send — see the pipeline notes in
  `sideml/feed/mod.rs`
- every tool result's id matches a call in the same trace, and a call is never answered twice.
  Results with no id are outside this check: a result whose framework identifies it only by name
  is linked to its call by position (oldest unclaimed call of that name), and where no call is
  available it stays unlinked rather than acquiring an invented id
- no empty text or thinking blocks
- a view holding a user message also holds something from the assistant or a tool. Every other
  invariant here is about not returning the *wrong* thing; this is the only one that notices
  content which never arrives at all, which is how CrewAI's answers went missing for as long as
  they did — the extractor read the reply from a field it only consulted when no history was
  present, so exactly the runs that had a conversation lost the response. A fixture that
  legitimately has no answer is exempted by name with its reason (only `strands/error`, whose
  sample exists to fail), so the exemption is a claim someone made rather than a silent pass
- the projection is self-consistent (counts, role sequence and message list agree)
- all of the above hold for the **project feed** view as well, which has its own pipeline entry
  point (`process_feed`) and its own ordering - newest response first, each response read
  top-to-bottom. It was the one view outside the harness, and so the only place a duplicate could
  surface unchecked. The answer check is the weaker "something answered a question" there: no
  position in a feed is "the last turn", since it descends across responses and ascends within one
- processing the same fixture twice gives the same answer, checked once per suite
- **redundant evidence changes nothing**: re-delivering every span of a fixture yields the same
  messages in the same order, not merely the same count
- **arrival order decides nothing**: reversing the order spans reach the pipeline yields the same
  answer, so an extraction change that merely shifts arrival order cannot look like a content change
- **a matched tool result follows its call** - causality, not adjacency, since Vercel emits
  `call, call, result, result`. Not applied to the project feed, which descends across responses by
  design: there a call and an earlier response's result are legitimately reversed
- **survivors of one carrier keep the order that carrier stated**, compared only where the carrier
  *has* an order - two entries of one array. Paths that diverge at an object member are not compared,
  because members have no order: Anthropic puts its system prompt in a sibling of `messages` and the
  pipeline rightly renders it first

`UPDATE_GOLDENS=1` reports invariant violations instead of aborting, so one bad fixture does
not hide the rest.

## What is and is not covered

**252 tracked expectation files: 235 captured in 63 suites, plus 17 synthetic.** A suite is not a framework:
`strands`/`strands-js` and `claude-agent-sdk`/`claude-agent-sdk-js` are one framework each in two
languages; the eight .NET/JavaScript/Python/Rust suites are SDK conformance rather than framework
captures. The fixture families below cover **27 of the 32** frameworks SideSeat recognises. (32 is
the union of the server's `Framework` classifier and the SDK's framework list, excluding `Unknown`:
28 named server variants plus `anthropic`, `openai`, `google-genai` and `pydantic-ai`, which only the
SDK names.) Every framework is not covered, and the gap is deliberate rather than hidden:

| Covered by fixtures (27) | strands, langchain, langgraph, llama-index, crewai, google-adk, google-genai, vertex-ai, haystack, bedrock, openai, azure-openai, openai-agents, openinference, anthropic, pydantic-ai, autogen, ag2, agent-framework, agentscope, claude-agent-sdk, agno, semantic-kernel, smolagents, vercel-ai, logfire, traceloop — strands and claude-agent-sdk in both languages, vercel-ai in JS only |
| ------------------- | --- |
| Synthetic, not a framework | `_synthetic/*` — hand-written payloads for shapes no captured sample produces, counted in the file total and in neither the suites nor the frameworks. See below. |
| SDK conformance, not a framework | The `dotnet-{otel,sdk}/canonical`, `javascript-{otel,sdk}/canonical`, `python-{otel,sdk}/canonical`, and `rust-{otel,sdk}/canonical` pairs — the same real four-span, five-message conversation exported without and with each SideSeat SDK |
| Recognised, no fixtures (5) | azure-ai-foundry, browser-use, langflow, livekit, mlflow |

The second group shares extractors with covered frameworks, so the *parsing logic* is exercised
— but nothing here proves their emitted payloads match what those extractors expect. Adding a
sample suite is what closes that, not adding an expectation file.

Also uneven: 28 captured fixtures have no session view, because their sample never sets a session id.
Session views are built only for real session ids, since the endpoint cannot be asked for a
session that does not exist. Sessionised captures are what would cover those, not a synthetic
fallback.

## The synthetic fixtures

Hand-written payloads for shapes the captured corpus does not reach. They exist because an invariant
that holds trivially proves nothing: each of these makes a specific rule bite, and the mutation that
breaks that rule is named.

| Fixture | The shape | What it makes bite |
| --- | --- | --- |
| `tool_use/legacy` | a Strands call/result pair | the baseline hand-written case |
| `multi_turn_one_carrier/legacy` | nine turns in **one** carrier, in conversation order | carrier subsequence across many siblings - the ADK shape, where one span holds a whole conversation |
| `parallel_tool_calls/legacy` | two distinct calls in one response, then both results | causality *without* adjacency: `call, call, result, result` must be allowed |
| `resent_history/legacy` | a later span re-sending the earlier turn | the re-send collapses onto the original rather than duplicating it |
| `cross_span_tie/legacy` | a generation span and its tool span reporting the **identical** instant, with the tool span's id sorting *first* | `adopt_call_positions`. Disable it and this fixture reports the answer at index 1 before its question at index 3; every captured fixture stays green, because none of them ties |
| `agent_snapshot_reorders_answer/legacy` | a root agent span re-listing a whole turn **answer-first** while its child generation spans emit the calls and the answer separately — the shape the Vercel AI SDK's current integration produces | The **redundant re-listing** rule (`redundant_relistings`, `order_graph.rs`). Its golden records the correct conversation — question, calls, results, answer — and did not until that rule landed: `gen_ai.output.messages` reads as one atomic emission wherever it appears, so the re-listing's stated order was trusted and the answer sorted ahead of the calls that produced it. Two earlier attempts are recorded in `a_relisting_is_discounted_only_on_evidence_from_below_it`: declaring the carrier `accumulated_state` **lost a message** in `agent-framework/tool_use`, and discounting any instance whose messages appear below it fired 4,044 times across the corpus and broke `agent-framework/swarm` and `strands/image_gen`. The rule that works asks two questions instead — is every message witnessed by a *descendant*, and does the instance hold **both** a message the span produced and a result answering it, which no single model response can |

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
