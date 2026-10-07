# Framework rules engine

This document describes the rules engine as it exists today. It is normative for the boundary between
producer-specific telemetry knowledge and the generic Rust implementation.

## Purpose

Agent frameworks and providers emit equivalent concepts under different attribute names, payload shapes,
event names, roles, and precedence conventions. SideSeat stores and reconstructs that telemetry without
putting those producer-specific facts into executable branches.

The governing invariant is:

> Producer-specific telemetry knowledge lives in `server/assets/rules/`. Rust provides a fixed,
> producer-neutral interpretation model.

For a producer whose telemetry fits the existing model, support is added by changing rule assets and
fixtures. The assets are embedded in the server, so deploying an asset change still requires a server
release.

The engine covers telemetry interpretation:

- producer detection;
- carrier semantics and message extraction;
- event roles and message-member vocabulary;
- content-block and tool-definition shapes;
- observation and span classification;
- stored span-field resolution;
- framework-owned provider aliases needed by telemetry interpretation.

It does not cover executable provider connectors, authentication flows, the provider catalogue, or MCP
client setup. Those are separate adapter and product-metadata concerns.

## Architectural boundary

The implementation is split between three locations:

| Location | Responsibility |
| --- | --- |
| `server/assets/rules/` | Producer, convention, and shared-vocabulary declarations |
| `server/crates/rule-assets/` | Deterministically embeds the JSON assets |
| `server/crates/domain/src/rules/` | Parses, validates, compiles, and evaluates a typed ruleset |

The ingestion crate supplies span and event data to the domain engine and persists the extracted raw
messages. Query-time reconstruction uses the same compiled rules for carrier semantics, normalisation,
classification, ordering, and deduplication.

Producer detection returns a label for provenance, display, filtering, and statistics. No behavioural API
accepts that label, and detection never selects a parser. Behaviour is selected from observable evidence:
carrier names, span shape, attributes, events, scope, and payload structure.

```text
embedded JSON assets
        │
        ▼
parse and validate once
        │
        ▼
immutable typed Ruleset
        │
        ├── ingestion-time extraction
        └── query-time reconstruction
```

The compiled ruleset is stored in a `OnceLock`. Asset parsing, JSONPath parsing, validation, ordering, and
index construction therefore stay off the per-observation path.

## Asset organisation

The embedded corpus currently contains **58 assets holding 660 clauses** in three groups:

```text
server/assets/rules/
├── conventions/  # shared telemetry conventions and generic fallbacks
├── producers/    # one producer-oriented asset per supported dialect
└── vocabulary/   # shared engine vocabulary and reusable declarations
```

Files are organisational units, not runtime dispatch units. Their declarations compile into one global
plan. A convention therefore applies without every producer importing it, and producer detection does not
choose a file to execute.

Each asset has a stable `id`, optional prose documentation, and any subset of these sections:

| Section | Meaning |
| --- | --- |
| `detect` | Ranked evidence that produces a display label |
| `sdk_slugs` | Explicit SDK declarations used after telemetry detection fails |
| `carriers` | Carrier semantics and ordering-family membership |
| `messages` | How carriers and events produce canonical readings |
| `message_projections` | Stored message rows hidden from the read-time SideML projection |
| `fragments` | One-level reusable message-shape tables |
| `message_events` | Events that may contain messages |
| `log_events` | Log-record shapes that carry one of the `message_events`, and where their attributes are |
| `event_roles` | Roles implied by source or event names |
| `role_authority` | What each role spelling means, and its precedence over inferred roles |
| `message_members` | Content, message-shape, and content-block member vocabulary |
| `content_blocks` | Provider payloads that represent canonical content blocks, and parts holding a list of blocks that is spliced into a message's content |
| `tool_shapes` | Provider payloads that represent tool definitions |
| `span_fields` | Ordered sources for typed stored span fields |
| `observation_types` | Ranked observation classification |
| `span_categories` | Ranked broad span classification |
| `span_facts` | Producer-neutral facts established by producer-specific evidence |
| `provider_aliases` | Framework-owned aliases needed for pricing lookup |
| `finish_reasons` | What each finish-reason spelling means, compared with case and word boundaries canonicalised; declared only by the `finish-reasons` vocabulary asset |
| `convention_namespaces` | Namespaces owned by shared telemetry conventions |

