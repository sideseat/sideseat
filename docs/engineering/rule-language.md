# The rule language

How to read and write the assets under `server/assets/rules/`. This guide is enough to support a new framework
without reading the engine. The reference at the end lists every key. It is generated from
`server/assets/rules.schema.json`, which the engine generates from its own types, so neither can drift from what
the engine accepts. `framework-rules-engine.md` explains why the engine is built this way; this document explains
how to use it.

Every block below marked as an example is a complete asset, and `every_rule_language_example_compiles` compiles
each of them on its own, so none of them can show a spelling the engine refuses.

## Assets

An asset is one JSON file with a `$schema` pointing at `server/assets/rules.schema.json` (relative to the
asset, so editors validate it), an `id`, a `doc`, and any of the sections below. The examples here leave the
`$schema` out, since they are not files in that directory. `conventions/` holds the published conventions,
`producers/` holds one file per framework or provider, and `vocabulary/` holds tables several producers share.
Clauses carry an `id`, unique within the corpus or within the clause that holds it, so a diagnostic and the explain
trace can name them; a few table entries (`provider_aliases`, for one) are keyed by what they map instead. A
`doc` is optional almost everywhere and expected everywhere: it says why the clause is written the way it is.

| Section | What it decides |
| --- | --- |
| `detect` | which producer a span came from; its `label` fills the `framework` column |
| `carriers` | what an attribute or event that holds messages is evidence of: ordering, history, direction |
| `messages` | which carriers a span's messages are read from, and how each becomes a message |
| `message_events` | which span events carry messages |
| `log_events` | which OTLP log records carry one of those events, and where each keeps its attributes |
| `event_roles` | which role a carrier's source name implies |
| `role_authority` | how far a role a producer states is believed |
| `message_projections` | which stored bookkeeping rows a conversation omits |
| `message_members` | which member names a producer uses, and what their presence means |
| `content_blocks` | how a provider's content block becomes a SideML block |
| `tool_shapes` | how a provider writes a tool definition |
| `span_fields` | where each stored span field comes from |
| `span_facts` | facts about a span any dialect can establish, such as that it runs a tool |
| `observation_types` | what kind of observation a span is |
| `span_categories` | the category a span is filed under |
| `event_categories` | the category an event's name establishes |
| `finish_reasons` | what each spelling of a finish reason means |
| `provider_aliases` | a framework name written where a provider is expected |
| `synthetic_call_ids` | separators of the call ids producers build as `{name}<separator>{index}` |
| `convention_namespaces` | attribute namespaces a published convention owns |
| `sdk_slugs` | the slugs SideSeat's own SDKs declare |
| `fragments` | named sets of readings several message rules share |

Some sections are owned by one asset: `role_authority`, the meanings in `finish_reasons` and
`convention_namespaces` are declared once, and a producer adds spellings or sources to them rather than a second
table.

## Sources

A source names where a value is.

- `span_name`: the span's name.
- `attr:<key>`: a span attribute.
- `attr_keys`: the set of the span's attribute keys, asked about existentially (some key starts with ...).
- `scope.name`: the instrumentation scope's name.
- `resource:<key>`: a resource attribute.
- A JSONPath (RFC 9535) such as `$.content[0].text`: a member of a parsed value.

A field that can only name an attribute takes the bare key, because the field already says what it is:
`"attribute": "gen_ai.input.messages"`.

