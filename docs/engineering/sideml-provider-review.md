# SideML against the current provider APIs

A review of what SideML and its content-block vocabularies read today, against what the providers' APIs
currently send and return, with a sliced plan. The review is accepted; the slices land in the order S2, S3, S1, S4-S8, and S10 goes to the
rule-language track. S9 is documentation only until an instrumentation exports either surface.

## Method and evidence

Every API claim was checked on 2026-10-08 against the provider's own published type definitions, not
memory. The OpenAI documentation site refuses automated fetches (HTTP 403), so for OpenAI, and for the
others too, the evidence is each provider's official SDK, which is generated from the provider's API
specification:

| Provider | Source read |
|---|---|
| OpenAI | [openai-python](https://github.com/openai/openai-python/tree/main/src/openai): the `types/responses` package, the `decisions` resource and the `decision*` types |
| Anthropic | [anthropic-sdk-python](https://github.com/anthropics/anthropic-sdk-python/tree/main/src/anthropic/types): the `*_block` types and the `beta` types |
| Google | [python-genai](https://github.com/googleapis/python-genai/tree/main/google/genai): `types.py` for generateContent, and the `_gaos` interactions types for the Interactions API |
| Bedrock | [botocore](https://github.com/boto/botocore/tree/develop/botocore/data/bedrock-runtime): the `bedrock-runtime` 2023-09-30 service model |
| Mistral | [client-python](https://github.com/mistralai/client-python/tree/main/src/mistralai/client/models): the `*chunk` models |
| OTel GenAI | [semantic-conventions-genai](https://github.com/open-telemetry/semantic-conventions-genai/tree/main/model/gen-ai): the input and output message schemas |

What SideSeat reads was taken from the tree:
- the SideML block types in `server/crates/domain/src/sideml/types.rs`;
- the 87 content-block cases in the eight `content-blocks-*` assets under `server/assets/rules/vocabulary/` and in `server/assets/rules/conventions/semconv.json`;
- the Responses item fragment in `server/assets/rules/producers/langfuse.json`;
- the finish-reason table in `server/assets/rules/vocabulary/finish-reasons.json`;
- the 1,047 captured goldens under `server/tests/fixtures/messages/`.

## Where SideML stands

Entry types across every span, trace and session view of the corpus:

| entry type | count |
|---|---|
| text | 15,681 |
| tool_use | 6,291 |
| tool_result | 5,039 |
| redacted_thinking | 240 |
| thinking | 160 |
| image | 155 |
| document | 134 |
| json | 64 |
| context | 14 |
| unknown | 8 |

- **The unknown entries:** all 8 are Haystack's own image wrapper.
- **The 240 redacted_thinking entries:** these are largely Claude's withheld thinking mislabelled as redacted, which the rubric track's signed-reasoning batch is correcting.
- **What the corpus reaches:** the shapes it holds are read well. The gaps below are shapes the current APIs define that no captured run has produced, so most of them fall through to `unknown`, or are dropped from the view while raw keeps them.

**A constraint that shapes the plan.** The clause gate counts content-block cases and declared members, alongside the message rules (`server/tests/message_goldens_parts/part_06_tests.rs`, `UNOBSERVED` and the shrink-only unreached ledger). So a new case must be reached by a capture when it lands.

Every vocabulary slice is therefore paired with a capture. Live captures are limited to Bedrock. The other route is the harness's fake provider servers in `examples/python/harness/harness/fakes/` (`openai.py`, `anthropic.py`, `google_genai.py`): the real SDK and its real instrumentation run against a scripted response, which is how most suites are captured today. Extending a fake to answer with a new shape is that existing practice; the fixture itself is still captured, not written.

## Gaps by provider

Key: ✓ read today · ✗ not read (falls to `unknown`, or is dropped from the view; raw keeps it).

### OpenAI: Responses API

Items, from `response_input_item_param.py` and `response_output_item.py`, a 28-member output union:

| Item | Today | Proposed SideML |
|---|---|---|
| `message` | ✓ (langfuse `item` fragment) | - |
| `function_call` / `function_call_output` | ✓ | `output` may be a list of `input_text`/`input_image`/`input_file`; check it normalises as blocks |
| `reasoning`: `summary[].summary_text`, `content[].reasoning_text`, `encrypted_content` | ✗: "an encrypted reasoning item matches nothing", and the visible **summary text is dropped** | `thinking`: text from the summary or reasoning text; `signed` when `encrypted_content` is present; a `summary` flag when the text is the summary |
| `custom_tool_call` (`input`: free text) / `custom_tool_call_output` (string or parts) | ✗ | `tool_use` with a string input / `tool_result` |
| `web_search_call` (`action`: `search` with queries/sources, `open_page`, `find_in_page`) | ✗ | `tool_use`, provider-executed |
| `file_search_call` (`queries`, `results[]`: file_id, filename, score, text) | ✗ | `tool_use` + `tool_result`, provider-executed |
| `code_interpreter_call` (`code`, `outputs[]`: `logs` / `image`.url) | ✗ | `tool_use` (code) + `tool_result` (logs, image), provider-executed |
| `image_generation_call` (`result` base64, `revised_prompt`) | ✗ | assistant `image` |
| `computer_call` (`action(s)`, `pending_safety_checks`) / `computer_call_output` | ✗ | `tool_use` / `tool_result` (screenshot image) |
| `local_shell_call(_output)`, `shell_call(_output)`, `apply_patch_call(_output)` | ✗ | `tool_use` / `tool_result` |
| `mcp_call`, `mcp_list_tools`, `mcp_approval_request` / `_response` | ✗ | `tool_use`+`tool_result` (server_label as the tool's origin), `tool_definitions`, approval as a call/result pair |
| `tool_search_call` / output, `program` / `program_output` (programmatic tool calling; `caller` on calls) | ✗ | `tool_use` / `tool_result`; `caller` kept in raw |
| `compaction` (`encrypted_content`), `compaction_trigger` | ✗ | new `compaction` block, signed |
| `item_reference` (`id`), `additional_tools` (developer role, tools) | ✗ | reference rendered as such; `tool_definitions` |

Content parts:
- `input_text` and `output_text` ✓, but `output_text.annotations` are dropped. These are `url_citation`, `file_citation`, `container_file_citation` and `file_path`, i.e. what the model cited.
- `input_image` ✓ by `image_url`, but ✗ by `file_id` alone: the case requires a URL.
- `input_file` ✓ (data, URL, id), `input_audio` ✓, `refusal` ✓.

Chat Completions, message-level members:
- `refusal` beside a null `content` is ✗: the refusal text is lost. `message-members.json` declares no refusal member.
- `audio` (assistant audio output, with its transcript) is ✗.
- `annotations` (url_citation) is ✗.

Finish: `incomplete_details.reason` (`max_output_tokens`, `max_messages`, `content_filter`, `steered`) is ✗ in the finish table.

### OpenAI: Decisions API

It exists: `POST /decisions`, verified in the SDK's `decisions` resource and `decision*` types.
- **Input:** a string, or user messages of `input_text` / `input_image`. Images are data URLs only.
- **Questions:** `predicate`, `choice` or `score`, each with `instructions`.
- **Answers:**
  - `predicate`: probability;
  - `choice`: choice, confidence, probabilities;
  - `score`: score, confidence, probabilities;
  - `refusal`.
- **Usage:** adds `cache_write_tokens` beside `cached_tokens`. That spelling is already in `span-fields-usage.json`.

**Mapping.** The input reads with today's parts. The questions are the request's instructions, a `json` block on the request. The answers are the output: a `json` block, with a `refusal` answer as a `refusal` block.

**Status: no capture path.** Neither shipped OpenAI instrumentation wraps `/decisions`; to be confirmed when a suite is added. It waits for an instrumentation that exports it.

### Anthropic: Messages

From `content_block.py`, `content_block_param.py` and the beta types:

| Block | Today | Proposed |
|---|---|---|
| `text` with `citations` (char / page / content-block / web-search-result / search-result locations) | text ✓, **citations dropped** | `text` + normalised citations |
| `image`, source `base64` / `url` | ✓ | - |
| `image`, source `file` (`file_id`) | ✗ | `image`, source `file_id` |
| `document`, source `base64` / `url` | ✓ | - |
| `document`, source `text` (plain text), `content` (blocks), `file`; `title`, `context` | ✗ | `document` / its content blocks; title kept |
| `search_result` (source, title, content[text], citations) | ✗ | a document-like result block |
| `thinking` / `redacted_thinking` | ✓ (signed under way in the rubric track) | - |
| `server_tool_use` (`web_search`, `web_fetch`, `code_execution`, `bash_code_execution`, `text_editor_code_execution`, `tool_search_tool_regex` / `_bm25`) | ✗ | `tool_use`, provider-executed |
| `web_search_tool_result`, `web_fetch_tool_result`, `code_execution_tool_result`, `bash_code_execution_tool_result`, `text_editor_code_execution_tool_result`, `tool_search_tool_result` | ✗ | `tool_result`, provider-executed |
| `container_upload` (`file_id`) | ✗ | `file`, source `file_id` |
| beta `mcp_tool_use` (server_name) / `mcp_tool_result` | ✗ | `tool_use` / `tool_result` |
| beta `compaction` (content, encrypted_content, signature) | ✗ | `compaction` |
| `tool_reference` | ✗ | reference |

Stop reasons not in the finish table: `pause_turn`, `refusal`, `model_context_window_exceeded`.

### Google: Gemini generateContent

From the `Part` fields in `google/genai/types.py`:

| Part member | Today | Proposed |
|---|---|---|
| `text`, `thought` | ✓ | thought text marked as a summary (Gemini returns thought *summaries*) |
| `thought_signature` (on any part, including function calls) | ✗, ignored | `signed` on the thinking it seals; kept in raw |
| `inline_data` (camel and snake) | ✓ | - |
| `file_data` snake | ✓ | - |
| `fileData` camelCase | ✗: only the snake spelling is read, though `inlineData` has both | camelCase case |
| `function_call` (`id`, `args`, `partial_args`, `will_continue`) | ✓, ids read | the asset doc "Gemini's own API states no call id" is stale: `FunctionCall.id` exists |
| `function_response` (`response`, **`parts[]`** of inline/file data, `scheduling`) | ✓ response, ✗ parts | multimodal result content |
| `executable_code` (`code`, `language`, `id`) / `code_execution_result` (`outcome`, `output`, `id`) | ✗ | `tool_use` / `tool_result`, provider-executed; `OUTCOME_FAILED` sets `is_error` |
| `tool_call` / `tool_response` (`tool_type`: `GOOGLE_SEARCH_WEB`, `GOOGLE_SEARCH_IMAGE`, `URL_CONTEXT`, `GOOGLE_MAPS`, `FILE_SEARCH`, `MEDIA_PROCESSING`) | ✗ | `tool_use` / `tool_result`, provider-executed |
| `audio_transcription` | ✗ | text (what was said) |
| `video_metadata`, `media_resolution`, `part_metadata` | dropped | keep in raw only |

Finish reasons not in the table:
- `SAFETY` and `RECITATION` are present, but these are not: `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY`, `IMAGE_PROHIBITED_CONTENT`, `IMAGE_RECITATION`, `IMAGE_OTHER`, `NO_IMAGE`.
- `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `TOO_MANY_TOOL_CALLS`, `LANGUAGE`, `CONTINUATION`.

### Google: Interactions API

From the SDK's `_gaos` interactions types. It is a new surface with typed `steps`:
- `user_input` and `model_output`;
- `thought` (`signature`, and a `summary` of text or image);
- `function_call` (`arguments`, `id`, `name`) and `function_result` (`call_id`, `result`, `is_error`, `name`);
- call/result pairs for `code_execution`, `google_search`, `url_context`, `google_maps`, `file_search`, `mcp_server_tool` (with `server_name`), `retrieval` and `processing`.

Its contents are `text` (with `url_citation` / `file_citation` / `place_citation` annotations) and `image` / `audio` / `video` / `document` (`data` or `uri`, plus `mime_type`).

**Mapping.** It is OpenAI-like (type-discriminated) and maps one to one onto SideML: calls are `tool_use`, results are `tool_result`, the hosted ones provider-executed, and a thought is signed thinking.

**Status: no capture path yet.** It needs an instrumentation that exports interactions, and an `/interactions` route in the fake Gemini server.

### Bedrock: Converse

From `ContentBlock` in the service model:

| Member | Today | Proposed |
|---|---|---|
| `text`, `json`, `toolUse`, `toolResult`, `reasoningContent` (text with `signature`; `redactedContent`) | ✓ | signature → `signed` (rubric track) |
| `image` / `document` / `video` with `source.bytes` | ✓ | - |
| the same with `source.s3Location` | ✗ | media, source URL (`s3://`) |
| `document` with `source.text` / `source.content` | ✗ | document text or blocks |
| `audio` (new `AudioBlock`) | ✗ | `audio` |
| `citationsContent` (`content`: the model's generated text, plus `citations`) | ✗: **the model's answer itself falls to unknown** when citations are on | `text` + citations |
| `searchResult` | ✗ | as Anthropic's |
| `guardContent` (guarded user text or image) | ✗ | text / image |
| `toolResult.content` holding `image` / `document` / `video` / `searchResult` | check | result blocks |
| `toolAddition` / `toolRemoval` | ✗ | tool-definition changes |
| `cachePoint` | ✓ | - |

- **Usage:** `cacheReadInputTokens` / `cacheWriteInputTokens` (camelCase) are not in `span-fields-usage.json`.
- **Stop reasons not in the finish table:** `guardrail_intervened`, `content_filtered`, `malformed_model_output`, `malformed_tool_use`, `model_context_window_exceeded`.

### Mistral

From `*chunk.py`:
- `text` ✓.
- `image_url` ✓, through the OpenAI case.
- `thinking` ✓, but its `signature` is dropped.

Not read:
- `input_audio`: Mistral's is a **string**, while OpenAI's is an object. The OpenAI case reads `$.input_audio.data`, so it misses.
- `document_url` (with `document_name`).
- `file` (`file_id`): the OpenAI `file` cases require a `$.file` object.
- `reference` (`reference_ids`), `tool_file`, `tool_reference`, `resource`, `resource_link`.

**Capture:** there is no fake Mistral server, and Bedrock's Mistral models speak Converse, not these chunks.

### OTel GenAI conventions

From the conventions' input message schema. The convention most instrumentations are converging on:

| Part | Today | Proposed |
|---|---|---|
| `text`, `tool_call`, `tool_call_response`, `reasoning` | ✓ | - |
| `blob`, `uri` | ✓, but `modality` (`image`/`video`/`audio`/`document`) is unread, so without a `mime_type` the kind is guessed | read `modality` for the kind |
| `file` (`file_id`, `modality`, `mime_type`) | ✗ | media, source `file_id` |
| `compaction` (`id`, `content`) | ✗ | `compaction` |
| `server_tool_call` (`name`, `server_tool_call`: `{type, …}`) / `server_tool_call_response` | ✗ | `tool_use` / `tool_result`, provider-executed |

These four are the cheapest high-value cases: one convention, and every compliant instrumentation benefits.

## SideML type changes proposed

These are this track's to make, one approved slice at a time, with no API compatibility kept and the web types and rendering changed in the same slice:

1. **`thinking`:** build on the rubric track's `signed: true`; don't redo it. Add `summary: bool` for text that is a *summary* of reasoning (OpenAI `summary_text`, Gemini thought summaries), so a summary is never shown as the model's verbatim reasoning.
2. **`text`:** add optional `citations`, normalised to `{kind: url|file|document|search_result|place, title, url | file_id, start, end, cited_text}`. Raw keeps each provider's full location object.
3. **`tool_use` / `tool_result`:** add `provider_executed: bool` for hosted and server tools. They keep pairing, ordering and the rubric's call/result checks; the view can say "run by the provider".
4. **A new `compaction` block** (`summary: Option<String>`, `signed: bool`): the conversation state a provider compacted. It is not `context`, which is history a framework re-sends.
5. **Media sources:** `file_id` exists today. Add `s3` / `gs` URIs as `url`.

Nothing here names a provider in Rust. Every shape is read by vocabulary cases, and the types are SideML's own.

## Optimisation

**Measured**, on a machine at load average 97-141, so the figures are indicative only:
- normalising a content block costs about **2.8-5.0 µs**;
- every position walks its cases linearly, at the provider-formats position all 47 of them in the worst case;
- 52 of the 87 cases are discriminated by a top-level `$.type` `one_of`.

**Proposal: an index.** At compile time, index each position's cases by their declared `$.type` values. At run time, read `type` once and walk only the priority-ordered merge of that bucket and the undiscriminated cases.
- **Why the answers cannot change:** the order is preserved, so the same case answers.
- **Proof:** an equivalence test over every block in the corpus (indexed against linear), plus a property test.
- **Measurement:** re-measure on a quiet machine before and after.

This is engine code, so it goes to the rule-language track.

**Cleanup.** The OpenAI Responses item fragment lives in `server/assets/rules/producers/langfuse.json`, and openai-agents and logfire read it as `langfuse.item`. It is OpenAI wire vocabulary, so it belongs under `server/assets/rules/vocabulary/`, beside `content-blocks-openai.json`.

## Plan

Every slice follows the same discipline:
- **Changes:** the vocabulary and the SideML type change, if any, plus a capture that reaches every new case: a fake-server extension, or a live Bedrock run.
- **Truth:** the truth decoders in `examples/python/harness/harness/truth/` learn the new shape, in the rubric track's harness and parser-independently.
- **Checks:** goldens and truth are unchanged or better, `make verify-head ARGS=--test` is green, and a Codex round follows.

| # | Slice | Capture | Owner |
|---|---|---|---|
| S1 | OpenAI `reasoning` items: summary → thinking (summary); encrypted, re-sent → signed thinking with no text; move the Responses fragment to the vocabulary | **already in the corpus**: 25 logfire fixtures carry `encrypted_content`, and azure-openai carries `summary_text` | me, after the rubric track's signed change lands |
| S2 | Finish-reason spellings from all five providers, plus Bedrock's camelCase cache usage | unit tests on the table, if finish spellings sit outside the clause gate (to confirm); otherwise with S4-S7 | me |
| S3 | OTel `file`, `compaction`, `server_tool_call(_response)`, `modality` | an instrumentation that emits them, against a fake server | me + rubric (truth) |
| S4 | Anthropic server tools, citations, `search_result`, document sources, `container_upload`, MCP and compaction blocks | extend fake-anthropic; capture the anthropic suites offline | me + rubric |
| S5 | OpenAI hosted-tool and custom-tool items, annotations, `input_image` by file id; Chat Completions message `refusal` / `audio` | extend fake-openai (`/v1/responses` exists); capture the openai and openai-agents suites | me + rubric |
| S6 | Gemini code execution, `fileData` camelCase, `function_response.parts`, `thought_signature`, server tool invocations, transcription | extend fake-gemini; capture google-genai, vertex-ai and adk | me + rubric |
| S7 | Bedrock `citationsContent`, `searchResult`, document text/content sources, `toolResult` media | live Bedrock Converse; citations on Claude to be confirmed at capture. s3 and guardrails are skipped, because they need AWS writes | me |
| S8 | Mistral chunks | needs a fake Mistral server, or is parked | me, if approved |
| S9 | Gemini Interactions, OpenAI Decisions | waits for an instrumentation; record in `docs/engineering/integration-landscape.md` | me |
| S10 | Content-block type index | none, corpus equivalence | rule-language track |

**Order of value:**
1. S1 is visible text lost today, with captures in hand.
2. S3 and S4 matter most for agent frameworks: server tools and citations.
3. S5 and S6 next.
4. S2 is a quick fidelity win.