`RuleFile` uses `deny_unknown_fields`, and compilation rejects malformed or ambiguous declarations. A typo
cannot silently create a new section or field.

Every asset names the editor schema in its `$schema` member, as a path relative to the asset. That schema,
`server/assets/rules.schema.json`, is generated from the serde types (regenerate with `UPDATE_GOLDENS=1 cargo
test --locked -q -p sideseat-domain --lib schema_census`), lives outside the embedded directory, and a test
validates every asset against it, so the schema and the parser cannot disagree. The same walk records which
schema options the shipped assets use; an optional property or enum value none uses must be listed with a
reason in `rules/schema_census.rs`.

## Compilation model

Asset bytes are parsed once, in deterministic path order, into `ParsedAssets`; every section compiles from
that one parse into `Ruleset`. Compilation performs the work that must not be repeated for every span:

- parse and type-check JSON assets;
- reject duplicate asset, rule, and clause identities;
- compile RFC 9535 JSONPath expressions;
- resolve fragment references;
- validate mutually exclusive or incomplete options;
- establish priority order in every ordered arena and check `supersedes` against it;
- build exact-name and prefix indexes where lookup is naturally keyed;
- calculate the ruleset digest.

The digest is part of the reconstruction cache key. A cache entry therefore identifies both its stored rows
and the ruleset that interpreted them.

Some ordered candidate sets remain linear scans. In particular, ordered message and classification rules
are evaluated in their declared order. This is intentional for the current corpus and measured by the
existing read benchmarks; it is not an assertion that the ruleset may grow without further indexing.

## Selection and construction

The engine uses RFC 9535 JSONPath through `serde_json_path` for selection and filtering. JSONPath returns
borrowed references into the original `serde_json::Value`, so cloning a selected provider subtree preserves
its member order.

This property is why the engine does not use a projection language to construct arbitrary output objects.
Projection libraries may legally rebuild JSON objects in a different member order. Although JSON object
order is semantically insignificant, SideSeat treats the serialized provider payload as evidence and must
not rewrite it accidentally.

The processing seam is:

```text
declared carrier
  → parse
  → JSONPath selection and predicates
  → bounded structural operations
  → canonical SideML constructors
  → unchanged provider subtrees
```

Expressions select nodes and choose branches. They are not emitted directly. Rust owns constructors for
canonical SideML envelopes, content blocks, messages, and tools; assets own producer keys, aliases, tags,
paths, mappings, and precedence.

The structural vocabulary includes:

- exact, path, wildcard, and indexed-family selection;
- conjunction, disjunction, negation, alternatives, and fallback;
- JSON, recursively encoded JSON, and a bounded Python-literal subset;
- bounded tree walking with explicit match, prune, and emit behaviour;
- positional overlays between two representations of the same message;
- section and balanced-delimiter parsing;
- canonical message, content-block, tool-call, and tool-definition construction;
- carrier claiming and primary/fallback stages.

There are no inline scripts and no rule-selected Rust callbacks. A new primitive is acceptable only when it
describes a producer-neutral operation, has bounded cost, and can be tested without naming a producer.

### Conditions about a span