Wherever a value may come from several places, a field takes one source, or `{"first_of": [...]}` with two or
more. A list reads in one of two modes. **Present** (`present`), the default, commits to the first candidate that
is there, whatever it holds: two spellings in one payload are one producer's statement, so a badly written primary
does not hand over to an alias. **Usable** (`usable`, written `"mode": "usable"`) steps over a candidate it cannot
read at all: one whose path selects nothing, whose pipe step cannot apply, or (for a call's input) an empty
object. It does not check the type the field needs, so a present value of the wrong kind still ends the list and
the case declines. A field that reads one way states that mode and refuses the other, so `{"first_of": [...]}`
alone always means first present.

A message rule that reads every one of several keys as its own observation says `every`; that form exists for
`tool_repr` readings only.

## Conditions about a span

Detection, classification, span facts, span-field sources, message-rule gates, a compose member's fallback and
message projections ask their question about a span with one `where`. (Carrier clauses match an observation with
`match` instead: an exact event or attribute, an attribute prefix or dotted family, and optionally its observation
type. A span-field source may also be admitted by `when_json`, a JSON member's presence.)

An atom names a `source` and the tests asked of the value it selects:

| Test | Holds when the value | Sources that answer it |
| --- | --- | --- |
| `exists` | is there (`true`) or is not (`false`); never unknown | `attr:<key>` |
| `equals` | is exactly this text | `span_name`, `attr:<key>`, `scope.name` |
| `equals_ignore_case` | is this text, ignoring ASCII case | `attr:<key>` |
| `one_of` | is one of these texts | `span_name`, `attr:<key>`, `scope.name` |
| `starts_with` | begins with this text (for `attr_keys`, some key does) | `span_name`, `attr_keys`, `scope.name` |
| `contains` | contains this text | `attr:<key>`, `resource:<key>` |
| `contains_ignore_case` | contains this text, ignoring case | `span_name`, `attr:<key>` |
| `parses` | parses in the named encoding (`json`); false where it does not, so a value cut short by a length limit does not hold | `attr:<key>` |
| `version` | is a release in `[at_least, below)` of the stated `scheme` (`pep440` or `semver`) | `scope.version` |

Atoms combine with `all` (`{"all": [...]}`), `any` (`{"any": [...]}`), each with two or more members, and `not`
(`{"not": ...}`), and any of these may carry a `doc`. The logic is strong-Kleene: a value test on an absent value
is **unknown**, `not` of unknown is unknown, and a rule applies only where its condition is true. So `{"not":
{"source": "attr:k", "equals": "x"}}` does not hold on a span without `k`; write `{"source": "attr:k", "exists":
true}` beside the test to make that span false instead.

`version` is the last resort: a shape test says what changed, a version only when, so a range states `because` -
why no shape test can say this. It is asked alone in its atom, of `scope.version` only, which nothing else may
read; an absent or unparseable version is unknown, never "the latest". The range is an ordered interval in the
package's scheme, with no requirement-matcher policy: `7.0rc1` is below `7`, and a range that should include a
series' pre-releases starts at its first one (`6.dev0` in PEP 440). Only message projections are given the scope's
version today.

A phrase search may read several sources at once: with `contains_ignore_case` and nothing else, `source` may be
a list of two or more `span_name` and `attr:<key>` sources (the atom holds when one that has a value contains the
phrase) or `{"first_of": [...]}` of them (only the first that has a value is searched).

Each section can see only some sources: detection sees all of them but the scope's version; a message rule's gate
sees the span and its scope; classification and span-field sources see the span's name and attributes; span facts
see the attributes; a message projection sees a stored row's span name and its scope, version included, and no
attributes.
These are refused when the asset compiles: a condition that reads a source its section cannot see, a test its
source cannot answer, an empty prefix, substring, scope name or search phrase, and a disjunct its own group
already covers.

```json example
{
  "id": "acme-detect",
  "doc": "Detection: a scope, else an attribute namespace at a later priority of its own.",
  "detect": [
    {
      "id": "acme.detect",
      "doc": "The instrumentation names itself.",
      "label": "Acme",
      "priority": 9100,
      "where": {"source": "scope.name", "equals": "acme.instrumentation"},
      "alternatives": [
        {
          "id": "namespace",
          "doc": "Older releases leave the scope unnamed but write their own namespace.",
          "priority": 9101,
          "where": {
            "all": [
              {"source": "attr_keys", "starts_with": "acme."},
              {"not": {"source": "span_name", "contains_ignore_case": "proxy"}}
            ]
          }
        }
      ]
    }
  ]
}
```

```json example
{
  "id": "acme-classify",
  "doc": "Classification reads the span's name and attributes.",
  "observation_types": [
    {
      "id": "acme.observation.tool",
      "doc": "A span that states its kind, in any spelling the producer used.",
      "priority": 9100,
      "where": {
        "any": [
          {"source": "attr:acme.kind", "equals_ignore_case": "tool"},
          {"source": "attr:acme.type", "one_of": ["TOOL", "FUNCTION"]},
          {"source": ["span_name", "attr:acme.label"], "contains_ignore_case": "tool call"}
        ]
      },
      "result": "tool"
    }
  ]
}
```

## Conditions about a value

A condition on a JSON value is a `where` in the same grammar, over atoms that name a `path` into the value (absent:
the value itself) and the tests asked of what it selects:

| Test | Holds when the selected value |
| --- | --- |
| `exists` | is present (`true`) or absent (`false`) |
| `kind` | is an `object`, `array`, `string`, `number`, `bool` or `null` |
| `non_empty` | is a string, array or object that is not empty |
| `non_blank` | is a string holding something other than whitespace |
| `not_null` | is not JSON null |
| `identifier_like` | begins with a letter, a digit or an underscore |
| `starts_with` | is a string starting with this |
| `lacks_prefix` | is not a string starting with this (any non-string holds) |
| `one_of` | is one of these strings |
| `none_of` | is none of these strings (any non-string holds; alone, so does an absent member) |
| `equals` | is exactly this JSON value; `null` is not a test, so ask `kind: "null"` |
| `only_members` | is an object with no member outside these |

Every test of one atom is asked of one selected value, so `{"path": "$.items[*]", "starts_with": "a", "one_of":
[...]}` needs a single item satisfying both. A test of the wrong kind for the value (a string test on a number) is
unknown, and `"exists": false` beside it makes it false. Where the subject is not the value being read, the field
says so: `parent_where` (the value a selection came out of), `skip_where` (a section dropped where it holds),
`raw_where` (a carrier's text, as a JSON string, before it is parsed).

## Transforms

What a section does to a value after reading it is one vocabulary, `pipe`: an ordered list of steps, each a
name or a one-operation object.

| Step | Does |
| --- | --- |
| `lowercase` | folds text to lower case |
| `trim` | removes leading and trailing whitespace |
| `strip_bracket_tag` | removes a leading `[TAG]` line |
| `blank_is_absent` | treats a blank attribute as absent |
| `strip_prefix`, written `{"strip_prefix": p}` | removes `p` from the start, and is absent without it |
| `map`, written `{"map": {...}, "closed": true}` | replaces a listed value; an open map passes others through, a closed one makes them absent |
| `join`, written `{"join": s}` | joins every string the path selected with `s` |
| `parse`, written `{"parse": mode}` | decodes text the way a carrier is decoded; anything else passes through |
| `prepend`, written `{"prepend": p}` | puts `p` in front of a string |

Each section runs only some steps, in one order, and refuses a pipe that states anything else when the asset
compiles:

| Where | Steps it runs |
| --- | --- |
| span-field source | `[]` or `["lowercase"]` (text and list targets only); with `span_name: true`, also `[{"strip_prefix": p}]`, optionally then `"lowercase"` |
| message attachment | `blank_is_absent`, `strip_bracket_tag`, then `lowercase` or a closed one-entry `map` (a flag); the first two and the flag need `from`, a flag answers before any `parse` or `select` and so cannot sit beside them or beside a `value`, and `lowercase` folds an attribute or a `from_path` value, never a `from_value`. Every source reads, then `where` is asked of what it read, then a literal replaces it |
| attachment's span-name fallback, `or_span_name` | `["trim"]` or `[{"strip_prefix": p}, "trim"]`; a name blank after it supplies nothing |
| reading (`alternatives`, `also`, `fallback`) | `["trim"]`, applied before the reading's `where` |
| a wrap's `role_from` | one `map`, open or closed, whose outputs are roles |
| content-block member | exactly one of `join`, `parse`, `prepend`, or a closed `map` |

```json example
{
  "id": "acme-blocks",
  "doc": "Content blocks: a recognised shape, and the canonical block it becomes.",
  "content_blocks": [
    {
      "id": "acme.reasoning_part",
      "doc": "Reasoning as a list of chunks; the text ones are joined in order.",
      "at": "provider_formats",
      "priority": 9100,
      "where": {
        "all": [
          {"path": "$.type", "one_of": ["acme_reasoning"]},
          {"path": "$.chunks", "kind": "array", "non_empty": true}
        ]
      },
      "thinking": {
        "text": {"path": "$.chunks[?@.kind == 'text'].text", "pipe": [{"join": "\n\n"}]}
      }
    },
    {
      "id": "acme.tool_result_part",
      "doc": "A tool result whose failure is a status rather than a flag; the call id has two spellings.",
      "at": "provider_formats",
      "priority": 9101,
      "where": {"path": "$.type", "one_of": ["acme_result"]},
      "tool_result": {
        "tool_use_id": {"first_of": ["$.call_id", "$.id"], "mode": "usable"},
        "content": "$.output",
        "is_error": {"path": "$.status", "pipe": [{"map": {"error": true}, "closed": true}]}
      }
    }
  ]
}
```

## Span fields

Each stored field has exactly one resolver in the whole corpus, in `vocabulary/span-fields-*.json`: a list of
`sources` tried in order. A producer that writes a field under its own key adds a source to that resolver (with a
`where` gate where the key is not unambiguous), rather than declaring a second resolver. A source reads one place:
an `attribute` (or `{"first_of": [...]}` of spellings), a `json` member of an attribute, an `event_attribute`, the
`span_name`, or a literal `value` that a gated shape implies. `on_malformed` says whether a present but unreadable
value stops the chain (`stop`, the default for a `first_wins` field) or hands over to the next source
(`continue`); a field that combines with `merge_all` reads every source regardless, and a malformed `when_json`
stops a `first_wins` chain. `accept_empty` makes an empty value an answer rather than something to step over.

```json example
{
  "id": "acme-fields",
  "doc": "A resolver, as the shared vocabulary declares one; compiled here on its own.",
  "span_fields": [
    {
      "id": "acme.tool_name",
      "doc": "The tool a span ran.",
      "target": "gen_ai_tool_name",
      "sources": [
        {"id": "attribute", "doc": "Two spellings of one key; the first present answers.", "attribute": {"first_of": ["acme.tool", "acme.tool_name"]}},
        {"id": "payload", "doc": "Inside the serialised call.", "json": {"attribute": "acme.call", "path": "$.function.name"}},
        {"id": "span_name", "doc": "The span is named after the tool.", "span_name": true, "pipe": [{"strip_prefix": "run "}]}
      ]
    },
    {
      "id": "acme.finish_reasons",
      "doc": "Written in upper case by one release.",
      "target": "gen_ai_finish_reasons",
      "sources": [
        {"id": "attribute", "doc": "Folded: the stored form is lower case.", "attribute": "acme.finish", "pipe": ["lowercase"]}
      ]
    }
  ]
}
```

## Messages

A message rule has an `id`, a `priority`, exactly one way to read (`read` names one carrier; a `compose` rule
reads several attributes and a `branch_set` parent reads through its sub-rules), a `parse` for a scalar carrier
(`json`, `json_or_string`, `text`, and the other modes in the reference), and an `emit` target: `message`,
`tool_definitions`, `tool_names`, or `claim`, which takes the carrier off the table and emits nothing. A gate
`where` decides on which spans the rule applies, and `reads_tool_spans: true` lets it read a span that runs a
tool, which it otherwise skips. A rule that reads span events says `"source": {"event": {"names": [...]}}`, every
name declared in some `message_events`; its reads see the event's attributes and its gate the parent span.

The decoded value becomes messages through:

- `wrap`: the value, or the member `content_from` names, inside an envelope with a `role` (or `role_from`, a
  path whose value is the role, optionally through a role `map`), literal `members`, and `attach`ments taken from
  the value being wrapped (`from_value`), the payload (`from_path`), a sibling attribute (`from`), the span name
  (`or_span_name`), or a `default`, tried in that order. A `block` builds one content block instead of plain
  content: `tool_use` with its arguments under `input` (`"content_as": "input"`) and its `name` and `id` attached,
  or `tool_result` with its `tool_use_id` attached;
- `alternatives`: ordered readings, tried until one yields. A reading may `select` a path (absent: the value
  itself), read `each` element of an array, require a `where`, carry its own `wrap` and `emit`, apply a fragment's
  cases (`then_fragment`, naming `<asset-id>.<fragment>`) and add local ones (`extra_cases`). `also` readings all
  contribute; `fallback` readings all contribute where nothing else in the rule built anything;
- `sections`: the raw text split on `split_on` into `[TAG]` sections, each routed by its tag to a role, before
  any parsing;
- `compose`: one message assembled from several attributes;
- `elements` for an array read in passes, `walk` to apply the readings at every node of a bounded tree walk,
  `branch_set` for sub-rules with a local order, and `tool_repr` for a language's `repr` of tool schemas.

A section body, a selected value and an attached attribute are the producer's bytes: nothing is trimmed unless a
`pipe` says so. Every role a rule states, in any envelope or section route, must be a role.

Some producers re-send the turns of a tool loop as text in every request - the call written out as prose, the
result quoted back - while the call and the result are also on record losslessly, in the model's output and on
the tool's own span. A reading marks those messages with `rendering`, a value condition asked of the same value
as its `where` (on an indexed family's read, of each assembled entry). A rendering is shown on the span
that sent it, because it is what was sent, and is left out of the trace and session views, which already hold the
call and its result; nothing else about it changes - it is owned, ordered and counted on its span as any message
is. A fragment case's own `rendering` adds to its selection point's. `rendering` is refused on a reading that
does not emit messages, inside an aggregate, and on a read that is not an indexed family.

```json example
{
  "id": "acme-messages",
  "doc": "Messages: a carrier, its decoding, and the readings that yield messages.",
  "carriers": [
    {"id": "acme.history.carrier", "doc": "The conversation so far, re-sent with every request.", "match": {"attribute": "acme.history"}, "facts": {"preset": "snapshot", "carrier_holds_span_input": true}},
    {"id": "acme.arguments.carrier", "doc": "One call's arguments, written once.", "match": {"attribute": "acme.tool.arguments"}, "facts": {"preset": "emission"}}
  ],
  "messages": [
    {
      "id": "acme.history",
      "doc": "The conversation, as a list of turns or a bare answer beside it.",
      "read": {"attribute": "acme.history"},
      "parse": "json",
      "where": {"source": "scope.name", "equals": "acme.instrumentation"},
      "emit": "message",
      "priority": 9100,
      "alternatives": [
        {
          "id": "turns",
          "doc": "Each turn already carries its role; the tool loop it re-sends as text is a rendering.",
          "select": "$.turns",
          "each": true,
          "where": {"all": [{"path": "$.speaker", "kind": "string"}, {"path": "$.text", "exists": true}]},
          "rendering": {"path": "$.speaker", "one_of": ["tool-call", "tool-response"]},
          "wrap": {
            "role_from": {"path": "$.speaker", "pipe": [{"map": {"bot": "assistant"}}]},
            "content_from": "$.text"
          }
        },
        {
          "id": "answer",
          "doc": "Only a string: anything structured is state, not a reply.",
          "select": "$.answer",
          "pipe": ["trim"],
          "where": {"kind": "string"},
          "wrap": {"role": "assistant"}
        }
      ]
    },
    {
      "id": "acme.tool_call",
      "doc": "A tool call, as a tool_use block whose name falls back to the span name.",
      "read": {"attribute": "acme.tool.arguments"},
      "parse": "json_or_string",
      "emit": "message",
      "priority": 9101,
      "reads_tool_spans": true,
      "wrap": {
        "role": "assistant",
        "block": {
          "type": "tool_use",
          "content_as": "input",
          "attach": [
            {"from": "acme.tool.name", "as": "name", "or_span_name": [{"strip_prefix": "run "}, "trim"], "default": ""},
            {"from": "acme.tool.call_id", "as": "id", "pipe": ["blank_is_absent"]}
          ]
        }
      }
    }
  ]
}
```

```json example
{
  "id": "acme-sections",
  "doc": "A text carrier of tagged sections.",
  "messages": [
    {
      "id": "acme.transcript",
      "doc": "Sections separated by a rule line; a tool result is tagged with its call id.",
      "read": {"attribute": "acme.transcript"},
      "parse": "text",
      "where": {"source": "scope.name", "equals": "acme.instrumentation"},
      "emit": "message",
      "priority": 9100,
      "sections": {
        "split_on": "\n\n---\n\n",
        "routes": [
          {"id": "tool_result", "tag_prefix": "RESULT:", "role": "tool", "block": {"type": "tool_result", "capture_as": "tool_use_id"}},
          {"id": "as_user", "role": "user"}
        ]
      }
    }
  ]
}
```

## Carriers and facts

A carrier clause says what an attribute or event that holds messages is evidence of: whether a position proves a
distinct occurrence, whether it orders the messages, whether it restates earlier ones, whether it holds the span's
input or output, whether it is a detached request frame. A `preset` answers all of them (`emission`: each position
a distinct occurrence of the span's output; `snapshot`: a conversation re-listed in order; `accumulated_state`:
framework state that restates earlier messages), and named overrides change one; every preset leaves "holds the
span input" and "holds an expandable message array" false unless stated. Declare a clause for every carrier a
message rule reads: an undeclared carrier is read as a snapshot, the cautious default. A span fact is established
by any of the `signals` of any asset.

```json example
{
  "id": "acme-carriers",
  "doc": "Carrier semantics and a span fact.",
  "carriers": [
    {
      "id": "acme.history.carrier",
      "doc": "The whole conversation so far, re-sent on every request.",
      "match": {"attribute": "acme.history"},
      "facts": {"preset": "snapshot", "carrier_holds_span_input": true}
    }
  ],
  "span_facts": [
    {
      "id": "acme.tool_execution",
      "doc": "A span that runs a tool states its call id.",
      "fact": "tool_execution",
      "signals": [
        {"id": "call_id", "doc": "Only a running call carries one.", "where": {"source": "attr:acme.tool.call_id", "exists": true}}
      ]
    }
  ]
}
```

## Precedence and claiming

Order is stated, not inferred from how specific a condition looks, with one exception: carrier clauses, whose
match keys form a lattice, are ordered most specific first by subsumption.

- **`priority`**, an integer, lowest first, orders every other arena: detection rules and their alternatives,
  message rules, observation types, span and event categories, the cases of one content-chain position, tool
  shapes, and each ordered question of `message_members`. Two clauses of one arena with the same priority are
  refused. For message rules an arena is pairwise: two rules contend when they share an output axis and can meet.
  `message` and `claim` are one axis, and two rules on it meet when they run at the same stage and both read span
  attributes, or both read events whose names intersect. `tool_definitions` (a `repr` grammar included) and
  `tool_names` are an axis each, and two span rules on one meet at any stage, because the metadata path runs every
  stage. Which carriers they read does not enter into it.
- **`supersedes`** names the rules a detection clause is meant to beat where both match. It documents an overlap
  and is checked (each target exists, differs from the source, is named once, and comes later by priority). It
  never decides a label: deleting one changes no detection answer, only which overlaps the diagnostics report.
- Some clauses that could never win are refused: a literal another clause of its arena covers at an earlier
  priority, and a disjunct its own group covers. Coverage is decided by sound but incomplete implication over the
  conditions, so a refusal is never a guess, and a clause it cannot prove dead is accepted.
- **Claiming.** Message rules are consulted in priority order, and on each output axis a carrier is read by one
  rule: the first rule whose reading yields owns what that reading consumed, and a later rule's emission is
  dropped whole if it would read any carrier already owned. One rule may emit many observations from its own
  carrier. A gate that merely matches owns nothing, so a rule that holds but reads nothing leaves the carrier to
  the next; `emit: "claim"` still needs a reading that succeeds. Rules with `"source": {"span": {"stage":
  "fallback"}}` run where no other rule produced a message or a claim, and also where a generation span's answer
  is still unaccounted for, reading then only carriers no rule owns.
- Content-block cases are tried by `priority` within their position (`at`); the first that builds its target
  wins, and a case that recognises a block and cannot build it declines to the next, except that a matching
  `unwrap` whose member cannot be normalised ends its position. A `splice` is decided by the first envelope case
  that recognises the block.

## Refusals and diagnostics

A defective ruleset does not load. `Ruleset::build` compiles every section, even after one fails, and reports
`RulesetDiagnostics`: one entry per failing section naming the section, the clause ids involved, their assets
where known, and the reason, for example a duplicate priority, an unknown `supersedes` target, a condition
reading a source its section cannot see, a pipe step its section does not run, or a role that is not a role.
Within a section the first defect is reported, because later checks assume the earlier ones held; `log_events`
are compiled only once `message_events` have, and observation types and span categories are reported as one
section. Unknown keys are refused by every clause, so a misspelt or retired key fails rather than being ignored.

At run time a span-field source that matched and could not be used records a refusal (empty, malformed, wrong
member, out of range) beside the answer. A message reading that fails, or a content block that cannot be built,
yields nothing, and the next reading or case is tried.

## Adding a framework

1. Capture it: an example suite under `examples/<language>/<framework>/` with `native/` and `sdk/` modes,
   recorded with `make capture P=<framework>`.
2. Add `producers/<framework>.json` with a `detect` clause and the `carriers`, `messages` and `content_blocks`
   its telemetry needs, and add its keys as sources to the shared span-field resolvers. Reuse the conventions and
   vocabulary assets wherever its payloads already follow them; add only what differs.
3. Replay it with `make capture-offline P=<framework>` and review the views with
   `scripts/fixtures/review-goldens.py`. Every reconstructed conversation must meet the rubric in
   `server/tests/fixtures/messages/README.md` and match its truth under `server/tests/fixtures/truth/`.
4. If the language cannot say what the telemetry means, extend it generically: a new operator any asset can
   use, never a Rust branch that names the framework. The sweeps refuse framework names, framework attribute
   keys and producer words in production Rust.

## Adding a version

A release that writes a different shape gets one new branch where the shape differs: another `alternative`,
another `first_of` candidate, or another clause at its own priority, whose `where` recognises the new shape. No
asset is copied, and an unchanged clause gets nothing. Shape is the authority, so the same branch serves every
release that writes that shape. Record the release as a variant in the suite's `versions.toml` and capture its
fixtures, so the branch is exercised. Where nothing in the payload tells two releases apart, a `version` test on
the scope's version is the last resort, with its `because`.

A clause that only some releases reach states where it was seen, with `observed_in`: a list of
`{"package", "scheme", "since", "before", "profile"}` ranges (half-open, at least one bound, versions in the
package's `scheme`, the matrix `profile` optional). It is evidence and never a gate - the parse detaches every
range before any section compiles, so a clause behaves alike with or without one. It may be written on a
message rule or reading, whose firing the goldens attribute through its emissions, and not on a fragment's or a
selection point's shared cases. The goldens hold each range to the captured matrix: every range
must be exercised by a capture of a release inside it (`every_observed_range_is_exercised_by_a_capture_inside_it`),
a clause firing in a capture of its package outside every range it states is reported as an unexplained
observation, and a clause whose every range holds no release of the last twelve months has aged out and fails
(`no_observed_clause_has_aged_out`, against a committed window start). `report_clauses_whose_firing_depends_on_the_release`
(run with `--ignored`) lists the clauses whose firing differs between captured releases: the candidates for a
range. Each versioned capture directory (`native@<version>[+<profile>]`) must match its `versions.json` record.

## Testing

- `cargo test --locked -p sideseat-domain --lib` compiles the embedded assets, checks the refusals, and holds
  the schema (`server/assets/rules.schema.json`, regenerated with `UPDATE_GOLDENS=1 ... the_committed_schema_is_current`)
  and the census of unused options current.
- `MESSAGE_FIXTURES=tracked cargo test --locked -p sideseat-server --test message_goldens` replays every
  fixture and compares the reconstructed views with `expected.json` and the truths; a fixed defect must leave
  `known-violations.json`, which only shrinks. `UPDATE_GOLDENS=1` rewrites the expectations for review.
- `cargo test --locked -p sideseat-server --test repository rule_language` holds this document to the
  schema; `UPDATE_DOCS=1` regenerates the reference below.
- `make harden-spec` model-checks the precedence, three-valued logic, claiming and content-chain models in
  `server/specs/`; the domain tests hold the engine to the same manifests (`UPDATE_SPECS=1` regenerates them).

<!-- BEGIN GENERATED FROM server/assets/rules.schema.json: UPDATE_DOCS=1 cargo test --locked -p sideseat-server --test repository rule_language -->

## Reference

Every key an asset may write, generated from the schema the engine is compiled from. Each entry's text is the first paragraph of its documentation in the engine; the schema file holds the rest.

### Asset sections

| Section | Holds | What it is |
| --- | --- | --- |
| `$schema` | string | The editor schema this asset is written against. Document metadata: no section reads it, and the repository requires it to name the generated `rules.schema.json`. |
| `id` (required) | string | Stable id for diagnostics and explain traces. Not a framework identity anything branches on. |
| `doc` | string | What this file is for, in prose. Surfaced by the explain trace, which is why documentation is a field rather than a comment. |
| `carriers` | list of [`CarrierRule`](#carrierrule) | Carrier semantics declarations. |
| `detect` | list of [`DetectRule`](#detectrule) | Detection signals. Produce a **label** and nothing else: no behaviour reads it. |
| `messages` | list of [`MessageRule`](#messagerule) | Which carriers an ingestion reads on this dialect's spans, and how each is parsed. |
| `message_projections` | list of [`MessageProjectionRule`](#messageprojectionrule) | Stored message rows that are producer bookkeeping rather than another conversation. |
| `message_events` | list of [`MessageEvent`](#messageevent) | The events this dialect writes messages on. |
| `log_events` | list of [`LogEvent`](#logevent) | The log-record shapes that carry one of the `message_events`. |
| `tool_shapes` | list of [`ToolShapeRule`](#toolshaperule) | How a provider writes a tool *definition*, so the canonical shape is reached by declaration. |
| `convention_namespaces` | list of string | Attribute namespaces the **conventions** own, as opposed to a producer's own. |
| `event_roles` | list of [`EventRole`](#eventrole) | What role a message's source name implies, where the name decides it. |
| `role_authority` | list of [`RoleAuthority`](#roleauthority) | Which role spellings outrank the name a reading was found under. This engine's own vocabulary. |
| `content_blocks` | list of [`ContentBlockRule`](#contentblockrule) | Content-block shapes this dialect writes. |
| `provider_aliases` | list of [`ProviderAlias`](#provideralias) | A value a producer writes in `gen_ai.system` that names a **provider** the catalogue knows. |
| `finish_reasons` | list of [`FinishReasonSpellings`](#finishreasonspellings) | What each spelling of a finish reason means, in this engine's finish categories. |
| `synthetic_call_ids` | list of [`SyntheticCallId`](#syntheticcallid) | The shape of a call id a producer builds itself when the provider gave none, and which names the tool. |
| `event_categories` | list of [`EventCategoryRule`](#eventcategoryrule) | What an event no convention names is, by a word its name contains. |
| `message_members` | list of [`MessageMemberRule`](#messagememberrule) | Member names a producer uses, and what each one's presence means. |
| `span_categories` | list of [`ClassifyRule`](#classifyrule) | Which broad category a span falls in, as ordered first-match rules. |
| `observation_types` | list of [`ClassifyRule`](#classifyrule) | What kind of observation a span is, as ordered first-match rules. |
| `span_facts` | list of [`SpanFactRule`](#spanfactrule) | Facts about a *span* this dialect can establish, as opposed to about a carrier. |
| `fragments` | map of text to [`Fragment`](#fragment) | Named reading tables other rules may apply. |
| `sdk_slugs` | list of [`SdkSlug`](#sdkslug) | The slugs an SDK may write into `sideseat.framework` for this framework, and the label they resolve to. |
| `span_fields` | list of [`SpanFieldRule`](#spanfieldrule) | Which keys carry a *span field* - a scalar or list on the stored span, as opposed to a message. |

### `CarrierRule`

One carrier declaration: what to match, and what the matched carrier is evidence of.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | Stable clause id, reported by the explain trace. |
| `doc` | string |  |
| `match` (required) | [`MatchSpec`](#matchspec) |  |
| `facts` (required) | [`Facts`](#facts) | The preset this clause resolves to, optionally with named overrides. |
| `ordering_family` | string | The ordering family this carrier belongs to, when it is a *fragmented ordered input*: several attribute keys that are one array (`llm.input_messages.0.message` and `.1.message`). |

### `MatchSpec`

What an observation must look like for a clause to apply. Every field is optional and all present
fields must hold, so a clause constraining more dimensions is strictly more specific.

The dimensions are a fixed, finite set of orthogonal scalar constraints - which is what makes
specificity well-defined here, unlike the tree-shape predicates of the content chain, where
ordering has to be declared by name instead.

| Key | Type | What it is |
| --- | --- | --- |
| `event` | string | Exact OTel event name the observation was read from. |
| `attribute` | string | Exact attribute key. |
| `attribute_prefix` | string | Attribute key prefix, for indexed families (`llm.input_messages.0.message`). |
| `attribute_family` | string | A dotted attribute **family**: the root itself and every key below it. |
| `observation_type` | list of string | Any of these observation types. This is the dimension the span-blind lookup lacked: the same carrier name means different things on a generation span and on an aggregator. |

### `Facts`

A preset plus overrides rather than ten booleans spelled out per clause: the presets are the
vocabulary the model is stated in, and a clause that writes them all out invites one being wrong in
a way no reader notices.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `preset` (required) | [`CarrierPreset`](#carrierpreset) | The constructor the facts start from; the named overrides below change one fact each. |
| `position_proves_distinct_occurrence` | true or false |  |
| `position_provides_sequence_order` | true or false |  |
| `history_positions_provide_sequence_order` | true or false |  |
| `carrier_is_atomic_emission` | true or false |  |
| `may_restate_prior_observations` | true or false |  |
| `carrier_holds_span_output` | true or false |  |
| `carrier_is_detached_request_frame` | true or false |  |
| `carrier_holds_span_input` | true or false |  |
| `carrier_holds_expandable_message_array` | true or false |  |
| `carrier_replays_across_traces` | true or false |  |

### `CarrierPreset`

The **ten** carrier facts, named by preset with optional per-field overrides.

A preset is a constructor, not a category: `snapshot` and `accumulated_state` differ in one fact,
`carrier_holds_span_output`, and the name does not survive compilation. So each vector has one spelling: an
override that turns one preset into another is refused, naming the preset to write. Most shipped clauses
override something, and nearly all of those overrides are compensating for direction or encoding being
bundled into a preset that is otherwise about *reconstruction*.

The three constructors of a carrier's facts.

A closed set, so a misspelt preset is refused when the asset parses rather than reaching a compiler that
has to remember to.

- `"emission"`: One occurrence of the span's output per position.
- `"snapshot"`: A conversation re-listed in order, positions in sequence.
- `"accumulated_state"`: Framework state that restates earlier observations.

### `DetectRule`

One detection rule: signals that identify a producer, and the label they yield.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `label` (required) | string | The label written to the span's `framework` column. A display and filtering value. |
| `priority` (required) | integer | Where this rule sits in the ordered sweep, lowest first: the first rule that holds labels the span. |
| `supersedes` | list of string | Rule ids this rule is meant to beat where both match: an overlap the author owns. |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) | The evidence for the label. Detection reads the span's name and attributes, its instrumentation scope and its resource. |
| `alternatives` | list of [`DetectAlternative`](#detectalternative) | Further evidence for the same label, each at its **own** priority. |

### `Expr_SpanCondition`

- [`SpanCondition`](#spancondition)
- object with `all` (list of [`Expr_SpanCondition`](#expr_spancondition), required), `doc` (string)
- object with `any` (list of [`Expr_SpanCondition`](#expr_spancondition), required), `doc` (string)
- object with `not` ([`Expr_SpanCondition`](#expr_spancondition), required), `doc` (string)

### `SpanCondition`

One question about one source of the span: a selector and the tests asked of the value it selects.

Every test is asked of the **same** value, so several tests are their conjunction. `exists` asks about the
selection rather than the value and is total; every other test is unknown where the source has no value,
which is why `not` over a value test does not hold for an absent attribute - write `"exists": true` beside
the test to make it false instead.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `source` (required) | [`ConditionSource`](#conditionsource) | Where the value is: `span_name`, `attr:<key>`, `attr_keys` (the set of the span's attribute keys, asked existentially), `scope.name` (the instrumentation scope), `scope.version` (its version, for `version` only) or `resource:<key>`. Several sources are searched together only by `contains_ignore_case`: a list asks every source that has a value, and `{"first_of": [...]}` only the first that has one. |
| `exists` | true or false | The source has a value. Total: true or false, never unknown. |
| `equals` | string | The value is exactly this text. |
| `equals_ignore_case` | string | The value is this text, ignoring ASCII case. |
| `one_of` | list of string | The value is one of these texts. |
| `starts_with` | string | The value begins with this text. For `attr_keys`, some key does; for `scope.name`, unknown where the span reports no scope, so a `not` over it holds only for a scope that is there. |
| `contains` | string | The value contains this text. |
| `contains_ignore_case` | string | The value contains this text, ignoring case (Unicode lower-casing). |
| `parses` | [`Encoding`](#encoding) or null | The attribute's text parses in this encoding: `json`. False where it is present and does not - text cut short by an attribute length limit, say - and unknown where it is absent, so `not` over it holds only for a value that is there and does not parse. |
| `version` | [`VersionRange`](#versionrange) or null | The value is a release inside this half-open range, ordered by the package's scheme. Asked of `scope.version` only, and alone in its atom; a value that is absent or not a version is unknown, never "the latest". The last resort of the language: a shape test says what changed, a version only when. |

### `ConditionSource`

The source or sources a condition reads.

- [`SourceName`](#sourcename): One source.
- list of [`SourceName`](#sourcename): Every one of these that has a value.
- [`FirstOfSources`](#firstofsources): Only the first of these that has a value.

### `SourceName`

`span_name`, `attr_keys`, `scope.name`, `scope.version`, `attr:<key>` or `resource:<key>`.

- one of `"span_name"`, `"attr_keys"`, `"scope.name"`, `"scope.version"`
- string

### `FirstOfSources`

Only the first of several sources that has a value.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `first_of` (required) | list of [`SourceName`](#sourcename) |  |

### `Encoding`

An encoding an attribute's text may be written in.

- `"json"`: Any JSON value, as `serde_json` reads one.

### `VersionRange`

A half-open range of releases, `at_least <= v < below`, in one version scheme.

An ordered interval and nothing else: no requirement-matcher policy (PEP 440's `<7` specifier and npm ranges
exclude pre-releases of the bound; this does not - `7.0rc1` is below `7`). SemVer bounds may leave minor and
patch out; values may not.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `scheme` (required) | [`VersionScheme`](#versionscheme) | How the package numbers its releases. |
| `at_least` | string | The first release inside the range. |
| `below` | string | The first release after it. |
| `because` (required) | string | Why no shape test can say this: required, because a version gate stands for a change the payload does not show, and a reader has to be able to check that it still does not. |

### `VersionScheme`

How a package numbers its releases.

- `"pep440"`: Python's PEP 440: epochs, any number of release segments, `aN`/`bN`/`rcN`, `.postN`, `.devN`, `+local`.
- `"semver"`: Semantic Versioning 2.0.0: `MAJOR.MINOR.PATCH`, `-prerelease`, `+build`.

### `DetectAlternative`

One further body of evidence for a rule's label, at its own priority.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `priority` (required) | integer |  |
| `supersedes` | list of string | This alternative's own documented overlaps, as on a rule. Not inherited from the rule: an alternative exists to sit at a different priority, so an edge true of the rule's position may not be true of its. |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) |  |

### `MessageRule`

One message-extraction rule: a carrier to read, how to parse it, and what to emit.

**No longer small, and that is the finding.** It began as "read a carrier, parse it, emit it" and each
dialect added a generic field that prevented a measured defect. Every one is still declarative and
non-Turing-complete, but selection, projection, predicates and object construction have been built by
hand here - which is what an expression language already standardises, and a hand-built path resolver
is where a real bug lived (a literal dotted key read as a nested path).

So the shaping half of this type moved to a published selection language with a parser and quoted
identifiers. **RFC 9535 JSONPath**, not JMESPath: JMESPath was implemented first and reverted, because
every result comes back through its crate's sorted-map value tree, which alphabetises a selected payload's
members - and this repository treats serialised member order as observable. `message_rules.rs` records the
measurement. What stays here is the structural half - which carrier, who claims it, in what order, what it
emits - because that is ownership and policy rather than a transform.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `observed_in` | list of [`Observation`](#observation) | The releases this rule was observed firing in. Evidence for the coverage join, read by no answer. |
| `doc` | string |  |
| `source` | [`MessageSource`](#messagesource) or null | The *events* this rule applies to. Non-empty makes it an event rule: its reads resolve against the event's own attributes rather than the span's, and its observations are tagged as events. |
| `read` | [`ReadSpec`](#readspec) | The carrier to read. Absent for a `compose` rule, which has many sources rather than one. |
| `compose` | [`ComposeSpec`](#composespec) or null | Assemble one message from several attributes, rather than wrapping one read value. |
| `tool_repr` | [`ToolReprSpec`](#toolreprspec) or null |  |
| `parse` | [`ParseMode`](#parsemode) or null | How to turn its raw string into a value. Absent for an indexed family, which has no single string to parse - each member is read on its own. |
| `wrap` | [`WrapSpec`](#wrapspec) or null | Wrap the parsed value in a message envelope with this role. |
| `emit` | [`EmitTarget`](#emittarget) or null | Whether the observation is a message or a tool definition. |
| `aggregate_into_array` | true or false | Emit one observation whose value is the array of everything read, rather than one per reading. |
| `where` | [`Expr_SpanCondition`](#expr_spancondition) or null | A gate on the span: the rule is consulted only where this holds. It reads the span's name and attributes and the instrumentation scope - never the resource. |
| `alternatives` | list of [`Alternative`](#alternative) | Ordered readings of the parsed value, tried until one yields an observation. |
| `also` | list of [`Alternative`](#alternative) | Readings that all contribute, rather than the first that yields. |
| `fallback` | list of [`Alternative`](#alternative) | A reading used only when nothing else in this rule emitted anything. |
| `tag_as` | string | Tag the observation with this carrier, whatever alternative was read. |
| `reads_tool_spans` | true or false | May this rule read a *tool execution* span? |
| `branch_set` | [`BranchSet`](#branchset) or null | Several carrier readings with a *local* order between them. |
| `elements` | [`ElementsSpec`](#elementsspec) or null | Read an array-valued carrier element by element, in declared passes. |
| `walk` | [`WalkSpec`](#walkspec) or null | Apply this rule's readings at every node of a bounded tree walk. |
| `sections` | [`SectionsSpec`](#sectionsspec) or null | Split a text carrier into tagged sections and route each by its tag. |
| `raw_where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | A condition on the carrier's **raw text**, asked before it is parsed: the carrier is read only where it holds, the text standing as a JSON string. How a reading says an attribute present and empty - or blank - is not evidence of a message: `{"non_empty": true}` skips the empty string, `{"non_blank": true}` the whitespace too, and the two stay distinct because one dialect treats whitespace as absence and another does not. |
| `require_members` | [`MemberRequirements`](#memberrequirements) or null | What an indexed entry must carry to count as one. |
| `priority` | integer | Position in the consulted order, lowest first. Unique among the rules that contend with this one. |

### `Observation`

One release range a clause was observed firing in: positive evidence, never a gate.

Written on an executable leaf with a stable id - a message rule or reading - where the shape it reads is one
some releases write. Read by nothing that decides an answer: the parse
detaches every annotation before any section compiles, so a clause behaves alike with or without it. What
reads it is the coverage join over the captured matrix - each range must be exercised by a fixture inside it,
and a range no release inside the window falls in has aged out.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `package` (required) | string | The package whose releases the range counts, as the matrix's provenance names it. |
| `scheme` (required) | [`VersionScheme`](#versionscheme) | How that package numbers its releases: declared here, because nothing else says which scheme a package's versions are in. |
| `since` | string | The first release observed. Half-open: `since <= v < before`. |
| `before` | string | The first release no longer observed. |
| `profile` | string | The configuration of the release that was observed (`OTEL_SEMCONV_STABILITY_OPT_IN`, an instrumentation setting), as the matrix names it. Absent: any. |

### `MessageSource`

Where a message rule reads from.

Exactly one variant, so a rule cannot half-declare both: an event rule has no stage (the event path runs
every rule that names the event, in rank order) and a span rule has no event names.

- object with `span` ([`SpanSource`](#spansource), required): A span's attributes, at the named stage.
- object with `event` ([`EventSource`](#eventsource), required): The attributes of any of the named events.

### `SpanSource`

| Key | Type | What it is |
| --- | --- | --- |
| `stage` | [`MessageStage`](#messagestage) | With the dialects, or only if none of them produced anything. |

### `MessageStage`

- `"dialect"`: With the dialects, in rank order. The ordinary case.
- `"fallback"`: Only if no dialect-stage rule produced a message or a claim.

### `EventSource`

| Key | Type | What it is |
| --- | --- | --- |
| `names` (required) | list of string | The events this rule reads. An empty list is refused: it names nothing, and under the previous spelling it silently made the rule an ordinary span rule instead. |

### `ReadSpec`

The carrier a message rule reads: exactly one form, checked at compile time.

Optional fields rather than a tagged enum, for the same reason the carrier match spec uses them: an
externally-tagged enum needs `{"attribute": {"attribute": "k"}}` in JSON, which is the shape nobody writes
and serde rejects silently at the file level. Requiring exactly one is the check that makes this equivalent
while staying readable.

There is deliberately **no `event` form**. One existed, was accepted by this schema, and was refused
unconditionally by the compiler as unimplemented - so the format advertised four read forms and could
execute three. An author reading the schema as the format's reference was being told something untrue,
which is worse than the missing capability: a span's *events* are routed to `MessagePlan::from_event`,
where `source.event.names` selects the rule and the event's attributes are read exactly as a span's are.
If a rule ever needs to read one event while running over a span, that is a new construct to design rather
than a field to un-refuse.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `attribute` | [`FirstPresent_string`](#firstpresent_string) or null | The carrier: one attribute, or `{"first_of": [...]}` - several spellings of it, of which the first the span carries is read and the observation tagged with that key. A dialect that renamed a key keeps accepting the old one, and the tag has to be the key actually found or two spans carrying different spellings would be indistinguishable downstream. |
| `every` | list of string | **Every** one of these keys the span carries is read, each as its own observation. |
| `indexed_family` | string | An *indexed attribute family*: `<prefix>.0.role`, `<prefix>.0.content`, `<prefix>.1.role`, ... |
| `family` | string | A dotted attribute *family* read as one object: every key under the prefix (which ends in `.`), named by what follows it - `code.function.parameters.city.value = "Paris"` is `{"city.value": "Paris"}` - with each value read as the JSON it spells or as its text. |
| `rendering` | [`Expr_ValuePredicate`](#expr_valuepredicate) or null | A predicate applied to each fully assembled indexed entry. |
| `entry_member` | string | A sub-level of each indexed entry whose members are read at the top of the object. |
| `numeric_members` | list of string | Entry members to read as a number where the text is one. |
| `entry_value` | string | Read one *value* out of each indexed entry, rather than the entry's assembled members. |
| `entry_value_parse` | [`ParseMode`](#parsemode) or null | How the projected value is read. `json` **drops** an entry whose payload does not parse, which is what a schema that failed to parse always meant - an indexed member is otherwise sniffed, and a malformed schema would be emitted as the string it is, reported as a tool definition. |
| `overlay` | [`OverlaySpec`](#overlayspec) or null | A richer copy of these same messages, held by another carrier and matched by position. |

### `FirstPresent_string`

- string
- object with `first_of` (list of string, required), `mode` (`"present"`), `doc` (string)

### `Expr_ValuePredicate`

- [`ValuePredicate`](#valuepredicate)
- object with `all` (list of [`Expr_ValuePredicate`](#expr_valuepredicate), required), `doc` (string)
- object with `any` (list of [`Expr_ValuePredicate`](#expr_valuepredicate), required), `doc` (string)
- object with `not` ([`Expr_ValuePredicate`](#expr_valuepredicate), required), `doc` (string)

### `ValuePredicate`

A condition on a JSON value, or on a member of it.

One vocabulary for every question the rules ask *about a value*: whether an alternative's shape holds,
whether a source is eligible, whether a section is dropped. Before this there were three bespoke
spellings - a member-name list, an `is_object` flag, and a fused "capture lacks prefix and body starts
with" pair - and a fourth was about to be added for a dialect that needs "this member is an object, or
that one is a non-empty string". Three narrow predicates are harder to reason about than one, and the
fused pair was producer policy wearing a generic name.

Deliberately *not* used for attribute-key presence (`MemberRequirements`): that asks about a flat map of
dotted keys, where "nested" means "some other key starts with this one". Same word, different domain -
and one type spanning both would have to mean different things depending on where it was used.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this condition is the right one, where that is not obvious from the condition. A field rather than a comment, as everywhere else here, because the explain trace surfaces it. |
| `path` | string | A JSONPath to the value under test. Absent means the value itself. |
| `exists` | true or false | The member must be present. Implied when the predicate names nothing else. |
| `kind` | [`ValueKind`](#valuekind) or null | The value's JSON kind. |
| `non_empty` | true or false | A string, array or object must not be empty. Meaningless for other kinds, and refused there. |
| `identifier_like` | true or false | The value begins like an identifier - a letter, a digit or an underscore. |
| `non_blank` | true or false | A string holding something other than whitespace. Meaningless for other kinds, and unknown there. |
| `not_null` | true or false | The value is not JSON null. Distinct from `exists`, which a null member satisfies, and from `non_empty`, which is about a string, array or object having contents. |
| `starts_with` | string | A string must start with this. |
| `lacks_prefix` | string | A string must *not* start with this. |
| `one_of` | list of string | The value must be one of these strings. An absent member satisfies nothing. |
| `none_of` | list of string | The value must not be any of these strings. |
| `equals` | any JSON value | The value must be exactly this JSON value - for a flag or a number, where `one_of` asks only about strings. `null` cannot be written here (it reads as "no condition"); `kind: null` says it. |
| `only_members` | list of string | The value must be an object with no member outside these. A subset: a shape that must also hold one of them says so with a predicate of its own. |

### `ValueKind`

A JSON kind, for `ValuePredicate::kind`.

Written as one of `"object"`, `"array"`, `"string"`, `"number"`, `"bool"`, `"null"`.

### `ParseMode`

How a raw attribute string becomes a value.

- `"json"`: Parse as JSON; skip the carrier entirely if it does not parse.
- `"json_or_string"`: Parse as JSON, keeping the raw text as a string if it does not parse.
- `"json_structure_or_string"`: Parse as JSON where the text encodes an object, an array or a string; keep the raw text otherwise.
- `"stringified_array"`: Parse as JSON, then parse any *string* member of the resulting array as JSON too.
- `"python_constructor_repr"`: Parse a Python constructor `repr` into a JSON tree.
- `"python_constructor_repr_array"`: Parse a JSON array whose every element is a Python constructor `repr`, each into a JSON tree.
- `"python_constructor_repr_sequence"`: Parse text that is several Python constructor `repr`s written back to back, into an array of their trees.
- `"python_literal"`: Parse the Python `str()` of a dict or a list - single-quoted strings, `True`, `False`, `None` - into a JSON tree; the carrier is skipped when the whole text is not one.
- `"text"`: Keep the raw text. Some carriers hold prose, and parsing it would turn a bare word into a non-string or an accidental number into a number.

### `OverlaySpec`

Another carrier of the same span describing the same messages at higher fidelity.

Not an enrichment of what some other reader produced - both carriers are attributes of one span, and
the join is positional: entry *n* of the family and member *n* of the other carrier's list are the
same message. A flattened family loses whole content blocks and redacts urls, while the serialised copy
beside it keeps them, so where both describe one message the richer one is preferred.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `from` (required) | string | The attribute holding the richer copy. |
| `parse` | [`ParseMode`](#parsemode) or null |  |
| `select` (required) | [`FirstUsable_string`](#firstusable_string) | Ordered paths to the counterpart list; the first that resolves to an array is used. |
| `unwrap_single_element_list` | true or false | Unwrap a list of exactly one list. A serialiser that accepts a batch of conversations writes one conversation as a batch of one, and the members of *that* are the messages. |
| `witness` | [`Expr_ValuePredicate`](#expr_valuepredicate) | What the list must look like to be this dialect's own serialisation. Without it, any array of objects at that path would be treated as the same messages. |
| `when_member_prefix` (required) | string | Only entries carrying members under this prefix are overlaid - the flattened form of the content that is known to be lossy. |
| `content_from` (required) | [`FirstPresent_string`](#firstpresent_string) | Ordered paths to the counterpart's content. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | What that content must be for the overlay to be an improvement. |
| `as_member` (required) | string | The member the content becomes, replacing every member under `when_member_prefix`. |

### `FirstUsable_string`

- string
- object with `first_of` (list of string, required), `mode` (`"usable"`, required), `doc` (string)

### `ComposeSpec`

A message assembled from several attributes of one span.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `tag` (required) | string | The carrier the assembled message is tagged with. |
| `members` (required) | list of [`ComposeMember`](#composemember) | The members, in the order they are inserted - which is observable, since content identity is hashed from the payload. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | A condition on the **assembled** object, checked before it is emitted. |
| `as_tool_definition` | true or false | Emit the assembled object as a canonical **tool definition** rather than as a message. |
| `trailing` | object | Literal members added *after* every source member. |

### `ComposeMember`

One member of a composed message: a named source, or a sweep of a prefix.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `as` | string | The member's name. Absent for a sweep, which takes its names from the keys it finds. |
| `from` | [`FirstPresent_string`](#firstpresent_string) | The attribute, or the first of several the span carries. |
| `parse` | [`ParseMode`](#parsemode) or null | How to read it. Defaults to text. |
| `fallback` | [`ComposeFallback`](#composefallback) or null | A last-resort source, used only where the gate holds - and given up where another rule already owns the key: the compose then drops this member and keeps the rest, while a named member is left and its `where` still holds. A message compose's only. |
| `sweep_prefix` | string | Collect every attribute under this prefix, keyed by the remainder. |
| `except` | list of string | Names the sweep skips, because a named member above already read them. |

### `ComposeFallback`

A conditional last-resort source.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `from` (required) | string |  |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) | The evidence required before the fallback is read, over the span's name and attributes. |
| `parse` | [`ParseMode`](#parsemode) or null |  |

### `ToolReprSpec`

Tool definitions a carrier holds as a language's `repr` rather than as JSON.

The grammar is sealed in `rules::tool_repr` because it is a property of the *language*. Everything a
particular framework calls its own - which member holds the tools, which repr fields name them, which
labels its embedded documentation uses, how its type names map to JSON Schema's - is here, because
that is its vocabulary and not a fact about Python.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `entries` (required) | string | The carrier's entries. One entry at a time, so two entries each holding a list interleave as the payload has them rather than by path - which is what keeps the reported order the framework's own. |
| `candidates` (required) | list of string | Where an entry holds tools, in order. Each resolved value is tried as a tool definition. |
| `name_field` (required) | string | The repr fields naming a tool and its documentation - `name='search'`. |
| `description_field` (required) | string |  |
| `name_label` (required) | string | The labels the embedded documentation uses. |
| `description_label` (required) | string |  |
| `arguments_label` (required) | string |  |
| `repr_markers` (required) | list of string | What makes a string a `repr` rather than a bare tool name. Without these a name containing a space would be parsed as a repr and yield nothing. |
| `parameter_members` (required) | list of string | Where a tool object states its parameters, in order. |
| `field_terminators` | list of string | The repr fields that may follow a loosely-quoted one. Their appearance is where that value ends. |
| `type_map` (required) | list of list of values | The language's type names, mapped to JSON Schema's. Compared case-insensitively, and ordered because the first match wins. |
| `type_default` (required) | [`UnknownType`](#unknowntype) | What an argument whose type name the map does not hold becomes. |

### `UnknownType`

What an argument type the map does not name becomes.

- `"unconstrained"`: No `type` member at all, which is what "the widest type" means in JSON Schema - it constrains nothing.
- object with `map_to` (string, required): A named JSON Schema primitive, for a producer whose unrecognised names really are one kind of thing. Validated against the primitives, so a typo is not a schema every reader ignores.

### `WrapSpec`

The envelope a bare payload is wrapped in.

Some carriers hold a payload rather than a message - a tool's arguments, an instruction, a response's
text - and what that payload *is* is a fact about the carrier, so the envelope is declared beside it.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `role` | string | A literal role. One of this and `role_from` is required. |
| `role_from` | [`ValueSource`](#valuesource) or null | A JSONPath whose value is the role, relative to the reading being wrapped, or `{"path": ..., "pipe": [{"map": {...}}]}` to rename the roles the payload supplies. |
| `content_from` | [`FirstPresent_string`](#firstpresent_string) | Ordered paths for the content; the first that resolves wins. |
| `content_default` | any JSON value | The content when none of the paths above resolve. Absent means the reading is not this shape. |
| `content_as` | string | The member the read value becomes. Defaults to `content`. |
| `members` | object | Literal members added to the envelope. |
| `attach` | list of [`AttachSpec`](#attachspec) | Members taken from *other* attributes of the same span. |
| `prepend_block` | [`PrependSpec`](#prependspec) or null | Build a block from another member and put it **before** the content. |
| `tool_calls_from` | [`ToolCallsSpec`](#toolcallsspec) or null | Build the canonical tool-call list from an array of the dialect's own calls. |
| `only_plain_data` | true or false | Wrap only where the value is not already message-shaped. |
| `block` | [`BlockSpec`](#blockspec) or null | Wrap the value in a *content block* first, and make that block the message's only content. |

### `ValueSource`

Where one member of a canonical block comes from.

A JSONPath string reads the first value it selects, as it stands. The object form applies exactly one
bounded transform to it, and a transform that cannot apply leaves the source **absent**, so the next one in
the list is tried - which is how "the first usable spelling" is written.

- string: A member of the block, by RFC 9535 JSONPath.
- [`TransformedSource`](#transformedsource): A member with one transform applied.

### `TransformedSource`

A member with one transform applied: `{"path": ..., "pipe": [step]}`, the step one of `join` (every string the
path selects, joined), `parse` (a string decoded the way a carrier's text is; anything else passes through),
`prepend`, or a closed `map` (`{"map": {...}, "closed": true}`). A step that cannot apply leaves the source
**absent**, so the next candidate is tried.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `path` (required) | string |  |
| `pipe` (required) | list of [`Transform`](#transform) |  |

### `Transform`

One step of a pipe. A field states the steps it runs and their order.

- one of `"lowercase"`, `"trim"`, `"strip_bracket_tag"`, `"blank_is_absent"`
- object with `strip_prefix` (string, required)
- object with `join` (string, required)
- object with `prepend` (string, required)
- object with `parse` ([`ParseMode`](#parsemode), required)
- object with `map` (object, required), `closed` (true or false)

### `AttachSpec`

One member taken from a sibling attribute.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this member is taken from where it is, where that is not obvious. A field rather than a comment, as everywhere else here, because the explain trace surfaces it. |
| `from` | string | The attribute to read. One of this and `from_path` is required. |
| `from_value` | [`FirstPresent_string`](#firstpresent_string) | Ordered paths into the value being wrapped; the first that resolves wins. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | The value read must satisfy this, or the member is left off. Asked of what the source supplied after its own steps - `parse`, `select`, the pipe, the span name's prefix and trim - and before a literal (`value`, or a closed `map`'s) replaces it; a `default` is not read, so it is not asked. |
| `from_path` | string | A path into the rule's *own parsed payload*, rather than a sibling attribute. |
| `as` (required) | string | The member it becomes. |
| `parse` | [`ParseMode`](#parsemode) or null | How to read it: an attribute, or a string member a value path or payload path selects. Defaults to text, and a member that is not a string is attached as it stands. |
| `select` | string | The member of the parsed `from` attribute to attach, rather than the whole value. |
| `value` | any JSON value | The literal to attach instead of the source's value, for a flag - or on its own, for a member that is part of the shape rather than something read. Beside a closed `map`, which attaches its own literal, it is refused. |
| `pipe` | list of [`Transform`](#transform) | What happens to the value read, in this order: `blank_is_absent` (a blank attribute is treated as absent, so the fallbacks below apply), `strip_bracket_tag` (a leading `[TAG]` line removed before parsing - one dialect tags a structured payload with the tool it belongs to and writes the JSON beneath it), and then either `lowercase` (after parsing and selecting: a provider writes finish reasons in upper case and the canonical form is lower) or a closed `map` of **one** entry, `{"map": {"true": true}, "closed": true}`: attach the mapped literal only when the attribute is exactly that text and nothing otherwise - how a boolean flag arrives as the string `"true"`. Only `lowercase` applies to a payload path; nothing applies to a value path. |
| `or_span_name` | list of [`Transform`](#transform) | Fall back to the span name, through `[{"strip_prefix": p}, "trim"]`, when the other sources are absent: a name without the prefix, or one that is blank after it, supplies nothing. |
| `default` | any JSON value | Attach this literal when nothing else supplied a value. |
| `after_content` | true or false | Place this member *after* the content member rather than before it. |

### `PrependSpec`

A block built from another member of the same value, placed before the content.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `from` (required) | string | Where the block's content is, relative to the value being wrapped. Absent means no block is added, which is the ordinary case for a dialect that reports reasoning only sometimes. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | A condition on the value found there. A dialect writes this member as `null` when there was no reasoning, and a null is not a thought. |
| `block` (required) | [`BlockSpec`](#blockspec) | The block to build. Nested rather than flattened into this object: serde does not support `flatten` beside `deny_unknown_fields`, so a flattened block made key refusal depend on serde's buffering. |

### `BlockSpec`

A content block built around the read value.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `type` (required) | string | The block's `type` member - `tool_use`, `tool_result`. |
| `content_as` | string | The member the read value becomes inside the block. Defaults to `content`. |
| `attach` | list of [`AttachSpec`](#attachspec) | Members taken from sibling attributes, as on the envelope. |

### `ToolCallsSpec`

The canonical tool-call list, built from a dialect's own array of calls.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `select` (required) | string | The array of calls, relative to the value being wrapped. |
| `id` (required) | string | Where each call's id, name and arguments are. |
| `name` (required) | string |  |
| `arguments` (required) | string |  |
| `on_invalid_item` (required) | [`InvalidItem`](#invaliditem) | What to do with a call that has no id or no name. |

### `InvalidItem`

What a tool-call list does with a member it cannot build.

- `"skip"`: Leave it out and keep the rest. Reported either way - a dropped call is a producer defect, not a detail of the loop that read it.
- `"fail_message"`: The whole construction is malformed, so the coalesce moves on to the next shape and the rule's `fallback` gets its turn. Right where a missing call means the message misdescribes what happened.

### `EmitTarget`

What an emitted observation is.

- one of `"message"`, `"tool_definitions"`
- `"tool_names"`: A list of tool *names*, as opposed to their definitions. A framework that reports only the names has said which tools were available, not what they take.
- `"claim"`: The carrier is claimed and nothing is read from it.

### `Alternative`

One documented shape of a payload: where to look, what to require, and what to carry down.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `observed_in` | list of [`Observation`](#observation) | The releases this reading was observed firing in, as `MessageRule::observed_in`. Not on a fragment's cases, which every rule using the fragment shares. |
| `doc` | string |  |
| `select` | string | An RFC 9535 JSONPath into the parsed value. Absent means the value itself. |
| `each` | true or false | Treat the selected value as a list and read each element. |
| `descend` | string | After selecting an element, descend to this member - `choices[].message`. |
| `parse` | [`ParseMode`](#parsemode) or null | Decode the selected element when it is text: a list whose members are themselves serialised (`{"messages": ["ChatMessage(role=..., content=...)"]}`). An element that is not text, or does not decode, is not this shape and the reading moves on. |
| `lift` | list of [`LiftSpec`](#liftspec) | Members copied into the value being emitted, from the element or from the value it was selected out of. |
| `parent_where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | A condition on the value this selection came from, rather than on the selected element. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | The shape an observation must have to be emitted. |
| `wrap` | [`WrapSpec`](#wrapspec) or null | An envelope for *this* reading only. |
| `pipe` | list of [`Transform`](#transform) | `["trim"]` trims a string before testing and emitting it; no other step applies to a reading. |
| `rendering` | [`Expr_ValuePredicate`](#expr_valuepredicate) or null | The messages of this reading that are a **rendering**: turns the producer re-sent as text - a tool call written out as prose, a result quoted back - which another carrier holds losslessly. Asked of the same value as `where`. A rendering stays on the span that sent it, since it is what was sent, and is left out of the trace and session views, where the call and result it renders already are. Absent: none is; a fragment case's own `rendering` adds to its selection point's. |
| `then_fragment` | string | Apply this named fragment's cases to each selected element. |
| `emit` | [`EmitTarget`](#emittarget) or null | What this reading is, where it differs from the rule's own target. |
| `extra_cases` | list of [`Alternative`](#alternative) | Shapes recognised at *this* selection point only, tried after the shared fragment's own cases. |
| `then_select` | [`FirstPresent_string`](#firstpresent_string) | For each selected element, the first of these paths that names a member, chosen by **presence**. |
| `else_element` | true or false | Fall back to the element itself when `then_present_any_of` named nothing, or named a member of the wrong shape. |
| `collect_members` | [`CollectMembers`](#collectmembers) or null | Rebuild the candidate object from its members named `prefix` + *name* + `suffix`, as `{name: value}`. |

### `LiftSpec`

Members copied into an emitted value, and what happens where the target already has one.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `from` (required) | [`LiftSource`](#liftsource) | Where the members are read from. |
| `members` (required) | list of string |  |
| `on_conflict` (required) | [`LiftConflict`](#liftconflict) | What to do where the target already carries the member. Required: this was the difference between the two members that preceded it, and it was stated nowhere. |

### `LiftSource`

Which value a lift reads from.

- `"element"`: The value the selection landed on - used with `descend`, where the members sit beside the message.
- `"parent"`: The value the selection came out of, for a fact stated once for a batch.
- `"carrier"`: The other attributes of the carrier the reading came from - an event's attributes, or a span's - for a fact a producer states beside the attribute that holds the message (a finish reason next to the event's `content`). Each is read as the text it is; lifting one does not claim it.

### `LiftConflict`

What a lift does where the target already carries the member.

- `"keep_target"`: The target's own value wins: the lifted one is a fallback.
- `"replace_target"`: The lifted value wins.

### `CollectMembers`

The member-name pattern of [`Alternative::collect_members`].

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `prefix` (required) | string |  |
| `suffix` (required) | string |  |

### `BranchSet`

Several readings of one span with a local order between them.

Evaluated as: every `primary`; then every `fallback_if_primary_empty`, for each kind of output - a
conversation, tool definitions, tool names - the primaries produced none of; then every `always`, whatever
happened. Nesting is refused - a branch set inside a branch set would be a control structure rather than a
declaration.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `primary` (required) | list of [`MessageRule`](#messagerule) | The readings that normally supply the conversation. |
| `fallback_if_primary_empty` | list of [`MessageRule`](#messagerule) | Read for each kind of output no `primary` reading produced, and kept for those kinds only: a primary that found the tools leaves a fallback's conversation standing, and its tools not. |
| `always` | list of [`MessageRule`](#messagerule) | Read whatever the others did. |

### `ElementsSpec`

An array-valued carrier read element by element.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `tags_are_events` | true or false | The emitted carriers are *events*, not attributes. |
| `passes` (required) | list of [`ElementPass`](#elementpass) | Passes over the elements, in order. Each scans every element. |

### `ElementPass`

One pass over the elements.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `doc` | string |  |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | Which elements this pass reads. |
| `tag_from` | string | Emit the element itself, tagged with the value at this path. |
| `group` | [`GroupSpec`](#groupspec) or null | Instead of emitting each element, group runs of them and emit one message per run. |

### `GroupSpec`

Runs of consecutive elements collapsed into one message.

Bounded: one pass, no recursion, and a run ends as soon as the derived key changes.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `by` (required) | list of [`DerivedCase`](#derivedcase) | A decision table deriving the run key from an element - the first matching case wins, and an element matching none is skipped. |
| `collect` (required) | string | The part of each element collected into the message's content. |
| `key_as` (required) | string | The member the derived key becomes on the emitted message. |
| `tag_by_key` (required) | map of text to string | The carrier each derived key is tagged with. |

### `DerivedCase`

One case of a decision table: a condition, and the value it yields.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `doc` | string |  |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | The case holds where this does; absent, it always holds, which is how a table states its default. |
| `value` (required) | string |  |

### `WalkSpec`

A bounded walk over a state object.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `max_depth` (required) | integer | How many levels below the carrier to descend. Zero means the carrier itself only. |
| `prune` | list of [`PruneSpec`](#prunespec) | Members not descended into, **because a named clause took them here**. |
| `stop_on` | list of string | Stop descending below a node **one of these clauses recognised**, naming them by id. |

### `PruneSpec`

A member the walk does not descend into, and the clause whose reading justifies that.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `member` (required) | string | The member name. |
| `taken_by` (required) | string | The clause that consumes it. The member is skipped only at a node where that clause recognised something - elsewhere its contents have been read by nothing and are still worth visiting. |

### `SectionsSpec`

A text carrier read as tagged sections.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `split_on` (required) | string | The separator between sections. |
| `max_sections` | integer | Split at most this many times over, so the last section keeps every later separator: a carrier whose final part is free text that may itself contain the separator (a prompt with blank lines) stays one section. At least one. |
| `truncated_unless_length` | [`LengthWitness`](#lengthwitness) or null | The final section is **cut** where the carrier holds less text than this integer attribute states, and is then left out: a producer that truncates a long carrier states the whole length beside it, and a cut section is not what the producer said. SideML has no truncation marker, so the partial text is dropped rather than shown as if complete; every section before it is whole and is kept. An absent or unreadable length leaves the carrier as it stands. |
| `skip_sections_equal_to` | list of [`SourceName`](#sourcename) | Sections that are, once trimmed, exactly the whole value of one of these attributes (`attr:<key>`, trimmed) are left out: another carrier states that part of the text whole, and reading it here too would show it twice. |
| `routes` (required) | list of [`SectionRoute`](#sectionroute) | Routes, tried in order; the first whose tag matches wins, and a route with no `tag_prefix` is the default. |

### `LengthWitness`

An attribute stating how long a text is, and the unit it counts in.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `source` (required) | [`SourceName`](#sourcename) | The attribute, as `attr:<key>`, holding a non-negative integer. |
| `counts` (required) | [`LengthUnit`](#lengthunit) | What the producer counted: the language it is written in decides it, so it is declared, not guessed. |

### `LengthUnit`

The unit a stated text length counts.

- `"chars"`: Unicode scalar values, as Python's `len` counts.
- `"utf16_units"`: UTF-16 code units, as JavaScript's `length` counts.
- `"bytes"`: UTF-8 bytes.

### `SectionRoute`

What to do with a section whose tag matches.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `doc` | string |  |
| `tag_prefix` | string | The tag prefix this route claims. Absent means "any section not claimed above". |
| `role` (required) | string | The role the emitted message carries. |
| `block` | [`SectionBlock`](#sectionblock) or null | Build a content block instead of putting the body under `content`. |
| `skip_where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | Drop the section entirely when every one of these holds, tested against `{"capture": <tag remainder>, "body": <section body>}`. |

### `SectionBlock`

A block built from a section, carrying what the tag captured.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `type` (required) | string |  |
| `capture_as` | string | The member the tag's remainder becomes - an id that pairs this section with a call. |

### `MemberRequirements`

Which members an indexed entry must carry.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `all_of` | list of [`MemberRequirement`](#memberrequirement) | Every one of these must be present. |
| `any_of` | list of [`MemberRequirement`](#memberrequirement) | At least one of these must be present. |

### `MemberRequirement`

One member, and how its presence is decided.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `name` (required) | string |  |
| `presence` | [`MemberPresence`](#memberpresence) |  |

### `MemberPresence`

How a member's presence is established.

- `"exact"`: The member's own key exists.
- `"nested"`: Some key nested under it exists - the member is an array or object flattened into dotted keys.
- `"either"`: Either.

### `MessageProjectionRule`

A read-time projection decision for one producer-owned span shape.

The row is recognised by a `where` over what a stored row says of its span - its name and its
instrumentation scope, version included - and the condition must name the scope, so a producer rule cannot
suppress a broad class of ordinary input-only spans. `only_attribute_sources` means every extracted message
must come from one of the named attributes; an empty message list never matches.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | Stable clause id, reported by diagnostics. |
| `doc` | string |  |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) | The rows this applies to: their span name, `scope.name` and `scope.version`. It must name an instrumentation scope the row has to carry: in a conjunction at any depth, or in every branch of a disjunction. |
| `only_attribute_sources` (required) | list of string | The attributes every extracted message of the row came from: each message from one of them. Two or more where a request is split across carriers - its conversation in one, its system instructions in another. An empty list, an empty name and a name listed twice are refused. |
| `successful_only` (required) | true or false | Only a row whose span succeeded: a failure may have no completed companion, so it stays visible. |
| `action` (required) | [`MessageProjectionAction`](#messageprojectionaction) |  |

### `MessageProjectionAction`

What a matching read-time projection rule does.

Written as one of `"suppress_messages"`.

### `MessageEvent`

One event that carries messages.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This declaration's identity, required like every other clause's. |
| `name` (required) | string |  |
| `raw` | [`RawEventForm`](#raweventform) or null | What the event's **raw form** is: an ordinary message, or a container its readings replace. |
| `doc` | string |  |

### `RawEventForm`

What an event's own attributes are, once its readings have run.

- `"message"`: The event body is itself a message. The default, and the case for all but one declared event.
- `"replace"`: The event is a *container*: its own attributes are the messages, so emitting the container as well would report the conversation twice.

### `LogEvent`

One OTLP **log record** shape that carries a message event.

Several instrumentations emit the conversation as log records linked to a span instead of as span
events: the record names a `message_events` entry and carries what a span event would carry in its
attributes, either as the members of its body or as its own attributes. A declaration says which, and
where the record states its event name; the event is then read exactly as the span event of that name
is. Every `name` must also be a `message_events` entry, because the readings and the raw form are
declared there - a log event no reading recognises would be stored and never answer.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This declaration's identity, required like every other clause's. |
| `name` (required) | string | The event name, which must also be declared in `message_events`. |
| `name_from` (required) | [`FirstUsable_SourceName`](#firstusable_sourcename) | Where a record states its event name: `event_name`, the log record's own field, or `attr:<key>`, a record attribute, which is how producers that predate the field wrote it. Of several, the first that holds a non-empty name decides. |
| `payload` (required) | [`LogEventPayload`](#logeventpayload) | Where the event's attributes are on the record. |
| `doc` | string |  |

### `FirstUsable_SourceName`

- [`SourceName`](#sourcename)
- object with `first_of` (list of [`SourceName`](#sourcename), required), `mode` (`"usable"`, required), `doc` (string)

### `LogEventPayload`

Where a log event keeps what a span event of the same name keeps in its attributes.

- `"body_members"`: The record's body is a map, and its members are the event's attributes.
- `"attributes"`: The record's own attributes are the event's attributes.

### `ToolShapeRule`

One shape a provider writes a tool definition in, and how to read it as the canonical one.

The canonical form - `{"type":"function","function":{"name","description","parameters"}}` - is **ours**, and
stays in Rust. Every path into a producer's own shape is the asset's, which is the split `as_tool_definition`
already follows. Before this, five readers named `openai`, `anthropic`, `bedrock`, `gemini` and `cohere` in
production Rust and an unrecognised shape was passed through unchanged, then discarded because no name could
be extracted from it - so a producer's shape was not addable as data.

Worth stating why the framework sweep never caught them: markers are derived from *asset ids*, and those five
are **providers**, which the sweep excludes by design because the pricing catalogue is entitled to their
names. A fourth blind spot beside the three its own doc records.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `priority` (required) | integer | Ordered, lowest first, first match wins, and a shared priority is refused - two shapes that both recognise a payload must not be separated by which asset loaded first. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | What makes a payload this shape. Read on the tool value itself. |
| `each` | [`FirstPresent_string`](#firstpresent_string) | Where the definitions are, when one payload holds several. Empty means the payload is one definition. |
| `function` | string | The whole canonical `function` object, for a producer that already writes it. |
| `carry` | list of string | Members of the payload copied onto the canonical wrapper beside `function` - one producer carries `strict` there, and dropping it changes what the tool permits. |
| `name` | string |  |
| `description` | string |  |
| `parameters` | [`ParametersSpec`](#parametersspec) or null |  |

### `ParametersSpec`

Where a tool's parameters are and how they are encoded.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `from` | [`FirstPresent_string`](#firstpresent_string) | Ordered: the first path that resolves is the parameters. One producer writes `inputSchema.json` and the same producer sometimes writes `inputSchema` directly. |
| `encoding` (required) | [`ParametersEncoding`](#parametersencoding) | **Declared**, not guessed from the content. It was guessed: a member named `type` inside an argument map made the map look like a finished JSON Schema, so `{"type":"str","query":"str"}` was emitted as a schema whose type is `str`. The argument named `type` decided how the whole representation was read. |

### `ParametersEncoding`

How a producer encodes a tool's parameters.

- `"json_schema"`: Already a JSON Schema object: taken as it stands.
- `"argument_map"`: A map from argument name to its facts - `{"city": {"type": "string", "required": true}}` - which becomes a JSON Schema object. Every supported constraint is kept: a converter that dropped `required` said an argument was optional when the producer said it was not.

### `EventRole`

What role a message's **source name** implies, where the name itself decides it.

Its own section rather than a member of `message_events`, because the two lists are not the same
vocabulary. `message_events` says which *OTLP events* carry messages; this says what a **source name**
means, and a source name may also be one a rule assigned with `tag_as` - `gen_ai.tool.result` is exactly
that, a tag no producer emits. Putting the role on the event entry would have made declaring the role of
a tag impossible without also claiming a producer emits it.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This declaration's identity, required like every other clause's. The compiled form used to *synthesize* one from the asset and the event name, which is not an identity a declaration can be held to: two assets agreeing about a role produced one witness and the other's provenance was lost. |
| `name` (required) | string | The source name: an event a producer emits, or a name a rule assigns with `tag_as`. |
| `role` | string | The role on an ordinary span. Absent leaves the role to the content, which is a statement rather than an omission - most events say nothing about the role. |
| `role_in_tool_span` | string | The role instead, on a **tool execution** span. |
| `direction` | [`MessageDirection`](#messagedirection) or null | Which side of a generation the source's messages are on: what it was given, or what it produced. |
| `doc` | string |  |

### `MessageDirection`

Which side of a generation a source's messages are on.

Written as one of `"input"`, `"output"`.

### `RoleAuthority`

What authority a **stated role** carries, as two independent facts.

They were fused, differently on each path: whether a stated role survives event-name derivation was a
hardcoded Rust list, and whether it outranks a *tagged attribute name* was that list **or** whatever the role
alias table happened to fold. The alias table's job is folding spellings onto four canonical roles, which is
not a statement about authority - so adding an alias silently granted it authority over a declared tag.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `role` (required) | string | The spelling, as a payload states it. Matched case-insensitively, so it is declared in lower case. |
| `survives_event_derivation` | true or false | A stated role of this spelling is **not** replaced by the role an event name declares. |
| `outranks_a_tag` | true or false | A stated role of this spelling outranks the name a *tagged attribute* reading was found under. |
| `means` | [`ChatRole`](#chatrole) or null | The canonical role this spelling means, for a spelling that is not itself one. |

### `ChatRole`

Standard chat roles

One `strum` declaration for the canonical spelling, which is also the JSON one. The *aliases* - many
producers' words for these four roles - are declared beside each spelling's authority in the assets'
`role_authority` section, and `try_from_str` reads them from there.

Written as one of `"system"`, `"user"`, `"assistant"`, `"tool"`.

### `ContentBlockRule`

One content-block shape, and the canonical block it becomes.

Exactly one target form per rule, checked at compile time. The forms are the canonical SideML blocks,
so this is not a general object builder: a rule says *where* a call's name is, never what a tool_use
block looks like.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `at` (required) | [`ChainPosition`](#chainposition) | Where in the normalisation chain this case is tried. Declared, because the chain's order decides which dialect answers for a shape more than one of them recognises. |
| `priority` (required) | integer | Position among the cases at that point, lowest first. Unique within the position. |
| `where` | [`Expr_ValuePredicate`](#expr_valuepredicate) | The shape this case recognises. |
| `tool_use` | [`ToolUseBlock`](#tooluseblock) or null |  |
| `tool_result` | [`ToolResultBlock`](#toolresultblock) or null |  |
| `json` | [`JsonDataBlock`](#jsondatablock) or null |  |
| `text` | [`TextBlock`](#textblock) or null |  |
| `media` | [`MediaBlock`](#mediablock) or null |  |
| `thinking` | [`ThinkingBlock`](#thinkingblock) or null |  |
| `unwrap` | [`UnwrapSpec`](#unwrapspec) or null |  |
| `splice` | [`SpliceSpec`](#splicespec) or null |  |
| `refusal` | [`RefusalBlock`](#refusalblock) or null |  |
| `redacted_thinking` | [`RedactedThinkingBlock`](#redactedthinkingblock) or null |  |
| `unknown` | [`UnknownBlock`](#unknownblock) or null |  |

### `ChainPosition`

Where a content-block case sits relative to the provider wire formats.

- `"message_envelope"`: Before any provider format, and **only when normalising a message's own content block**.
- `"before_provider_formats"`: Tried before any provider format. For a dialect whose own spelling a provider format would otherwise claim.
- `"provider_formats"`: The provider wire formats themselves - the shapes a model API defines and every framework relays. Between the envelopes and `after_provider_formats`, where the retired Rust readers sat.
- `"after_provider_formats"`: Tried after them, which is where a dialect's additions to a provider's vocabulary belong.

### `ToolUseBlock`

A model asking for a tool to be run.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `id` | [`FirstUsable_IdSource`](#firstusable_idsource) | Ordered: the first member holding a non-blank string, else a declared `template`; absent is reported as null, because a provider that omits an id has still made the call. |
| `name` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | Required: a nameless call names nothing to run, so the case does not recognise the block. |
| `input` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | Ordered, and an **empty object counts as absent** - a dialect that renamed this member leaves the unused one present as `{}`, so "the first that resolves" would always pick the empty one. |

### `FirstUsable_IdSource`

- [`IdSource`](#idsource)
- object with `first_of` (list of [`IdSource`](#idsource), required), `mode` (`"usable"`, required), `doc` (string)

### `IdSource`

One place a call's id may come from.

- string: A member of the block, by RFC 9535 JSONPath.
- [`IdTemplate`](#idtemplate): An id built from the call itself, for a provider that states none.

### `IdTemplate`

A synthetic id: literal text with closed placeholders - `{name}` (the call's resolved name) and
`{stable_hash(input)}` (sixteen hex digits of 64-bit FNV-1a over the resolved input's serialisation). The last
source of an id, since it always yields; two calls of one tool with different arguments get different
ids, and the same call re-sent gets the same one.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `template` (required) | string |  |

### `FirstUsable_ValueSource`

- [`ValueSource`](#valuesource)
- object with `first_of` (list of [`ValueSource`](#valuesource), required), `mode` (`"usable"`, required), `doc` (string)

### `ToolResultBlock`

What a tool returned.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `tool_use_id` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |
| `name` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | Ordered; omitted when no path resolves. A result may carry both the id that pairs it exactly and the human-readable tool name, and keeping the latter can make an aggregate snapshot at least as rich as a duplicate tool-span observation. |
| `content` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | What the tool returned. Where it is normalised (`content_as` other than `blocks`) it re-enters the chain, so a selector naming the block itself (`$`), or a closed `map` to a value this case recognises, is refused: it would re-enter the case for ever. Re-entry through several cases is bounded at run time, the innermost levels kept as they stand. |
| `content_as` | [`ResultContent`](#resultcontent) | How the selected content is shaped. |
| `is_error` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `ResultContent`

How a tool result's content is shaped once it has been selected.

- `"normalized"`: Normalised as a returned value: a list's members as blocks, a provider block as that block, anything else as it stands.
- `"value"`: The same, and then a list of blocks reduced to the value it holds - a lone text is its string, a lone structured value is that value - which is the form a tool-role message's content takes. For a format that writes a result as a list of blocks, so one result reads alike however it was written.
- `"blocks"`: A list of content blocks built from the value: Python constructor reprs where it holds them, otherwise structured data as one `json` block, text as one `text` block, and nothing as an empty list. For a format whose response member holds what the tool returned, encoded or not.

### `JsonDataBlock`

Structured data that is not prose.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `data` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `TextBlock`

Prose. Only a string is text.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `text` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `MediaBlock`

Bytes, or a reference to them. The block's kind and whether it is a reference are both *derived*.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `kind` | [`MediaKind`](#mediakind) or null | The block's kind, where the format states it rather than leaving it to the media type: an image part is an image whatever its bytes are labelled. Absent, the kind is derived from the media type, and a block with none is a `file`. |
| `media_type` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |
| `media_type_default` | string | The media type when no source states one. |
| `missing_media_type` | [`MissingMediaType`](#missingmediatype) | What a block whose media type is stated nowhere becomes. |
| `data` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |
| `source` | [`MediaSource`](#mediasource) | What the data is: derived from the value, or stated by the format. |
| `name` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | Optional display name, such as the filename a framework retained beside the bytes. |
| `detail` | [`FirstUsable_ValueSource`](#firstusable_valuesource) | How closely a vision model is asked to look at an image. |

### `MediaKind`

A media block's canonical kind, spelled as the SideML block type.

Written as one of `"image"`, `"audio"`, `"video"`, `"document"`, `"file"`.

### `MissingMediaType`

What a media block whose media type is stated nowhere becomes.

- `"decline"`: Not media: the case declines. Under `decoded` the bytes are asked first, since base64 names its own type often enough to be worth asking.
- `"null"`: Media of no stated type, recorded as `null`.
- `"omit"`: Media of no stated type, with no media-type member at all - a reference whose type is the referenced object's business.

### `MediaSource`

What a media block's data is.

- `"decoded"`: Derived from the value: a stored file reference, a `data:` URL (whose payload alone is kept), a URL, or the bytes themselves. A stored reference's or data URL's media type wins over a declared one.
- object with `reference_or` (string, required): A stored file reference where the value is one, else this: for a member the format defines as holding bytes (`base64`) or a location (`url`), which a value's shape must not second-guess. The declared media type stands.
- object with `literal` (string, required): Always this, for a member that holds an identifier rather than content.

### `ThinkingBlock`

A model's own reasoning.

`text` is **not** required: a producer that wraps its reasoning in a member holding no text has still said
the block is reasoning, and the retired reader emitted an empty one rather than falling through - which is
what stops a signature-only block from being read as something else.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `text` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |
| `signature` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `UnwrapSpec`

A wrapper: the block's content is *inside* a member, and the member is normalised in its place.

The one form that does not build a block. Several dialects wrap a content block in a member of their own -
a serialisation envelope, a constructor's keyword arguments - and what is inside is an ordinary block of
whatever shape. So the case selects it and the chain starts again from the top with that value.

A case whose member does not normalise answers nothing, which leaves the **original** block to the rest of
the chain: that is what the retired readers did, and it is why an unwrap is not a claim.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `from` | [`FirstPresent_string`](#firstpresent_string) | The first member that is present is unwrapped, whether or not it normalises. |
| `parse_json` | true or false | The member is a block serialised as JSON text, decoded before it is normalised. A member that does not decode leaves the original block to the rest of the chain, as one that does not normalise does. |

### `SpliceSpec`

Several blocks written as one: the block's member is a **list** of blocks, and each takes the block's place.

The one form that answers with more than one block, so it is legal only at `message_envelope` - a
message's content is a list a block can be spliced into, while every other caller of the chain asks for a
single block. A dialect that wraps a provider's whole content list in one part of its own (a text part
whose content is the list) otherwise renders the list as one unknown block. Each member is normalised on its
own terms, and may itself be spliced; the recursion is bounded because a member is always strictly inside
the block that held it. A member that is not a list leaves the block to the chain.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `from` | [`FirstPresent_string`](#firstpresent_string) | The first member that is present is the list, whether or not it is one. |

### `RefusalBlock`

A model's refusal to answer. Only a string is a refusal message; a case whose member holds anything else
has not recognised one, and the block is left to the rest of the chain.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `message` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `RedactedThinkingBlock`

Reasoning the provider withheld, kept as the opaque payload a later request replays. The payload is not
required, as reasoning's text is not: a producer that names the block has said what it is.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `data` | [`FirstUsable_ValueSource`](#firstusable_valuesource) |  |

### `UnknownBlock`

A block of a recognised kind in a variant nothing reads, kept whole rather than misread. For a format
that announces new variants of a block: claiming it as unknown stops a later case from reading the
announcement as something else, and the raw block survives for whoever reads it next.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |

### `ProviderAlias`

One `gen_ai.system` value, and the catalogue provider it means.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `system` (required) | string | The value as the producer writes it, **after** separator and case normalisation - which stays in Rust, since it is about spelling rather than about who wrote it. |
| `provider` (required) | string | The provider in the catalogue's own vocabulary. |

### `FinishReasonSpellings`

One finish category, and every spelling producers write it in.

A provider's word for why a response ended is a fact about that provider (`end_turn`, `STOP`, `endTurn`),
and as a Rust match arm it was a list of providers the code had to know. The category is this engine's
own vocabulary; the spellings are the assets'.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `means` (required) | [`FinishReason`](#finishreason) | The category every spelling below means. |
| `spellings` (required) | list of string | The spellings, compared with case and word separators (`_`, `-`, space) folded away - so `end_turn`, `END_TURN` and `endTurn` are one spelling and an asset states it once. |

### `FinishReason`

Why a response ended, in this engine's own categories.

Which provider word means which category is declared in the assets' `finish_reasons` sections, not here.

- `"stop"`: The model finished its answer.
- `"length"`: A token limit cut the answer off.
- `"tool_use"`: The model asked for a tool.
- `"content_filter"`: A content or safety filter stopped the answer.
- `"error"`: The generation failed.

### `SyntheticCallId`

A call id a producer synthesises when the provider supplied none, in a form that names the tool.

Correlation reads it to repair a result whose synthetic id dangles: the id's name part may identify the
call it answers. The template is closed - `{name}`, a literal separator, `{index}` - and the name is
everything before the **last** separator, so a tool name that contains the separator survives.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `template` (required) | string | `{name}<separator>{index}`: a non-empty name, the separator, and a non-empty run of ASCII digits. |

### `EventCategoryRule`

What an event a producer names in its own words is, by a word the name contains.

For the events no convention names: a producer's retrieval or evaluation event is recognisable by its name
long before anyone lists it. Ordered by `priority`, lowest first; the first whose word the name contains answers.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `priority` (required) | integer |  |
| `contains` (required) | list of string | Words any one of which the event name contains, case-sensitively. |
| `category` (required) | [`EventCategory`](#eventcategory) |  |

### `EventCategory`

The categories an event's name alone may establish.

- `"retrieval"`: Material retrieved for the conversation.
- `"observation"`: A score, an evaluation, an observation about the run.

### `MessageMemberRule`

One member name, and what its presence means.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `members` (required) | list of string | The member, in every spelling producers write it - **one declaration, one flag vector**. |
| `priority` | integer | Where this sits among the members answering the one **ordered** question it answers - each `holds_*` below picks one member, so each is ordered. Required for those, and refused otherwise: a priority that orders nothing is a statement the engine does not read. |
| `holds_content` | true or false | This member holds a message's content, at the rank above. |
| `means_message_shaped` | true or false | Its presence means the value is message-shaped, so it is not bare structured output to be wrapped. |
| `means_content_block` | true or false | Its presence means the value is a content **block** - so a block carrying it that no case recognised is a *malformed* block rather than plain data, and is reported as unknown instead of as JSON. |
| `means_tool_call` | true or false | Its presence on a content block means the block is a tool **call** - the calling side of a tool message. |
| `means_tool_result` | true or false | Its presence on a content block means the block is a tool **result**. |
| `holds_tool_calls` | true or false | This member holds a message's tool calls, so a tool message carrying it is the calling side. |
| `holds_bundled_tool_result` | true or false | This member holds one result of a bundle: a tool message whose content lists several items holding it is several results, split so each pairs with its own call. |
| `holds_result_call_id` | true or false | Inside a bundled result, this member holds the id of the call it answers. |
| `holds_streamed_reply` | true or false | On a message-array carrier, this member holds a streamed response's combined text instead of messages: one assistant reply. |
| `holds_detached_system` | true or false | Beside a message array, this member holds the system prompt - as text, or as blocks whose text is joined - which the array itself does not carry. |
| `marks_control_block` | true or false | A structured-data block holding exactly this member, as an object, is a provider **control** instruction - a cache marker - and not conversation content, so the feed omits it. |
| `wraps_structured_value` | true or false | An object holding this member is a wrapper around the structured value under it: a tool result's identity is the value's, not the wrapper's. |
| `alias_of` | string | This member is another producer's spelling of one of SideSeat's own members, read where that one is absent. Ordered among the spellings of the same member. |
| `wraps_tool_call` | true or false | An entry of a message's tool calls holding this member has the call under it rather than in itself. |
| `holds_context` | string | Beside a message's content, this member holds context of this kind - grounding, citations, sources - that the message shows as a context block. |
| `holds_context_parts` | [`ContextParts`](#contextparts) or null | Beside a message's content, this member is an object whose members are contexts: the ones named here under their own kind, everything else together under `rest`. |
| `may_hold_media_bytes` | true or false | A member that may hold inline media bytes, which ingestion stores once and replaces with a reference. |
| `holds_prose` | true or false | A member holding prose a person wrote or read, never replaced as media whatever it looks like. |
| `marks_producer_shape` | true or false | A block holding this member beside SideML's own members is a producer's shape, not a canonical block, so the canonical passthrough leaves it to the declared cases. |

### `ContextParts`

An object member whose own members are contexts.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `parts` (required) | map of text to string | Members read as their own context, by the kind each is. |
| `rest` (required) | string | The kind every other member is, together. |

### `ClassifyRule`

One classification rule: the conditions a span must satisfy, and what it is then.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `priority` (required) | integer | Where this sits in the ordered sweep, lowest first. The first rule that holds answers. |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) | When the span is this. Classification is given the span's name and attributes only. |
| `result` (required) | string | What the span is, in the stored vocabulary. Mapped to the enum by the caller, which is the one thing about this that is not a producer's business. |
| `replaces_legacy_result` | string | The retired heuristic sweep's answer when this rule intentionally corrects it. |

### `SpanFactRule`

One dialect's evidence for a fact about a span.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `fact` (required) | [`SpanFact`](#spanfact) | The fact this is evidence of. |
| `signals` (required) | list of [`SpanSignal`](#spansignal) | Any one of these establishes it. |

### `SpanFact`

A fact about a span that rules and readers ask about by name.

- `"tool_execution"`: The span *is a tool running*, so its messages are that tool's input and result rather than a model's turn. Rules that must not read such a span are gated on it (`reads_tool_spans`).
- `"tool_failed"`: The tool the span ran **failed**, by the producer's own statement, whatever the span's status says: the span is reported as an error.

### `SpanSignal`

One piece of evidence. At least one form, and both together read as a conjunction.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `doc` | string |  |
| `where` (required) | [`Expr_SpanCondition`](#expr_spancondition) | The evidence, over the span's name and attributes. A conjunction is written as `all`: a tool name alone sits on a model span that merely mentions a tool, while the name *and* a call id together are a call being run. |

### `Fragment`

A named table of readings, applied wherever a rule references it.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string |  |
| `cases` (required) | list of [`Alternative`](#alternative) | The cases, tried in order; the first that yields wins. |

### `SdkSlug`

One SDK-declared slug and the label it resolves to.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `slug` (required) | string |  |
| `label` (required) | string |  |

### `SpanFieldRule`

One stored field, and every key a producer might carry it under.

Ordered: the **first source that yields a value of the target's type** wins, which is what a fallback
chain means. Not claimed, unlike a message carrier - a key naming a model is evidence about the model
whoever else reads it, and two fields legitimately read one key (`gen_ai.request.model` answers both the
request model and, for a provider that never states a response model, nothing else).

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string |  |
| `doc` | string |  |
| `target` (required) | [`FieldTarget`](#fieldtarget) | The stored field this resolves. This engine's own vocabulary, not any producer's. |
| `combine` | [`FieldCombine`](#fieldcombine) | How several yielding sources combine. |
| `sources` (required) | list of [`FieldSource`](#fieldsource) | The sources, in the order they are consulted. |

### `FieldTarget`

A stored field a rule may resolve.

An enum rather than a free string: a typo in an asset would otherwise be a field that is silently never
filled, and the sink has to know each field's *type* - the outcome of reading `http.status_code` is an
integer or a malformed value, and "the string 200" is not an answer this can store.

- one of `"usage_candidate_output"`, `"usage_candidate_cache_read"`, `"usage_candidate_total"`, `"usage_summed_output"`, `"usage_output_tokens"`, `"usage_total_tokens_reported"`, `"usage_cache_read_tokens"`, `"usage_cache_write_tokens"`, `"usage_reasoning_tokens"`, `"reported_cost_input"`, `"reported_cost_output"`, `"session_id"`, `"gen_ai_system"`, `"gen_ai_operation_name"`, `"gen_ai_request_model"`, `"gen_ai_response_model"`, `"gen_ai_response_id"`, `"gen_ai_temperature"`, `"gen_ai_top_p"`, `"gen_ai_top_k"`, `"gen_ai_max_tokens"`, `"gen_ai_frequency_penalty"`, `"gen_ai_presence_penalty"`, `"gen_ai_stop_sequences"`, `"gen_ai_finish_reasons"`, `"gen_ai_agent_id"`, `"gen_ai_agent_name"`, `"gen_ai_tool_name"`, `"gen_ai_tool_call_id"`, `"gen_ai_server_ttft_ms"`, `"gen_ai_server_request_duration_ms"`, `"user_id"`, `"http_method"`, `"http_url"`, `"http_status_code"`, `"db_system"`, `"db_name"`, `"db_operation"`, `"db_statement"`, `"storage_system"`, `"storage_bucket"`, `"storage_object"`, `"messaging_system"`, `"messaging_destination"`, `"tags"`
- `"usage_candidate_input"`: Usage **candidates**: what one dialect's embedded object states, resolved whatever the counter chains answered.
- `"usage_summed_input"`: Usage a dialect records **per message**, summed. Candidates rather than counter sources, because the decision reading them is a *pair*: that dialect fills both sides together when either summed to anything, so one side's silence is not the same fact as the pair being absent. The paths are data; the pairing is arithmetic and stays code.
- `"usage_input_tokens"`: Token counters. These do **not** reach the stored span through `apply_field`: the columns are `i64` and never null, so writing one there would lose the difference between a counter nothing carried and a genuine `0` - which every framework fallback downstream needs, and which no arithmetic can recover.
- `"reported_cost_total"`: A cost the producer priced itself and stated on the span. The fallback when our own pricing knows nothing about the model; which of the two a reader sees is enrichment's decision, not the asset's.
- `"display_span_name"`: The name a reader sees. **Not** the raw span name, which everything behavioural keys on: detection, token scoping, classification and every rule are given the producer's own name, and this is a presentation value stored beside it.
- `"metadata"`: A producer's own metadata object, stored as the JSON it encodes; text that is not JSON is no metadata.

### `FieldCombine`

What happens when more than one source yields.

- `"first_wins"`: The first yielding source answers and the rest are not consulted.
- `"merge_all"`: Every source contributes, in order, duplicates dropped. Only a list field may say this.

### `FieldSource`

One place a field's value may be written.

| Key | Type | What it is |
| --- | --- | --- |
| `id` (required) | string | This clause's own name, unique within the rule or fragment that holds it. |
| `doc` | string |  |
| `on_malformed` | [`MalformedPolicy`](#malformedpolicy) | What a **present but unreadable** value means for the rest of the chain. |
| `accept_empty` | true or false | Whether an **empty** value from this source is an answer rather than something to step over. |
| `attribute` | [`FirstPresent_string`](#firstpresent_string) or null | A flat span attribute holding the value directly - or, as `{"first_of": [...]}`, several spellings of one value where the **first present** one is the answer. |
| `json` | [`JsonFieldSource`](#jsonfieldsource) or null | A member of a JSON-valued attribute, reached by RFC 9535 JSONPath. |
| `event_attribute` | [`EventAttributeSource`](#eventattributesource) or null | An attribute of one of the span's **events**. |
| `span_name` | true or false | The span's own name, as the producer wrote it. |
| `pipe` | list of [`Transform`](#transform) | What happens to the value read, in this order: `strip_prefix` (on the span name only), then `lowercase` - for a field whose values are a **case-insensitive enum** and are stored lower case (one dialect writes `STOP` where the conventions write `stop`). No other step and no other order. |
| `when_json` | [`JsonFieldSource`](#jsonfieldsource) or null | A JSON member whose *presence* admits this source, whatever it holds. |
| `value` | string | A literal this engine states because a *shape* implies it. |
| `where` | [`Expr_SpanCondition`](#expr_spancondition) or null | Consulted only when this holds of the span, over its name and attributes. |

### `MalformedPolicy`

What a source does when the value it names is present and cannot be read as the field's type.

Declared per source because the retired chains disagreed, and each disagreement was deliberate. A status
code written as a phrase means the producer's own status attribute is wrong, and answering from a *second*
key reports another attribute's number as this call's. A request parameter written badly is different: the
flat attribute is one of several places a framework may state it, and the retired code fell through to the
serialised parameter object - which is the same value from the same producer, not a different call's.

- `"stop"`: The field is not filled, and the source that stopped it is named in the diagnosis.
- `"continue"`: Step over it and keep looking, as a chain does for an empty value.

### `JsonFieldSource`

A value inside a JSON-valued attribute.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `attribute` (required) | string | The attribute whose text is parsed. Parsed once per span however many sources name it. |
| `path` | [`FirstPresent_string`](#firstpresent_string) or null | Where in it the value sits - or, as `{"first_of": [...]}`, several spellings of one member where the **first present** one is the answer. |
| `reduce` | [`Reduction`](#reduction) or null | Combine every match of a plural path into one value, rather than taking one of them. |
| `scalar_only` | true or false | Each match must be a **scalar string**; an array at the match is malformed here. |

### `Reduction`

How several matches of one path become one value.

- `"sum"`: Add them. A match that is not a number makes the whole reduction malformed rather than contributing nothing: an unreadable count and a zero are different statements, and the field's `on_malformed` decides.
- `"collect_all"`: Keep every match, in the order the path found them.

### `EventAttributeSource`

An attribute of one of a span's events.

| Key | Type | What it is |
| --- | --- | --- |
| `doc` | string | Why this is declared the way it is, for a reader and the explain trace. Read by nothing. |
| `event` (required) | string | The event whose attributes are read. |
| `attribute` (required) | string | The attribute on that event. |
| `occurrence` | [`EventOccurrence`](#eventoccurrence) | Which occurrence answers, when a span carries the event more than once. |
| `path` | string | Where in the attribute's JSON the value sits, by RFC 9535 JSONPath; absent means the attribute's text is the value. The event counterpart of a `json` source's `path`: an event can carry a serialised payload as a span attribute can, and the conventions' inference-details event holds the output messages, each stating why the model stopped. The first match that yields answers. |
| `scalar_only` | true or false | Each match must be a scalar string, as a `json` source's `scalar_only` says. |

### `EventOccurrence`

Which occurrence of a repeated event supplies the value.

- `"first_yielding"`: The first occurrence that holds the attribute at all. What the retired reader did.

<!-- END GENERATED -->
