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

## The rubric (v2)

Every reconstruction is held to two independent references. The first is the committed
`expected.json` with the invariants of `message_goldens` (no duplicates within a trace, tool calls
before and paired with their results, scope, session partition, an answer for every run, identical
native and SDK conversations). The second, since rubric v2, is the parser-independent **truth** under
[`../truth/`](../truth): one `sideseat.truth/3` document per producer and scenario, derived by
`python -m harness truth` from the recorded model responses and the scenario scripts, never from a
reconstruction. `expected.json` keeps a 240-character preview of each block; the truth checks read the
full content.

The truth comparison runs inside `message_goldens`, on the four views it already built
(`server/tests/message_truth/`), and requires:

1. **The truth is sound.** Ids are unique and every reference resolves; outputs belong to their call
   and conversation; every fact is sequenced once; an unasserted fact has a gap naming it; the source's
   SHA-256 matches the file; every fixture has a truth or a stated reason, and no truth names a missing
   fixture; a requirement names every view its anchor reaches; every prompt, result and pair of
   consecutive calls has its edge; gaps use a closed vocabulary and a per-fact gap names its fact
   (`truth_documents_are_internally_consistent`).
2. **Each call has exactly one span.** Calls are matched to spans injectively, by response id first,
   else by the spans whose output shows the call's asserted output - typed generation spans before
   others, the innermost before an enclosing re-listing. Two equal candidates are a violation, never a
   "closest" pick; a call whose output the truth cannot know takes an unclaimed generation span by
   count and order; an unclaimed generation span that speaks is unexpected; a failed attempt's span
   must say nothing. The matched span's model (or `provider/model`), response model, response id,
   finish (normalised on both sides) and every usage count the wire states must agree; a value the span
   does not state is a violation only when the raw span carries it.
3. **Assistant text is exact** against the full block: `exact` byte for byte, `json` by parsed value,
   a segmented answer as one block or as its consecutive segments.
4. **Tool calls** carry the wire's id, name (a framework namespace such as `travel-` or `mcp__x__` is
   accepted) and JSON-equal arguments. An id the framework rewrote consistently is its own violation,
   `tool_call.id_rewritten`, and the result then pairs by the rewritten id. A call the framework
   executes on the model's behalf - an action listed inside a plan call (`[tool.sideseat-example.truth]`
   in the suite manifest says which) - never had a wire id: its fact's id is `null`, it is shown under
   the id the framework assigns, and its result pairs by that id. A block (or span) showing a call
   under its own id is always preferred to one showing it under another.
5. **Tool results** pair by call id, one per executed call, and carry the deterministic tool's value:
   JSON equality with numbers by value, through a JSON string, `[{type: text, text}]` parts, one
   `{type: json, data}` part, or a single-member `result`/`error`/`content`/`output` envelope. A Python
   literal is a rendering defect. An error result contains the tool's message; a success may not be
   flagged as an error.
6. **Reasoning** the wire returned visibly is a thinking block with its exact text and is never shown as
   assistant text. Signed reasoning with no text is a thinking block with empty text marked `signed`, never
   `redacted_thinking`, which is a provider's own redaction, and no view carries the signature itself. It is
   owed by presence, role and place, in every view and on the span sent it. Each such fact carries a `seal`,
   its signature's SHA-256, which is what an absence proof searches for. What a framework leaves out is
   declared per scenario and proven: the step (`withheld_reasoning`), its signature everywhere
   (`reasoning_signature`, shown unsigned) or on its producing span (`reasoning_span_signature`), or the step
   on its producing span only, a later request re-sending it (`reasoning_output`).
7. **Prompts, system prompt and attachments** are present: a prompt contained in (or equal to) a user
   text block; an echoed system prompt exactly; an attachment by modality, media type and the SHA-256 of
   its decoded bytes wherever the bytes are kept inline (long data that does not decode is damaged
   bytes, not a placeholder).
8. **Order** follows the truth: a response's parts in wire order (also in the feed), a prompt before its
   response, a call before its result, responses in call order (reversed in the feed, which is newest
   response first), a call's inputs between the previous response and its own, sessions' conversations in
   order, and in trace and session views the whole conversation sequence, where only a run of inputs
   between two responses (parallel results, a prompt and its attachments) may permute.
9. **Placement.** Facts are assigned to blocks by maximum bipartite matching per view, so two identical
   facts need two blocks; candidates are tried oldest first in every view (the feed is read in reverse),
   and identical facts take their blocks in the truth's order. A model call's facts must appear exactly once in its span's output, its
   trace, its session and the feed; a conversation's facts exactly once in the trace of their call, its
   session and the feed. A second identical block is a duplicate, one in another trace a leak; the set of
   blocks showing a fact is the same in every view; every message sits on the span that recorded its
   call (a prompt on that span or one enclosing it), and a later call's span does not start first.
10. **Nothing unexplained.** A trace or session view holds the truth's conversation and nothing else:
    every block no fact claims must be accounted for by a gap of the truth (an unrecorded system prompt,
    an unknowable answer or tool result, reasoning without text, multi-agent routing), one block per gap
    and only in the traces of the calls the gap concerns.

Two properties are checked over every fixture in `make test`
(`no_delivery_or_framework_release_changes_a_conversation`): the views do not change when the rows
arrive reversed, when every span is delivered twice, when every span is exported in a request of its
own, or when every timestamp is offset uniformly; and a scenario captured on another release of its
framework (`<producer>/<mode>@<version>/<scenario>`, replayed from the same recorded responses) holds
the same conversation as the current release's.

What the truth cannot know is not asserted, and says so: which agent made a call and what one agent
passed another (`multi_agent_routing`), a system prompt the cassette never recorded
(`request_body_unrecorded`), the framework's own per-step state message that restates the prompt in a
later call's request (`framework_restates_prompt`, one user text containing that turn's prompt per
such call, in its trace; an allowance nothing uses contradicts the declaration and is `gap.unused`), which span reports a tool result, and whether a streamed and an unstreamed
run of a scenario are equivalent (the truth records `streamed` per call but no scenario pair it calls
equivalent). Sub-millisecond clock jitter is not asserted either: it crosses the pipeline's stated
1 ms tie tolerance, where a changed answer is legitimate.

A fact the framework never exported - Codex's text between tool calls, a result an instrumentor
drops - is withdrawn by a `not_exported` gap only with an absence proof
(`server/tests/message_truth/absence.rs`, `every_absence_gap_is_proven`): every captured trace request
and log export of every fixture the truth describes is decoded (JSON in strings, Python renderings,
base64, data URIs, backslash escapes) and searched for the fact's content, and the gap is refused when
its witness is found (a call's id with its arguments or name, a result's value with its call id, a
text or a long fragment of it, an attachment's digest), when only part of it is found, or when the
content is too weak to search for. Two narrower absences are proven the same way: `id_not_exported`,
a tool call's wire id that no payload carries, where no payload gives the call another id either (the
call is still asserted, under whatever id it is shown), and `call_not_exported`, a model call whose
response no payload carries as a unit - every part a tool call whose id is absent, no message (an
object stating a role) holding any of its calls' arguments, no carrier holding two of its calls - as
when automatic function calling records one span for a whole run. Such a call needs no span; its
parts are checked where the conversation shows them, still in their order (reversed in the feed,
where each is recorded on its own), but its calls need not all precede its results; and the span
recording the run may report the run's usage where its payload states that total. A gap may hold
for some capture modes only (`modes`, `native@1.0b1`), when one release exports what another omits;
it is proven against exactly those fixtures. `metadata_not_exported` covers a call's model, response
model, response id or finish that the producer states wrongly while the right value is in no payload
(a finish counts as stated only under a member naming a finish). A span's finish also agrees when it
is the provider's own word for it (Gemini's `STOP` for a function call), and a finish word anywhere
else in the span is not the span stating one. Each gap reason has declared effects (`truth::gap_effects`): an
absence withdraws its fact but never explains an extra block. The proven gaps are listed, per
framework and per capture, under "Framework limitations" in
`docs/src/content/docs/docs/reference/production-readiness.mdx`, generated by
`framework_limitations_are_documented` (`UPDATE_LIMITATIONS=1` rewrites it).