Every section that asks a question about a span - detection, the classifications, span facts, span-field
sources, message-rule gates and a compose member's fallback - asks it with one `where`, in the expression grammar
of `server/crates/domain/src/rules/expr.rs`: an atom, `{"all": [...]}`, `{"any": [...]}` or `{"not": ...}`,
strong-Kleene, holding only when true. An atom names a `source` - `span_name`, `attr:<key>`, `attr_keys` (the
span's attribute keys, asked existentially), `scope.name`, `resource:<key>` - and the tests asked of the value it
selects (`exists`, `equals`, `equals_ignore_case`, `one_of`, `starts_with`, `contains`, `contains_ignore_case`); all
of an atom's tests are asked of the same value. `exists` is total; a value test on an absent value is unknown, so
`not` over it does not hold - writing `"exists": true` beside the test makes it false instead. Each section is
given only some sources (detection all of them; message gates the span and its scope; classification and
span-field sources the span; span facts its attributes), and a condition reading another is refused when the
rule compiles, as are an empty literal, a test a source cannot answer, and a disjunct its own group already
covers. `server/crates/domain/src/rules/span_conditions.rs` lowers a `where`, evaluates it and decides
implication (shadowing, and which of two contending message rules suppresses the other), soundly and without
assuming excluded middle. `server/specs/ThreeValuedPredicate.tla` checks the logic's laws, and
`three_valued_predicate_instances` holds `Expr::eval` to them.

### Conditions about a value

A condition on a JSON value - which shape a content-block case or tool shape recognises, which readings,
elements, sections, attachments and overlay entries apply, what a carrier's raw text must be before it is
parsed - is a `where` in the same expression grammar, over atoms that name a `path` into the value (absent: the
value itself) and the tests asked of what it selects: `exists`, `kind`, `non_empty`, `non_blank`, `not_null`,
`identifier_like`, `starts_with`, `lacks_prefix`, `one_of`, `none_of`, `equals`, `only_members`. Every test of
an atom is asked of one selected value, so `{path: "$.items[*]", starts_with: "a", one_of: [...]}` needs a single
item satisfying both. Where the subject differs the field says so: `parent_where` (the value a selection came
out of), `witness` (an overlay's counterpart list), `entry_where` (an assembled indexed entry), `skip_where` (a
section dropped where it holds), `raw_where` (a carrier's text, as a JSON string, before parsing).

### Sources and fallbacks

Wherever a value may come from one of several places, a field takes one source written bare, or
`{"first_of": [...]}` naming two or more. A list reads in one of two modes, and says which where it is not the
default: **present** (omitted, or `"mode": "present"`) commits to the first candidate that is there, whatever it
holds - two spellings in one payload are one producer's statement, so a badly written primary does not hand over to
an alias - and **usable** (`"mode": "usable"`, required on the fields that read this way: content-block members,
call ids, an overlay's counterpart list, a log record's name) steps over a candidate it cannot read as the field
needs. Attribute sources are `attr:<key>` wherever a name is typed beside other kinds (`event_name` on a log
record); a message rule reading every one of several keys as its own observation says `every`.

### Content blocks

A content block is normalised by one chain: the canonical SideML passthrough, then the declared
`content_blocks` cases at four positions - `before_provider_formats`, `message_envelope` (consulted only for a
message's own content, never for a tool's returned value), `provider_formats` (the model APIs' wire
vocabularies: OpenAI, Anthropic, Bedrock Converse, Gemini, and the conventions' part types) and
`after_provider_formats` - then the generic media and unknown fallbacks. Within a position the lowest
`priority` is tried first; a case that recognises a block and cannot build its target declines, and the
next case is tried. Each position is its own arena: a matching `unwrap` whose member cannot be read ends its
position only, and a `splice` is tried over the envelopes alone, which one merged order could not express.

Each case names one canonical target form (`text`, `json`, `media`, `thinking`, `redacted_thinking`,
`refusal`, `tool_use`, `tool_result`, `unknown`, `unwrap`, `splice`) and where its members come from. A
member source is a JSONPath, or a JSONPath with exactly one bounded transform: `join` (every string
selected, joined), `parse` (the carriers' decoding modes), `prepend`, or a closed `map`. A transform that
cannot apply leaves the source absent, so a list of sources means "the first usable spelling". Media cases
declare what the format states - a block kind, whether the data is a location, bytes or an identifier, what
a missing media type means - and derive the rest from the value. Their `where` conditions add `equals` (any
JSON value) and `only_members` (an object with no member outside a set) to the value vocabulary.

## Carrier semantics

A carrier declaration answers independent questions instead of assigning one broad preset:

- whether a position proves a distinct occurrence;
- whether a position provides sequence order;
- whether the carrier is an atomic emission;
- whether it may contain history or accumulated state;
- whether it holds the span input;
- whether it holds the span output;
- whether it is a detached request frame;
- whether it holds an expandable message array;
- which named ordering family it belongs to.

The booleans are deliberately independent. Conversation snapshots, accumulated state, detached requests,
and single emissions overlap in several properties and differ in others. Compilation rejects combinations
that the ordering and deduplication model cannot interpret coherently.

Semantics are resolved from the carrier and available span context. They are not persisted, so corrections
to embedded rules apply to stored observations during reconstruction without re-ingestion. Message
extraction itself occurs during ingestion; newly recognisable messages require re-ingestion because their
raw message rows do not yet exist.

## Ordering and precedence

Order is policy and is always explicit, and every ordered arena states it the same way
(`server/crates/domain/src/rules/precedence.rs`):

- **`priority`**, an integer, lowest first: detection rules and their alternatives, message rules,
  observation types, span categories, the cases of one content-chain position, tool shapes, event categories,
  and each ordered question of `message_members`. Two clauses of one arena sharing a priority are refused.
  An arena is the set of clauses whose relative order is observable; for message rules that is a pairwise
  relation (same stage, an overlapping output axis, an overlapping input domain), so two rules that never
  contend may share a number.
- **`supersedes`** (detection) names the rules a clause is meant to beat where both match. It documents an
  overlap, waives the overlap report for that pair, and is checked: each target must exist, differ from the
  source, be named once, and come after the source by priority. It is never executed, so deleting an edge
  changes no answer. A rule beats one tried ahead of it by placing the evidence that should win in an
  `alternative` at an earlier priority.
- Span-field sources are ordered lists, and carrier ordering uses named ordering families and resolved
  carrier semantics; carrier claims are ordered by subsumption of their match keys, which form a lattice.

`server/specs/CheckedPrecedence.tla` model-checks the arena semantics over a bounded space and the hand-written
cases of `server/specs/instances/CheckedPrecedence.json`: the winner is the lowest-priority match, a shadowed
clause never wins, and wherever the edges agree with the priorities the retired edge-ordered resolution gives the
same answer. `checked_precedence_instances` holds the detection and classification compilers to the same
manifest and space.

The engine does not infer precedence from apparent predicate specificity. Two structural predicates can
overlap without either being intrinsically more specific, so inferred precedence would be difficult to
review and unstable as assets evolve.

## Results, refusals, and diagnostics

An absent answer and an unusable declared source are different outcomes.

- No matching clause means no declaration answered.
- A `Refusal` records a declaration that did match a source but could not use it.
- `Unusable` distinguishes empty, malformed, wrong-member, out-of-range, and unanswerable-gate failures.
- A successful resolution may still carry refusals from other candidates.

This separation keeps diagnostics orthogonal to the optional result. A malformed optional wrapper, for
example, can be reported while a valid enclosing value still produces an answer.

A defective ruleset is refused with `RulesetDiagnostics`: one entry per failing section, naming the section,
the asset path, id and clause of every clause the defect involves, and the reason. Sections are compiled
even after one fails, so one run reports every broken section; within a section the first defect is
reported, because later checks there assume the earlier ones held.

Every compiled clause retains its asset id, rule id, clause path, and documentation. Compile-time and
runtime diagnostics can therefore name the declaration responsible for a decision. A complete rendered
per-message explain trace is not yet implemented.

## Verification

The rules boundary is protected by complementary checks:

1. Embedded assets must parse and compile.
2. Focused equivalence oracles compare migrated declarations with the retired implementation retained under
   `#[cfg(test)]`.
3. Golden fixtures compare reconstructed message order, roles, content, digests, tools, finish reasons, and
   observation types.
4. Corpus-wide comparisons exercise classification and member-vocabulary behaviour.
5. Repository tests forbid concrete producer knowledge and asset-declared producer telemetry keys in
   production Rust outside narrowly scoped non-parser exemptions. A third sweep forbids, in the crates that
   interpret telemetry, any string literal equal to a word only a producer or vocabulary asset declares - a
   payload member, block type, role spelling, or event name - derived from the assets by position, with
   published conventions and SideSeat's own SideML vocabulary exempt and the remaining sites on a shrink-only
   list tied to the leak inventory.
6. Refusal, precedence, ambiguity, determinism, deduplication, tool-id correspondence, and carrier
   subsequence properties have focused tests.
7. Architecture diagrams and this document have machine-checked asset and clause counts.

Goldens are regression evidence, not proof of complete support. Updating a golden cannot replace reviewing
the behavioural change, and an invariant failure remains a failure even when golden regeneration is
requested.

## Known limits

The guarantees above have explicit boundaries:

- Captured fixtures cover only part of the declared producer set and only the SDK versions represented by
  those captures.
- Some declared message-rule leaves and message members are listed as unobserved rather than treated as
  exercised.
- The corpus object walk used by an equivalence oracle is depth-bounded.
- Source sweeps enforce the syntactic invariants they state; they cannot prove the absence of unnamed magic
  values, runtime-computed names, or undeclared producer keys.
- Production assets are immutable and release-coupled; there is no hot reload.
- The engine has feature-local traversal limits but no single global budget for depth, decoded bytes,
  iterations, or emitted values.
- Some ordered rule families are scanned linearly.
- The per-message explain trace is incomplete.
- What Rust still names is named by decision, not by omission: the published conventions (event names,
  `gen_ai.*` attributes and part shapes), SideSeat's own vocabulary (SideML members and block types, the
  extraction roles `tool_call`, `tools`, `data`, `context`, `documents`, the intermediate call shape
  `tool_calls`/`tool_call_id`), generic decodings (JSON, Python literals), and the model providers the pricing
  catalogue and connectors name. Every producer's member, block type and role spelling is declared; the third
  sweep holds the telemetry-interpreting crates to that with no allowlist beyond a short reasoned list of sites
  where a published format or SideSeat itself owns the word. Provider spellings, model-name normalisation, and cache and
  reasoning counter policy used for pricing also remain in Rust; `provider_aliases` covers only a
  framework that names itself where a provider is expected.

These are design boundaries, not implied guarantees. New work should either preserve them explicitly or
change the architecture and its verification together.

## Extending the ruleset

To add or change producer support:

1. Add or update the smallest appropriate asset under `producers/`, `conventions/`, or `vocabulary/`.
2. Prefer an existing shared fragment or primitive over duplicating a shape.
3. Add captured or synthetic fixtures that exercise each new branch.
4. Add focused refusal and precedence cases for overlapping or malformed inputs.
5. Verify that production Rust gained no producer-specific literal, mapping, or branch.
6. Run the domain, ingestion, golden, repository, and workspace checks.

If the telemetry shape cannot be expressed, first specify the missing producer-neutral operation. Do not
add a producer-named callback, a hidden parser dispatch, or a framework label to a behavioural API.

## Non-goals and stop conditions

Stop and redesign the change if it introduces any of the following:

- detection selecting a parser;
- framework identity controlling extraction, normalisation, ordering, token, or cost behaviour;
- a producer-specific operation disguised as a generic primitive;
- unbounded scripting or callbacks in rule data;
- executable provider connectors represented as transformation assets;
- silent fallback from malformed declarations;
- precedence derived from file order or inferred specificity;
- claims of complete support based only on the captured corpus.

The desired outcome is not “all behaviour is data.” It is a smaller and enforceable statement: producer
telemetry policy is data, while the generic, bounded algebra that interprets that policy is code.
