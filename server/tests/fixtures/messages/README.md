# Message-parsing fixtures

Inputs for `server/src/domain/traces/message_goldens_tests.rs`, which checks that message
**count, content, ordering and absence of duplicates** are correct for every framework, in all
three views the API exposes:

| View    | Row set                                                     | API endpoint                     |
| ------- | ----------------------------------------------------------- | -------------------------------- |
| span    | `WHERE span_id = ?`, no content filter                      | `/spans/{trace}/{span}/messages` |
| trace   | whole session when the trace has one, then scoped back       | `/traces/{id}/messages`          |
| session | every row of every trace in the session                      | `/sessions/{id}/messages`        |

All three call `process_spans`. `process_feed` is the **project feed** endpoint and sorts DESC;
using it for the session view tested ordering no session request can return. Trace and session
row sets apply `MESSAGE_CONTENT_FILTER` and `ORDER BY timestamp_start ASC`, exactly as the
queries do — feeding unfiltered rows made whole sessions come back empty.

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

Then record the expectations and **read the diff before committing**:

```bash
UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens
git diff server/tests/fixtures/messages
```

Recording is a separate, explicit step on purpose: a golden written straight from current
output enshrines whatever bugs exist today. A regenerated file has to be reviewed. The
invariant checks in the test exist precisely because they hold regardless of what the golden
says — they catch bugs a blind snapshot would bless:

- indices dense and ascending
- roles limited to system/user/assistant/tool (an unmapped role silently becomes `user`)
- no duplicate (role, kind, content) within one view
- every returned block belongs to the scope requested (a span view never leaks a sibling span)
- every tool result's id matches a call in the same trace (trace and session views)
- no empty text or thinking blocks
- a trace whose spans carry messages is never itself empty
- processing the same fixture twice gives the same answer

`UPDATE_GOLDENS=1` reports invariant violations instead of aborting, so one bad fixture does
not hide the rest.

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
