# Request context: showing what a delta-exporting call was sent

Design note, not yet implemented. Proposed by the rubric track and revised after a Codex review against every
captured `claude_code.llm_request` span; to be implemented in `domain::rules` and `sideml::feed` by the
rule-language track.

## The problem

A span view promises the request a model call was sent: system parts, then every message, in order. The rubric
holds it to that (`check_requests` in `server/tests/message_truth/requests.rs`), against the request bytes the
fixture's run recorded (`model-requests.json`).

Some producers do not export the request. They export what is new since the previous request of the same
conversation. Claude Code's `claude_code.llm_request` span (Claude Code CLI, Claude Agent SDK for Python and
TypeScript) carries:

| Attribute | Holds |
| --- | --- |
| `new_context` | the messages added since the previous request of this thread, as `[USER]`, `[TOOL RESULT: id]` sections |
| `new_context_message_count` | how many *messages* that is - not sections: four results in one message count once |
| `system_prompt_preview`, `system_prompt_length` | this request's system prompt, previewed (cut at 500 UTF-16 units) |
| `system_reminders` | the reminders this request carries |
| `response.model_output` | the text the model answered |
| `query_source`, `agent_id` | the thread: `sdk` for the main agent, `agent:custom:<name>` or `agent:builtin:<name>` with an agent id for a subagent |
| `attempt` | the retry number |
| `session.id` | the session, shared across interactions and traces |

The model's tool calls are exported on `claude_code.tool` spans (`tool_use_id`, `tool_input`), siblings of the
request under the interaction or the subagent's tool execution, without `query_source`. So call 2 of a tool loop
exports `[TOOL RESULT: ...]` only; the prompt it re-sent is on call 1's `new_context`, and the call it re-sent is on
a tool span. The Anthropic API is stateless, so the bytes call 2 put on the wire held the whole history; only the
telemetry is a delta.

This is 127 of the 349 `request.missing` entries at `00c1477b` (claude-agent-sdk 60, claude-agent-sdk-js 48,
claude-code 19). The rubric is right to report them: the history is in the payloads, on earlier spans, so the span
view can show it. Owing less on delta spans would excuse a view that shows less than the telemetry carries, which is
loosening.

## What the captures show

All 114 `llm_request` spans of the 52 Claude Code fixtures, read by Codex and by hand:

- **Threads.** Main-agent requests have `query_source = sdk` and parent to the interaction; subagent requests have
  their own `query_source` and `agent_id` and parent to the tool execution that started them. Log-only captures
  have no `query_source`. The SDK `multi_turn` scenario keeps one session across interactions, `session` starts
  two sessions, and the CLI's `multi_turn` keeps one session across traces.
- **No retries.** Every `attempt` is 1 and every request succeeded; the error scenarios fail tools, not requests.
- **Resumed processes re-export history.** In the CLI's `multi_turn` (`req-002.json`), span `427ba40634ca182a`
  exports both earlier user turns again and `357f6f87de9597e7` all three: a delta may restate what an earlier
  request already sent.
- **System-role messages are history too.** Request 2 of the SDK's `multi_turn` keeps request 1's environment block
  between the first prompt and the first answer: a reminder or environment block sent as a message is a message of
  the conversation, inherited like any other. Only the system prompt is per request.
- **Counts do not count sections.** `new_context_message_count` is 2 for a delta of four tool results and a
  reminder; a subagent's first request carries a reminder inside its user message.
- **Parallel batches.** A request that answers a batch sends all the calls, then all the results.

## Which producers export deltas

In the captured corpus, only the Claude Code family. Checked against every `request.missing` whose proof is
`Present` at `00c1477b`, the others are not deltas. Either the call's own span carries the part and its view drops
it - a reconstruction defect, fixed where it is (tool results on agno's `input.value`, autogen's flattened
`llm.input_messages`, the Bedrock SDK's request event) - or the producer leaves one kind of part out of every request
it exports and records it on another span of the same step (tool results on llama-index's
`aggregate_tool_results`, langgraph's `tools`, semantic-kernel's `execute_tool` spans). That second shape is a part
of one request held elsewhere, not thread history, and is out of scope here.

Outside the corpus, event-per-item telemetry (an agent CLI emitting one event per new conversation item, a realtime
session emitting item-created events) is likely to have the same shape. Nothing is designed for it beyond what the
protocol generalises to.

Not a delta in this sense: OpenAI Responses with `previous_response_id`. There the request on the wire really was
only the new input, and the transcript records exactly that, so the rubric already owes only the delta.

## The protocol, declared in assets

Everything producer-specific is declared in the producer's asset; the engine interprets it.

1. **Delta carrier.** A carrier clause declares that a carrier holds a request delta (a new carrier fact,
   `carrier_holds_request_delta`). The system prompt preview stays an ordinary input carrier: it is the request's
   own, whole or cut.
2. **Thread key.** The rule names the span sources whose values identify a thread: `session.id`, `query_source`,
   `agent_id`. Two requests share a thread when every source agrees, absent included. The key never names the
   trace, so a thread continues across interactions and traces of one session. It is extracted at ingest into a
   derived column beside the span's messages - a cache a re-parse rebuilds, like every extracted column - because
   the read path has the span's messages, not its attributes.
3. **Sequence.** Within a thread, requests are ordered by span start, then a declared tie-break source where the
   producer has one, then span id; never by arrival.
4. **Committed response.** What request *n*'s predecessor produced: its own output carriers
   (`response.model_output`), and the tool calls whose results a later delta of the *same thread* returns, matched
   by call id. Ownership is by result, so the parent's continuation gets its `Agent` call and not the researcher's
   weather calls or its `SubagentHandback`. A batch keeps calls before results; the calls' order is the order their
   tool spans started, declared, and is checked against the results' order where both are known.
5. **Composition.** `messages(n) = align(messages(p) + response(p), delta(n))`, where `p` is the predecessor and
   `align` is occurrence-aware: the leading items of a delta that restate the composed history are matched to the
   occurrences they restate - role-aware, in order, by the same replay matching cross-trace prefix stripping already
   uses (`CrossTracePrefixState`) - and only what follows them is appended. A delta that restates the whole history,
   as a resumed process does, composes to itself. The request's system prompt is its own; system-role messages in
   the history are inherited as messages.
6. **Retries.** An attempt repeats a request: the asset declares the attempt source, and an attempt after the first
   shares its first attempt's predecessor and delta; only the response of the attempt that succeeded is committed.
   No capture has a retry, so this rule ships disabled until one does, and a thread with an attempt after the first
   composes nothing past it.
7. **Forks.** A subagent's thread has a different key, so it starts empty: its first delta is its own first
   message. A fork that imports its parent's history must be declared (a source naming the parent thread and the
   point it forked at); none is captured, so a subagent request never shows the parent's history.