What still fails is recorded in the shrink-only ledger
[`../truth/known-violations.json`](../truth/known-violations.json): one entry per fixture, view,
assertion and subject, with a reason and the backlog item that fixes it. A violation not in the ledger
fails, and so does an entry that no longer occurs; `truth_violation_ledger_only_shrinks_against_main`
refuses entries added after the reviewed baseline; `truth_violation_ledger_is_well_formed` refuses
wildcards, duplicates, untriaged entries and entries about gaps. The goal is zero entries. To triage
one, `TRUTH_FIXTURE=<label> cargo test --locked -p sideseat-server --test message_goldens -- --ignored
--nocapture truth_explain` prints the fixture's views and violations; `UPDATE_TRUTH_LEDGER=1` rewrites
the ledger, keeping every surviving entry's triage and marking new ones `UNTRIAGED`, which the ledger
refuses until a person gives them an issue.

The rubric is itself tested: `truth_rubric_rejects_each_mutation` applies a catalogue of defects -
deleting, altering, retyping, swapping, duplicating and misattributing parts, calls, prompts,
attachments and results; rewriting, reusing and removing call ids; changing models, ids, finishes and
every usage count; leaking across traces and views; failed attempts that speak, suppressed retries,
ambiguous matches - to clean reconstructions, and requires each to add a violation, every registered
check (`ASSERTION_FAMILIES`) to be fired by some mutation, and the positive controls (encodings of a
result, parallel completion order, a failed attempt then its retry, namespaced tool names, a terminal
answer tool without a result, a system prompt the truth cannot know) to add none. And every check fires
end to end on a committed fixture: `truth_adversarial_fixtures_fire_their_checks` runs the hand-written
`_synthetic/adversarial_*` captures and the truths of `fixtures/truth-adversarial/cases.json` (a base
truth patched one way per case) through ingestion, the views and the rubric, and each case records
exactly which checks fire.

`MESSAGE_FIXTURES=tracked` restricts every test in `message_goldens` to the samples committed at `HEAD`, so a change
can be verified (and `UPDATE_GOLDENS=1` or `UPDATE_TRUTH_LEDGER=1` run) against the committed corpus while
captures still being recorded sit untracked in the same tree; a prefix in `TRUTH_FIXTURE` (`haystack/`)
makes `truth_explain` print the violations of every fixture under it.

## The request truth (rubric v3, slice 1)

A truth describes the conversation that every fixture of a scenario shares. What the framework *sent*
differs per fixture, because native, SDK and old releases serialise one conversation differently. So
each fixture's run records its own `model-requests.json`, the transcript of every request the model
received:
- written by the recording proxy, whether it records or replays a cassette;
- written by the fake model servers;
- scrubbed like a cassette;
- refreshed without touching the telemetry by
  `python -m harness capture <producer> --transcript-only`.

The truth's `requests` member holds, for each fixture with a transcript, each call's decoded request.
That request is the system parts and the messages in order, and every part names its lineage:
- `new_fact` is the first time a conversation fact reaches a model;
- `replay_of` is a fact already established;
- `new` / `replay_of` an `rq-*` id is content no fact holds, such as the system instruction or a
  framework-composed turn;
- `lineage_unknown` is a part that is ambiguous, or a model-side part no output matches.

The lineage is derived from the requests and the outputs, never from the reconstruction.

Each call whose span its *output* established - a successful call the matcher tied by content - has its
request assigned to that span's input blocks. The assignment is exact, injective and ordered across
messages, through the same predicates the facts use; the parts of one message are a batch, because a
provider's parallel calls and their results may be shown in completion order. A call id the framework
reissued consistently is resolved first, so a rewrite is reported once, by `tool_call.id_rewritten`.
What is left over is a violation. A part the span does not show is first searched for in the fixture's own
payloads, by the same prover the truth gaps use, so a producer's limitation is never reported as a parsing
defect. No payload carries it: the producer does not export that part of a request, which is documented in
the generated limitations table rather than recorded as a violation. A payload does carry it and no input
shows it: the reconstruction lost it. An absence that cannot be proven fails closed into the violation, as
every absence claim does.
- `request.missing`: a payload carries the part, and no input shows it;
- `request.extra`: a block the request did not send;
- `request.duplicated`: a second copy of a sent part;
- `request.order`: a sent part shown elsewhere;
- `request.role`: a sent part shown under another role.

The call itself is found by its output, never by the input under test. A fixture without a transcript
has no request truth. Its calls are counted as unrecorded, never as passing.

## Layout

```
<producer>/<mode>/<scenario>/req-001.pb     captured OTLP payload (protobuf, or .json)
<producer>/<mode>/<scenario>/req-002.pb     one file per exported batch, in capture order
<producer>/<mode>/<scenario>/logs-001.pb    captured OTLP log export, attached to the spans it names
<producer>/<mode>/<scenario>/expected.json  committed expectation
<producer>/<mode>/<scenario>/model-requests.json  what the run sent the model (`sideseat.transcript/1`)
<producer>/<mode>@<version>[+<profile>]/<scenario>/  the same, captured from a historical release
<producer>/versions.json                    provenance of each versioned mode (generated)
_synthetic/<sample>/                        hand-written shapes no producer emits on its own
```

A producer is a framework or provider (`strands`, `openai-agents`, `bedrock`) or an SDK conformance
program (`python`, `javascript`, `dotnet`, `rust`). The mode says who configured the telemetry.

Most producers have two modes, `native` and `sdk`, and a few have a third, `logs`: the producer's own
**logging channel**, with nothing else instrumented, for a producer that reports a conversation through
structured log records rather than through span attributes. A logs-mode capture is judged by this rule:

> A logs-mode capture is compared with its native pair on the **conversation facts both channels carry**,
> not on span structure. The logging channel of a producer that logs model calls carries a call's messages
> and its response; where it carries no agent or tool spans, span counts, trace counts and tool executions
> are not comparable and are not compared.
>
> What is compared: for every fact the native mode's views show that the logs mode's channel can carry -
> the user turns, the system instruction, the assistant's text and the model's tool calls - the two
> reconstruct identically, in the same order.
>
> What the logs mode cannot carry is a **declared** `not_exported` for that mode, with the reason, and the
> absence probe proves each one against the capture's own payloads. A fact that is neither compared nor
> declared fails the suite. So "the logs channel carries less" is a statement the corpus proves, never a
> comparison that was skipped.

Until those declarations exist, a logs-mode fixture is not listed in its scenario's truth: `autogen/logs/*`
reconstructs its conversations correctly - the goldens show every turn, each tool call under the provider's
id and each result against it - and the truth's **per-call** checks cannot be satisfied by the channel,
because its records attach to the turn's span rather than to a span per model call, so several calls share
one. That is the channel's shape, not a parsing defect, and it is the declaration the rubric owes.

