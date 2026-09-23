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
- provider aliases needed by telemetry interpretation.

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

The embedded corpus currently contains **43 assets holding 382 clauses** in three groups:

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
| `fragments` | One-level reusable message-shape tables |
| `message_events` | Events that may contain messages |
| `event_roles` | Roles implied by source or event names |
| `role_authority` | Precedence of explicit and inferred role spellings |
| `message_members` | Content, message-shape, and content-block member vocabulary |
| `content_blocks` | Provider payloads that represent canonical content blocks |
| `tool_shapes` | Provider payloads that represent tool definitions |
| `span_fields` | Ordered sources for typed stored span fields |
| `observation_types` | Ranked observation classification |
| `span_categories` | Ranked broad span classification |
| `span_facts` | Producer-neutral facts established by producer-specific evidence |
| `provider_aliases` | Framework-owned aliases needed for pricing lookup |
| `convention_namespaces` | Namespaces owned by shared telemetry conventions |

`RuleFile` uses `deny_unknown_fields`, and compilation rejects malformed or ambiguous declarations. A typo
cannot silently create a new section or field.

## Compilation model

Asset bytes are read in deterministic path order and compiled into `Ruleset`. Compilation performs the work
that must not be repeated for every span:

- parse and type-check JSON assets;
- reject duplicate asset, rule, and clause identities;
- compile RFC 9535 JSONPath expressions;
- resolve fragment references;
- validate mutually exclusive or incomplete options;
- establish ranked and named-chain order;
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

Order is policy and is always explicit.

- Detection, message rules, observation types, span categories, and field sources use declared ranks or
  ordered source lists.
- Content-block handlers use named chain positions plus a rank within each position.
- Carrier ordering uses named ordering families and resolved carrier semantics.
- Equal-precedence declarations that would make the answer depend on file load order are rejected.

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
   production Rust outside narrowly scoped non-parser exemptions.
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