8. **Compaction.** A client that replaces history with a summary starts a new base. The signal is declared once a
   capture shows one; until then the engine has no way to recognise it, which the next section accounts for.

## What the engine cannot know, and does not claim

The composition is evidence, not a guarantee. Nothing in a Claude Code capture proves that a thread is unbroken:
deleting the SDK `multi_turn`'s second request leaves the third apparently well formed and missing its question
and answer, and the counts do not count what a delta holds. So the engine claims no completeness:

- it never invents a message: every composed block is one observation of the thread, in thread order;
- it states where the composition came from (the requests it read), so the view can say "composed from N exported
  requests";
- a thread with a detectable break - an attempt it cannot place, a sequence position two requests share with
  different content - composes nothing past the break.

Whether the composed request is the request is the rubric's question, answered against the transcript, never the
engine's self-assessment. Nothing the engine reports may excuse a part the payloads hold.

## Read-time, rebuilt identically, bounded

Raw telemetry stays the authority. Nothing new is stored but the extracted thread key, which a re-parse rebuilds;
the composition is a function of the thread's rows and the assets, recomputed on read.

Cost, as it would be, not as the span endpoint is today (`get_span_messages` reads one span's rows):

- **Reads.** A span view of a delta request needs its thread's earlier requests. That is one extra query, scoped
  by the derived thread key and the span's sequence position, projecting only the delta, output and tool carriers.
  Its cost is the bytes of those carriers for the thread's prefix - the request's own content plus a per-span
  constant - not the session's rows.
- **Work.** One pass over the thread's requests in sequence, keeping per request a reference to its predecessor's
  composition plus its own appended items, so no request copies its history; materialising the requested span's
  messages is linear in their number, which is the size of the answer.
- **Memory.** The memo is the existing byte-bounded `ReconstructionCache`, keyed by the thread's projected rows.
  It bounds what is retained, not what one read hydrates; a thread whose prefix exceeds the budget is composed in
  a streaming pass that holds one request's items at a time and is not memoised.

The trace and session views already read the thread's rows, so they compose in the same pass at no extra read.
Embedded and distributed backends return the same rows for a thread, so the composition is identical on both; the
backend parity suites gain a delta-producer fixture.

## How the rubric proves it

`check_requests` stays as it is and keeps owing the whole request. On top of it:

- **Occurrence provenance.** Every composed block must be a specific observation - span, carrier and position - of
  an earlier request of the same thread or of a tool span that thread owns, and composed blocks must keep the
  thread's order. Content identity alone is not enough: a block matching the right text from the wrong occurrence
  is `request.provenance`.
- **Thread isolation.** A request showing a block only another thread carried is `request.thread_leak`.
- **No excuse from the engine.** A part the transcript sent and the payloads hold is `request.missing` whatever
  the engine reported about the thread.
- **Mutations** (`server/tests/message_truth/mutations.rs`), each of which must be caught:
  - drop the first, and separately a middle, predecessor's row: what only that row carried must leave the view,
    and nothing may be invented in its place (content a later delta re-exports may stay);
  - drop a tool span a predecessor owns: its call must leave the composed request;
  - duplicate a delta row, and reverse a thread's rows: the view must not change;
  - move a reminder from one request's delta to another's: the composed order must follow;
  - re-export the history in a later delta, as a resumed process does: nothing may be shown twice;
  - give a subagent request the parent's thread key, and a parent request a sibling subagent's tool call: thread
    isolation and ownership must fire;
  - permute a batch's results against its calls: the calls must keep their declared order;
  - put a failed attempt before a successful retry: only the retry's response may be committed.
- **Adversarial truth cases** (`truth-adversarial/cases.json`): a fork that imports nothing, a resumed process, and
  a failed attempt with its retry.

The expected effect on the ledger is the 127 Claude Code `request.missing` entries, with none added; any entry the
change adds is a defect of the change until shown otherwise.

## Open questions

- Retries and compaction have no capture. A long Claude Code session with a forced retry is the capture that would
  settle both, through `make capture P=claude-code`.
- The thread key's sources are Claude Code's; whether another delta producer needs a key the span conditions cannot
  express is for its capture to show.