| Mode | Telemetry configured by |
| --- | --- |
| `_synthetic` | hand-written shapes, no SDK | 43 | 43 |
| `adk-go/native` | Google ADK for Go 1.8.0 / Google GenAI for Go 1.57.0 / OpenTelemetry Go 1.47.0 on Go 1.27.1, ADK's telemetry on the application's tracer and logger providers with message content captured, against the harness's fake Gemini server; the conversation is only in GenAI log events, each holding the model API's whole message | 11 | 11 |
| `adk-go/sdk` | The same under SideSeat's OpenTelemetry recipe for Go (`sideseat.framework` on the resource) | 11 | 11 |
| `adk-java/native` | Google ADK for Java 1.11.0 (ADK's Claude model on the Anthropic Java SDK 2.15.0's Bedrock backend, Claude Sonnet 5.5) / ADK's own tracing on the global OpenTelemetry Java 1.66.0 SDK on Temurin 25; the suite leaves the model's thinking blocks out of the responses ADK sees, because ADK's Claude model converts only text and tool use; no `streaming`, `reasoning`, `files` or `mcp_tools`, which ADK's Claude model does not support, and a failed booking is the tool's result, because ADK's function tool replaces any exception with a fixed message | 7 | 7 |
| `adk-java/native@0.9.0` | adk 0.9.0 / opentelemetry 1.66.0, released 2026-03-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk-java/native@1.3.0` | adk 1.3.0 / opentelemetry 1.66.0, released 2026-05-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk-java/native@1.5.0` | adk 1.5.0 / opentelemetry 1.66.0, released 2026-06-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk-java/sdk` | The same under SideSeat's OpenTelemetry recipe for the JVM (`sideseat.framework` on the resource) | 7 | 7 |
| `adk/native` | Google ADK 2.11.0 / LiteLLM 1.104.0 (Bedrock Converse) / OpenTelemetry Python 1.42.1 on CPython 3.14.7, ADK's own tracing and GenAI log events on global providers, as `maybe_set_otel_providers` sets them up; `call_llm`'s copy of a request leaves out inline parts, which the request's log events carry, and `transfer_to_agent`'s result reaches the next agent only as quoted context. `tool_use` and `streaming`, which the version matrix compares its releases with, are still traces alone | 11 | 13 |
| `adk/native@1.17.0` | google-adk 1.17.0 / litellm 1.104.0 / opentelemetry-sdk 1.37.0, released 2025-10-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.22.1` | google-adk 1.22.1 / litellm 1.104.0 / opentelemetry-sdk 1.37.0, released 2026-01-12; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.23.0` | google-adk 1.23.0 / litellm 1.104.0 / opentelemetry-sdk 1.37.0, released 2026-01-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.24.1` | google-adk 1.24.1 / litellm 1.104.0 / opentelemetry-sdk 1.39.1, released 2026-02-06; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.27.2` | google-adk 1.27.2 / litellm 1.104.0 / opentelemetry-sdk 1.38.0, released 2026-03-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.27.2+semconv-latest` | google-adk 1.27.2 / litellm 1.104.0 / opentelemetry-sdk 1.38.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-03-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.31.1+semconv-latest` | google-adk 1.31.1 / litellm 1.104.0 / opentelemetry-sdk 1.38.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-04-21; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.39.1` | google-adk 1.39.1 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, released 2026-08-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@1.39.1+semconv-latest` | google-adk 1.39.1 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-08-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.0.0` | google-adk 2.0.0 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, released 2026-05-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.0.0+semconv-latest` | google-adk 2.0.0 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-05-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.11.0+semconv-latest` | google-adk 2.11.0 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-10-02; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.2.0` | google-adk 2.2.0 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, released 2026-06-04; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.2.0+semconv-latest` | google-adk 2.2.0 / litellm 1.104.0 / opentelemetry-sdk 1.41.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-06-04; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.6.3` | google-adk 2.6.3 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, released 2026-08-07; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.6.3+semconv-latest` | google-adk 2.6.3 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-08-07; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.7.0` | google-adk 2.7.0 / litellm 1.104.0 / opentelemetry-sdk 1.43.0, released 2026-08-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.7.0+semconv-latest` | google-adk 2.7.0 / litellm 1.104.0 / opentelemetry-sdk 1.43.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-08-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.7.1` | google-adk 2.7.1 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, released 2026-08-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.7.1+semconv-latest` | google-adk 2.7.1 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-08-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.9.2` | google-adk 2.9.2 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, released 2026-09-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/native@2.9.2+semconv-latest` | google-adk 2.9.2 / litellm 1.104.0 / opentelemetry-sdk 1.42.1, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-09-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `adk/sdk` | SideSeat Python 2.0.0 / Google ADK 2.11.0 / LiteLLM 1.104.0 (Bedrock Converse) / OpenTelemetry Python 1.42.1 on CPython 3.14.7; the integration restores the image and PDF ADK leaves out | 11 | 12 |
| `ag2/native` | AG2 1.1.1 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native TelemetryMiddleware; AG2's telemetry records no system prompt, binary input, or reasoning, so `files` and `reasoning` carry their text only | 11 | 17 |
| `ag2/native@1.0.2` | ag2 1.0.2 / opentelemetry-sdk 1.45.0, released 2026-08-15; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `ag2/native@1.0.3` | ag2 1.0.3 / opentelemetry-sdk 1.45.0, released 2026-08-28; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `ag2/native@1.0.4` | ag2 1.0.4 / opentelemetry-sdk 1.45.0, released 2026-09-07; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `ag2/sdk` | SideSeat Python 2.0.0 / AG2 1.1.1 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `agent-framework/native` | Agent Framework 1.19.0 (core) / agent-framework-anthropic 1.0.0b260918 / Anthropic 0.116.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, `enable_sensitive_telemetry()` on a plain provider; no `files` (neither Bedrock client sends documents) | 10 | 13 |
| `agent-framework/native@1.0.1` | agent-framework-core 1.0.1 / agent-framework-anthropic 1.0.0b260409 / agent-framework-orchestrations 1.0.0b260409 / opentelemetry-sdk 1.45.0, released 2026-04-10; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `agent-framework/native@1.8.1` | agent-framework-core 1.8.1 / agent-framework-anthropic 1.0.0b260604 / agent-framework-orchestrations 1.0.0rc3 / opentelemetry-sdk 1.45.0, released 2026-06-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `agent-framework/native@1.9.0` | agent-framework-core 1.9.0 / agent-framework-anthropic 1.0.0b260618 / agent-framework-orchestrations 1.0.0 / opentelemetry-sdk 1.45.0, released 2026-06-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `agent-framework/sdk` | SideSeat Python 2.0.0 / Agent Framework 1.19.0 (core) / agent-framework-anthropic 1.0.0b260918 / Anthropic 0.116.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 10 |
| `agent_snapshot_reorders_answer/legacy` | a root agent span re-listing a whole turn **answer-first** while its child generation spans emit the calls and the answer separately — the shape the Vercel AI SDK's current integration produces | The **redundant re-listing** rule (`redundant_relistings`, `order_graph.rs`). Its golden records the correct conversation — question, calls, results, answer — and did not until that rule landed: `gen_ai.output.messages` reads as one atomic emission wherever it appears, so the re-listing's stated order was trusted and the answer sorted ahead of the calls that produced it. Two earlier attempts are recorded in `a_relisting_is_discounted_only_on_evidence_from_below_it`: declaring the carrier `accumulated_state` **lost a message** in `agent-framework/tool_use`, and discounting any instance whose messages appear below it fired 4,044 times across the corpus and broke `agent-framework/swarm` and `strands/image_gen`. The rule that works asks two questions instead — is every message witnessed by a *descendant*, and does the instance hold **both** a message the span produced and a result answering it, which no single model response can |
| `agentscope/native` | AgentScope 2.0.9 / Anthropic 1.11.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native `TracingMiddleware` | 11 | 15 |
| `agentscope/sdk` | SideSeat Python 2.0.0 / AgentScope 2.0.9 / Anthropic 1.11.0 (Bedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `agno/native` | Agno 3.1.0 / Anthropic 1.11.0 (Bedrock) / OpenInference Agno instrumentor 1.0.13 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; `files` records no image or PDF because the instrumentor drops media from `llm.input_messages` | 11 | 19 |
| `agno/native@0.1.34` | openinference-instrumentation-agno 0.1.34 / agno 3.1.1 / filetype 1.2.0 / opentelemetry-sdk 1.45.0, released 2026-05-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `agno/sdk` | SideSeat Python 2.0.0 / Agno 3.1.0 / Anthropic 1.11.0 (Bedrock) / OpenInference Agno instrumentor 1.0.13 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 12 |
| `anthropic/sdk` | SideSeat Python 2.0.0 / Anthropic 1.11.0 (AnthropicBedrock) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.13.7; SDK only, because native Logfire 5.1.1 abandons the span of every Anthropic 1.11 call made without tools (it JSON-encodes the SDK's `Omit` sentinel), which the SideSeat integration repairs | 10 | 10 |
| `autogen/logs` | The same releases, and AutoGen's own structured events as OTLP log records with no span instrumentation: `autogen_core` logs one event per model call and per tool run, each event's JSON body naming its kind, so the conversation arrives as records rather than as span attributes. Its spans carry no messages, and it reports no span per model call, which is why the per-call truth has nothing to key on | 10 | 11 |
| `autogen/native` | AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenAI 3.24.0 / OpenInference AutoGen instrumentor 0.1.21 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup, against the harness's deterministic fake-openai endpoint; no `files` (AgentChat messages carry no documents) or `reasoning` (its chat client reads no reasoning from Chat Completions) | 10 | 12 |
| `autogen/sdk` | SideSeat Python 2.0.0 / AutoGen AgentChat 0.7.5 / AutoGen Ext 0.7.5 / OpenAI 3.24.0 / OpenInference AutoGen instrumentor 0.1.21 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, against fake-openai | 10 | 13 |
| `azure-openai/native` | OpenAI 3.24.0 (`AzureOpenAI`, Chat Completions on a deployment route, and the Responses API for `reasoning`) / OpenInference OpenAI instrumentor 0.1.63 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup, against the harness's fake OpenAI server because Bedrock credentials cannot reach Azure; the instrumentor writes `__REDACTED__` in place of a base64 image over its 32,000-character default, so `files` carries the image as a placeholder | 9 | 9 |
| `azure-openai/native@0.1.45` | openinference-instrumentation-openai 0.1.45 / openai 3.24.0 / opentelemetry-sdk 1.45.0, released 2026-04-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `azure-openai/sdk` | SideSeat Python 2.0.0 / OpenAI 3.24.0 (`AzureOpenAI`) / OpenInference OpenAI instrumentor 0.1.63 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, against the harness's fake OpenAI server | 9 | 9 |
| `bedrock/legacy` | boto3 (bedrock runtime); only `invoke_model` remains, because no catalog scenario calls InvokeModel | 1 | 3 |
| `bedrock/native` | boto3 1.43.107 Converse and ConverseStream / OpenTelemetry botocore instrumentation 0.66b0, messages as log events / OpenTelemetry Python 1.45.0 on CPython 3.14.7; records no tool definitions and its model calls are transport spans, both declared parity allowances | 10 | 12 |
| `bedrock/native@0.63b1` | opentelemetry-instrumentation-botocore 0.63b1 / boto3 1.43.108 / opentelemetry-sdk 1.42.1, released 2026-05-21; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `bedrock/sdk` | SideSeat Python 2.0.0 / boto3 1.43.107 Converse and ConverseStream / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 10 |
| `browser-use/native` | Browser Use 0.13.10 / Laminar 0.7.64 / Anthropic 0.76.0 (AnthropicBedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, Laminar's OTLP export with its Anthropic and Bubus instruments, driving Playwright's headless Chromium shell over a travel site the suite serves locally; every step's reply is one `AgentOutput` schema call whose actions run under tool spans of their own, which carry each call and its result. No `streaming` (Browser Use never streams a step), `reasoning`, `files`, `multi_agent` or `mcp_tools` | 6 | 7 |
| `browser-use/sdk` | SideSeat Python 2.0.0 / Browser Use 0.13.10 / Laminar 0.7.64 / Anthropic 0.76.0 (AnthropicBedrock) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 6 | 8 |
| `claude-agent-sdk-js/native` | Claude Agent SDK for TypeScript 0.3.289 (Claude Code CLI 2.1.289 telemetry) on Bedrock / OpenTelemetry JS 2.11.0 on Node.js 25.2.1; like the Python suite, the CLI exports attachments as text placeholders and no thinking | 11 | 23 |
| `claude-agent-sdk-js/sdk` | SideSeat JavaScript 3.0.0 / Claude Agent SDK for TypeScript 0.3.289 (Claude Code CLI 2.1.289) on Bedrock / OpenTelemetry JS 2.11.0 on Node.js 25.2.1 | 11 | 23 |
| `claude-agent-sdk/native` | Claude Agent SDK 0.2.163 (Claude Code CLI telemetry) on Bedrock / OpenTelemetry Python 1.45.0 on CPython 3.13.7; the CLI exports attachments as text placeholders and no thinking | 11 | 23 |
| `claude-agent-sdk/sdk` | SideSeat Python 2.0.0 / Claude Agent SDK 0.2.163 on Bedrock / OpenTelemetry Python 1.45.0 on CPython 3.13.7 | 11 | 23 |
| `claude-code/logs` | Claude Code 2.1.288 (`claude -p`, resumed per turn) on Bedrock, enhanced tracing without the detailed tier, log events and metrics on, `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_DETAILS` and `OTEL_LOG_TOOL_CONTENT`: the conversation comes from log events and `tool.output`; tool calls show no input, because the input is only on the log record written after the call (backlog cli-2), and a failed command's result is the log's `Shell command failed`, because no signal exports its output; the sub-agent's calls are in the parent's trace, nested under the `Agent` tool span | 4 | 6 |
| `claude-code/native` | Claude Code 2.1.288 (`claude -p`, resumed per turn) on Bedrock, detailed beta tracing plus log events and metrics, `OTEL_LOG_USER_PROMPTS` and `OTEL_LOG_TOOL_DETAILS`; tool content off, since beside the detailed tier it duplicates every result (backlog cli-1); a resumed session's first request restates the earlier prompts (cli-6) and a failed command shows the log's error summary (cli-3); no SDK half (`producer.json`) | 4 | 6 |
| `codex/native` | Codex CLI 0.160.0 (`codex exec`, `resume --last` per turn) against Bedrock's Responses endpoint through a custom provider, `[otel]` logs, traces and metrics with `log_user_prompt` and `log_agent_responses`; Codex exports no system prompt and no text but each turn's final reply, and no span names the session, so `multi_turn` is three traces; a sub-agent's task, calls and report are in the parent's trace, and its report is also the content of `wait_agent`'s and `close_agent`'s results, as Codex returned it; no SDK half (`producer.json`) | 4 | 23 |
| `crewai/native` | CrewAI 1.15.23 / OpenInference CrewAI instrumentor 1.1.20 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, native OTLP setup; no `structured_output`, `files`, or `streaming`: CrewAI's Bedrock provider forces tool choice, refuses media for Claude 5, and does not run a streamed tool call | 8 | 13 |
| `crewai/native@1.1.10` | openinference-instrumentation-crewai 1.1.10 / crewai 1.15.23 / opentelemetry-sdk 1.45.0, released 2026-06-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `crewai/native@1.1.3` | openinference-instrumentation-crewai 1.1.3 / crewai 1.15.23 / opentelemetry-sdk 1.45.0, released 2026-04-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `crewai/native@1.1.7` | openinference-instrumentation-crewai 1.1.7 / crewai 1.15.23 / opentelemetry-sdk 1.45.0, released 2026-05-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `crewai/sdk` | SideSeat Python 2.0.0 / CrewAI 1.15.23 / OpenInference CrewAI instrumentor 1.1.20 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 8 | 8 |
| `cross_span_tie/legacy` | a generation span and its tool span reporting the **identical** instant, with the tool span's id sorting *first* | `adopt_call_positions`. Disable it and this fixture reports the answer at index 1 before its question at index 3; every captured fixture stays green, because none of them ties |
| `dotnet/native` | OpenTelemetry .NET 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `dotnet/sdk` | SideSeat .NET 1.0.0 / OpenTelemetry 1.19.1 on .NET SDK 10.0.401 | 1 | 1 |
| `genkit-go/native` | Genkit for Go 1.13.1 (Google AI plugin) / OpenTelemetry Go 1.47.0 on Go 1.27.1, Genkit tracing on the global tracer provider, against the harness's fake Gemini server; no `mcp_tools` | 10 | 10 |
| `genkit-go/native@1.12.0` | github.com/firebase/genkit/go 1.12.0 / go.opentelemetry.io/otel/sdk 1.47.0, released 2026-08-17; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `genkit-go/sdk` | The same under SideSeat's OpenTelemetry recipe for Go | 10 | 10 |
| `google-genai/native` | Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, against the harness's fake Gemini server; reasoning thoughts arrive as text parts (the instrumentation drops Gemini's `thought` flag) and a failed tool call as an error message rather than a tool result | 9 | 9 |
| `google-genai/native@0.6b0+semconv-latest` | opentelemetry-instrumentation-google-genai 0.6b0 / google-genai 1.60.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-01-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `google-genai/native@0.7b1+semconv-latest` | opentelemetry-instrumentation-google-genai 0.7b1 / google-genai 2.4.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-05-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `google-genai/native@1.0b1` | opentelemetry-instrumentation-google-genai 1.0b1 / google-genai 2.11.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-07-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `google-genai/native@1.1b1` | opentelemetry-instrumentation-google-genai 1.1b1 / google-genai 2.19.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-08-21; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `google-genai/sdk` | SideSeat Python 2.0.0 / Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, against the harness's fake Gemini server | 9 | 9 |
| `haystack/native` | Haystack 3.3.0 / Amazon Bedrock Haystack 8.3.0 / MCP Haystack 1.5.1 / opentelemetry-haystack 1.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; Haystack's tracer writes a placeholder in place of image and file bytes, so `files` carries the request text only | 11 | 16 |
| `haystack/sdk` | SideSeat Python 2.0.0 / Haystack 3.3.0 / Amazon Bedrock Haystack 8.3.0 / MCP Haystack 1.5.1 / opentelemetry-haystack 1.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `javascript/native` | OpenTelemetry JS 2.11.0 / OTLP exporter 0.222.0 on Node.js 26.9.0 | 1 | 1 |
| `javascript/sdk` | SideSeat JavaScript 2.0.0 / OpenTelemetry JS 2.11.0 on Node.js 26.9.0 | 1 | 1 |
| `koog/native` | Koog 1.3.0 (Kotlin 2.4.20, Koog's Bedrock client on Converse, Claude Sonnet 5.5) / Koog's OpenTelemetry feature on its own SDK with an OTLP exporter, session and user stamped by its span adapter / OpenTelemetry Java 1.66.0 on Temurin 25; no `multi_turn`, `streaming`, `structured_output`, `reasoning`, `files` or `mcp_tools` yet (Koog's MCP module is beta only) | 5 | 7 |
| `koog/sdk` | The same under SideSeat's OpenTelemetry recipe for the JVM (`sideseat.framework` on the resource) | 5 | 7 |
| `langchain/native` | LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; no `multi_agent`, because LangChain's multi-agent patterns run on LangGraph | 10 | 12 |
| `langchain/sdk` | SideSeat Python 2.0.0 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 11 |
| `langchain4j/native` | LangChain4j 1.21.0 (AI services, Bedrock Converse, Claude Sonnet 5.5) / OpenInference LangChain4j instrumentor 0.1.9 (AI-service listeners) / OpenTelemetry Java 1.66.0 on Temurin 25; no `multi_agent` or `mcp_tools`, and no `streaming`, whose service span the instrumentor fills with the response's run-dependent `toString()` | 8 | 8 |
| `langchain4j/native@0.1.8` | openinference-langchain4j 0.1.8 / opentelemetry 1.66.0, released 2026-04-04; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `langchain4j/sdk` | The same under SideSeat's OpenTelemetry recipe for the JVM | 8 | 8 |
| `langfuse/native` | Langfuse 4.16.0 (the `langfuse.openai` drop-in client, and `@observe` around the agent loop and each tool) / OpenAI 3.24.0 (Chat Completions, and the Responses API wherever tools are declared, on GPT-6.1-sol) / OpenTelemetry Python 1.45.0 on CPython 3.14.7, Langfuse given the application's tracer provider and an exporter that sends nothing to Langfuse's cloud, with media upload off so attachments stay inline; replays the `openai` suite's model traffic, because the requests are identical | 8 | 8 |
| `langfuse/native@4.13.2` | langfuse 4.13.2 / openai 3.24.0 / opentelemetry-sdk 1.45.0, released 2026-07-08; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `langfuse/native@4.6.1` | langfuse 4.6.1 / openai 3.24.0 / opentelemetry-sdk 1.45.0, released 2026-05-08; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `langfuse/native@4.9.0` | langfuse 4.9.0 / openai 3.24.0 / opentelemetry-sdk 1.45.0, released 2026-06-16; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `langfuse/sdk` | SideSeat Python 2.0.0 / Langfuse 4.16.0 / OpenAI 3.24.0 (Chat Completions and Responses on GPT-6.1-sol) / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 8 | 8 |
| `langgraph/native` | LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup | 11 | 16 |
| `langgraph/sdk` | SideSeat Python 2.0.0 / LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenInference LangChain instrumentor 0.1.78 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `langsmith/native` | LangSmith 0.14.4 in OpenTelemetry mode (`LANGSMITH_TRACING_MODE=otel`, nothing sent to LangSmith's API) / LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; replays the `langgraph` suite's model traffic, because the requests are identical | 11 | 11 |
| `langsmith/sdk` | SideSeat Python 2.0.0 / LangSmith 0.14.4 / LangGraph 1.2.12 / LangChain Core 1.6.6 / LangChain AWS 1.8.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `legacy` | an older capture with no native/SDK pair; removed as each producer is recaptured |
| `logs` | the producer's own setup with its conversation in log events rather than span attributes, where it documents both (Claude Code without its detailed tracing tier) |
| `llama-index/native` | LlamaIndex Core 0.14.25 / LlamaIndex Bedrock Converse 0.15.3 / LlamaIndex MCP tools 0.6.0 / OpenInference LlamaIndex instrumentor 4.5.4 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; the instrumentor records no document blocks or reasoning and redacts large inline images, and `structured_output` is not captured yet: its reformatting request does not reconstruct | 10 | 21 |
| `llama-index/native@4.3.10` | openinference-instrumentation-llama-index 4.3.10 / llama-index-core 0.14.25 / llama-index-llms-bedrock-converse 0.15.3 / llama-index-tools-mcp 0.6.0 / opentelemetry-sdk 1.45.0, released 2026-05-10; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `llama-index/native@4.5.2` | openinference-instrumentation-llama-index 4.5.2 / llama-index-core 0.14.25 / llama-index-llms-bedrock-converse 0.15.3 / llama-index-tools-mcp 0.6.0 / opentelemetry-sdk 1.45.0, released 2026-09-10; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `llama-index/sdk` | SideSeat Python 2.0.0 / LlamaIndex Core 0.14.25 / LlamaIndex Bedrock Converse 0.15.3 / LlamaIndex MCP tools 0.6.0 / OpenInference LlamaIndex instrumentor 4.5.4 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 10 | 11 |
| `log_and_span_event_overlap/legacy` | a dual emitter: the question as a span event **and** a log record, the answer only as a log record | the repeated question collapses to one turn, and the answer appears only because logs are joined (`log_carried_messages_reach_every_view_exactly_once`) |
| `log_events_before_span/legacy` | the OpenTelemetry OpenAI v2 / botocore default: the whole conversation as log records whose body members are the event, exported before the span, one naming its event through the legacy `event.name` attribute | `log_events` recognition by field and by attribute, and the read-time join; without the logs every view is empty |
| `log_inference_details/legacy` | `gen_ai.client.inference.operation.details` as a log record whose **attributes** carry the request and response as structured values | the container readings applied to a log record, with structured `AnyValue`s kept whole |
| `logfire/native` | Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0, Logfire's own setup with a scrubbing callback that keeps `session.id` | 11 | 11 |
| `logfire/native@4.12.0` | logfire 4.12.0 / opentelemetry-sdk 1.37.0, released 2025-10-08; version matrix variant, replayed offline from the suite's cassettes | 1 | 1 |
| `logfire/native@4.18.0` | logfire 4.18.0 / opentelemetry-sdk 1.39.1, released 2026-01-12; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.20.0` | logfire 4.20.0 / opentelemetry-sdk 1.39.1, released 2026-01-26; version matrix variant, replayed offline from the suite's cassettes | 2 | 3 |
| `logfire/native@4.28.0` | logfire 4.28.0 / opentelemetry-sdk 1.39.1, released 2026-03-11; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.31.0` | logfire 4.31.0 / opentelemetry-sdk 1.39.1, released 2026-03-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.31.1` | logfire 4.31.1 / opentelemetry-sdk 1.39.1, released 2026-04-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.32.1` | logfire 4.32.1 / opentelemetry-sdk 1.40.0, released 2026-04-15; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.36.0` | logfire 4.36.0 / opentelemetry-sdk 1.42.1, released 2026-06-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/native@4.41.0` | logfire 4.41.0 / opentelemetry-sdk 1.44.0, released 2026-08-20; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `logfire/sdk` | SideSeat Python 2.0.0 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 | 11 | 11 |
| `multi_turn_one_carrier/legacy` | nine turns in **one** carrier, in conversation order | carrier subsequence across many siblings - the ADK shape, where one span holds a whole conversation |
| `native` | the framework's own documented OpenTelemetry setup, or plain OpenTelemetry for a conformance program |
| `openai-agents/legacy` | openai-agents >=0.12.1; two fixtures remain. `image_gen` holds two different exports of the same spans, one scrubbed by Logfire and one not, which makes it the corpus's only fixture whose duplicate copies differ - `which_copy_survives_does_not_change_the_order` needs one - and makes it tie under reversed arrival, so `agent_core` stays the producer's sample for `the_order_spans_arrive_in_does_not_change_the_answer` | 2 | 7 |
| `openai-agents/native` | OpenAI Agents SDK 0.23.1 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 on CPython 3.14.7, Logfire's `instrument_openai_agents` with a scrubbing callback that keeps `session.id`; no `reasoning`, because Bedrock rejects `reasoning.summary` for GPT-6.1-sol, and `files` records the image as its data URL in text and the PDF as its file name, because Logfire's `events` flattens both (the bytes are only in `raw_input`, which no rule reads); in `multi_agent` the handoff's result follows the writer's instructions, because only the writer's request records it | 11 | 19 |
| `openai-agents/native@0.18.1` | openai-agents 0.18.1 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-07-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai-agents/native@0.19.4` | openai-agents 0.19.4 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-08-05; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai-agents/native@0.20.0` | openai-agents 0.20.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-08-11; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai-agents/native@0.21.0` | openai-agents 0.21.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-08-15; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai-agents/sdk` | SideSeat Python 2.0.0 / OpenAI Agents SDK 0.23.1 / Logfire 5.1.1 / OpenAI 3.24.0 (Responses API on GPT-6.1-sol) / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 11 | 13 |
| `openai/native` | OpenAI 3.24.0 (Chat Completions, and the Responses API wherever tools are declared, on GPT-6.1-sol) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7, Logfire's documented `instrument_openai` setup with a scrubbing callback that keeps `session.id`; no `reasoning`, because Bedrock's GPT-6.1-sol rejects `reasoning.summary` and returns its reasoning only encrypted | 8 | 13 |
| `openai/native@4.12.0` | logfire 4.12.0 / openai 3.24.0 / opentelemetry-sdk 1.37.0, released 2025-10-08; version matrix variant, replayed offline from the suite's cassettes | 1 | 1 |
| `openai/native@4.18.0` | logfire 4.18.0 / openai 3.24.0 / opentelemetry-sdk 1.39.1, released 2026-01-12; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.20.0` | logfire 4.20.0 / openai 3.24.0 / opentelemetry-sdk 1.39.1, released 2026-01-26; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.28.0` | logfire 4.28.0 / openai 3.24.0 / opentelemetry-sdk 1.39.1, released 2026-03-11; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.31.0` | logfire 4.31.0 / openai 3.24.0 / opentelemetry-sdk 1.39.1, released 2026-03-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.31.1` | logfire 4.31.1 / openai 3.24.0 / opentelemetry-sdk 1.39.1, released 2026-04-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.32.1` | logfire 4.32.1 / openai 3.24.0 / opentelemetry-sdk 1.40.0, released 2026-04-15; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.36.0` | logfire 4.36.0 / openai 3.24.0 / opentelemetry-sdk 1.42.1, released 2026-06-09; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/native@4.41.0` | logfire 4.41.0 / openai 3.24.0 / opentelemetry-sdk 1.44.0, released 2026-08-20; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openai/sdk` | SideSeat Python 2.0.0 / OpenAI 3.24.0 (Chat Completions and Responses on GPT-6.1-sol) / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 8 | 8 |
| `openinference/native` | OpenInference Bedrock instrumentor 0.1.56 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0, native OTLP setup; the instrumentor keeps only the last of a turn's parallel tool results, redacts images by default and drops documents | 12 | 17 |
| `openinference/native@0.1.35` | openinference-instrumentation-bedrock 0.1.35 / openinference-instrumentation 0.1.70 / opentelemetry-sdk 1.45.0, released 2026-04-22; version matrix variant, replayed offline from the suite's cassettes | 1 | 1 |
| `openinference/native@0.1.42` | openinference-instrumentation-bedrock 0.1.42 / openinference-instrumentation 0.1.70 / opentelemetry-sdk 1.45.0, released 2026-06-24; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openinference/native@0.1.49` | openinference-instrumentation-bedrock 0.1.49 / openinference-instrumentation 0.1.70 / opentelemetry-sdk 1.45.0, released 2026-08-31; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `openinference/sdk` | SideSeat Python 2.0.0 / OpenInference Bedrock instrumentor 0.1.56 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0 | 12 | 12 |
| `otel-genai/native` | opentelemetry-util-genai 1.2b0 / OpenAI 3.26.1 (Responses API against the local OpenAI endpoint) / OpenTelemetry Python 1.45.1, an application recording each model call as a GenAI inference with message content on the span; `server_tools` is the provider's own web search, recorded as a server tool call and its response | 8 | 8 |
| `otel-genai/native@1.2b0` | opentelemetry-util-genai 1.2b0 / openai 3.24.0 / opentelemetry-sdk 1.45.0, released 2026-09-24; version matrix variant, replayed offline against the local OpenAI endpoint | 2 | 2 |
| `otel-genai/sdk` | SideSeat Python 2.0.0 with no framework integration / opentelemetry-util-genai 1.2b0 / OpenAI 3.26.1 (Responses API against the local OpenAI endpoint) / OpenTelemetry Python 1.45.1 | 8 | 8 |
| `parallel_tool_calls/legacy` | two distinct calls in one response, then both results | causality *without* adjacency: `call, call, result, result` must be allowed |
| `pydantic-ai/native` | Pydantic AI 2.53.0 / OpenTelemetry Python 1.44.0 on CPython 3.14.7, `Agent.instrument_all()` on a plain provider | 11 | 17 |
| `pydantic-ai/native@1.1.0` | pydantic-ai-slim 1.1.0 / opentelemetry-sdk 1.37.0, released 2025-10-15; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.104.0` | pydantic-ai-slim 1.104.0 / opentelemetry-sdk 1.42.1, released 2026-05-29; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.107.0` | pydantic-ai-slim 1.107.0 / opentelemetry-sdk 1.42.1, released 2026-06-10; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.107.7` | pydantic-ai-slim 1.107.7 / opentelemetry-sdk 1.44.0, released 2026-09-30; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.11.0` | pydantic-ai-slim 1.11.0 / opentelemetry-sdk 1.38.0, released 2025-11-05; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.31.0` | pydantic-ai-slim 1.31.0 / opentelemetry-sdk 1.39.1, released 2025-12-12; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.41.0` | pydantic-ai-slim 1.41.0 / opentelemetry-sdk 1.39.1, released 2026-01-10; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.67.0` | pydantic-ai-slim 1.67.0 / opentelemetry-sdk 1.40.0, released 2026-03-06; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.70.0` | pydantic-ai-slim 1.70.0 / opentelemetry-sdk 1.40.0, released 2026-03-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.73.0` | pydantic-ai-slim 1.73.0 / opentelemetry-sdk 1.40.0, released 2026-03-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.74.0` | pydantic-ai-slim 1.74.0 / opentelemetry-sdk 1.40.0, released 2026-03-31; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.75.0` | pydantic-ai-slim 1.75.0 / opentelemetry-sdk 1.40.0, released 2026-04-01; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.76.0` | pydantic-ai-slim 1.76.0 / opentelemetry-sdk 1.40.0, released 2026-04-02; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.77.0` | pydantic-ai-slim 1.77.0 / opentelemetry-sdk 1.40.0, released 2026-04-03; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.81.0` | pydantic-ai-slim 1.81.0 / opentelemetry-sdk 1.41.0, released 2026-04-14; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.86.0` | pydantic-ai-slim 1.86.0 / opentelemetry-sdk 1.41.0, released 2026-04-23; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.88.0` | pydantic-ai-slim 1.88.0 / opentelemetry-sdk 1.41.1, released 2026-04-29; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.94.0` | pydantic-ai-slim 1.94.0 / opentelemetry-sdk 1.41.1, released 2026-05-12; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@1.99.0` | pydantic-ai-slim 1.99.0 / opentelemetry-sdk 1.42.0, released 2026-05-20; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.0.0` | pydantic-ai-slim 2.0.0 / opentelemetry-sdk 1.42.1, released 2026-06-23; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.13.0` | pydantic-ai-slim 2.13.0 / opentelemetry-sdk 1.44.0, released 2026-07-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.21.0` | pydantic-ai-slim 2.21.0 / opentelemetry-sdk 1.44.0, released 2026-07-30; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.25.0` | pydantic-ai-slim 2.25.0 / opentelemetry-sdk 1.44.0, released 2026-08-06; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.29.0` | pydantic-ai-slim 2.29.0 / opentelemetry-sdk 1.44.0, released 2026-08-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.35.3` | pydantic-ai-slim 2.35.3 / opentelemetry-sdk 1.44.0, released 2026-08-28; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/native@2.5.1` | pydantic-ai-slim 2.5.1 / opentelemetry-sdk 1.43.0, released 2026-07-07; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `pydantic-ai/sdk` | SideSeat Python 2.0.0 / Pydantic AI 2.53.0 / Logfire 5.1.1 / OpenTelemetry Python 1.44.0 on CPython 3.14.7 | 11 | 11 |
| `python/native` | OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `python/sdk` | SideSeat Python 2.0.0 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 1 | 1 |
| `quoted_steps_on_two_carriers/legacy` | ADK's request on two carriers - the `call_llm` span's `llm_request` attribute and the `generate_content` span's `gen_ai.user.message` log records - quoting each earlier agent step behind the same preamble, the answer only in a log record | each step keeps its own preamble from one carrier, in the request's order: the two carriers must agree that a position is an occurrence, or the preambles are matched to the wrong messages |
| `resent_history/legacy` | a later span re-sending the earlier turn | the re-send collapses onto the original rather than duplicating it |
| `rust/native` | OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `rust/sdk` | SideSeat Rust 0.2.0 / OpenTelemetry Rust 0.33.0 on Rust 1.94.1 | 1 | 1 |
| `sdk` | the SideSeat SDK, with the same scenario code |
| `<mode>@<version>[+<profile>]` | the same mode, replayed against a historical release of the framework (and an opt-in profile) by `python -m harness matrix`; see `docs/engineering/framework-versions.md` |
| `semantic-kernel/native` | Semantic Kernel 1.44.1 Bedrock connector / boto3 1.42.97 / OpenTelemetry Python 1.45.0 on CPython 3.12.8, its GenAI diagnostics switched on, messages as log records exported through a `LoggingHandler`; tool results are the Python `str()` of the function result, which is what Semantic Kernel sends the model | 8 | 14 |
| `semantic-kernel/native@1.37.1` | semantic-kernel 1.37.1 / opentelemetry-sdk 1.45.0, released 2025-10-30; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `semantic-kernel/sdk` | SideSeat Python 2.0.0 / Semantic Kernel 1.44.1 Bedrock connector / boto3 1.42.97 / OpenTelemetry Python 1.45.0 on CPython 3.12.8 | 8 | 8 |
| `smolagents/native` | Smolagents 1.26.0 / LiteLLM 1.103.2 (Bedrock) / OpenInference Smolagents instrumentor 0.1.42 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, native OTLP setup; no `structured_output` or `files` (unsupported) and no `mcp_tools` (its MCP adapter misreads the server schema) | 8 | 15 |
| `smolagents/native@0.1.26` | openinference-instrumentation-smolagents 0.1.26 / smolagents 1.26.0 / litellm 1.104.0 / opentelemetry-sdk 1.45.0, released 2026-04-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `smolagents/native@0.1.27` | openinference-instrumentation-smolagents 0.1.27 / smolagents 1.26.0 / litellm 1.104.0 / opentelemetry-sdk 1.45.0, released 2026-04-29; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `smolagents/native@0.1.32` | openinference-instrumentation-smolagents 0.1.32 / smolagents 1.26.0 / litellm 1.104.0 / opentelemetry-sdk 1.45.0, released 2026-05-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `smolagents/sdk` | SideSeat Python 2.0.0 / Smolagents 1.26.0 / LiteLLM 1.103.2 (Bedrock) / OpenInference Smolagents instrumentor 0.1.42 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 8 | 8 |
| `spring-ai/native` | Spring AI 2.0.1 (`ChatClient`, Bedrock Converse, Claude Sonnet 5.5) / OpenInference Spring AI instrumentor 0.1.10 as an observation handler / OpenTelemetry Java 1.66.0 on Temurin 25; the instrumentor records model calls only, and of a message carrying several tool results only the first; no `reasoning`, `multi_agent` or `mcp_tools` | 8 | 10 |
| `spring-ai/native@0.1.9` | openinference-springai 0.1.9 / opentelemetry 1.66.0, released 2026-04-04; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `spring-ai/sdk` | The same under SideSeat's OpenTelemetry recipe for the JVM | 8 | 8 |
| `strands-js/native` | Strands Agents for TypeScript 1.19.0 (Bedrock) / OpenTelemetry JS 2.11.0, OTLP exporter 0.219.0 on Node.js 25.2.1, `setupTracer`; Strands reports no reasoning in a turn's output, so `reasoning` shows the answer only and `multi_turn` shows a turn's redacted reasoning after its answer, where the next request's history first carries it | 11 | 12 |
| `strands-js/native@1.11.2+semconv-latest` | @strands-agents/sdk 1.11.2 / @opentelemetry/sdk-trace-base 2.11.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-07-27; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `strands-js/native@1.19.0+semconv-latest` | @strands-agents/sdk 1.19.0 / @opentelemetry/sdk-trace-base 2.11.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-09-22; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `strands-js/sdk` | SideSeat JavaScript 3.0.0 / Strands Agents for TypeScript 1.19.0 (Bedrock) / OpenTelemetry JS 2.11.0 on Node.js 25.2.1 | 11 | 11 |
| `strands/native` | Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7, `StrandsTelemetry` | 11 | 14 |
| `strands/native@1.12.0` | strands-agents 1.12.0 / opentelemetry-sdk 1.45.0, released 2025-10-10; version matrix variant, replayed offline from the suite's cassettes | 3 | 21 |
| `strands/native@1.12.0+semconv-latest` | strands-agents 1.12.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2025-10-10; version matrix variant, replayed offline from the suite's cassettes | 3 | 21 |
| `strands/native@1.33.0` | strands-agents 1.33.0 / opentelemetry-sdk 1.45.0, released 2026-03-24; version matrix variant, replayed offline from the suite's cassettes | 3 | 11 |
| `strands/native@1.33.0+semconv-latest` | strands-agents 1.33.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-03-24; version matrix variant, replayed offline from the suite's cassettes | 3 | 11 |
| `strands/native@1.35.0` | strands-agents 1.35.0 / opentelemetry-sdk 1.45.0, released 2026-04-08; version matrix variant, replayed offline from the suite's cassettes | 3 | 21 |
| `strands/native@1.35.0+semconv-latest` | strands-agents 1.35.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-04-08; version matrix variant, replayed offline from the suite's cassettes | 3 | 21 |
| `strands/native@1.46.0+semconv-latest` | strands-agents 1.46.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-07-08; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.50.2+semconv-latest` | strands-agents 1.50.2 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-07-27; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.53.0` | strands-agents 1.53.0 / opentelemetry-sdk 1.45.0, released 2026-08-21; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.53.0+semconv-latest` | strands-agents 1.53.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-08-21; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.55.0` | strands-agents 1.55.0 / opentelemetry-sdk 1.45.0, released 2026-09-08; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.55.0+semconv-latest` | strands-agents 1.55.0 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-09-08; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/native@1.57.2+semconv-latest` | strands-agents 1.57.2 / opentelemetry-sdk 1.45.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-10-01; version matrix variant, replayed offline from the suite's cassettes | 3 | 3 |
| `strands/sdk` | SideSeat Python 2.0.0 / Strands Agents 1.57.2 / OpenTelemetry Python 1.45.0 on CPython 3.14.7 | 11 | 11 |
| `tool_use/legacy` | a Strands call/result pair | the baseline hand-written case |
| `adversarial_*` | captures that are wrong on purpose - a repeated part, a prompt on a sibling span, parts out of wire order, two spans showing one answer, a failed attempt that speaks - and clean ones the truths in `fixtures/truth-adversarial/` contradict | every rubric v2 check fires on some case (`truth_adversarial_fixtures_fire_their_checks`), each case recording exactly which |
| `traceloop/native` | TraceLoop SDK 0.62.4 / Bedrock instrumentation 0.62.4 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0, native OTLP setup; image and document bytes are exported empty | 12 | 18 |
| `traceloop/native@0.53.3` | traceloop-sdk 0.53.3 / requests 2.34.2 / httpx 0.28.1 / opentelemetry-sdk 1.45.0, released 2026-03-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `traceloop/native@0.56.0` | traceloop-sdk 0.56.0 / requests 2.34.2 / httpx 0.28.1 / opentelemetry-sdk 1.45.0, released 2026-03-30; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `traceloop/native@0.61.0` | traceloop-sdk 0.61.0 / requests 2.34.2 / httpx 0.28.1 / opentelemetry-sdk 1.45.0, released 2026-05-31; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `traceloop/sdk` | SideSeat Python 2.0.0 / TraceLoop SDK 0.62.4 / Bedrock instrumentation 0.62.4 / boto3 1.43.108 Converse / OpenTelemetry Python 1.45.0 | 12 | 13 |
| `vercel-ai-js/legacy` | ai ^7.0.79; only `tool-use` remains, the sole capture of the `ai.*` attributes the `vercel-ai.prompt`, `prompt_tools`, `response`, `toolcall_args` and `toolcall_result` rules read, which current releases no longer write | 1 | 2 |
| `vercel-ai-js/native` | AI SDK 7.0.127 / @ai-sdk/otel 1.0.127 / @ai-sdk/amazon-bedrock 5.0.105 / OpenTelemetry JS 2.11.0 on Node.js 25.2.1, `registerTelemetry(new OpenTelemetry())`; `structured_output` offers the schema as a `trip_plan` tool, and `mcp_tools` takes its tools from the official MCP client, because `@ai-sdk/mcp` 2.0 opens a handshake current MCP servers refuse | 11 | 12 |
| `vercel-ai-js/native@1.0.43` | @ai-sdk/otel 1.0.43 / @opentelemetry/sdk-trace-base 2.11.0, released 2026-07-30; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vercel-ai-js/native@1.0.52` | @ai-sdk/otel 1.0.52 / @opentelemetry/sdk-trace-base 2.11.0, released 2026-08-04; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vercel-ai-js/native@1.0.68` | @ai-sdk/otel 1.0.68 / @opentelemetry/sdk-trace-base 2.11.0, released 2026-08-18; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vercel-ai-js/sdk` | SideSeat JavaScript 3.0.0 / AI SDK 7.0.127 / @ai-sdk/otel 1.0.127 / @ai-sdk/amazon-bedrock 5.0.105 / OpenTelemetry JS 2.11.0 on Node.js 25.2.1 | 11 | 11 |
| `vertex-ai/native` | Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, native Logfire setup with the current `enterprise=True` Vertex mode, against the harness's fake Gemini server; reasoning thoughts arrive as text parts (the instrumentation drops Gemini's `thought` flag) and a failed tool call as an error message rather than a tool result | 9 | 9 |
| `vertex-ai/native@0.7b1+semconv-latest` | opentelemetry-instrumentation-google-genai 0.7b1 / google-genai 2.4.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, profile `semconv-latest` (OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental), released 2026-05-19; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vertex-ai/native@1.0b1` | opentelemetry-instrumentation-google-genai 1.0b1 / google-genai 2.11.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-07-13; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vertex-ai/native@1.1b1` | opentelemetry-instrumentation-google-genai 1.1b1 / google-genai 2.19.0 / logfire 5.1.1 / opentelemetry-sdk 1.44.0, released 2026-08-21; version matrix variant, replayed offline from the suite's cassettes | 2 | 2 |
| `vertex-ai/sdk` | SideSeat Python 2.0.0 / Google GenAI 2.28.0 / Logfire 5.1.1 / Google GenAI OTel instrumentor 1.2b0 / OpenTelemetry Python 1.44.0 on CPython 3.13.7, current `enterprise=True` Vertex mode, against the harness's fake Gemini server | 9 | 9 |

The carrier-overlap defect is documented by `reading_more_carriers_only_adds_messages` rather than by a
fixture: it runs every fixture through both extraction modes and reports what each gains and what
reorders (today: 20 views gain messages, 10 reorder, all named). A hand-written payload for that shape
was tried and dropped - it reproduced the *attributes* but not the behaviour, because the LangGraph
reader claims only on a `langgraph.*` marker and parses a narrower message shape than the one written
by hand, so its exemption would have asserted a cause the fixture did not exhibit.

## Capability exemptions

`PAIRING_EXEMPT` in the test names fixtures whose *source* telemetry cannot satisfy tool
pairing, with the reason; it is empty today. A capability limit of a framework is recorded per fixture rather than
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

`adversarial_composed_*` are the three thread shapes a delta-exporting producer can hold
(`docs/engineering/request-context.md`): a resumed process whose every delta restates the conversation, a
subagent whose thread starts empty beside its parent's, and a failed attempt whose retry re-sent the
conversation itself. Each is a real captured `claude_code.llm_request` span shape - the same resource, scope,
span names and attributes as `claude-agent-sdk/native` - with **only the thread structure varied**, so what
they prove is the composition over that structure and nothing else. Their truths are written from the requests
each capture states, never from the composed output.

Real captures are preferred for every framework, and these are not a substitute for one: each exists
because a defect was found in a shape the corpus did not hold, and the fixture is what makes the
answer to that shape reviewable. They are also the only fixtures that survive a checkout with no
credentials, so they keep the harness itself under test.

## Not committed

`crewai/agent_core` is gitignored: CrewAI serialises its entire model config into a span
attribute, so the captured payload contained a live `aws_secret_access_key` and
`aws_session_token`. A secret in a fixture goes straight into git history, where it cannot be
taken back — the capture tool (`examples/python/harness/harness/capture.py`) now discards any run whose
payload holds an AWS credential value rather than leaving the decision to a later reader.

The capture tool prints a warning for any payload over 1 MB, so a capture that
inlines megabytes of media is a decision rather than a surprise.
