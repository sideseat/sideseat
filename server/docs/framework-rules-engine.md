# The framework rules engine

Design accepted with Codex in thread `01a0764b`, after one revision round; the amendments it required are
folded in below rather than appended, so this file is the agreed design and not a transcript. The review
continued across later threads, and the **acceptance of the finished work** was given in thread `01a07fef`
at cycle 40 — quoted verbatim under **Acceptance**.

> **How to read this file.** Everything up to **Migration** is the *design as accepted*, written before it
> was built. Everything from **Migration** down is what landed. Where the two differ the Migration section
> and **Acceptance** are authoritative, and the difference is marked at each design section rather than left
> for a reader to discover — a design document in the present tense reads as a description of the code, which
> is the one thing it must not be mistaken for.

## The mandate

Every piece of knowledge about a *specific* framework or provider moves out of Rust and into
declarative per-framework assets, interpreted by a generic engine. The success criterion, stated
literally: **the Rust code knows nothing about any specific framework, and every framework still
parses correctly.**

Mechanised, so it is a gate rather than an aspiration:

> Adding a currently-expressible framework consists only of adding data files and fixtures. The generic
> Rust **source** contains no producer ids, carrier keys, tags, type mappings, or producer-specific
> branches outside the embedded rule data, and no behavioural API even *accepts* a framework label. Legacy
> and rules output are identical before *and* after reconstruction, across the documented support matrix.

The *binary* necessarily contains every producer id, because the assets are embedded in it — that is what
makes a release self-contained. The claim is about the Rust that interprets them.

### Scope, stated narrowly on purpose

The mandate covers **observability interpretation** — detection, extraction, carrier semantics,
normalisation, ordering, token and cost conventions — plus **declarative product metadata** (the MCP
setup guide, the provider catalogue, credential labels, environment variables, ambient-detection
metadata).

It does **not** cover concrete provider *connectors*. `providers/test_connection.rs` constructs real
clients with real auth flows; that is executable adapter code, and calling it "plain data" would be
dishonest. Connector implementations stay in Rust, outside this engine. If they ever need to be
pluggable, that is a separate driver interface, designed separately.

**What the cycle-40 acceptance covers is narrower than this mandate**, and the difference matters because a
document that states one scope and accepts another reads as accepting the larger. The acceptance is about
**agent-framework telemetry interpretation**: detection, extraction, carrier semantics, normalisation,
classification, content forms, and the token / cost / model conventions. The *product metadata* half of the
mandate - the MCP setup guide and the provider catalogue - is **step 10, and it is not done**: both are
Rust tables today, held instead by the two scoped exemptions the sweep records. They are outside the
acceptance rather than covered by it, so "the work is complete" below means complete against the two success
criteria, not against every step listed here.

## Verdict: a versioned, non-Turing-complete tree-transformation DSL, compiled to a typed plan

| | Model | Ruling |
| --- | --- | --- |
| **A** | Pure-data rules over a fixed vocabulary of generic primitives | **Chosen** |
| B | Embedded sandboxed script (Rhai / WASM / JSONLogic) | Rejected — buys arbitrary expressiveness by *weakening* the mandate: rule files would hold logic, and the engine would become a language runtime without the tooling one needs |
| C | Rules selecting named Rust `Transform` impls | **Prohibited.** A transform that exists only for one framework is that framework's policy wearing a generic name |

B is warranted only under a requirement nobody has stated — "an arbitrary future framework must be
addable without releasing a new server, whatever its shape". A cannot promise that, and the mandate
does not ask for it.

## What the survey changed: framework identity is not behavioural

`Framework::<Variant>` appears outside `data/types/enums.rs` **only** in `attributes.rs` — the
detection rules that *produce* the label. Nothing downstream (extraction, normalisation, dedup, cost,
feed, query, API) branches on it. The stored string is a DB column, a list/stats filter, a display
field, and one opaque input to the feed cache digest.

So there are essentially **no `if framework == X` conditionals to untangle**. All framework behaviour
is already keyed on *data*: attribute keys and prefixes, event names, role strings, content-shape keys,
provider aliases, span-name prefixes, carrier names. The work is externalising tables and
vocabularies, which is why model A is achievable here rather than aspirational.

The target is therefore stronger than a renamed enum:

- `framework_label: Option<String>` — provenance, display, filtering, statistics. Produced by
  detection rules; consumed by nothing that decides behaviour.
- `RuleId` — diagnostics, explain traces, metrics.
- **No framework identifier reaches carrier, extraction, normalisation, ordering, dedup, token or cost
  behaviour.** Rust cannot know about frameworks if nothing consults framework identity.

Rules compile into **global indexes** by carrier, event, key, scope, span shape and content shape.
Rule files are organisational units; their clauses are indexed globally. Detection never selects a
parser.

