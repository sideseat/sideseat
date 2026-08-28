# Message-parsing fixtures

Inputs for `server/src/domain/traces/message_goldens_tests.rs`, which checks that message
**count, content, ordering and absence of duplicates** hold for every framework *that has a
fixture here*, in all four views the API exposes. Coverage is 14 of the 33 frameworks SideSeat
recognises and not every fixture has a session view - see [What is and is not
covered](#what-is-and-is-not-covered), which is the honest version of this sentence:

| View    | Row set                                                | API endpoint                     |
| ------- | ------------------------------------------------------ | -------------------------------- |
| span    | `WHERE span_id = ?`, no content filter                 | `/spans/{trace}/{span}/messages` |
| trace   | whole session when the trace has one, then scoped back | `/traces/{id}/messages`          |
| session | every row of every trace in the session                | `/sessions/{id}/messages`        |
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
<suite>/<sample>/req-001.pb        captured OTLP payload (protobuf, or .json)
<suite>/<sample>/req-002.pb        one file per exported batch, in capture order
<suite>/<sample>/expected.json     committed expectation
```

The fixture is the **raw OTLP payload the framework actually sent**, not database rows. That
is the only input the server really receives, so a fixture cannot drift from reality. The test
replays it through the real ingestion path (`extract_attributes_batch`,
`extract_messages_batch`, SideML conversion, enrichment) before comparing.

## Capturing a suite

Needs working model credentials, since the samples call a real model.

```bash
misc/capture-message-fixtures.sh                    # every suite
misc/capture-message-fixtures.sh strands            # one suite
misc/capture-message-fixtures.sh strands tool_use   # one sample
```

Then record the expectations, **read them**, and only then let them gate:

```bash
UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens   # write expectations
misc/review-message-goldens.py                                   # read them: counts, roles, content
misc/review-message-goldens.py --suspicious                      # only fixtures with warnings
misc/review-message-goldens.py strands/tool_use                  # one sample, full detail
git diff server/tests/fixtures/messages
cargo test -p sideseat-server message_goldens                    # from now on it gates
```

`review-message-goldens.py` exists because `git diff` on this much JSON is unreadable. It
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
- a session's trace views partition its session view exactly — summing them must equal it. Not
  asserted for a session that shares a trace with another session (ADK emits its own session id
  alongside the sample's, so one trace belongs to two); the test says so per fixture rather than
  passing quietly
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

`UPDATE_GOLDENS=1` reports invariant violations instead of aborting, so one bad fixture does
not hide the rest.

## What is and is not covered

14 suites, 107 samples. Every framework SideSeat *recognises* is not covered, and the gap is
deliberate rather than hidden:

| Covered by fixtures | strands, langgraph, crewai, adk, bedrock, openai, openai-agents, anthropic, agent-framework, claude-agent-sdk, and the strands / vercel-ai / claude-agent-sdk JS suites |
| ------------------- | --- |
| Has samples, no fixtures | `autogen` — its runner has no Bedrock path, so capturing it needs a first-party key. Listed in the capture script and skipped with a message, so its absence is visible. |
| Recognised, no samples | LangChain, PydanticAI, Agno, Smolagents, AgentScope, Langflow, AG2, Haystack, browser-use, Google GenAI, Vertex AI, LlamaIndex, Semantic Kernel, Azure OpenAI / AI Foundry, Logfire, MLflow, TraceLoop, LiveKit |

The second group shares extractors with covered frameworks, so the *parsing logic* is exercised
— but nothing here proves their emitted payloads match what those extractors expect. Adding a
sample suite is what closes that, not adding an expectation file.

Also uneven: 30 fixtures have no session view, because their sample never sets a session id.
Session views are built only for real session ids, since the endpoint cannot be asked for a
session that does not exist. Sessionised captures are what would cover those, not a synthetic
fallback.

## Capability exemptions

`PAIRING_EXEMPT` in the test names fixtures whose *source* telemetry cannot satisfy tool
pairing, with the reason. Both `claude-agent-sdk*/subagents` are listed: the Claude Code CLI
emits a subagent's tool executions without the matching `tool_use` block, so the result is
callless upstream. A capability limit of a framework is recorded per fixture rather than
weakening the check for everyone.

## `_synthetic/`

Hand-written, not captured: a Strands-shaped tool-use conversation used to exercise the
harness itself where no captured fixture is available. Its event shapes were taken from the
assertions in `server/src/domain/traces/extract/messages_tests.rs` rather than invented — an
unrealistic fixture would produce confident but meaningless results.

Real captures are preferred for every framework. Keep this one: it is the only fixture that
survives a checkout with no credentials, so it keeps the harness itself under test.

## Not committed

`crewai/agent_core` is gitignored: CrewAI serialises its entire model config into a span
attribute, so the captured payload contained a live `aws_secret_access_key` and
`aws_session_token`. A secret in a fixture goes straight into git history, where it cannot be
taken back — `capture-message-fixtures.sh` now discards any fixture whose payload matches that
shape rather than leaving the decision to a later reader.

`strands-js/image-gen` and `vercel-ai-js/image-gen` are gitignored. Those suites inline
generated images as base64 in the OTLP JSON — 7MB and 15MB for a single request — which would
sit in git history permanently for no extra parsing coverage. The Python `image_gen` fixtures
exercise the same path in under 100KB each, because media is rewritten to file URIs before
storage. Capture the JS ones locally when working on image handling; the harness discovers
whatever is present and skips the rest. `capture-message-fixtures.sh` prints a warning for any
payload over 1MB so the next such case is a decision rather than a surprise.