*What landed:* the last sentence holds — detection never selects a parser. The indexing is partial:
**carrier** rules are indexed by exact name and by prefix, and everything else is a compiled vector scanned
per span (`from_event` filters on `when_event`, `stage` walks a stage's rules). See **Performance** for the
three parts of that design that are not built.

`scope_name` / `scope_version` (instrumentation-library identity, already extracted, persisted, and
part of the feed cache digest) is **high-confidence matching evidence, not an exclusive dispatch key**:
historical rows may lack it, several frameworks share instrumentation packages, and versions are not
reliably semantic. Scope constraints *narrow* candidates; carrier and shape remain the fallback.

### Three corrections to the original premise, verified in the code

1. **Extraction runs at ingestion, not query time.** `pipeline.rs:2082` extracts; the message queries
   select `messages_json` and never `raw_span`. So "a fix applies to history without re-ingestion"
   covers *normalisation, carrier semantics and ordering* — query time — and **not** extraction.
   Moving extraction to rules does not make extraction retroactive, and promising otherwise would
   mislead operators. Making it retroactive is a separate storage project (a versioned raw envelope
   plus a parse cache keyed by ruleset hash); note `MESSAGE_CONTENT_FILTER` excludes rows whose
   ingestion-time extraction produced nothing, so re-interpreting archived spans at query time would
   still miss exactly the spans a new rule would newly recognise.
2. `EXTRACTORS` held **16** entries — 15 framework extractors plus `raw_io`. It no longer exists.
3. The six carrier facts are **not** the whole policy surface. `order_graph.rs:188` hardcodes
   `llm.input_messages` as the fragmented-input family — a seventh semantic fact hiding in Rust — and
   source-direction, event→role and expandable-array tables live in `normalize.rs` and
   `feed/types.rs`. A migration that touches only `try_*` fails the criterion.

   *What landed, all four:* there are **eight** carrier facts now plus a named `ordering_family`, read at
   `feed/order_graph.rs:206` — so the seventh fact is declared rather than hardcoded. Source direction is
   `carrier_holds_span_input` / `carrier_holds_span_output` and the expandable array is
   `carrier_holds_expandable_message_array`. Event→role was the last one and was still a `match` in
   `normalize.rs` until `8e3ab002`; it is `event_roles` in the assets now, which is where the one arm that
   existed for a single CLI's `tool.output` event belongs. This correction was right, and finding the fourth
   item took auditing the document against the code rather than the reverse.

## Is a generic primitive honest? The six tests

The hard question is whether a `parse_python_literal` primitive — reachable, when this was written, only
from `try_crewai` — is a generic capability or CrewAI policy under an assumed name. (`try_crewai` is an
equivalence oracle now; production reaches that grammar through a declared `ToolReprSpec`, and the module
holding it names no framework.) A primitive is generic
only if it passes **all six**:

1. **Name erasure** — its implementation and tests read sensibly with every framework name and carrier
   key removed.
2. **Policy separation** — it does not decide *when* it runs, which carrier wins, the role, the
   framework, precedence, or output direction.
3. **Parameterised literals** — keys, tags, type names, labels and mappings are rule arguments, never
   constants in the implementation.
4. **General contract** — it implements a grammar or a general data operation (parse, walk, join,
   group, project), not "normalise framework X".
5. **Change locality** — a framework changing a key, tag or type mapping is a rule-file edit alone.
6. **Synthetic testability** — it can be tested against examples from no framework at all.

`parse_python_literal` **passes**: it implements an external serialisation grammar, and the codebase
already carries effectively the same parser generically in `content.rs`. Its accepted language must be
*specified* rather than described as "Python repr" — today it handles dict/list-shaped strings, quotes
and `True`/`False`/`None`, not arbitrary Python literals.

`crewai_args_to_json_schema`, `map_crewai_type` and string literals such as `"Tool Arguments:"`
**fail**: those are mappings, and mappings are data.

### Escape-hatch policy

No inline scripts. No rule-selected Rust callbacks. A new primitive needs a short RFC showing it
passes the six tests, has synthetic tests, bounded complexity and a benchmark. Adding one is
*acceptable engine evolution* and does not breach the mandate — unless its implementation embeds
producer policy. A shape no primitive expresses stays unsupported until a generic primitive exists; it
never gets a private hook.

## The primitive vocabulary

Derived from what the 16 extractors measurably do, not invented up front.

| Capability | Required by |
| --- | --- |
| Exact / path / wildcard selection, indexed-prefix capture, dotted-key unflattening | semconv and OpenInference indexed attributes |
| `exists`, type and shape predicates, `all` / `any` / `not`, ordered coalesce, decision tables | detection, AutoGen inference, role and content classification |
| JSON decode, recursive stringified-JSON decode, Python-literal subset, scalar coercion | Vercel tools, AutoGen arguments, CrewAI repr |
| `map`, `filter`, `flat_map`, stable `dedupe`, `best_by`, consecutive grouping | AutoGen multi-result, Logfire role grouping, CrewAI tool quality |
| Bounded tree `walk` with match / prune / emit clauses | LangGraph state |
| Split, captured header/tag, quoted-field and balanced-delimiter extraction | Claude Code, CrewAI |
| Bind / join / lookup by captured index or key | OpenInference multimodal enrichment |
| Object and list construction, conditional fields, canonical message/block/tool emitters, emit-many | every extractor |
| Carrier claiming, fallback conditions, provenance, semantics assignment | the existing per-carrier dispatch |

Qualifications that matter:

- `recursive_find(key, depth)` is **too weak** for LangGraph, which recognises message-shaped values,
  message arrays, a sibling `raw` answer and nested state, and prunes what it already processed. It
  needs a general bounded `walk`.
- AutoGen is a **decision table** plus `flat_map`/emit-many, despite the procedural size it has today.
- Claude Code is parameterised section splitting with tag capture; its constants belong in its file.
- OpenInference needs a **cross-carrier indexed join**, not path mapping. *This turned out to be wrong,
  and it was the design's own blocker:* both carriers are attributes of **one span** and the join is
  positional, so it landed as an `overlay` declared beside the family that needs it, with no query-time
  stage. Step 8 records why.

"Minimal" cannot be proved from goldens. A **branch ledger** — proposed here, **never built**; what was
built instead is the sixteen equivalence oracles, which hold each migration to the code it replaced rather
than to an inventory of its branches — maps every legacy branch and helper to a
rule clause, a primitive and a focused test, and dual-run comparison happens **before** downstream
dedup, because final goldens can hide extractor differences.

## The selection language: RFC 9535 JSONPath, and why not a projection language

The rule vocabulary grew a path resolver, a predicate evaluator, a selector and an object constructor by
hand — the four things an expression language already standardises, and the hand-built resolver is where a
real bug lived (a literal dotted key read as a nested path). So selection and filtering move to an
existing standard with an existing parser: **RFC 9535 JSONPath**, via `serde_json_path`.

**JMESPath was implemented first and reverted.** It is the more capable language — a multiselect hash
*constructs* objects, so `contents[].{role: role, content: parts}` expresses a dialect's whole reading in
one expression, which JSONPath cannot do. It was rejected on a measurement:

> Every JMESPath result is re-materialised through the crate's own **sorted-map** value tree. A payload
> merely *selected* comes back with its object members alphabetised — even `contents` with no transform at
> all.

On the corpus that reordered the provider's own tool-result payload in `adk/image_gen` and `adk/tool_use`:
`{"status":"success","content":[…]}` became `{"content":[…],"status":"success"}`. Identity was provably
unaffected — **every content digest was identical**, because the feed sorts keys before hashing — so
deduplication and ordering could not have noticed. But this repository declares serialised member order
observable, and a debugger that silently re-orders what a provider sent reports something it was not
given. Identity and fidelity are separate contracts.

JSONPath avoids it structurally rather than by luck: its queries return **borrowed references** into the
original `serde_json::Value`, so cloning a selected subtree keeps the `preserve_order` map. It also has
the right existence semantics — a filter on `@.parts` holds when `parts` is `[]`, which is what the
structural vocabulary means by "present", and exactly where JMESPath differed (an empty array is
false-like there, so a real turn was dropped).

The general point, not a fault of one crate: **JSON objects are semantically unordered, so no portable
construction language can promise member order.** Adopting one forfeits it.

So the seam is:

```
declared carrier → parse → JSONPath select/filter (borrowed) → structural first/all/fallback/walk/join/
group → typed SideML constructor → provider subtrees cloned unchanged
```

with these rules:

- an expression may select nodes or decide a branch, and its result is **never** emitted directly;
- provider objects and arrays are always cloned from the original value;
- Rust owns constructors only for the canonical SideML targets — message envelopes, content blocks, tool
  definitions — while their fields and aliases stay rule data;
- no general object-construction DSL, and no "scalars-only" exception, which would be hard to review and
  easy to violate silently;
- custom functions are not registered, in either language: they accept arbitrary Rust closures and would
  put loops and I/O back inside something that looks declarative.

`the_selection_language_behaves_as_the_engine_assumes` pins all of it, including a byte-identical
member-order assertion, because the choice rests on that one property.

## Ordered chains: named anchors and relations, never numbers

Several of these tables are ordered `or_else` chains whose **order is policy** — most sharply
`content.rs:321`, where the sequence canonical → wrappers → OpenAI → Anthropic → Bedrock → Gemini →
Vercel → media-fallback → unknown is load-bearing. The same applies to the five-way `tool_use_id`
chain in `tools.rs` and to detection.

Rejected: **global integer priorities** (collisions, renumbering, cross-file ownership) and
**inferred specificity** (there is no reliable specificity ordering over overlapping tree-shape
predicates).

Adopted: an **authoritative named chain**. Rule files define handlers; a shared *composition manifest*
owns their ordering by name. Extensions may declare explicit `before` / `after` / `supersedes` edges.
The compiler topologically sorts and **rejects at compile time**: cycles, unknown anchors, duplicate
handler ids, and ambiguous overlapping handlers with no ordering relation between them. The runtime
explain output shows every matching handler and why one won.

> **Not built. This whole section is a target.** An asset declaring `before` or `after` is refused by
> `deny_unknown_fields` as an unknown member, not topologically sorted. What landed is a **rank and named
> position** model:
>
> | Chain | How order is decided |
> | --- | --- |
> | Content blocks | three named `ChainPosition`s (`message_envelope`, `before_provider_formats`, `after_provider_formats`) and a `legacy_rank` within each |
> | Messages, and both classifications | an explicit integer `rank`, sorted at compile time; a **shared** rank within one classification fails compilation, because which applied would otherwise depend on load order |
> | Detection | first match in rank order, with `legacy_rank` reproducing the retired answer |
>
> So collisions are caught (a shared rank is refused) and renumbering is the cost the design wanted to
> avoid. `overlapping_candidates` exists and is **test-only**, not a production metric, and there is no
> runtime explain output — see the **Explainability** bullet, which is the same gap from the other side.

Detection uses the same machinery, plus sufficiency:

```
any_of: [ all_of: [...], all_of: [...] ]
not: [...]
supersedes: [...]
```

Candidates are collected independent of load order; a unique sufficient candidate wins; an explicit
data-only `supersedes` resolves a known overlap; otherwise an ambiguity metric is reported with all
evidence. During migration only, `legacy_rank` reproduces today's answer while conflicts are
collected, and it is removed as predicates become genuinely sufficient. No opaque weighted scoring —
that replaces visible ordering with ordering nobody can debug.

> **Also a target.** `legacy_rank` is what ships: detection is first-match in rank order, `supersedes` and
> the ambiguity metric do not exist, and `not` / `any_of` / `all_of` landed as `all_of: Vec<DetectMatch>`
> with each conjunct internally a disjunction. The last sentence did hold — nothing is weighted — and the
> rank *is* explicit and refused when shared, which is the property the design wanted from sufficiency.

## Assets

Two kinds, deliberately not one mechanism:

**Transformation rules** (the DSL, compiled to a typed plan) — one entry file per framework, JSON
(zero new dependencies, the house format, and every clause carries a `doc` string the explain trace
can surface, which is strictly better than a comment).

The section names below were the design's; **`RuleFile` is what an asset may actually contain**, and
`deny_unknown_fields` refuses anything else — so the design's `imports`, `attributes`, `content`, `tools`,
`feed`, `tokens`, `cost` and `media` are not members an asset can write:

| Section | What it declares |
| --- | --- |
| `detect` | label-only signals, in explicit rank order |
| `sdk_slugs` | the names an SDK declares itself by |
| `carriers` | carrier + qualifiers → the **eight** facts and the named `ordering_family` |
| `messages` | per-carrier readings, in rank order, with a stage |
| `fragments` | named alternative sets a reading may reuse, which is what replaced `imports` |
| `message_events` | which OTLP events carry messages |
| `event_roles` | what a source name — an event, or a name a rule assigned — says about the role |
| `message_members` | one member vocabulary answering three questions: holds content (ordered), means message-shaped, means a content block |
| `content_blocks` | content-block forms, at a named `ChainPosition` |
| `span_fields` | one ordered resolver per typed target, covering the design's `attributes`, `tokens` and `cost` |
| `observation_types`, `span_categories` | the two classifications, ordered first-match |
| `span_facts` | facts about a span any dialect can establish |
| `provider_aliases` | a `gen_ai.system` value that is a framework's own name and means a provider the catalogue prices |

Two of the design's sections have no counterpart because the question turned out to belong elsewhere:
`feed`'s input/output declarations are carrier *facts* rather than a section of their own, and `media` is a
content-block form (`media`) rather than a section.

**Plain typed manifests** (no DSL, generic rendering) — the MCP setup guide entries, the provider
catalogue, credential labels, environment variables, ambient-detection metadata. No tree transformation is
involved and pulling them into the DSL buys nothing. **None of these has moved**; they are step 10 and
outside the acceptance. Provider *aliases* are not among them: a framework naming itself in
`gen_ai.system` is framework knowledge, so it is a rule section (`provider_aliases`) rather than a manifest.

The generic `gen_ai.*` semconv is itself just a rule file. It is **not imported**: every asset's clauses go
into one global plan, so the conventions' rules apply to every span without a framework file asking for
them — which is why `imports` was never needed and why an asset naming no framework can carry the
conventions for all of them.

**Provider vocabulary reconciliation is still open**, and the prerequisite turned out to be narrower than
this paragraph assumed. Three namespaces coexist (litellm provider names, `gen_ai.system` aliases, UI
credential keys such as `vertex-ai` / `azure-ai-foundry`). What the token and cost migration actually needed
was the part a *framework* owns - where a producer names a provider under its own spelling, which is
`provider_aliases` - and that moved. The remaining two tables are provider vocabulary rather than framework
knowledge, so they sit with step 10, outside the acceptance.

## Verification

- The **120 committed goldens** are the equivalence gate per framework — 104 captured and 16 `_synthetic` —
  and are *not* proof of coverage: 13 captured suites plus `_synthetic`, reaching 9 of the 30 producers the
  assets declare. A strong regression gate, nothing more. (A working copy holds 122: two `image-gen`
  fixtures are gitignored and excluded from the support matrix deliberately, so a count taken from disk
  overstates what a checkout can verify.)
- The **independent invariants** must keep holding (scope containment, per-trace dedup, tool-id
  correspondence, answer-present, determinism, carrier subsequence).
- A **structural gate** that is more than a name grep: no producer ids, carrier keys, tags or type
  mappings in the engine; **no behavioural API accepting a framework label** (a generic-looking
  `String` parameter can smuggle it, so this is a dataflow check, not a literal search); no callback or
  custom-transform registration; and one-way module dependency. The last clause of the original design -
  *every enabled rule has fixtures or a tested shared-dialect inheritance* - is **not** what holds: 39
  message-rule leaves have neither, and `no_declared_rule_is_dead_across_the_corpus` requires each to be
  listed with a reason instead. An enumerated gap, which is weaker than the design asked for and stronger
  than silence.
- **Explainability** — a **design target**, partly built. Explain *data* is on every compiled clause
  (asset, id, doc) and compile-time diagnostics name the rule and the reason; the **rendered per-message
  trace** - every emitted *and* rejected message reporting rule, clause, selected path, transformation,
  carrier claim and rejection reason - is not built. Step 3 is marked landed on its behavioural conditions;
  condition 13 below is this one, and it is the part outstanding.
- Dual-run comparison covers canonical attributes, raw message content, tool definitions and tool names,
  assigned carrier semantics (as a table over the whole carrier vocabulary), and all four views. **Not**
  provenance or position: a golden records per message its index, role, entry type, content, content digest,
  tool name, finish reason and observation type - so a `PositionPath` that changed while every one of those
  stayed equal would pass. Ordering *is* compared, through the index and the role sequence. Goldens are
  never regenerated to make a migration pass, and `UPDATE_GOLDENS=1` still exits non-zero when an invariant
  was violated, so known-bad output cannot be committed as reviewed.

## Performance

Rules compile **once per ruleset** to a typed plan — never parsed, path-resolved, regex-compiled or
string-dispatched per span. That much is built: every JSONPath is a `JsonPath` compiled at load, decision
tables are typed, iteration order is stable, and `MessagePlan::metadata_candidates` precomputes which rules
can contribute a tool definition so the metadata path evaluates thirteen rules rather than sixty on every
span.

Three things in the original design are **not built**, listed because the rest of this section reads as a
description of the runtime:

| Target | What happens today |
| --- | --- |
| Index candidate pipelines by event / exact key / prefix | `from_event` filters the rule list linearly on `when_event`; `stage` walks the rules of a stage. Linear in the ruleset per span |
| Parse each carrier at most once, sharing decoded values | Each reading parses the attribute it reads |
| Depth / iteration / emitted / decoded-byte budgets | No budget exists in `domain/rules`. The bounded walks that *do* exist are per-feature constants elsewhere (`LANGGRAPH_STATE_DEPTH`, the corpus oracle's depth 8), not an engine-wide guard |

None is on the critical path of the acceptance — the read benchmarks hold their ceilings with the ruleset as
it stands, and `bench_session_scaling` shows the pipeline linear in its input. They matter as the ruleset
grows, and a budget matters for a different reason: it is the difference between a hostile payload costing
time and costing the process.

The **ruleset hash joins the reconstruction cache key**. That cache is a memo over a pure function of
the rows (`feed/cache.rs`); once rules can change, they are part of that function, and a dev hot-load
would otherwise serve answers built by another ruleset. Production rules are embedded and immutable, which
is what ships today; **there is no reload mechanism** - the atomic swap of a validated plan and the cache
generation that goes with it are a design target, and the digest is already in the cache key so that
building one cannot silently serve the previous ruleset's answers.

A bounded LRU may cache plan selection by span-shape fingerprint, but indexed attributes make shape
cardinality unbounded, so it is a cache and never a correctness assumption.

## Migration: ten steps, no big bang

The inert enum removes detection as a prerequisite, so it is no longer first.

Each landed step is proved by an **equivalence oracle**: the table it replaced is kept under
`#[cfg(test)]` and compared against the rules, rather than deleted and the goldens trusted. Goldens can
bless a regression; an oracle cannot.

1. ✅ Tighten scope and freeze the inventory.
2. ✅ Rule schema, validator, compiler, immutable typed plan, ruleset hash. Explain data is on every
   compiled clause (asset, id, doc); the *rendered* trace is still to come.
3. ✅ **Carrier semantics** — 32 clauses over 7 dialects *at the time this step landed*, 55 now, proven
   equivalent by
   `the_rules_reproduce_the_legacy_carrier_table`. The aggregator defect is diagnosed, measured and
   deliberately not shipped; see below.
4. ✅ **Label-only detection** — 28 rules and 26 SDK slugs in the assets, proven equivalent by
   `the_rules_reproduce_the_legacy_detection` over 66 cases covering every rule, every dimension and
   the orderings that matter (mutation-verified: moving the Strands rule to the front breaks 3 cases).
   `SpanData.framework` and `NormalizedSpan.framework` are now `Option<String>`; the `Framework` enum
   is reachable only from the oracle, so adding a framework is an asset edit and not a code change.

   All four **Rust matcher callbacks are gone**, which was the real test of the vocabulary: one merely
   duplicated the existing prefix dimension, two were "a resource attribute contains this", and the
   fourth a case-insensitive phrase search over the span name or a named attribute. Two new generic
   dimensions covered all four, so no producer needed a hole opened for it.

   Detection order is **explicit rank**, not file position: the signals genuinely overlap, and a shared
   rank fails compilation because their relative order would otherwise depend on load order. That order
   is load-bearing — the SideSeat SDK defaults `service.name` to one framework's name, so that rule's
   service-name signal must be last or it claims every span of every framework using the SDK.
5. ✅ **Feed source / event / replay / ordering-family tables** — 9 `message_events` say which events
   carry messages and 9 `event_roles` say what a source name implies about the role, and the replay and ordering knowledge is **eight** independent facts per carrier plus a named
   `ordering_family`, declared by each of the 55 `carriers` (`CarrierSemantics`, `sideml/carrier.rs`). Four
   are about what *position* within the carrier proves: `position_proves_distinct_occurrence`,
   `position_provides_sequence_order`, `carrier_is_atomic_emission`,
   `carrier_may_contain_history_or_state`. Four are about what the carrier *is*:
   `carrier_holds_span_input`, `carrier_holds_span_output`, `carrier_is_detached_request_frame`,
   `carrier_holds_expandable_message_array`. They are separate booleans rather than one enum because a
   conversation snapshot and accumulated framework state are both ordered and both may hold history, and
   differ only in whether position proves multiplicity.
6. ✅ **Content and tool normalisation, with explicit named-chain precedence** — 9 `content_blocks` over
   three named chain positions (`message_envelope`, `before_provider_formats`, `after_provider_formats`),
   which are named rather than numbered because the envelope position exists for a reason a number cannot
   record: the nested chain, a tool's returned value, must not consult the envelopes.
7. ✅ **Framework-owned span fields and provider aliases** — 47 `span_fields` across the semantic, GenAI,
   display and usage targets, one ordered resolver per typed target, plus `provider_aliases` where a
   producer names a provider the catalogue prices under another name.

   **Renamed, because the original title claimed more than landed.** Reconciling the *three provider
   namespaces* is not done: two framework-owned Google ADK aliases moved, and the litellm spelling table
   and the UI credential catalogue are still separate Rust tables. That reconciliation belongs with step 10
   and is outside the acceptance - it is provider vocabulary, not framework knowledge, and the token and
   cost conventions that depend on a *framework's* spelling are the part that moved.
8. ✅ **Message extraction** — done. **Every** framework extractor entry is retired. At the time this
   paragraph was first written two entries remained, both naming no framework — the generic
   `declared_rules` entry and the `raw_io` fallback — with 52 message rules declared. Neither remains:
   `EXTRACTORS` no longer exists at all (see below) and there are **71** message rules. Each retirement is
   proved by `the_rules_reproduce_the_extractors_they_replaced` against the code it replaced.

   OpenInference was expected to be the hard one and was almost entirely already
   expressible: `indexed_family` + `entry_member` + `require_members` with `nested` presence
   had been built for exactly its shape, and nine probe shapes disagreed in **one** place —
   a relevance score arriving as `"0.9"` where a score is a number (`numeric_members`, named
   rather than sniffed, because a version or a postcode is text that happens to parse).

   Codex's blocker turned out to be a claim about the implementation rather than about the
   information. `enrich_oi_multimodal_from_input_value` mutates the messages that extractor
   produced, which is why it read as "one rule reading another's output" — but both carriers
   are attributes of **one span**, and the join is positional: entry *n* of the flattened
   family and member *n* of the serialised list are the same message. So it is an
   `overlay`, declared beside the family that needs it, and no query-time stage was
   required. The flattened form loses whole content blocks and writes `__REDACTED__` for a
   url while the serialised copy keeps both, so where both describe one message the richer
   one wins — guarded by a witness predicate (this dialect's own serialisation, not any
   array of objects that happens to sit at that path) and applied only to an entry whose
   content actually arrived flattened.

   An aggregated indexed family may now carry a `wrap`: a retrieval result set is **one**
   observation holding every document, not one message per document, and one array needs an
   envelope saying what it is.

   **`EXTRACTORS` no longer exists.** It held sixteen entries, then two, then one - and a table with one
   entry only hides which code runs, so `try_declared_rules` is called directly. Everything a framework
   writes is declared: messages, tool definitions, tool *names*, and events.

   | What moved | Where it lives now |
   | --- | --- |
   | Sixteen framework message extractors | 71 rules across 32 asset files |
   | The always-on tool-definition readers, including the convention's single-tool triple and its identifier test | `emit: tool_definitions` / `tool_names` rules, read by `MessagePlan::tool_definitions` on every span |
   | `try_raw_io`, the generic last resort | `stage: fallback` rules; the stage is the engine's, *when* it runs is the caller's (generic: nothing recognised the span, or a generation span's answer is unaccounted for) |
   | The event whitelist and its two Strands shapes | `message_events` per asset, plus `when_event` rules read by `MessagePlan::from_event` |
   | CrewAI's Python-`repr` tool grammar | sealed in `rules/tool_repr.rs`, reached through a declared `ToolReprSpec` |

   Measured and **enforced**: `messages.rs` has 12 functions outside `#[cfg(test)]`, not one names a
   framework, and `message_extraction_names_no_framework` reads the source for producer names *and* carrier
   keys *and* the constant identifiers that stand for them, case-insensitively, with an empty
   stated-exceptions list. It is mutation-verified, and it earned its keep: matching lowercase text alone
   had let `keys::AI_PROMPT` and `"VercelAISDK"` past a pass that reported zero.

   The last always-on framework code was a **tool-definition grammar**: a framework that builds its tools
   as objects and logs them with `str()` leaves a string that is neither JSON nor prose. That grammar is a
   property of the *language*, so it is sealed in `rules/tool_repr.rs` and reached only through a declared
   `ToolReprSpec` — which member of a carrier holds the tools, which repr fields name them, which labels
   its embedded documentation uses, how its type names map to JSON Schema's, and what makes a string a
   `repr` rather than a bare tool name. Nineteen functions moved; the module names no framework and the
   type table is data.

   Two facts that shaped it. The entries are walked **one at a time**, with the declared paths tried inside
   each — path-major order would report two entries' tool lists interleaved differently from the way the
   framework wrote them. And the rule is evaluated by `MessagePlan::tool_definitions`, *not* by `run`:
   a tool definition is not a message, the tool-definition path has always run on every span, and carrier
   claiming is about messages — a framework may state its tools on a carrier another rule reads as a
   conversation, and both statements are true.

   AutoGen was the largest and shaped seven primitives, each named for a fact about a
   carrier rather than for the framework: a **claim** target (an aggregate span's
   `input.value` is a Python `repr` of framework internals, so the rule says "mine, and
   holds no message"); `prepend_block` (reasoning in a sibling member becomes a thinking
   block ahead of the reply, one turn rather than two); a canonical `tool_calls_from` and
   singular `tool_call_from` constructor (both are canonical SideML targets, and only the
   sources are data); `lift_from_parent` and `require_parent` (a batch of tool results
   states its type and its shared call id on the message *enclosing* them, while the
   reading is one message per element — the discriminator and the selection sit at
   different levels); a closed `role_map` (`source: "planner"` names a *speaker*, so which
   of the two a member is has to be declared); and `extra_cases` (a shared table says what
   a message looks like; one place that dialect writes them accepts one shape more loosely,
   and putting that in the table would loosen every other reader).

   Two validations were relaxed, each with the shape that forced it recorded in the code:
   `alternatives` and `also` may now coexist (a response member holds the reply *and* the
   inner turns that produced it — "which shape is this" and "read this as well, always" are
   different questions, and refusing the pair pushed one carrier into two rules, which the
   ownership check rightly refuses); and a reading may name its own `emit` target (a logged
   model call carries its conversation and the tools it was offered in one carrier, and
   those are not the same kind of thing).

   **An extractor moves wholesale or not at all**, learned by breaking it: migrating
   Vercel's prompt and tool call while leaving its response gave those spans *two*
   claimants, so the narrower `FirstMatch` reading changed shape and
   `reading_more_carriers_only_adds_messages` — the only monotonicity check — stopped
   modelling anything.
9. ✅ **The hard cases above**, each needing a primitive designed on its own evidence — AutoGen's seven are
   recorded under step 8, each named for a fact about a carrier rather than for the framework.
10. ⬜ **Externalise the plain provider and MCP manifests; delete the legacy tables** — **not done, and
    outside the cycle-40 acceptance.** Both are Rust tables, held by the two scoped exemptions the sweep
    records: `api/mcp/tools.rs` with its argument schema, and the three Azure AI Foundry connector files.
    Neither interprets telemetry, which is why the acceptance stands without them.

    The legacy tables are also **deliberately not deleted**: each is retained under `#[cfg(test)]` as an
    equivalence oracle, because a golden can be regenerated and bless a regression while an oracle cannot.
    So this step's second clause is superseded rather than pending.

Detection may move later than step 4, but it must never regain parser-selection authority.

## Step 3 in full, because it is the first behavioural slice

`semantics_for` is span-blind: `gen_ai.output.messages` is classified `EMISSION` unconditionally. On a
generation span that is right. On an **aggregator** span — Vercel's `invoke_agent` re-listing the whole
turn — it is a re-listing of state, and reading it as an emission makes the final answer sort *before*
the tool calls that produced it. `_synthetic/agent_snapshot_reorders_answer` is that shape.

The correct resolution is **`ACCUMULATED_STATE`**, not `SNAPSHOT`: the carrier both re-lists history
*and* holds the span's output, and those are separate facts in the model already.

Context propagation was **part of this slice and is now solved**: `BlockEntry` carries `span_name`,
`scope_name`, `scope_version` and `observation_type`, and `parse_span_rows` forwards all four from
`MessageSpanRow`. (When this was written it carried `observation_type` alone.)

The slice is landed only when all of these are true:

1. Every current carrier declaration *and* ordering-family fact is in rule data.
2. No engine carrier literal, and no framework label, controls behaviour.
3. Match context includes the available query-time facts: carrier, observation type, span name, scope
   name and version.
4. Semantics are resolved once per observation and carried through to the order and dedup consumers.
5. Resolved semantics are **not persisted**, so historical rows benefit immediately.
6. The ingestion-side `carrier_holds_span_output` read uses the same compiled rules.
7. Qualified clauses beat generic clauses; equal-precedence collisions **fail compilation**.
8. An unknown carrier retains a conservative default.
9. The Vercel fixture orders question → tool calls → tool results → final answer.
10. Existing goldens are unchanged, except explicitly reviewed defect corrections.
11. A persisted-row test proves the fix applies without re-ingestion.
12. The feed cache key includes the ruleset digest.
13. Explain output identifies the matched clause and the fallback path. **Outstanding** — the compile-time
    diagnostics do this and the per-message rendered trace does not, so step 3 is landed on its behavioural
    conditions with this one open. Recorded here rather than quietly satisfied: it is the condition that
    makes a wrong answer diagnosable, which is worth more the further the ruleset grows.
14. Read and feed benchmarks stay within the existing ceilings.

## Stop conditions

Redesign or halt if any of these appears:

- the migration targets only `try_*` (framework policy also lives in attributes, keys, tool
  extraction, content normalisation, query expansion, source direction and ordering exceptions);
- detection chooses one parser;
- a framework label reaches a behavioural API, under any parameter name;
- a "generic transform" is a renaming trick;
- the DSL grows into an unbounded language — B, built by accident, without B's sandbox or tooling;
- the 120 committed goldens are treated as complete coverage;
- historical rule fixes are promised without changing filtering and materialisation;
- rules are not explainable;
- one-file-per-framework duplicates a dialect;
- executable provider connectors are described as data;
- the structural gate is just a framework-name grep.

The last one was reached and answered rather than avoided; see **Acceptance** below for what the gate does
and does not establish.

## Acceptance

The mandate was that the engine be *designed and accepted together with Codex*, so the acceptance is its
statement rather than a summary of it. Forty review cycles; cycle 40 found nothing, on `d57afd7c`.

### What the whole-server sweep establishes

Recorded in Codex's words, because the distinction is the point and a paraphrase would blur it:

> The sweep is recordable as enforcement of the syntactic invariant it states: concrete framework names may
> not appear in production server tokens outside the scoped exemptions. It is not semantic proof against
> unnamed magic values, prose, externally assembled source, or names constructed from non-adjacent parts.

Three cycles were spent getting it to that standing, and each gap is worth keeping in view because each one
made a passing sweep mean less than it appeared to:

| Gap | Why a passing sweep meant nothing |
| --- | --- |
| A hand-written marker inventory | It omitted six declared frameworks. A list maintained beside the thing it describes is a list that drifts, so the markers are **derived** from the asset ids, with aliases each tied to an existing asset |
| A whole-file exemption | A hole the size of the file: `fn parse_haystack_telemetry` written into a provider connector passed. Exemptions are **marker-scoped** now, and each allowance must be used or it fails |
| Reading source a line at a time | Not one bypass but a class. A brace inside a string literal sent the `#[cfg(test)]` stripper hunting for a close brace and it consumed every production item after it; a name written as `concat!("lang", "graph")` across lines could not be joined at all. Tokenised (`proc-macro2`), so both are gone by construction |

Two consequences of tokenising are themselves decisions. **Prose is skipped** — the explanations in this tree
name the producers whose telemetry motivated each rule, deliberately, and reading them would force either mass
whole-file exemptions or deleting the explanations. And a marker is a **word**, not a substring: `agno` is a
framework and also the middle of `diagnostic` and `backend-agnostic`, so `contains` accused the S3 error
formatter. A matcher that is too strict is the quieter failure, which is why
`a_marker_matches_a_word_and_not_a_fragment` pins thirteen cases in both directions.

### The statement

Codex's acceptance, verbatim:

> **Acceptance — cycle 40, `d57afd7c`**
>
> The declarative framework-rules migration satisfies both success criteria.
>
> **(a) Framework independence.** Production telemetry parsing, extraction, classification, and
> normalisation contain no concrete agent-framework knowledge. Framework-specific facts, carrier names,
> precedence, message shapes, classifications, content forms, and provider aliases are declared in embedded
> rule assets and interpreted by generic Rust engines. The only production naming exceptions are explicitly
> scoped non-parser concerns: the MCP integration-guide catalogue and Azure AI Foundry provider-connector
> code.
>
> This boundary is enforced by rule compilation, exact vocabulary tests, and a tokenised whole-server source
> sweep whose marker inventory is derived from the assets, whose aliases are tied to existing assets, and
> whose exemptions are marker-scoped and self-checking.
>
> **(b) Parsing correctness.** Every framework represented in the captured corpus parses equivalently to the
> retired implementation. This is supported by unchanged golden fixtures, targeted precedence and refusal
> tests, classifier shadow comparisons over every captured span, and message/member equivalence comparisons
> over captured JSON objects.
>
> The acceptance is intentionally corpus-bounded. It does not prove unseen producer versions or shapes.
> Eight declared message members remain explicitly listed in `UNOBSERVED`; the JSON-object walk is bounded
> to depth eight. The source sweep detects framework names, not unnamed framework-specific magic values; it
> excludes prose and documentation, source outside `server/src`, and names assembled from non-adjacent or
> computed parts. Those limits are documented rather than presented as guarantees.
>
> Within those stated boundaries, the work is complete.

### What is declared, measured

41 assets holding 347 rules:

| Kind | Count | Kind | Count |
| --- | --- | --- | --- |
| `messages` | 71 | `span_categories` | 22 |
| `carriers` | 55 | `message_events` | 9 |
| `span_fields` | 47 | `event_roles` | 9 |
| `observation_types` | 33 | `content_blocks` | 9 |
| `message_members` | 32 | `span_facts` | 4 |
| `detect` | 28 | `provider_aliases` | 2 |
| `sdk_slugs` | 26 | | |

Each retirement is held to the code it replaced by an **equivalence oracle** rather than by the goldens,
because a golden can be regenerated and bless a regression. **Sixteen**, in two shapes, and one thing that
is not an oracle at all. Codex's classification, at cycle 42 when there were fifteen:

> Thirteen focused equivalence tests compare individual migrations with the retired implementation. Two
> further corpus-wide equivalence tests compare classifications across every captured span and
> member-vocabulary answers across captured JSON objects. Separately,
> `no_declared_rule_is_dead_across_the_corpus` is a bidirectional coverage inventory.

The focused thirteen are `the_rules_reproduce_the_legacy_carrier_table`,
`the_rules_reproduce_the_legacy_detection`, `the_rules_reproduce_the_extractors_they_replaced`, the three
field-chain and token-table oracles, the two content-block readers, the member lists, the single-tool
triple, the span facts, and the two classification sweeps in their focused form. The corpus-wide two are
`the_declared_classification_matches_the_sweep_across_the_corpus` and
`the_member_vocabulary_answers_as_it_did_across_the_corpus`. The fourteenth focused one is
`the_declared_event_roles_reproduce_the_table_they_replaced`, added with the event-role move (`8e3ab002`) —
which is why the total is sixteen and the quotation says fifteen.

Keeping the coverage inventory out of that count is the point: it says which rules were *exercised*, which
is a different question from whether the ones that ran agree with the code they replaced.

### The limits, in one place

Stated here because a document about enforcement reads as exhaustive, and none of these is a defect to be
fixed later — each is a boundary of what the evidence can carry.

- **Corpus-bounded equivalence.** 13 captured suites at the SDK versions the fixtures were captured at,
  reaching **9 of the 30 producers the assets declare** - the other two suites, `openai` and `anthropic`,
  are provider telemetry read by the conventions rather than by a dialect of their own. So **21 declared
  producers have rules and no captured fixture at all**.
- **Unexercised rules, which is a different count.** `no_declared_rule_is_dead_across_the_corpus` lists
  **39 message-rule leaves** that no fixture reaches, each with the reason. Not the same set as the 21
  producers: it includes shapes belonging to producers the corpus *does* cover, and it is scoped to message
  rules rather than to all 338 declarations. Conflating the two reads as though covering those producers
  would exercise everything. "All frameworks parse correctly" is true of the corpus and is an open-world claim
  beyond it; `the_corpus_matches_the_support_matrix` is what keeps the boundary legible rather than implied.
- **Eight `UNOBSERVED` members.** Declared in the member vocabulary and reached by no captured object. The
  set is exact and asserted, so it shrinks when a fixture reaches one and cannot grow silently.
- **A depth-8 walk.** The member-vocabulary comparison walks captured JSON to depth eight. Deeper nesting is
  unexamined by that oracle.
- **Names, not values.** The sweep reads framework *names*. A module hard-coding a framework's magic
  attribute value without naming it is not caught — which is why the oracles matter more than the sweep.
- **Prose.** Doc comments are not read, including the one place `schemars` turns a doc comment into a
  shipped schema description.
- **Outside `server/src`.** The SDKs are separate crates and are *meant* to name the framework they
  instrument.
- **Names that are computed.** A name assembled from non-adjacent parts, or built from a variable rather
  than written, is not matched. Adjacent literals are, wherever they are laid out.
- **Data-only extension is about the *change*, not about the deployment.** Adding a framework whose
  telemetry fits the existing primitives requires only asset and fixture changes, but production assets are
  embedded and immutable, so it still ships in a server release. A new shape additionally requires a
  producer-neutral primitive. So "add a framework without shipping code" is a claim about which files a
  change touches, and there is no reload mechanism that would make it a claim about a running installation.
