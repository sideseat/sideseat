//! The gates on the carrier slice: equivalence with the table it replaced, and the defect it fixes.

use crate::domain::sideml::carrier::{self, CarrierSemantics};

use super::carrier_rules::{CarrierContext, CompileError, compile};
use super::{ruleset, schema};

/// Every carrier name either source knows about.
///
/// Written out rather than derived from the rules: a list derived from the thing under test would shrink
/// silently when a clause was deleted, and the equivalence claim would still pass.
const CARRIER_EVENTS: &[&str] = &[
    "gen_ai.choice",
    "gen_ai.content.completion",
    "gen_ai.output.messages",
    "gen_ai.tool.result",
    "gen_ai.tool.message",
    "gen_ai.user.message",
    "gen_ai.system.message",
    "gen_ai.assistant.message",
    "gen_ai.content.prompt",
    "gen_ai.input.messages",
    "some.event.nobody.declared",
];

const CARRIER_ATTRIBUTES: &[&str] = &[
    "output.value",
    "input.value",
    "message",
    "messages",
    "gen_ai.output.messages",
    "gen_ai.output.messages.0.parts",
    "llm.output_messages",
    "llm.output_messages.0.message",
    "ai.response",
    "ai.response.text",
    "ai.response.toolCalls",
    "gen_ai.input.messages",
    "gen_ai.input.messages.1.parts",
    "llm.input_messages",
    "llm.input_messages.0.message",
    "ai.prompt",
    "ai.prompt.messages",
    "request_data",
    "new_context",
    "gen_ai.system_instructions",
    "user_system_prompt",
    "system_prompt",
    "ai.toolCall.result",
    "gen_ai.tool.call.result",
    "ai.toolCall.args",
    "gen_ai.tool.call.arguments",
    "tool_name",
    "response.model_output",
    "exception",
    "some.framework.newAttribute",
];

#[test]
fn the_rules_reproduce_the_legacy_carrier_table() {
    // The migration's whole claim: read without span context, the rules answer exactly what the Rust
    // table answered. Anything else is a behaviour change hiding inside a refactor.
    for event in CARRIER_EVENTS {
        let legacy = carrier::legacy_declared_semantics(Some(event), None);
        let rules = carrier::declared_semantics(Some(event), None);
        assert_eq!(
            legacy, rules,
            "event carrier `{event}` reads differently under the rules than under the table it \
             replaced"
        );
    }
    for attribute in CARRIER_ATTRIBUTES {
        let legacy = carrier::legacy_declared_semantics(None, Some(attribute));
        let rules = carrier::declared_semantics(None, Some(attribute));
        assert_eq!(
            legacy, rules,
            "attribute carrier `{attribute}` reads differently under the rules than under the table \
             it replaced"
        );
    }
}

#[test]
fn an_unknown_carrier_still_takes_the_cautious_reading() {
    let unknown = carrier::semantics_for(None, Some("some.framework.newAttribute"));
    assert_eq!(
        unknown,
        CarrierSemantics::SNAPSHOT,
        "an unclassified carrier must not invent occurrences: it can under-report, which the answer \
         invariant catches, but over-reporting shows as duplicates a user sees"
    );
    assert!(
        carrier::declared_semantics(None, Some("some.framework.newAttribute")).is_none(),
        "and `declared` must still say nobody classified it, which is a different fact from the value"
    );
}

/// The engine can express a span-qualified carrier; the aggregator repair needs more than it.
///
/// This is the defect the slice set out to fix, and the measurement that says a carrier fact cannot fix
/// it. `gen_ai.output.messages` on a generation span is the model's own emission; on a root agent span
/// it is that span re-listing the turn its children produced, answer first. Read as an emission its
/// stated order is trusted, so the final answer sorts ahead of the tool calls that produced it -
/// `_synthetic/agent_snapshot_reorders_answer` and `vercel-ai-js/image-gen` both show it.
///
/// Declaring the aggregator case `accumulated_state` fixes both of those and costs two others, measured
/// over the corpus:
///
/// - `agent-framework/tool_use` **loses a message** (16 roles become 15). `EMISSION` proves distinct
///   occurrences by position and `ACCUMULATED_STATE` does not, and `dedup` reads exactly that fact - so
///   an agent span that genuinely lists two identical tool results has them collapsed into one.
/// - `agent-framework/swarm` re-batches three specialists' prompts ahead of all three replies, undoing
///   a documented repair, because `carrier_is_atomic_emission: false` disables the contraction that
///   keeps each prompt with its own reply.
///
/// A duplicate or a loss is the one thing the feed must never produce, so a mis-order is not traded for
/// a dropped message. And the discriminator is not the carrier: it is whether the listed messages are
/// *also* covered by a child span's own emission - Vercel's `chat` spans emit them, so the aggregator
/// adds nothing but a bad order, while agent-framework's agent span is the only witness. That is
/// resolution confidence in the order resolver, not a static fact about a carrier name.
///
/// So this test pins two things: the engine really can qualify a clause by span (the mechanism works),
/// and the generic reading is what ships until the resolver can rank a re-listing below the dataflow.
#[test]
fn a_span_qualified_clause_is_expressible_and_the_aggregator_repair_needs_the_resolver() {
    let qualified = br#"{
      "id": "probe", "doc": "d",
      "carriers": [
        {"id": "generic", "doc": "d", "match": {"event": "gen_ai.output.messages"},
         "facts": {"preset": "emission"}},
        {"id": "aggregator", "doc": "d",
         "match": {"event": "gen_ai.output.messages", "observation_type": ["agent", "chain", "span"]},
         "facts": {"preset": "accumulated_state", "position_provides_sequence_order": false}}
      ]
    }"#;
    let sources =
        std::collections::BTreeMap::from([("probe.json".to_string(), qualified.to_vec())]);
    let plan = compile(&sources).expect("a span-qualified clause compiles beside its generic form");

    let on_generation = plan
        .resolve(&CarrierContext {
            event: Some("gen_ai.output.messages"),
            observation_type: Some("generation"),
            ..CarrierContext::default()
        })
        .expect("the generic clause covers a generation span");
    assert_eq!(on_generation.clause_id, "generic");
    assert!(on_generation.semantics.carrier_is_atomic_emission);

    let on_aggregator = plan
        .resolve(&CarrierContext {
            event: Some("gen_ai.output.messages"),
            observation_type: Some("agent"),
            ..CarrierContext::default()
        })
        .expect("the qualified clause covers an orchestration span");
    assert_eq!(
        on_aggregator.clause_id, "aggregator",
        "the more specific clause wins, which is the whole point of carrying span context"
    );
    assert!(!on_aggregator.semantics.carrier_is_atomic_emission);

    // And what actually ships: the generic reading, because the qualified one costs a message.
    let shipped = carrier::semantics_for_context(&CarrierContext {
        event: Some("gen_ai.output.messages"),
        observation_type: Some("agent"),
        ..CarrierContext::default()
    });
    assert!(
        shipped.carrier_is_atomic_emission,
        "the embedded ruleset declares no aggregator override: enabling one dropped a tool result \
         from `agent-framework/tool_use`, and a mis-order is not worth a lost message. Delete this \
         assertion when the resolver can rank a re-listing below the dataflow that contradicts it"
    );
}

#[test]
fn a_caller_with_no_span_context_gets_the_generic_clause() {
    // A qualified clause claims something about the span. A caller that cannot say what the span was
    // has not established it, so it must not match - otherwise the answer would depend on which call
    // site asked, which is the span-blindness this slice removes, inverted.
    let unqualified = carrier::semantics_for(Some("gen_ai.output.messages"), None);
    assert!(
        unqualified.carrier_is_atomic_emission,
        "with no observation type the generic clause applies"
    );
}

#[test]
fn the_ordering_family_comes_from_the_rules() {
    // The seventh carrier fact, previously a hardcoded key comparison in the order resolver. Read off
    // the resolved clause, which is also how the resolver reads it - so the test cannot pass through an
    // accessor the production path does not use.
    let family_of = |attribute: &str| -> Option<String> {
        ruleset()
            .carriers
            .resolve(&CarrierContext::carrier_only(None, Some(attribute)))
            .and_then(|c| c.ordering_family.clone())
    };
    let first = family_of("llm.input_messages.0.message");
    assert!(
        first.is_some(),
        "an indexed input array declares the family that groups its members"
    );
    assert_eq!(
        first,
        family_of("llm.input_messages.1.message"),
        "two members of one array share a family, which is what makes them one sequence"
    );
    assert!(
        family_of("ai.prompt").is_none(),
        "a carrier that is already whole declares no fragmented family - broadening this to Vercel's \
         `ai.prompt` was measured and regressed a sequential two-step trace"
    );
}

#[test]
fn the_ruleset_compiles_and_is_not_empty() {
    let plan = &ruleset().carriers;
    assert!(
        plan.clause_count() >= CARRIER_EVENTS.len(),
        "the embedded assets compiled to {} clauses, which is too few to be the real table",
        plan.clause_count()
    );
    for clause in plan.clauses() {
        assert!(
            !clause.clause_id.is_empty() && !clause.rule_file.is_empty(),
            "every clause carries the file and id the explain trace reports"
        );
        assert!(
            clause.doc.is_some(),
            "clause `{}` has no doc: a rule nobody can read is how the table it replaced went wrong",
            clause.clause_id
        );
    }
}

#[test]
fn the_digest_changes_when_an_asset_changes() {
    // The digest joins the reconstruction cache key, so it has to be a function of the asset bytes. If
    // it were not, a hot-loaded rule change would be served answers built by the previous ruleset from
    // rows that had not changed.
    let mut sources = schema::embedded_sources();
    let before = schema::digest_of(&sources);
    sources.insert("zz-extra.json".to_string(), b"{}".to_vec());
    let after = schema::digest_of(&sources);
    assert_ne!(
        before, after,
        "adding an asset must change the ruleset digest"
    );

    let mut moved = schema::embedded_sources();
    let (path, bytes) = moved
        .iter()
        .next()
        .map(|(p, b)| (p.clone(), b.clone()))
        .expect("assets exist");
    moved.remove(&path);
    moved.insert(format!("renamed-{path}"), bytes);
    assert_ne!(
        before,
        schema::digest_of(&moved),
        "the path is hashed too, so moving a declaration between files changes the digest"
    );
}

#[test]
fn two_clauses_that_could_both_match_fail_compilation() {
    // Equal specificity on the same carrier means load order would decide, which is exactly the
    // invisible precedence this design refuses. Verified by construction, because no embedded asset may
    // contain it.
    let collide = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d", "match": {"event": "x.y"}, "facts": {"preset": "emission"}},
        {"id": "b", "doc": "d", "match": {"event": "x.y"}, "facts": {"preset": "snapshot"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), collide.to_vec())]);
    assert!(
        matches!(compile(&sources), Err(CompileError::Ambiguous { .. })),
        "two clauses matching one carrier at equal specificity must be refused, not resolved by order"
    );

    // Disjoint qualifiers on the same carrier are fine: they cannot both hold.
    let disjoint = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d", "match": {"event": "x.y", "observation_type": ["generation"]},
         "facts": {"preset": "emission"}},
        {"id": "b", "doc": "d", "match": {"event": "x.y", "observation_type": ["agent"]},
         "facts": {"preset": "snapshot"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), disjoint.to_vec())]);
    assert!(
        compile(&sources).is_ok(),
        "clauses whose qualifiers cannot both hold are not ambiguous"
    );
}

#[test]
fn a_clause_constraining_no_carrier_is_refused() {
    let bare = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d", "match": {"observation_type": ["agent"]},
         "facts": {"preset": "emission"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), bare.to_vec())]);
    assert!(
        matches!(compile(&sources), Err(CompileError::NoPrimaryKey { .. })),
        "a clause naming no event, attribute or prefix would claim every observation of a span"
    );
}

/// A clause qualified by span-name prefix is refused, because the resolver is not given that name.
///
/// Carrier semantics are resolved when a span is **read**, from a stored row whose `span_name` is the *display*
/// name - and for a dialect writing an unresolved template that is not the name the producer sent. Such a
/// clause would hold during ingestion and fail on the same span at query time, with the generic reading winning
/// and ordering and deduplication silently changing. Refused rather than documented, which is the discipline
/// everywhere else here: a declaration that cannot work is not a declaration.
///
/// It becomes expressible once the raw name is persisted beside the display name. Nothing declares one today,
/// which is why no fixture moves.
#[test]
fn a_clause_qualified_by_span_name_is_refused_until_the_raw_name_is_stored() {
    let qualified = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d",
         "match": {"attribute": "k", "span_name_prefix": "claude_code."},
         "facts": {"preset": "emission"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), qualified.to_vec())]);
    assert!(
        matches!(
            compile(&sources),
            Err(CompileError::UnavailableDimension {
                dimension: "span_name_prefix",
                ..
            })
        ),
        "the query-time resolver sees the display name, so this clause could not hold there"
    );

    // The same clause without that dimension is ordinary and compiles, so this is a refusal of one dimension
    // rather than of span-qualified clauses in general - `observation_type` still qualifies one.
    let ok = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d",
         "match": {"attribute": "k", "observation_type": ["generation"]},
         "facts": {"preset": "emission"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), ok.to_vec())]);
    assert!(
        compile(&sources).is_ok(),
        "a clause qualified by observation type is unaffected: {:?}",
        compile(&sources).err()
    );
}

#[test]
fn a_duplicate_clause_id_is_refused() {
    let dup = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d", "match": {"event": "x.y"}, "facts": {"preset": "emission"}},
        {"id": "a", "doc": "d", "match": {"event": "z.w"}, "facts": {"preset": "emission"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), dup.to_vec())]);
    assert!(
        matches!(
            compile(&sources),
            Err(CompileError::DuplicateClauseId { .. })
        ),
        "clause ids are what the explain trace names, so they must be unique"
    );
}

#[test]
fn an_unknown_preset_is_refused() {
    let bad = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "a", "doc": "d", "match": {"event": "x.y"}, "facts": {"preset": "whatever"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), bad.to_vec())]);
    assert!(
        matches!(compile(&sources), Err(CompileError::UnknownPreset { .. })),
        "a preset the engine does not define must fail the build, not default to something"
    );
}

#[test]
fn a_longer_prefix_wins_over_a_shorter_one() {
    let nested = br#"{
      "id": "test", "doc": "d",
      "carriers": [
        {"id": "broad", "doc": "d", "match": {"attribute_prefix": "a.b"},
         "facts": {"preset": "snapshot"}},
        {"id": "narrow", "doc": "d", "match": {"attribute_prefix": "a.b.c"},
         "facts": {"preset": "emission"}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), nested.to_vec())]);
    let plan = compile(&sources).expect("nested prefixes are ordered, not ambiguous");
    let hit = plan
        .resolve(&CarrierContext::carrier_only(None, Some("a.b.c.d")))
        .expect("the longer prefix matches");
    assert_eq!(
        hit.clause_id, "narrow",
        "a longer prefix is the more specific statement about a key; the specificity score cannot \
         express that because both clauses constrain the same one dimension"
    );
    let hit = plan
        .resolve(&CarrierContext::carrier_only(None, Some("a.b.x")))
        .expect("the shorter prefix still matches what the longer one does not");
    assert_eq!(hit.clause_id, "broad");
}

/// The success criterion, mechanised: the engine knows no framework.
///
/// Deliberately more than a grep for framework names. A carrier *key* is producer knowledge too, and a
/// generic-looking `String` parameter can smuggle a framework label past a name search - so this checks
/// three things: no producer or carrier literals in the engine source, no framework label on the match
/// context, and every framework fact reachable only through the assets.
///
/// Bounded, and worth stating: it reads the engine's own source. It cannot prove that some other module
/// does not branch on a framework - what it establishes is that the engine does not, which is the claim
/// the rule files rest on.
#[test]
fn the_engine_names_no_framework() {
    // Every engine source. `detect_rules.rs` was missing from this list, which is precisely how a gate
    // stops being one: it passed while the file it did not read was free to name any producer it liked.
    const ENGINE_SOURCES: &[(&str, &str)] = &[
        ("mod.rs", include_str!("mod.rs")),
        ("schema.rs", include_str!("schema.rs")),
        ("carrier_rules.rs", include_str!("carrier_rules.rs")),
        ("detect_rules.rs", include_str!("detect_rules.rs")),
        ("message_rules.rs", include_str!("message_rules.rs")),
        ("tool_repr.rs", include_str!("tool_repr.rs")),
        ("content_blocks.rs", include_str!("content_blocks.rs")),
        ("span_fields.rs", include_str!("span_fields.rs")),
        ("classify.rs", include_str!("classify.rs")),
        ("members.rs", include_str!("members.rs")),
        ("expr.rs", include_str!("expr.rs")),
    ];

    // The engine directory holds nothing else. A new module would otherwise be exempt by omission -
    // the same way `detect_rules.rs` was.
    let declared: Vec<&str> = ENGINE_SOURCES.iter().map(|(name, _)| *name).collect();
    let module_decls = include_str!("mod.rs");
    let mut previous = "";
    for line in module_decls.lines() {
        let trimmed = line.trim();
        // A test module is not engine source; it exists to name the frameworks the assets declare.
        let is_test_module = previous == "#[cfg(test)]";
        previous = trimmed;
        if is_test_module {
            continue;
        }
        // `mod` as well as `pub mod`: a private module is engine source too, and `tool_repr` was one -
        // which is how the last framework grammar sat unread by this gate while it named two of that
        // framework's own repr fields.
        if let Some(rest) = trimmed
            .strip_prefix("pub mod ")
            .or_else(|| trimmed.strip_prefix("mod "))
            && let Some(name) = rest.strip_suffix(';')
        {
            assert!(
                declared.contains(&format!("{name}.rs").as_str()),
                "engine module `{name}` is not covered by this gate: add it to ENGINE_SOURCES"
            );
        }
    }

    // Producer identities. Any of these in the engine means the engine knows a framework.
    const PRODUCER_NAMES: &[&str] = &[
        "crewai",
        "langgraph",
        "langchain",
        "autogen",
        "pydantic",
        "openinference",
        "logfire",
        "mlflow",
        "livekit",
        "bedrock",
        "vertex",
        "vercel",
        "claude_code",
        "strands",
        "traceloop",
        "langsmith",
        "llamaindex",
        "haystack",
        "smolagents",
        "agentscope",
        "langflow",
        "semantic_kernel",
        "browser_use",
        "google.adk",
        "azure",
    ];

    // Carrier vocabulary. A key is a producer's spelling, so it belongs in an asset even though it is
    // not a producer's name.
    const CARRIER_LITERALS: &[&str] = &[
        "gen_ai.",
        "llm.",
        "ai.",
        "output.value",
        "input.value",
        "new_context",
        "request_data",
        "response.model_output",
        "system_prompt",
        "tool_name",
    ];

    for (name, source) in ENGINE_SOURCES {
        // Comments are prose about the design and may legitimately name a framework to explain *why* a
        // mechanism exists. Code may not.
        let code: String = source
            .lines()
            .filter(|line| {
                let t = line.trim_start();
                !t.starts_with("//") && !t.starts_with("///") && !t.starts_with("//!")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let lowered = code.to_lowercase();
        for producer in PRODUCER_NAMES {
            assert!(
                !lowered.contains(producer),
                "engine source `{name}` names the producer `{producer}` in code. Framework identity \
                 belongs in an asset under `server/rules/`, never in the engine"
            );
        }
        for literal in CARRIER_LITERALS {
            assert!(
                !code.contains(literal),
                "engine source `{name}` contains the carrier literal `{literal}` in code. A carrier \
                 key is a producer's spelling and belongs in an asset"
            );
        }
    }

    // No behavioural API accepts a framework label. The dataflow half of the gate: a `framework` field
    // on the match context would let every clause key on identity, which is what the design forbids -
    // and it would read as a perfectly ordinary `Option<&str>` to any name search.
    let context_source = include_str!("carrier_rules.rs");
    let context_decl = context_source
        .split("pub struct CarrierContext")
        .nth(1)
        .and_then(|s| s.split('}').next())
        .expect("the match context is declared here");
    for forbidden in ["framework", "rule_id", "label"] {
        assert!(
            !context_decl.contains(forbidden),
            "`CarrierContext` carries `{forbidden}`. A carrier's meaning is a property of its structure \
             and the span that wrote it, never of a name something matched earlier"
        );
    }
}

/// Every framework fact this slice moved is reachable only through the assets.
///
/// The counterpart to the source gate: the assets must actually carry the table, so "the engine names no
/// framework" cannot be satisfied by an engine that also does nothing.
#[test]
fn the_assets_carry_the_framework_facts() {
    let sources = schema::embedded_sources();
    assert!(
        sources.len() >= 7,
        "expected one asset per dialect, found {}",
        sources.len()
    );
    let all: String = sources
        .values()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();
    // A sample of the vocabulary the engine may not contain: if these are not in the assets, they are
    // nowhere, and the equivalence test would be passing against an empty table.
    for key in [
        "gen_ai.choice",
        "llm.input_messages",
        "ai.response",
        "output.value",
        "new_context",
        "request_data",
        "system_prompt",
    ] {
        assert!(
            all.contains(key),
            "carrier `{key}` is declared in no asset, so nothing declares it at all"
        );
    }
}

/// The facts about the selection language this engine depends on, pinned rather than assumed.
///
/// RFC 9535 JSONPath, and the decisive property is the one asserted last: it returns *borrowed* references
/// into the original value, so a selected subtree cloned out of it keeps the provider's member order.
///
/// That is why it is this language and not JMESPath. JMESPath was implemented first and reverted: it can
/// *construct* objects, which JSONPath cannot, but every result passes through its own sorted-map value
/// tree - so a payload merely selected came back alphabetised, even with no transform at all. On the corpus
/// that reordered the provider's own tool-result payload in two ADK fixtures. Identity was provably
/// unaffected (every content digest identical), but this repository declares serialised member order
/// observable, and a debugger that silently re-orders what a provider sent reports something it was not
/// given. Construction therefore stays structural, cloning provider subtrees unchanged.
///
/// JSON objects are semantically unordered, so *no* portable construction language can promise member
/// order - that is the general answer, not a fault of one crate.
#[test]
fn the_selection_language_behaves_as_the_engine_assumes() {
    use serde_json_path::JsonPath;

    // 1. Compiled paths are `Send`/`Sync`, which the `OnceLock` plan requires.
    fn assert_sync<T: Send + Sync>() {}
    assert_sync::<JsonPath>();

    // 2. A quoted member name reads a *literal* dotted key. This is the bug the hand-built path resolver
    //    had: one dialect's events carry a member called `event.name`, indistinguishable from a nested
    //    `event` -> `name`, and reading it as nested silently found nothing.
    let quoted = JsonPath::parse("$['event.name']").expect("quoted member compiles");
    let event = serde_json::json!({"event.name": "gen_ai.choice"});
    assert_eq!(
        quoted
            .query(&event)
            .exactly_one()
            .ok()
            .and_then(|v| v.as_str()),
        Some("gen_ai.choice"),
        "a quoted member must read the literal key, not descend"
    );

    // 3. Existence, not truthiness. A filter on a member's existence holds even when the value is empty -
    //    which is what the structural vocabulary means by "present", and where JMESPath differed: to it an
    //    empty array is false-like, so a turn whose `parts` is `[]` - a real turn one dialect emits - was
    //    dropped.
    let present = JsonPath::parse("$.contents[?@.role && @.parts]").expect("filter compiles");
    let empty_parts = serde_json::json!({"contents": [{"role": "user", "parts": []}]});
    assert_eq!(
        present.query(&empty_parts).len(),
        1,
        "a filter on existence keeps a member whose value is empty"
    );

    // 4. **Selection preserves the provider's member order, byte for byte.** The property the whole choice
    //    rests on: the query borrows from the original value, so cloning a selected node yields the same
    //    bytes as cloning it directly.
    let payload = serde_json::json!({
        "contents": [{"role": "user", "parts": [{"status": "loaded", "path": "/x", "day": 1}]}]
    });
    let direct = serde_json::to_string(&payload["contents"][0]).expect("json");
    for expression in ["$.contents[*]", "$.contents[?@.role]", "$.contents[0]"] {
        let path = JsonPath::parse(expression).expect("compiles");
        let selected = path.query(&payload);
        let node = selected.first().expect("one node");
        assert_eq!(
            serde_json::to_string(node).expect("json"),
            direct,
            "`{expression}` must return the provider's payload unchanged, member order included"
        );
    }
}

/// The *extraction* layer names no framework either.
///
/// `the_engine_names_no_framework` reads the rules engine; this reads the code that calls it. That was the
/// gap Codex named: every framework message and tool-definition carrier had moved into the assets, and
/// nothing held the file to it - a new hardcoded carrier key would compile, pass, and quietly re-open the
/// hole. Measured on the source, ignoring `#[cfg(test)]` items, which are the retired reference
/// implementations the equivalence oracles compare against and legitimately name every dialect.
#[test]
fn message_extraction_names_no_framework() {
    // Both extraction files. `attributes.rs` reached zero the same way `messages.rs` did - every chain, table
    // and sweep moved to an asset - and a *measured* zero decays, so it is gated by the same instrument.
    const SOURCES: &[(&str, &str)] = &[
        ("messages.rs", include_str!("../traces/extract/messages.rs")),
        (
            "attributes.rs",
            include_str!("../traces/extract/attributes.rs"),
        ),
    ];

    // Producer names, the *carrier keys* that are producer knowledge even when the identifier is not, and
    // the **constant identifiers** that stand for those keys. Matching lowercase text alone was itself a
    // source of false confidence: `keys::AI_PROMPT` and `"VercelAISDK"` are a framework fact that no
    // lowercase marker sees, which is how two debug gates survived a pass that reported zero.
    const FRAMEWORK_MARKERS: &[&str] = &[
        "langgraph",
        "langchain",
        "crewai",
        "crew_",
        "autogen",
        "vercel",
        "strands",
        "pydantic",
        "logfire",
        "mlflow",
        "livekit",
        "traceloop",
        "langsmith",
        "openinference",
        "claude_code",
        "gcp_vertex",
        "gcp.vertex",
        "ai.prompt",
        "ai.toolCall",
        "ai.result",
        "ai.response",
        "llm.tools",
        "llm.input_messages",
        "llm.output_messages",
        "lk.",
        "pydantic_ai",
        "OI_TOOL",
        "RAW_INPUT",
        "SYSTEM_PROMPT",
        "REQUEST_DATA",
        "raw_input",
        // Constant identifiers for the same keys, and the SDK names used in log messages.
        "AI_PROMPT",
        "AI_TOOLCALL",
        "AI_RESPONSE",
        "AI_RESULT",
        "LLM_TOOLS",
        "LLM_INPUT",
        "LLM_OUTPUT",
        "GCP_VERTEX",
        "vercelaisdk",
        "langchain",
        // The keys and identifiers the *field* and *classification* migrations retired. A span kind, a tag
        // list, a serialised request or response, an embedded usage object, and the bare counter names one
        // CLI writes - each is a producer's spelling and belongs in an asset.
        "openinference.span",
        "langsmith.span",
        "logfire.tags",
        "logfire.msg",
        "response_data",
        "request_data",
        "mlflow.chat",
        "crew_agents",
        "models_usage",
        "token_usage",
        "cached_prompt_tokens",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_creation_tokens",
        "OPENINFERENCE_SPAN_KIND",
        "LANGSMITH_SPAN_KIND",
        "LOGFIRE_MSG",
        "MLFLOW_CHAT",
        "RESPONSE_DATA",
        "GCP_VERTEX_LLM",
        "AI_MODEL_ID",
        "AI_MODEL_PROVIDER",
        "AI_OPERATION_ID",
        "AI_TELEMETRY",
        "LANGGRAPH_THREAD_ID",
        "MLFLOW_TRACE",
        "SemanticKind",
    ];

    // A marker counts only as a whole key: `gen_ai.prompt` is the *convention's* request family, not one
    // dialect's `ai.prompt`, and a substring search cannot tell them apart.
    fn names_marker(line: &str, marker: &str) -> bool {
        line.match_indices(marker).any(|(at, _)| {
            let preceded_by = line[..at].chars().next_back();
            !preceded_by.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
    }

    // No stated exceptions. There were two - one dialect's stand-ins for the generic input/output pair,
    // kept in Rust because "only if nothing recognised this span" is cross-rule state the engine forbids.
    // That turned out to be a *stage*, which the engine can own: they are declared with `stage: fallback`
    // now, and this list is empty. An enumerated exception would still be better than a silent one, so the
    // list stays as the place a future one has to be written down.
    const STATED_EXCEPTIONS: &[&str] = &[];

    let mut offenders = Vec::new();
    for (file, source) in SOURCES {
        let mut in_test_item = false;
        let mut seen_open = false;
        let mut test_depth: i32 = 0;
        let mut pending_test = false;
        for (number, line) in source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.contains("#[cfg(test)]") {
                pending_test = true;
                continue;
            }
            if pending_test {
                // The item the attribute applies to: skipped until its braces balance, or - for a `const` or a
                // `use` - until its statement ends.
                in_test_item = true;
                seen_open = false;
                test_depth = 0;
                pending_test = false;
            }
            if in_test_item {
                test_depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
                if line.contains('{') {
                    seen_open = true;
                }
                if (seen_open && test_depth <= 0) || (!seen_open && trimmed.ends_with(';')) {
                    in_test_item = false;
                }
                continue;
            }
            if trimmed.starts_with("//") || STATED_EXCEPTIONS.contains(&trimmed) {
                continue;
            }
            // Case-insensitively, because a constant is upper case and a key is lower, and the same fact must
            // not evade the gate by how it happens to be spelled.
            let folded = line.to_ascii_lowercase();
            if let Some(marker) = FRAMEWORK_MARKERS
                .iter()
                .find(|m| names_marker(&folded, &m.to_ascii_lowercase()))
            {
                offenders.push(format!(
                    "  {file}:{}: {} <- `{marker}`",
                    number + 1,
                    trimmed
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "extraction names {} framework fact(s) in production code. Every carrier, counter, spelling and \
         precedence a framework writes belongs in `server/rules/*.json`:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// Every `PredicateSet` the schema declares is validated by somebody, tracked by `Struct::field`.
///
/// The recursive pass grew one location at a time, each added after a review found it unvisited -
/// `require_parent`, an overlay's witness, a fragment's cases, an element pass and its derived cases. What
/// makes the *next* one fail loudly is this list, and it has to be keyed by struct as well as field: three
/// structs declare a `require`, so matching on the name alone would accept a fourth without comment.
#[test]
fn every_predicate_set_in_the_schema_is_validated() {
    const SCHEMA: &str = include_str!("schema.rs");

    // `Struct::field` → where its validation lives. The first group is reached by
    // `message_rules::predicate_sets`; the rest are separate domains, named so an exemption is a statement.
    const VALIDATED: &[(&str, &str)] = &[
        ("OverlaySpec::witness", "predicate_sets"),
        ("OverlaySpec::require", "predicate_sets"),
        ("WrapSpec::require_after", "predicate_sets"),
        (
            "AttachSpec::require",
            "predicate_sets, via every envelope's attachments",
        ),
        ("Alternative::require_parent", "predicate_sets"),
        (
            "Alternative::require",
            "predicate_sets, including fragment and extra cases",
        ),
        ("ComposeSpec::require", "predicate_sets"),
        ("SectionRoute::skip_when", "predicate_sets"),
        ("ElementPass::when", "predicate_sets"),
        (
            "DerivedCase::when",
            "predicate_sets, via an element pass's grouping",
        ),
        ("PrependSpec::require", "predicate_sets"),
        (
            "ContentBlockRule::require",
            "content_blocks::compile, its own plan",
        ),
    ];

    let mut declared: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in SCHEMA.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            current = rest
                .trim_end_matches(" {")
                .split(&['<', ' '][..])
                .next()
                .unwrap_or_default()
                .to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("pub ")
            && let Some((name, kind)) = rest.split_once(": ")
            && kind.trim_end_matches(',') == "PredicateSet"
        {
            declared.push(format!("{current}::{name}"));
        }
    }

    assert!(
        !declared.is_empty(),
        "the schema should declare predicate sets; the reader is broken"
    );
    for location in &declared {
        assert!(
            VALIDATED.iter().any(|(known, _)| known == location),
            "`{location}` is a `PredicateSet` the validation does not know about. Add it to \
             `message_rules::predicate_sets`, or - if it belongs to a separate domain - name it in \
             VALIDATED with where that validation lives."
        );
    }
    // And the reverse: a location that stops existing should be removed here, not left as a claim about a
    // field nobody declares.
    for (known, _) in VALIDATED {
        assert!(
            declared.iter().any(|location| location == known),
            "VALIDATED names `{known}`, which the schema no longer declares"
        );
    }
}

/// `exists: false` beside a condition that needs a value is refused, for every such condition.
///
/// The absent-value branch returns before most conditions are consulted, so each one declared beside
/// `exists: false` is silently ignored rather than failing. `one_of` was missed twice - once when the check
/// was written, and once when I added it to the wrong block - so every member of the set is asserted here
/// rather than trusted.
#[test]
fn exists_false_beside_a_value_condition_is_refused() {
    use crate::domain::rules::schema::{PredicateSet, ValuePredicate};

    let with = |mutate: fn(&mut ValuePredicate)| -> PredicateSet {
        let mut predicate: ValuePredicate =
            serde_json::from_value(serde_json::json!({"path": "$.x", "exists": false}))
                .expect("a predicate parses");
        mutate(&mut predicate);
        PredicateSet {
            all: vec![predicate],
            any: Vec::new(),
        }
    };

    let cases: Vec<(&str, PredicateSet)> = vec![
        (
            "kind",
            with(|p| p.kind = Some(crate::domain::rules::schema::ValueKind::String)),
        ),
        ("non_empty", with(|p| p.non_empty = Some(true))),
        ("not_null", with(|p| p.not_null = Some(true))),
        ("identifier_like", with(|p| p.identifier_like = Some(true))),
        (
            "starts_with",
            with(|p| p.starts_with = Some("a".to_string())),
        ),
        (
            "lacks_prefix",
            with(|p| p.lacks_prefix = Some("a".to_string())),
        ),
        ("one_of", with(|p| p.one_of = vec!["a".to_string()])),
    ];
    for (name, set) in cases {
        assert!(
            crate::domain::rules::message_rules::predicate_defect(&set).is_some(),
            "`exists: false` beside `{name}` compiled, and the condition is ignored at runtime"
        );
    }

    // `none_of` is the exception, and it is documented: its reading accepts absence, which is how a
    // dialect's unnamed events fall through to the reading that handles them.
    let none_of = with(|p| p.none_of = vec!["a".to_string()]);
    assert!(
        crate::domain::rules::message_rules::predicate_defect(&none_of).is_none(),
        "`none_of` accepts absence by design and must stay legal beside `exists: false`"
    );
}

/// The **selected** root-level defects are refused, and every satisfiable predicate is still accepted.
///
/// Selected, deliberately: this is not a satisfiability decision procedure. It proves a chosen set of
/// defects at the *root*, where the value always exists, is singular, and has one spelling - and accepts
/// everything else, including a genuine contradiction on a singular member path. Under-refusing is the right
/// failure here: every over-refusal in this area has been mine, and each one rejected a working rule.
///
/// A **curated** table, not a sample. My first version generated field pairs and judged them by whether they
/// held over eight example values, which accused `starts_with: "a"` on the root of being impossible - it is
/// satisfiable, just not by any of those eight. "Unsatisfied by my sample" is not "unsatisfiable", and a test
/// that confuses them argues for removing real rules.
///
/// Each defect in this class was found one at a time by review - `exists: false` against five conditions, a
/// text condition against five kinds, a root tautology in three spellings, then set-level negation - so the
/// satisfiable half matters as much: it is what stopped the fix from over-refusing.
#[test]
fn the_selected_root_level_predicate_defects_are_refused() {
    use crate::domain::rules::message_rules::predicate_defect;
    use crate::domain::rules::schema::PredicateSet;
    use serde_json::json;

    let set = |value: serde_json::Value| -> PredicateSet {
        serde_json::from_value(value).expect("the probe set parses")
    };

    // Refused: each holds for everything, or for nothing.
    let refused: Vec<(&str, serde_json::Value)> = vec![
        ("the root always exists", json!({"all": [{}]})),
        (
            "the root, spelled out",
            json!({"all": [{"path": "$", "exists": true}]}),
        ),
        (
            "the root cannot be absent",
            json!({"all": [{"path": "$", "exists": false, "none_of": ["x"]}]}),
        ),
        (
            "null and not-null",
            json!({"all": [{"path": "$.v", "kind": "null", "not_null": true}]}),
        ),
        (
            "a kind that is not null, beside not_null false",
            json!({"all": [{"path": "$.v", "kind": "string", "not_null": false}]}),
        ),
        (
            "one value both required and forbidden",
            json!({"all": [{"path": "$.v", "one_of": ["a"], "none_of": ["a"]}]}),
        ),
        (
            "a text condition on a number",
            json!({"all": [{"path": "$.v", "kind": "number", "starts_with": "a"}]}),
        ),
        (
            "an any set holding a root condition and its negation",
            json!({"any": [{"not_null": true}, {"not_null": false}]}),
        ),
        (
            "the same, with the two set conditions",
            json!({"any": [{"one_of": ["a"]}, {"none_of": ["a"]}]}),
        ),
        (
            "an all set naming two root kinds",
            json!({"all": [{"kind": "string"}, {"kind": "number"}]}),
        ),
        (
            "an all root kind that every any member contradicts",
            json!({"all": [{"kind": "string"}], "any": [{"kind": "number"}]}),
        ),
        (
            "a root kind that is not null, beside a required branch asserting it is null",
            json!({"all": [{"kind": "string"}], "any": [{"not_null": false}]}),
        ),
        (
            "an any set holding identifier-like and its negation",
            json!({"any": [{"identifier_like": true}, {"identifier_like": false}]}),
        ),
        (
            "a required prefix that begins with the forbidden one",
            json!({"all": [{"path": "$.v", "starts_with": "ab", "lacks_prefix": "a"}]}),
        ),
    ];
    for (why, value) in refused {
        assert!(
            predicate_defect(&set(value.clone())).is_some(),
            "{why}: accepted, and it says nothing - {value}"
        );
    }

    // Accepted: each is an ordinary statement some value satisfies and some does not.
    let accepted: Vec<(&str, serde_json::Value)> = vec![
        ("a member is present", json!({"all": [{"path": "$.v"}]})),
        (
            "a member is absent",
            json!({"all": [{"path": "$.v", "exists": false}]}),
        ),
        (
            "the root is a string",
            json!({"all": [{"path": "$", "kind": "string"}]}),
        ),
        (
            "non-overlapping prefixes",
            json!({"all": [{"path": "$.v", "starts_with": "a", "lacks_prefix": "b"}]}),
        ),
        (
            "absence, or a value not in a set - the reading that lets an unnamed event fall through",
            json!({"all": [{"path": "$.v", "exists": false}], "any": [{"path": "$.w", "none_of": ["x"]}]}),
        ),
        (
            "two kinds as alternatives, which is what `any` is for",
            json!({"any": [{"path": "$.v", "kind": "string"}, {"path": "$.v", "kind": "number"}]}),
        ),
        (
            "conditions on different members",
            json!({"all": [{"path": "$.a", "kind": "string"}, {"path": "$.b", "kind": "number"}]}),
        ),
        // The three shapes a set-level check must *not* refuse. Each was refused by my first version, which
        // compared any two members sharing a rendered path.
        (
            "a member and its negation: when the member is absent both fail, so the pair means it exists",
            json!({"any": [{"path": "$.v", "not_null": true}, {"path": "$.v", "not_null": false}]}),
        ),
        (
            "two kinds against a plural path, which each predicate tests existentially",
            json!({"all": [{"path": "$.*", "kind": "string"}, {"path": "$.*", "kind": "number"}]}),
        ),
        (
            "a forbidden prefix longer than the required one - `ac` satisfies both",
            json!({"all": [{"path": "$.v", "starts_with": "a", "lacks_prefix": "ab"}]}),
        ),
        // A complement pair whose branches *narrow*: false for a non-null number, so not a tautology.
        (
            "one branch of a complement pair carries another condition",
            json!({"any": [{"kind": "string", "not_null": true}, {"not_null": false}]}),
        ),
        // The forbidden set is wider than the required one, so `"b"` satisfies neither branch.
        (
            "a forbidden set wider than the required one",
            json!({"any": [{"one_of": ["a"]}, {"none_of": ["a", "b"]}]}),
        ),
        // Accepted, and stated as such: a singular *member* path contradiction is outside what this proves.
        (
            "two kinds on a singular member path - a real contradiction this deliberately does not catch",
            json!({"all": [{"path": "$.v", "kind": "string"}, {"path": "$.v", "kind": "number"}]}),
        ),
    ];
    for (why, value) in accepted {
        assert!(
            predicate_defect(&set(value.clone())).is_none(),
            "{why}: refused, and it is satisfiable - {value}"
        );
    }
}

/// A compose owns every attribute it read, not just its synthetic tag.
///
/// Reachable in the shipped assets: Vercel's `ai.response` compose falls back to `output.value` on an `ai.*`
/// span carrying `ai.prompt.messages`, and LangGraph reads `output.value` directly. Owning only the tag left
/// the attribute unclaimed, so a span holding both dialects' evidence emitted the answer twice - and rank
/// could not separate them, because the two rules were never competing for the same name.
#[test]
fn a_compose_claims_the_attributes_it_read_not_only_its_tag() {
    let plan = &super::ruleset().messages;
    let mut attrs = std::collections::HashMap::new();
    // Vercel's own evidence, with no `ai.response.text`: the compose reaches its `output.value` fallback.
    attrs.insert(
        "ai.prompt.messages".to_string(),
        r#"[{"role":"user","content":"q"}]"#.to_string(),
    );
    // LangGraph's evidence, so its `output.value` rule is live on the same span.
    attrs.insert("langgraph.node".to_string(), "agent".to_string());
    attrs.insert(
        "metadata".to_string(),
        r#"{"langgraph_step":1}"#.to_string(),
    );
    attrs.insert(
        "output.value".to_string(),
        r#"{"role":"assistant","content":"the answer"}"#.to_string(),
    );
    let ctx =
        super::message_rules::MessageContext::for_span("ai.generateText.doGenerate", &attrs, false);

    let emissions = plan.run(&ctx);
    // Not "who declared ownership" but "who emitted the payload": with the compose owning only its tag,
    // both rules read `output.value` and the span reported the answer twice.
    let readers: Vec<&str> = emissions
        .iter()
        .filter(|e| e.value.to_string().contains("the answer"))
        .map(|e| e.rule_id)
        .collect();
    assert_eq!(
        readers.len(),
        1,
        "two rules emitted the same output.value payload: {readers:?} - the compose must own what it read"
    );
}

/// An indexed family owns every physical key it read, including a positional overlay's payload.
///
/// Reachable in the shipped assets, as the compose case was: OpenInference assembles a user turn from
/// `llm.input_messages.0.message.*` and overlays the richer serialised copy out of `input.value`, while
/// LangGraph reads `input.value` as a node's state. With the family owning only its assembled entry tag -
/// `llm.input_messages.0.message`, a name no producer wrote - the overlay's payload stayed unclaimed and the
/// same user turn came back twice.
#[test]
fn an_indexed_family_claims_the_overlay_payload_it_joined_against() {
    let plan = &super::ruleset().messages;
    let mut attrs = std::collections::HashMap::new();
    attrs.insert(
        "llm.input_messages.0.message.role".to_string(),
        "user".to_string(),
    );
    // Flattened content, which is the shape the overlay exists to improve on.
    attrs.insert(
        "llm.input_messages.0.message.contents.0.message_content.type".to_string(),
        "text".to_string(),
    );
    attrs.insert(
        "llm.input_messages.0.message.contents.0.message_content.text".to_string(),
        "look".to_string(),
    );
    // The serialised copy, in this dialect's own shape so the overlay's witness holds.
    attrs.insert(
        "input.value".to_string(),
        r#"{"messages":[{"id":["langchain","schema","messages","HumanMessage"],"type":"human","content":[{"type":"text","text":"look at the picture"}]}]}"#
            .to_string(),
    );
    // LangGraph's evidence, so its own reading of `input.value` is live on this span.
    attrs.insert("langgraph.node".to_string(), "agent".to_string());
    attrs.insert(
        "metadata".to_string(),
        r#"{"langgraph_step":1}"#.to_string(),
    );

    let ctx = super::message_rules::MessageContext::for_span("RunnableSequence", &attrs, false);
    // Both dialects recognise this payload on their own, which is what makes the claim load-bearing rather
    // than incidental: without it OpenInference emitted the turn from its overlay and LangGraph emitted the
    // same turn from the same attribute.
    let readers: Vec<&str> = plan
        .run(&ctx)
        .iter()
        .filter(|e| e.value.to_string().contains("look at the picture"))
        .map(|e| e.rule_id)
        .collect();
    assert_eq!(
        readers.len(),
        1,
        "two rules emitted the same input.value payload: {readers:?} - the overlay's source must be owned"
    );
}

/// A classification rule must answer in the vocabulary its classification owns.
///
/// A misspelling was silently the *fallback* answer - a plain span, or `other` - which is exactly what "no rule
/// held" means, so a typo was indistinguishable from a rule that did not apply. And the two classifications
/// have different vocabularies, so a valid answer for one is not automatically valid for the other.
#[test]
fn a_classification_rule_answers_in_its_own_vocabulary() {
    use super::classify::{ClassifyCompileError, compile};

    let refused = [
        (
            "a misspelled observation type",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{"attr_exists":["k"]}],"result":"genration"}]}"#
                .to_vec(),
        ),
        (
            "a category answer given as an observation type",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{"attr_exists":["k"]}],"result":"llm"}]}"#
                .to_vec(),
        ),
        (
            "an observation type given as a category",
            br#"{"id":"t","doc":"d","span_categories":[
                {"id":"x","rank":1,"all_of":[{"attr_exists":["k"]}],"result":"generation"}]}"#
                .to_vec(),
        ),
        (
            "no answer at all",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{"attr_exists":["k"]}],"result":""}]}"#
                .to_vec(),
        ),
        (
            "no condition, which would answer every span",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[],"result":"span"}]}"#
                .to_vec(),
        ),
        (
            "a condition that can never hold",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{}],"result":"span"}]}"#
                .to_vec(),
        ),
        (
            "a resource dimension, which classification is never given",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{"service_name":["svc"]}],"result":"span"}]}"#
                .to_vec(),
        ),
        (
            "two rules of one classification sharing a rank",
            br#"{"id":"t","doc":"d","observation_types":[
                {"id":"x","rank":1,"all_of":[{"attr_exists":["a"]}],"result":"span"},
                {"id":"y","rank":1,"all_of":[{"attr_exists":["b"]}],"result":"agent"}]}"#
                .to_vec(),
        ),
        (
            "one id naming a rule in each classification",
            br#"{"id":"t","doc":"d",
                "observation_types":[{"id":"x","rank":1,"all_of":[{"attr_exists":["a"]}],"result":"span"}],
                "span_categories":[{"id":"x","rank":1,"all_of":[{"attr_exists":["b"]}],"result":"other"}]}"#
                .to_vec(),
        ),
    ];
    for (what, asset) in refused {
        let sources = std::collections::BTreeMap::from([("t.json".to_string(), asset)]);
        assert!(
            compile(&sources).is_err(),
            "should have been refused: {what}"
        );
    }

    // The same rank in *different* classifications means nothing and is accepted, which is what keeps the
    // refusal above a statement about precedence rather than about numbers.
    let across = br#"{"id":"t","doc":"d",
        "observation_types":[{"id":"o","rank":1,"all_of":[{"attr_exists":["a"]}],"result":"span"}],
        "span_categories":[{"id":"c","rank":1,"all_of":[{"attr_exists":["b"]}],"result":"other"}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), across.to_vec())]);
    assert!(
        compile(&sources).is_ok(),
        "a rank means nothing across classifications: {:?}",
        compile(&sources).err()
    );

    // And the error names what was expected, so a typo is fixable from the message alone.
    let typo = br#"{"id":"t","doc":"d","observation_types":[
        {"id":"x","rank":1,"all_of":[{"attr_exists":["k"]}],"result":"genration"}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), typo.to_vec())]);
    assert!(
        matches!(
            compile(&sources),
            Err(ClassifyCompileError::UnknownResult { .. })
        ),
        "the refusal says the answer is unknown, and lists the ones that are not"
    );
}

/// A declared provider alias must be reachable, must not shadow the catalogue, and must name a provider it knows.
///
/// The alias table answers only where the catalogue's own table says nothing, so a shadowing declaration cannot
/// take effect - and a declaration that cannot take effect reads as one that does, which is the class this engine
/// refuses. Two more, for the same reason: a key normalisation would never produce is unreachable, and a target
/// the catalogue does not read as a provider resolves to a name nothing prices, which looks like a priced call.
#[test]
fn a_provider_alias_must_be_reachable_and_not_shadow_the_catalogue() {
    use super::schema::RuleFile;

    let compiled = |alias: serde_json::Value| {
        let file: RuleFile = serde_json::from_value(serde_json::json!({
            "id": "probe",
            "provider_aliases": [alias],
        }))
        .expect("the probe asset parses");
        super::compile_provider_aliases(&[file])
    };

    for (what, alias) in [
        (
            "a key the catalogue already reads as a provider",
            serde_json::json!({"system": "openai", "provider": "gemini"}),
        ),
        (
            "a key normalisation would never produce",
            serde_json::json!({"system": "google-adk", "provider": "gemini"}),
        ),
        (
            "a key with upper case, which normalisation folds away",
            serde_json::json!({"system": "Google_ADK", "provider": "gemini"}),
        ),
        (
            "a provider the catalogue does not know",
            serde_json::json!({"system": "some_framework", "provider": "not_a_provider"}),
        ),
        (
            "an empty system",
            serde_json::json!({"system": "", "provider": "gemini"}),
        ),
    ] {
        assert!(compiled(alias).is_err(), "should have been refused: {what}");
    }

    // And the shape the shipped asset uses compiles.
    assert!(
        compiled(serde_json::json!({"system": "some_framework", "provider": "gemini"})).is_ok(),
        "a framework naming a provider the catalogue knows is the whole point"
    );
}

/// Assets that identify **no producer**: the conventions and this engine's own shared vocabulary. Each is
/// checked to declare no `detect` rule, which is the property that makes it shared - so an entry that does
/// identify a producer cannot hide here.
const SHARED_VOCABULARY: &[&str] = &[
    "semconv",
    "generic-io",
    "message-members",
    "observation-types",
    "span-categories",
    "span-fields-display",
    "span-fields-genai",
    "span-fields-semantic",
    "span-fields-usage",
    "content-blocks-vercel",
    "content-blocks-wrappers",
];

/// Assets naming a **provider** rather than a framework. The pricing catalogue is entitled to those names,
/// and each is checked against the catalogue's own table - so a *framework* cannot hide here either.
///
/// Exclusions rather than a list of frameworks, because the frameworks are the part that grows: a new asset
/// is a new name the sweep must know, and deriving it means adding one cannot be forgotten. And exclusions
/// need a property each, or the list is a way to make the sweep quiet.
const PROVIDERS: &[&str] = &["bedrock", "azure-openai", "vertex-ai"];

/// **No production module names a framework**, across the whole server, with the exemptions named.
///
/// The two extraction files have their own gate above, with the carrier keys and constant identifiers that are
/// framework facts even where the identifier is not. This one is the wider claim, and it exists because that
/// claim was being kept by review and by a sweep I ran by hand: a framework-specific branch in an unguarded
/// module escaped both. It reads the source tree, so a new module is covered the day it is written.
///
/// What it **does** see is the whole tree's production tokens, and the two things that make that a claim rather
/// than a grep are that the marker list is *derived* from the assets (plus the aliases below, each tied to one)
/// and that an exemption is scoped to the names it allows rather than to a file.
///
/// Four things it cannot see, stated because a sweep reads as exhaustive:
///
/// - A framework named by a **value** rather than a name. A key like `crew_key` is caught by the marker list; a
///   bare `"kwargs"` is not. That is why the equivalence oracles matter more than this test does.
/// - **Prose.** A doc comment is skipped, and the explanations in this tree name the producers whose telemetry
///   motivated each rule - deliberately, because that is what such an explanation is. So is a doc comment that
///   `schemars` renders into a shipped description, which is the one place prose becomes data the server sends.
/// - Anything outside `server/src`. The SDKs are separate crates and *are* meant to name the framework they
///   instrument, so the claim is about the server's own parsing.
/// - A name assembled at runtime from parts that are not adjacent - `[a, b].concat()`, or a name built from a
///   variable. Adjacent literals **are** caught wherever they are laid out, because the source is tokenised and a
///   run of literals is joined as tokens - so `concat!("lang", "graph")` is `langgraph` even split across lines.
#[test]
fn no_production_module_names_a_framework() {
    /// A shorter name the same producer goes by, tied to the asset it belongs to.
    ///
    /// Deriving separator variants of an asset id is not enough, and the gap is the shape the code actually used:
    /// `vercel-ai` yields `vercel-ai`, `vercel_ai` and `vercelai`, none of which is in `try_vercel_format` - the
    /// very name this migration removed from production. Each alias names its asset so a renamed or deleted asset
    /// breaks the list loudly instead of leaving a marker that matches nothing.
    ///
    /// Deliberately not derived from an id's segments: that yields `ai`, `agent`, `sdk`, `framework` and `openai`,
    /// which are words this server is entitled to use. So the aliases are the ones a producer is *actually* known
    /// by, and `openai-agents` gets none - `openai` is a provider the catalogue prices and a name the connectors
    /// carry, so as a marker it would say nothing about framework knowledge.
    const ALIASES: &[(&str, &str)] = &[
        ("vercel-ai", "vercel"),
        ("google-adk", "adk"),
        ("pydantic-ai", "pydantic"),
        ("azure-ai-foundry", "foundry"),
        // The CLI the SDK spawns names its spans `claude_code.*`, and the retired extractor was `try_claude_code`.
        ("claude-agent-sdk", "claude_code"),
        ("claude-agent-sdk", "claudecode"),
        // The package is `llama_index`, which no separator variant of the id spells.
        ("llamaindex", "llama_index"),
        ("llamaindex", "llama-index"),
    ];

    // The framework names, derived from the assets and spelled every way a separator can be written - a name is
    // `google-adk` in an asset id and `google_adk` or `googleadk` in code - plus the aliases above.
    let sources = crate::domain::rules::schema::embedded_sources();
    let ids: Vec<String> = sources
        .keys()
        .map(|path| path.trim_end_matches(".json").to_string())
        .collect();
    assert!(ids.len() > 20, "only {} assets were found", ids.len());
    for named in SHARED_VOCABULARY
        .iter()
        .chain(PROVIDERS)
        .chain(ALIASES.iter().map(|(id, _)| id))
    {
        assert!(
            ids.iter().any(|id| id == named),
            "an exclusion or alias names `{named}`, which is not an asset"
        );
    }
    // Each exclusion carries a property, or the list is a way to make the sweep quiet about a framework.
    for shared in SHARED_VOCABULARY {
        let file: crate::domain::rules::schema::RuleFile =
            serde_json::from_slice(&sources[&format!("{shared}.json")]).expect("the asset parses");
        assert!(
            file.detect.is_empty(),
            "`{shared}` is excluded as shared vocabulary and declares detection, so it identifies a producer"
        );
    }
    for provider in PROVIDERS {
        let normalised = provider.replace('-', "_");
        assert!(
            !crate::domain::pricing::builtin_provider(&normalised).is_empty(),
            "`{provider}` is excluded as a provider and the catalogue does not read it as one"
        );
    }
    // A marker set per framework, because an exemption is scoped to the names it allows rather than to a file.
    let markers: std::collections::BTreeMap<String, Vec<String>> = ids
        .iter()
        .filter(|id| !SHARED_VOCABULARY.contains(&id.as_str()) && !PROVIDERS.contains(&id.as_str()))
        .map(|id| {
            let mut names: Vec<String> = [id.clone(), id.replace('-', "_"), id.replace('-', "")]
                .into_iter()
                .chain(
                    ALIASES
                        .iter()
                        .filter(|(asset, _)| asset == id)
                        .map(|(_, alias)| alias.to_string()),
                )
                .collect();
            names.sort();
            names.dedup();
            (id.clone(), names)
        })
        .collect();
    // An alias no shorter than a name already derived from its own asset is dead weight that reads as protection.
    for (asset, alias) in ALIASES {
        let derived = [
            asset.to_string(),
            asset.replace('-', "_"),
            asset.replace('-', ""),
        ];
        assert!(
            !derived
                .iter()
                .any(|name| name == alias || alias.contains(name)),
            "the alias `{alias}` is already covered by a name derived from `{asset}`"
        );
    }
    let every_marker: Vec<(&str, &str)> = markers
        .iter()
        .flat_map(|(id, names)| names.iter().map(move |name| (id.as_str(), name.as_str())))
        .collect();

    /// What an exempt file is allowed to name.
    ///
    /// Scoped to markers, not to the file: exempting a whole file is a hole the size of the file, and a
    /// connector module is entitled to the product it connects to, not to every framework's telemetry dialect.
    /// Verified by `an_exempt_file_may_name_only_what_its_exemption_allows`.
    enum Allowed {
        /// Every framework, which only a module whose subject *is* the list of them can claim.
        EveryFramework,
        /// These assets' names, and no others.
        Only(&'static [&'static str]),
    }

    /// Files allowed to name something, what each may name, and why. Each is a *statement*, not an oversight.
    ///
    /// All four are code whose subject **is** the named thing rather than telemetry it wrote. The engine's own
    /// module documentation says as much about the connectors: they build real clients with real auth flows, and
    /// calling that data would be dishonest.
    const EXEMPT: &[(&str, Allowed, &str)] = &[
        (
            "src/api/mcp/tools.rs",
            Allowed::EveryFramework,
            "generates integration documentation for an AI assistant, so naming each framework is its job - it \
             interprets no telemetry",
        ),
        (
            "src/domain/providers/catalog.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "catalogues the inference providers a user may connect to, where the name is the subject rather \
             than a producer's spelling",
        ),
        (
            "src/domain/providers/test_connection.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "builds a real client against a named provider to test a credential - an adapter, not a parser",
        ),
        (
            "src/domain/providers/service.rs",
            Allowed::Only(&["azure-ai-foundry"]),
            "manages those connections by provider key, which is the catalogue's own vocabulary",
        ),
    ];
    for (file, allowed, _) in EXEMPT {
        if let Allowed::Only(assets) = allowed {
            for asset in *assets {
                assert!(
                    markers.contains_key(*asset),
                    "`{file}` is allowed to name `{asset}`, which is not a framework asset"
                );
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();
    let mut exempt_used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut exempt_names_found: std::collections::BTreeSet<(&str, &str)> =
        std::collections::BTreeSet::new();
    let mut files = 0_usize;
    // Counted apart from `files`, because an exemption matching everything would leave the offender list empty
    // and every other assertion satisfied - the sweep has to say how much it actually *checked*.
    let mut checked = 0_usize;
    let mut skipped_tests = 0_usize;
    walk_rust_sources(&root, &mut |path, source| {
        files += 1;
        let relative = path
            .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // A test module is not production: the oracles name every framework they reproduce, deliberately.
        if relative.contains("_tests.rs") || relative.ends_with("/tests.rs") {
            skipped_tests += 1;
            return;
        }
        // Source pulled in from elsewhere is source this walk never sees. `#[path]` is used throughout for
        // test modules that sit **beside** their subject - files this walk reads directly and skips as
        // tests - so those are fine; anything else, and any `include!`, is a hole in the claim.
        assert_no_foreign_source(source, &relative);

        let exemption = EXEMPT.iter().find(|(file, _, _)| relative.ends_with(file));
        if let Some((name, _, _)) = exemption {
            exempt_used.insert(name);
        } else {
            checked += 1;
        }
        for (number, text) in production_names(source, &relative) {
            for (asset, marker) in &every_marker {
                if !names_as_a_word(&text, marker) {
                    continue;
                }
                match exemption {
                    // An exemption for a name a file does not use is unnecessary, and an unnecessary exemption
                    // is how such a list grows until it covers something that matters.
                    Some((name, Allowed::EveryFramework, _)) => {
                        exempt_names_found.insert((name, asset));
                    }
                    Some((name, Allowed::Only(assets), _)) if assets.contains(asset) => {
                        exempt_names_found.insert((name, asset));
                    }
                    // Exempt for one product's name, and this is a different one.
                    _ => offenders.push(format!("  {relative}:{number}: {text} <- `{marker}`")),
                }
            }
        }
    });

    assert!(
        files > 50,
        "the sweep read only {files} files, which cannot be the whole tree"
    );
    assert!(
        checked + EXEMPT.len() + skipped_tests == files,
        "the sweep read {files} files and checked {checked} of them - an exemption is matching more than the \
         file it names, which would make an empty offender list mean nothing"
    );
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "{} production line(s) name a framework. Every fact a framework writes belongs in \
         `server/rules/*.json`; if a module genuinely has to name one, add it to EXEMPT with the \
         reason:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
    // An exemption for a file that no longer exists, or for a name it no longer uses, is a statement about
    // nothing - and the allowance is per asset, so each one has to be earned.
    for (file, allowed, _) in EXEMPT {
        assert!(
            exempt_used.contains(file),
            "EXEMPT names `{file}`, which the sweep did not find"
        );
        match allowed {
            Allowed::EveryFramework => assert!(
                exempt_names_found.iter().any(|(name, _)| name == file),
                "EXEMPT names `{file}`, which names no framework - the exemption is unnecessary, and an \
                 unnecessary exemption is how such a list grows until it covers something that matters"
            ),
            Allowed::Only(assets) => {
                for asset in *assets {
                    assert!(
                        exempt_names_found.contains(&(*file, *asset)),
                        "`{file}` is allowed to name `{asset}` and does not - the allowance is unnecessary, \
                         and an unnecessary allowance is how such a list grows until it covers something that \
                         matters"
                    );
                }
            }
        }
    }
}

/// Every framework marker an *exempt* file may not name is still reported.
///
/// A whole-file exemption is a hole the size of the file: with one, a module entitled to name the provider it
/// connects to could parse any framework's telemetry beside it and the sweep would say nothing. So the connector
/// exemptions allow one product's name and nothing else, and this is that property, checked directly - the sweep
/// itself cannot show it, because a passing sweep is consistent with every exemption being unscoped.
#[test]
fn an_exempt_file_may_name_only_what_its_exemption_allows() {
    // The shape Codex's review used: a parser for a *different* framework, written into a connector module.
    let intruder = "fn parse_haystack_telemetry() {}";
    let names = production_names(intruder, "intruder");
    assert!(
        names
            .iter()
            .any(|(_, text)| text.to_ascii_lowercase().contains("haystack")),
        "the sweep's own reader must see the name it is meant to catch"
    );
    // And the name the connectors *are* allowed is not the same name, so allowing one cannot allow the other.
    assert!(
        !"azure-ai-foundry".contains("haystack") && !"haystack".contains("azure-ai-foundry"),
        "the allowance and the intruder must be distinguishable for the scoping to mean anything"
    );
}

/// `include!` and a `#[path]` that leaves this walk's reach are holes in the claim, so both are refused.
#[cfg(test)]
fn assert_no_foreign_source(source: &str, relative: &str) {
    for line in source.lines() {
        let line = line.trim();
        assert!(
            !line.contains("include!("),
            "`{relative}` includes source from elsewhere, which this sweep does not follow"
        );
        if let Some(rest) = line.strip_prefix("#[path = \"")
            && let Some(target) = rest.split('"').next()
        {
            assert!(
                target.ends_with("_tests.rs") && !target.contains('/'),
                "`{relative}` names module source at `{target}`, which is neither a sibling test file nor \
                 something this sweep follows"
            );
        }
    }
}

/// Every `.rs` file under a directory, with its contents.
#[cfg(test)]
fn walk_rust_sources(dir: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
    // Fails closed: a directory or file the sweep cannot read is not a file it may skip. Silently returning
    // would make an unreadable subtree indistinguishable from an empty one, and the whole claim rests on having
    // looked at everything.
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("the sweep could not read `{}`: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| {
                panic!(
                    "the sweep could not read an entry of `{}`: {e}",
                    dir.display()
                )
            })
            .path();
        if path.is_dir() {
            walk_rust_sources(&path, visit);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("the sweep could not read `{}`: {e}", path.display()));
            visit(&path, &source);
        }
    }
}

/// Every name a file's **production** tokens contain, with the line each sits on: comments are gone, and so is
/// every `#[cfg(test)]` item.
///
/// Tokenised rather than read line by line, which removes a *class* of bypass instead of another special case:
///
/// - A brace inside a string literal is a string. `#[cfg(test)] const X: &str = "{";` used to send the stripper
///   hunting for a closing brace, and it consumed every production item after it to the end of the file.
/// - A name written in pieces is the name it builds. Adjacent literals are joined *as tokens*, so
///   `concat!("lang", "graph")` is `langgraph` however it is laid out - including across lines, which matching a
///   line at a time could not do at all.
/// - An item's extent is exact. A braced group is one token, so the first one at an item's own level *is* its
///   body, and neither a multi-line `const` nor a multi-line `fn` signature can be mistaken for the other.
///
/// Fails closed twice over: a file that does not tokenise is not a file this sweep may skip, and a `cfg`
/// predicate that mentions `test` in any shape other than `cfg(test)` is refused rather than guessed at - reading
/// `cfg(not(test))` as a test item would strip production code, and reading `cfg(any(test, ...))` as production
/// would let a test item's names count as the server's.
#[cfg(test)]
fn production_names(source: &str, what: &str) -> Vec<(usize, String)> {
    let stream: proc_macro2::TokenStream = source
        .parse()
        .unwrap_or_else(|e| panic!("the sweep could not tokenise `{what}`: {e}"));
    let mut out = Vec::new();
    collect_production_names(stream, what, &mut out);
    out
}

#[cfg(test)]
fn collect_production_names(
    stream: proc_macro2::TokenStream,
    what: &str,
    out: &mut Vec<(usize, String)>,
) {
    use proc_macro2::{Delimiter, TokenTree};
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut index = 0;
    while index < tokens.len() {
        // Prose, not code. A `///` or `//!` comment becomes a `#[doc]` attribute in the token stream, and the
        // explanations in this tree name the producers whose telemetry motivated each rule - which is what an
        // explanation of a producer's shape has to do. Reading them would force either mass exemptions or
        // deleting the explanations; the limit is stated on the test instead.
        if let Some(width) = doc_attribute_width(&tokens, index) {
            index += width;
            continue;
        }
        if is_cfg_test_attribute(&tokens, index, what) {
            // Everything the attribute applies to: any further attributes, the item's head, and then whichever
            // comes first - the braced body or the `;` of a statement item.
            index += 2;
            while index < tokens.len() {
                let done = match &tokens[index] {
                    TokenTree::Group(group) => group.delimiter() == Delimiter::Brace,
                    TokenTree::Punct(punct) => punct.as_char() == ';',
                    _ => false,
                };
                index += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        match &tokens[index] {
            TokenTree::Ident(ident) => {
                out.push((ident.span().start().line, ident.to_string()));
                index += 1;
            }
            TokenTree::Literal(first) => {
                // A run of literals separated by nothing or by commas is one candidate as well as several: that
                // is what `concat!` builds, and what splitting a name into pieces looks like.
                let line = first.span().start().line;
                let start = index;
                let mut joined = String::new();
                while index < tokens.len() {
                    match &tokens[index] {
                        TokenTree::Literal(literal) => {
                            let text = literal.to_string();
                            joined.push_str(literal_text(&text));
                            out.push((literal.span().start().line, text));
                            index += 1;
                        }
                        TokenTree::Punct(punct)
                            if punct.as_char() == ','
                                && matches!(tokens.get(index + 1), Some(TokenTree::Literal(_))) =>
                        {
                            index += 1;
                        }
                        _ => break,
                    }
                }
                if index > start + 1 {
                    out.push((line, joined));
                }
            }
            TokenTree::Group(group) => {
                collect_production_names(group.stream(), what, out);
                index += 1;
            }
            TokenTree::Punct(_) => index += 1,
        }
    }
}

/// A string literal's own text, without the quoting a `contains` search would have to see through.
#[cfg(test)]
fn literal_text(text: &str) -> &str {
    text.trim_start_matches('r')
        .trim_matches('#')
        .trim_matches('"')
}

/// Whether the token at `index` begins a `#[cfg(test)]` attribute.
#[cfg(test)]
fn is_cfg_test_attribute(tokens: &[proc_macro2::TokenTree], index: usize, what: &str) -> bool {
    use proc_macro2::TokenTree;
    if !is_attribute_named(tokens, index, "cfg") {
        return false;
    }
    let Some(TokenTree::Group(group)) = tokens.get(index + 1) else {
        return false;
    };
    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
    let predicate = match inner.get(1) {
        Some(TokenTree::Group(predicate)) => predicate.stream().to_string(),
        _ => return false,
    };
    // Whitespace-free, because a token stream renders `not(test)` as `not (test)`.
    let predicate: String = predicate.chars().filter(|c| !c.is_whitespace()).collect();
    // The two shapes this tree uses, and they are opposites: `cfg(test)` is a test item, `cfg(not(test))` is
    // production-only code. Anything else mentioning `test` has no safe default - one reading strips production
    // code, the other counts a test item's names as the server's - so it is a decision to be made here rather
    // than a guess made silently.
    match predicate.as_str() {
        "test" => true,
        "not(test)" => false,
        other => {
            assert!(
                !mentions_test(other),
                "`{what}` carries `#[cfg({other})]`, and this sweep only knows `cfg(test)` and \
                 `cfg(not(test))` - decide whether that item is production and teach \
                 `is_cfg_test_attribute` the answer"
            );
            false
        }
    }
}

/// How many tokens a doc attribute at `index` occupies, or `None` where there is not one.
///
/// Two shapes, and missing the second cost a round: `///` is an outer attribute (`#`, `[...]`) and `//!` is an
/// inner one (`#`, `!`, `[...]`), so a module's own explanation is three tokens rather than two.
#[cfg(test)]
fn doc_attribute_width(tokens: &[proc_macro2::TokenTree], index: usize) -> Option<usize> {
    use proc_macro2::TokenTree;
    if is_attribute_named(tokens, index, "doc") {
        return Some(2);
    }
    let Some(TokenTree::Punct(hash)) = tokens.get(index) else {
        return None;
    };
    let Some(TokenTree::Punct(bang)) = tokens.get(index + 1) else {
        return None;
    };
    if hash.as_char() != '#' || bang.as_char() != '!' {
        return None;
    }
    bracket_group_named(tokens.get(index + 2), "doc").then_some(3)
}

/// Whether the token at `index` begins an outer attribute with the given name - `#[name...]`.
#[cfg(test)]
fn is_attribute_named(tokens: &[proc_macro2::TokenTree], index: usize, name: &str) -> bool {
    use proc_macro2::TokenTree;
    let Some(TokenTree::Punct(hash)) = tokens.get(index) else {
        return false;
    };
    if hash.as_char() != '#' {
        return false;
    }
    bracket_group_named(tokens.get(index + 1), name)
}

/// Whether a token is a `[...]` group whose first token is the given identifier.
#[cfg(test)]
fn bracket_group_named(token: Option<&proc_macro2::TokenTree>, name: &str) -> bool {
    use proc_macro2::{Delimiter, TokenTree};
    let Some(TokenTree::Group(group)) = token else {
        return false;
    };
    group.delimiter() == Delimiter::Bracket
        && matches!(group.stream().into_iter().next(), Some(TokenTree::Ident(ident)) if ident == name)
}

/// Whether `text` names `marker` as a word rather than as a fragment of one.
///
/// `contains` was wrong in a way no amount of care repairs: `agno` is a framework and also the middle of
/// `diagnostic` and of `backend-agnostic`. A boundary is the start or end of the text, a character that is not
/// alphanumeric - so `_`, `-`, `.` and a quote all separate - or a change of case, which is what makes
/// `VercelFormat` a naming and `agnostic` not.
#[cfg(test)]
fn names_as_a_word(text: &str, marker: &str) -> bool {
    let folded = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(offset) = folded[from..].find(marker) {
        let at = from + offset;
        let end = at + marker.len();
        let left = match at.checked_sub(1).map(|i| bytes[i]) {
            None => true,
            Some(before) => {
                !before.is_ascii_alphanumeric()
                    || (!before.is_ascii_uppercase() && bytes[at].is_ascii_uppercase())
            }
        };
        let right = match bytes.get(end).copied() {
            None => true,
            Some(after) => {
                !after.is_ascii_alphanumeric()
                    || (after.is_ascii_uppercase() && !bytes[end - 1].is_ascii_uppercase())
            }
        };
        if left && right {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Whether a `cfg` predicate mentions the `test` configuration, as a whole word.
#[cfg(test)]
fn mentions_test(predicate: &str) -> bool {
    predicate
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word == "test")
}

/// The sweep's reader keeps production code, drops test items, and sees a name written in pieces.
///
/// It is load-bearing - the whole-server claim rests on it - and every one of these was a real bypass. Three came
/// from reading the source a line at a time and are gone by construction now that it is tokenised: a multi-line
/// `const` read as braced (consuming the item after it, so a production function was never seen); a multi-line
/// `fn` signature read as a statement (resuming inside its own body, so everything in it counted as production);
/// and a brace inside a **string literal** sending the search for a closing brace off to the end of the file. The
/// fourth is the reverse - a name assembled from adjacent literals, which no line match could join across lines.
#[test]
fn the_test_stripper_keeps_production_code_and_drops_test_items() {
    let source = r##"
#[cfg(test)]
const WRAPPED: &str =
    "x";

fn production_after_a_multiline_const() {
    let marker = "first";
}

#[cfg(test)]
pub(super) fn oracle_with_a_wrapped_signature(
    attrs: &Map,
) -> Reading {
    let inner = "second";
}

fn production_after_a_wrapped_signature() {
    let marker = "third";
}

#[cfg(test)]
mod tests {
    fn inside() {
        let marker = "fourth";
    }
}

fn production_after_a_module() {
    let marker = "fifth";
}

#[cfg(test)]
const BRACE_IN_A_STRING: &str = "{";

fn production_after_a_brace_in_a_string() {
    let marker = "sixth";
}

#[cfg(not(test))]
fn production_only() {
    let marker = "seventh";
}

fn a_name_in_pieces() -> String {
    concat!(
        "lang",
        "graph"
    )
    .to_string()
}
"##;

    let names = production_names(source, "the stripper's own fixture");
    let kept: Vec<String> = names.iter().map(|(_, text)| text.clone()).collect();
    let joined = kept.join("\n");

    for expected in [
        "production_after_a_multiline_const",
        "\"first\"",
        "production_after_a_wrapped_signature",
        "\"third\"",
        "production_after_a_module",
        "\"fifth\"",
        // A brace inside a string is a string, so the item after it is still read.
        "production_after_a_brace_in_a_string",
        "\"sixth\"",
        // `cfg(not(test))` is production, and reading it as a test item would strip it.
        "production_only",
        "\"seventh\"",
        // Adjacent literals are joined as tokens, so the name is the name however it is laid out.
        "langgraph",
    ] {
        assert!(
            kept.iter().any(|name| name == expected),
            "production name dropped: `{expected}`\nkept:\n{joined}"
        );
    }
    for dropped in [
        "WRAPPED",
        "\"x\"",
        "oracle_with_a_wrapped_signature",
        "\"second\"",
        "\"fourth\"",
        "BRACE_IN_A_STRING",
    ] {
        assert!(
            !kept.iter().any(|name| name == dropped),
            "test name kept: `{dropped}`\nkept:\n{joined}"
        );
    }
}

/// A marker is a **word**, not a fragment, and both directions cost something.
///
/// `contains` was the first spelling and it cannot work: `agno` is a framework and also the middle of
/// `diagnostic`, so the sweep reported the s3 error formatter and the backend-agnostic data layer. Requiring a
/// non-alphanumeric boundary on both sides fixes that and loses `VercelFormat`, where the boundary is a change of
/// case - which is why a case change counts as one. Load-bearing in the quiet direction: a matcher that is too
/// strict makes the whole sweep pass while naming nothing.
#[test]
fn a_marker_matches_a_word_and_not_a_fragment() {
    for (text, marker) in [
        ("agno", "agno"),
        ("agno_client", "agno"),
        ("try_vercel_format", "vercel"),
        ("VercelFormat", "vercel"),
        ("read_llama_index_state", "llama_index"),
        ("\"strands\"", "strands"),
        ("LlamaIndexReader", "llamaindex"),
        ("googleAdkResult", "adk"),
    ] {
        assert!(
            names_as_a_word(text, marker),
            "`{text}` names `{marker}` and the sweep would not see it"
        );
    }
    for (text, marker) in [
        ("diagnostic", "agno"),
        ("backend-agnostic", "agno"),
        ("is_agnostic", "agno"),
        ("stranded", "strands"),
        ("readka", "adk"),
    ] {
        assert!(
            !names_as_a_word(text, marker),
            "`{text}` does not name `{marker}`, and reporting it would be a false accusation"
        );
    }
}

/// A `cfg` predicate this sweep does not know is refused, not guessed at.
///
/// There is no safe default: reading `cfg(not(test))` as a test item strips production code, and reading
/// `cfg(any(test, feature = "x"))` as production counts a test item's names as the server's. So an unfamiliar
/// shape mentioning `test` stops the sweep and asks for a decision.
#[test]
#[should_panic(expected = "this sweep only knows `cfg(test)`")]
fn an_unfamiliar_cfg_mentioning_test_is_refused() {
    production_names(
        "#[cfg(any(test, feature = \"x\"))]\nfn ambiguous() {}",
        "an unfamiliar cfg",
    );
}

/// The `repr` primitive accepts exactly the language its documentation specifies.
///
/// The design's fifth test required the accepted language to be *specified* rather than described as
/// "Python repr", which names something far larger. A specification nobody checks is a comment, so each
/// line of that table is a case here - and the refusals matter more than the acceptances: a refusal leaves
/// the string to be read as text, while a wrong parse invents a tool definition.
#[test]
fn the_repr_primitive_accepts_the_language_it_specifies() {
    use crate::domain::rules::tool_repr::python_literal_to_json_for_test as parse;
    for accepted in [
        "{'k': True}",
        "{\"k\": False}",
        "{'k': None}",
        "[{'a': 1}, {'b': 2}]",
        "{'k': 'Nonetheless'}",
        "{'nested': {'deep': [1, 2, 3]}}",
    ] {
        assert!(
            parse(accepted).is_some(),
            "`{accepted}` is in the specified language and was refused"
        );
    }
    // A whole-word check, not a substring one: a value that merely starts with `None` is a string.
    assert_eq!(
        parse("{'k': 'Nonetheless'}").and_then(|v| v["k"].as_str().map(str::to_owned)),
        Some("Nonetheless".to_string()),
        "a string containing a literal's name must survive as that string"
    );
    for refused in [
        // Not object- or array-shaped: refused before anything else, so prose cannot become a definition.
        "True",
        "search",
        "Tool Arguments: none",
        "",
        // Python literals outside the accepted subset. Each must refuse rather than parse to something.
        "(1, 2)",
        "{'k': (1, 2)}",
        "{'k': b'bytes'}",
        "{'k': 1_000}",
        "{'k': 0x1f}",
        "{'k': inf}",
        "{'k': 'a' 'b'}",
        "{'k': 1,}",
        "{1, 2}",
        "{'k': f'x'}",
    ] {
        assert!(
            parse(refused).is_none(),
            "`{refused}` is outside the specified language and was accepted - a wrong parse invents a \
             tool definition, where a refusal only fails to find one"
        );
    }
}

/// Every refusal `compile_event_roles` makes, as a test rather than as a mutation I ran once.
///
/// Mutation verification shows a check works today; it does not stop the check being removed tomorrow. Each
/// of these is a declaration that could not take effect, or one whose effect would depend on load order, and
/// both read as a statement that holds.
#[test]
fn an_event_role_declaration_must_be_able_to_answer_and_must_not_depend_on_load_order() {
    use super::schema::RuleFile;

    // A probe asset carrying an event, a tag, and whatever event roles the case declares.
    let compiled = |roles: serde_json::Value| {
        let probe = serde_json::json!({
            "id": "probe",
            "message_events": [{"name": "probe.event"}],
            "messages": [{
                "id": "probe.tagging_rule",
                "read": {"attribute": "probe.attribute"},
                "emit": "message",
                // Nested in a **branch leaf**, deliberately: that is where one asset's tag already sits, and
                // reading only the top level left it out of both indexes at once.
                "branch_set": {
                    "primary": [{
                        "id": "probe.branch_leaf",
                        "read": {"attribute": "probe.other"},
                        "emit": "message",
                        "tag_as": "probe.tag",
                    }],
                },
            }],
            "event_roles": roles,
        });
        let file: RuleFile = serde_json::from_value(probe.clone()).expect("the probe asset parses");
        // The tags the probe's own rules assign, gathered the way the ruleset gathers them - so a tag nested
        // in a branch leaf is reachable here too, which is the defect the recursive walk fixed.
        let tags = super::tag_names(std::slice::from_ref(&file));
        super::compile_event_roles(&[file], &tags)
    };

    for (what, roles) in [
        (
            "a role outside the vocabulary, which would silently leave the role to the content",
            serde_json::json!([{"name": "probe.event", "role": "narrator"}]),
        ),
        (
            "a tool-span role outside it, which the ordinary role would mask on a chat span",
            serde_json::json!([{"name": "probe.event", "role": "user", "role_in_tool_span": "narrator"}]),
        ),
        (
            "no role at all, which states nothing and would replace a real declaration",
            serde_json::json!([{"name": "probe.event"}]),
        ),
        ("no name", serde_json::json!([{"name": "", "role": "user"}])),
        (
            "a name nothing produces - neither an event nor any rule's tag",
            serde_json::json!([{"name": "probe.absent", "role": "user"}]),
        ),
        (
            "two declarations that disagree, where which applies depends on load order",
            serde_json::json!([
                {"name": "probe.event", "role": "user"},
                {"name": "probe.event", "role": "assistant"},
            ]),
        ),
        (
            "two that disagree only about the tool span, which is the half easiest to overlook",
            serde_json::json!([
                {"name": "probe.event", "role": "user", "role_in_tool_span": "tool"},
                {"name": "probe.event", "role": "user"},
            ]),
        ),
    ] {
        assert!(compiled(roles).is_err(), "{what} was accepted");
    }

    // And the two shapes that must be accepted, or the refusals are simply a ban.
    for (what, roles) in [
        (
            "a repeat that agrees, which is a dialect re-stating a convention",
            serde_json::json!([
                {"name": "probe.event", "role": "user"},
                {"name": "probe.event", "role": "user"},
            ]),
        ),
        (
            "a role for a name a rule assigns with `tag_as`, which no producer emits",
            serde_json::json!([{"name": "probe.tag", "role": "tool"}]),
        ),
    ] {
        assert!(compiled(roles).is_ok(), "{what} was refused");
    }
}

/// **No production module carries a framework's telemetry key as a literal**, which is the blind spot the
/// name sweep documents and cannot close.
///
/// The name sweep reads framework *names*. It passed while `role_from_event_name_with_context` held an arm
/// that existed only because one CLI writes its tool result on `tool.output` - a framework fact spelled as a
/// value, naming nobody. That arm invalidated the acceptance it was given under, so the class is worth a gate
/// of its own rather than a sentence saying it is not covered.
///
/// A key counts as a framework's when it appears in **exactly one** framework asset and in no shared or
/// provider asset, so the conventions' own vocabulary is not implicated. Only dotted names, and only as a
/// whole string literal, because a bare word is not evidence of anything.
///
/// What it still cannot see, stated for the same reason the other sweep states its limits: a key no asset
/// declares (nothing identifies it as a producer's), one two frameworks share, a value that is not a dotted
/// key - a role string, a magic number - and a key assembled at runtime.
#[test]
fn no_production_module_carries_a_framework_telemetry_key() {
    /// Files whose subject is the key itself, and why.
    const EXEMPT: &[(&str, &str)] = &[(
        "src/api/mcp/tools.rs",
        "generates integration documentation, which has to show the attribute names a framework writes",
    )];

    let inventory = producer_key_inventory();
    assert!(
        inventory.len() > 30,
        "only {} producer keys were derived, which cannot be right",
        inventory.len()
    );
    let exclusive: Vec<(&String, &String)> = inventory.iter().collect();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();
    let mut exempt_used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    walk_rust_sources(&root, &mut |path, source| {
        let relative = path
            .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains("_tests.rs") || relative.ends_with("/tests.rs") {
            return;
        }
        if let Some((name, _)) = EXEMPT.iter().find(|(file, _)| relative.ends_with(file)) {
            exempt_used.insert(name);
            return;
        }
        for (number, text) in production_names(source, &relative) {
            // A whole string literal, which is what a key is written as. `production_names` yields a literal
            // with its quotes, so this is an exact comparison rather than a search.
            let literal = text.trim_matches('"');
            if literal.len() == text.len() {
                continue;
            }
            if let Some((key, asset)) = exclusive.iter().find(|(key, _)| *key == literal) {
                offenders.push(format!("  {relative}:{number}: \"{key}\" is {asset}'s"));
            }
        }
    });
    for (file, _) in EXEMPT {
        assert!(
            exempt_used.contains(file),
            "EXEMPT names `{file}`, which the sweep did not find"
        );
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "{} production line(s) carry a framework's telemetry key. A key a framework writes belongs in its \
         asset; if a module genuinely has to name one, add it to EXEMPT with the reason:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// Every value an asset states about a producer's telemetry: a key, a prefix, an event name, or a value it
/// matches on.
///
/// **Everything dotted except a closed list of the engine's own vocabulary**, which is the opposite of the
/// first version's allowlist of key-bearing members. That allowlist was a hand-maintained projection of the
/// schema and it was already incomplete - `attribute_any_of`, `from_any_of`, `attrs_present`, `attr:`-encoded
/// text sources and a prefix ending in `.` all held keys it never read - so a key declared only through one of
/// those could be hard-coded in Rust and this sweep would say nothing.
///
/// Inverted, the maintenance burden fails **loudly**: a new key-bearing member is covered the day it exists,
/// and a new *engine* member holding a dotted token that nobody excludes makes the sweep demand it be
/// accounted for, which is a visible failure rather than a silent hole.
#[cfg(test)]
fn collect_telemetry_keys(
    value: &serde_json::Value,
    under: Option<&str>,
    out: &mut std::collections::BTreeSet<String>,
) {
    /// Members holding the **engine's** own dotted vocabulary rather than a producer's: a rule id, prose, a
    /// fragment's name, an ordering class's name. Everything else dotted is taken as a producer's.
    const OURS: &[&str] = &[
        "id",
        "doc",
        "then_fragment",
        "ordering_family",
        "supersedes",
    ];
    match value {
        serde_json::Value::String(text)
            if text.contains('.') && !under.is_some_and(|m| OURS.contains(&m)) =>
        {
            // A JSONPath is a path into a *payload*, not an attribute key, and its `$` root is nobody's
            // namespace. Left in, it made `$` look like a namespace several dialects write under.
            if text.starts_with('$') {
                return;
            }
            // A text source names its attribute through an **encoded selector**, `attr:<key>`. Stored as
            // written, the inventory held `attr:logfire.tags` and a production literal `"logfire.tags"` went
            // unnoticed - the sweep compares whole literals, so an inventory entry that is not the key is not
            // an entry at all. Canonicalised here rather than at the comparison, so every reader of the
            // inventory sees the key.
            out.insert(text.strip_prefix("attr:").unwrap_or(text).to_string());
        }
        serde_json::Value::Object(members) => {
            for (member, inner) in members {
                collect_telemetry_keys(inner, Some(member), out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_telemetry_keys(item, under, out);
            }
        }
        _ => {}
    }
}

/// `scalar_only` is refused everywhere it could not apply.
///
/// It says each match of **one path** is a single string. On a witness it means nothing (a witness only asks
/// whether a member is there); beside a reduction it means nothing (a reduction is already per match); on a
/// first-present group it means nothing (that selects a *member* rather than matching many, and the code path
/// returns before the flag is read); and on a field that does not hold a list there is nowhere to lift the
/// string to. Each of those was accepted and did nothing, which reads as protection that is not there - and
/// the first-present case was the one where a caller could reasonably expect an array to be rejected and it
/// was not.
#[test]
fn a_scalar_only_that_cannot_apply_is_refused() {
    let compiled = |target: &str, json: serde_json::Value| {
        let asset = serde_json::json!({
            "id": "probe",
            "span_fields": [{
                "id": "probe.field",
                "target": target,
                "sources": [{"id": "probe.source", "json": json}],
            }],
        });
        super::span_fields::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };
    for (what, target, json) in [
        (
            "a first-present group, which selects a member rather than matching many",
            "gen_ai_finish_reasons",
            serde_json::json!({
                "attribute": "probe.payload",
                "first_present_of": ["$.reason"],
                "scalar_only": true,
            }),
        ),
        (
            "beside a reduction, which is already per match",
            "gen_ai_finish_reasons",
            serde_json::json!({
                "attribute": "probe.payload",
                "path": "$.choices[0:].finish_reason",
                "reduce": "collect_all",
                "scalar_only": true,
            }),
        ),
        (
            "on a field that holds no list",
            "gen_ai_system",
            serde_json::json!({
                "attribute": "probe.payload",
                "path": "$.reason",
                "scalar_only": true,
            }),
        ),
    ] {
        assert!(
            compiled(target, json).is_err(),
            "`scalar_only` on {what} was accepted, and it does nothing there"
        );
    }

    // A **witness**, valid in every other respect - one path, no reduction, a list-valued field - so only
    // `is_witness` can refuse it. Without a probe this narrow, removing that condition left the suite green.
    let witness = {
        let asset = serde_json::json!({
            "id": "probe",
            "span_fields": [{
                "id": "probe.field",
                "target": "gen_ai_finish_reasons",
                "sources": [{
                    "id": "probe.source",
                    "attribute": "probe.flat",
                    "when_json": {
                        "attribute": "probe.payload",
                        "path": "$.reason",
                        "scalar_only": true,
                    },
                }],
            }],
        });
        super::span_fields::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };
    assert!(
        witness.is_err(),
        "`scalar_only` on a witness was accepted - a witness only asks whether a member is there, so it says \
         nothing about the shape of what it holds"
    );
    // Its whole domain: an unreduced read through one path into a list-valued field.
    assert!(
        compiled(
            "gen_ai_finish_reasons",
            serde_json::json!({
                "attribute": "probe.payload",
                "path": "$.reason",
                "scalar_only": true,
            })
        )
        .is_ok(),
        "an unreduced path into a list-valued field is where `scalar_only` applies"
    );
}

/// A `lowercase` on a field that holds no text is refused, not silently ignored.
///
/// The flag says a producer's casing is not information. On a count there is no casing, so the declaration
/// could not take effect - and a declaration that cannot take effect reads as one that does, which is the same
/// objection this file makes to a dead event role and to an unused reduction.
#[test]
fn a_fold_that_can_do_nothing_is_refused() {
    let compiled = |target: &str, lowercase: bool| {
        let asset = serde_json::json!({
            "id": "probe",
            "span_fields": [{
                "id": "probe.field",
                "target": target,
                "sources": [{"id": "probe.source", "attribute": "probe.attribute", "lowercase": lowercase}],
            }],
        });
        super::span_fields::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };
    assert!(
        compiled("usage_input_tokens", true).is_err(),
        "folding a count was accepted, and it can do nothing there"
    );
    assert!(
        compiled("gen_ai_temperature", true).is_err(),
        "folding a number was accepted, and it can do nothing there"
    );
    // The two shapes it does apply to, or the refusal is simply a ban.
    for target in ["gen_ai_system", "gen_ai_finish_reasons"] {
        assert!(
            compiled(target, true).is_ok(),
            "folding `{target}` was refused, and its values are text"
        );
        assert!(compiled(target, false).is_ok(), "`{target}` must compile");
    }
}

/// The key sweep's derivation tells a producer's key from a convention, in both directions.
///
/// Load-bearing in the quiet direction: a derivation that stops recognising producer keys makes the sweep pass
/// while naming nothing, which is exactly how its first version passed while three production sites read one
/// dialect's attribute. Each case here is one the derivation got wrong at some point:
///
/// - reachable only through a member the hand-written allowlist missed (`from_any_of`, `attrs_present`);
/// - declared only in a **shared chain**, whose enumeration of producers' spellings was first read as
///   evidence the key is generic;
/// - a span-name prefix rather than an attribute;
/// - an OTel convention outside the `gen_ai.` namespace, which a namespace list written by hand omitted.
#[test]
fn the_key_sweep_tells_a_producer_key_from_a_convention() {
    let inventory = producer_key_inventory();
    for (key, owner) in [
        ("ai.result.object", "vercel-ai"),
        ("ai.toolCall.id", "vercel-ai"),
        ("ai.usage.promptTokens", "span-fields-usage"),
        ("llm.usage.prompt_tokens", "span-fields-usage"),
        ("lk.chat_ctx", "livekit"),
        ("gcp.vertex.agent.data", "google-adk"),
        ("LangGraph.", "langgraph"),
        // Named only through an **encoded selector** (`attr:logfire.tags`). Stored as written, the inventory
        // held a string no Rust literal can equal, so the key it names went unnoticed.
        ("logfire.tags", "observation-types"),
        // A producer's key under a namespace **no dialect file mentions**, declared only in a shared chain.
        // Reading "no framework declares under it" as evidence of convention ownership excused exactly this.
        ("tag.tags", "span-fields-semantic"),
        ("agent.name", "span-fields-genai"),
    ] {
        assert_eq!(
            inventory.get(key).map(String::as_str),
            Some(owner),
            "`{key}` is a producer's and the sweep would not recognise it"
        );
    }
    for conventional in [
        "session.id",
        "enduser.id",
        "user.id",
        "http.method",
        "db.system",
        "gen_ai.tool.name",
        // The engine's own namespace, which is not a convention either but is certainly not a producer's.
        "sideseat.project_id",
        // A JSONPath is a path into a payload, not an attribute key.
        "$.usage_metadata.prompt_token_count",
    ] {
        assert!(
            !inventory.contains_key(conventional),
            "`{conventional}` is the conventions' and reporting it would be a false accusation"
        );
    }
}

/// Every telemetry key the assets state about a **producer**, with the asset that declares it.
///
/// Its own function because two tests read it: the production sweep, and the one that holds the derivation to
/// account. A derivation that quietly stopped recognising producer keys would make the sweep pass while naming
/// nothing, which is how its first version passed while three production sites read one dialect's attribute.
#[cfg(test)]
fn producer_key_inventory() -> std::collections::BTreeMap<String, String> {
    let sources = crate::domain::rules::schema::embedded_sources();
    let mut per_asset: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for (path, bytes) in &sources {
        let id = path.trim_end_matches(".json").to_string();
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("the asset parses");
        let mut keys = std::collections::BTreeSet::new();
        collect_telemetry_keys(&value, None, &mut keys);
        per_asset.insert(id, keys);
    }
    // A key the **conventions** name is not one framework's, however many frameworks also write it. Only
    // `semconv` and `generic-io` count for that, and the distinction is the whole difficulty: the other
    // shared assets are ordered *fallback chains*, and a chain enumerates producers' spellings by design -
    // `span-fields-usage` lists one dialect's `gcp.vertex.agent.llm_response` beside the conventional
    // counter. Treating that as evidence the key is generic is what made the first version of this sweep
    // pass while three production sites read exactly that attribute.
    const CONVENTIONS: &[&str] = &["semconv", "generic-io"];
    let shared: std::collections::BTreeSet<&String> = per_asset
        .iter()
        .filter(|(id, _)| CONVENTIONS.contains(&id.as_str()) || PROVIDERS.contains(&id.as_str()))
        .flat_map(|(_, keys)| keys)
        .collect();
    let mut owners: std::collections::BTreeMap<&String, Vec<&String>> =
        std::collections::BTreeMap::new();
    // A neutral asset **contributes** keys while conferring no sharedness. Its chains enumerate producers'
    // spellings, so `ai.usage.promptTokens` sitting in a shared usage chain is still one producer's key and
    // hard-coding it in Rust is the same defect - while its presence there is no evidence that it is generic.
    // Skipping such assets entirely was the second half of the same mistake as treating them as conventions.
    for (id, keys) in &per_asset {
        if CONVENTIONS.contains(&id.as_str()) || PROVIDERS.contains(&id.as_str()) {
            continue;
        }
        for key in keys {
            owners.entry(key).or_default().push(id);
        }
    }
    // Which **namespaces** are the conventions', and this **fails closed**: a namespace is theirs only when
    // the conventions themselves declare something under it. The first version read "no framework asset
    // declares under it" as evidence too, which is not evidence of anything - a producer key declared only in
    // a shared chain, under a namespace no dialect file mentions, was classified conventional and could be
    // hard-coded in Rust unnoticed. Absence of a framework declaration says nothing about who owns a name.
    //
    // The conventions asset carries the policy data; the exact expected set below intentionally mirrors it.
    // That duplication is the review gate, not an independent derivation - which is what lets OTel's general
    // attributes (`session.id`, `enduser.id`, `http.method`) be recognised while a new entry stays a change
    // somebody has to look at.
    let namespace = |key: &str| key.split_once('.').map(|(head, _)| head.to_string());
    let declared_namespaces: std::collections::BTreeSet<String> = {
        // Only the conventions' asset may say which namespaces are the conventions'. Anywhere else the
        // declaration was silently ignored, which is worse than refusing it: a dialect could state that its
        // own namespace is a convention and read as having done so.
        for (path, bytes) in &sources {
            let file: crate::domain::rules::schema::RuleFile =
                serde_json::from_slice(bytes).expect("the asset parses");
            assert!(
                file.convention_namespaces.is_empty() || path == "semconv.json",
                "`{path}` declares `convention_namespaces`, which only the conventions' asset may do - \
                 elsewhere it is ignored, so a dialect could claim its own namespace is a convention and \
                 read as having done so"
            );
        }
        let conventions: crate::domain::rules::schema::RuleFile =
            serde_json::from_slice(&sources["semconv.json"]).expect("the conventions asset parses");
        conventions.convention_namespaces.iter().cloned().collect()
    };
    let from_conventions: std::collections::BTreeSet<String> = per_asset
        .iter()
        .filter(|(id, _)| CONVENTIONS.contains(&id.as_str()))
        .flat_map(|(_, keys)| keys.iter().filter_map(|key| namespace(key)))
        .collect();
    // Convention namespaces are an explicit **policy set** in `semconv.json`, mirrored by an exact test
    // expectation. No property independently proves their ownership; changes therefore require explicit
    // review.
    //
    // Two properties were tried and both accepted producer evidence, which is why there is none. "No framework
    // asset declares under it" is not evidence of anything. "Some shared asset writes under it" is worse,
    // because the shared assets are fallback chains that enumerate producers' spellings by design: add
    // `acme.trace.id` to a chain, declare `acme`, and the property passes while the inventory suppresses the
    // key. And `semconv` itself writes under neither `session.` nor `http.` - the general OTel attributes
    // reach this engine only through those chains - so requiring its evidence alone would reject the very
    // namespaces the set exists to recognise.
    let expected_namespaces: std::collections::BTreeSet<String> = [
        "aws",
        "cloud",
        "db",
        "enduser",
        "gen_ai",
        "http",
        "messaging",
        "rpc",
        "session",
        "url",
        "user",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(
        declared_namespaces, expected_namespaces,
        "`convention_namespaces` in `semconv` has changed. Each entry suppresses every key under it, so a new \
         one is how a producer's namespace gets excused - state the new namespace here and say why it is the \
         conventions' rather than a producer's"
    );
    let _ = &from_conventions;
    let conventional = |key: &str| {
        // `sideseat.` is **this server's** own namespace rather than a convention: it is written by the
        // ingestion path, so it is excluded here rather than added to the conventions' policy set, which is
        // about OTel's namespaces and not about ours.
        key.starts_with("sideseat.")
            || namespace(key).is_some_and(|head| {
                from_conventions.contains(&head) || declared_namespaces.contains(&head)
            })
    };
    // Pinned by `the_key_sweep_tells_a_producer_key_from_a_convention`, because a derivation that quietly
    // stopped recognising producer keys would make this whole sweep pass while naming nothing.
    let exclusive: Vec<(&String, &String)> = owners
        .iter()
        .filter(|(key, _)| {
            // **Every** non-convention key, not only one a single asset declares. Two frameworks sharing a
            // spelling makes it shared *dialect* knowledge, which is still not the engine's - the earlier
            // single-owner rule excused exactly the keys several producers agree on.
            !shared.contains(**key) && !conventional(key) && !key.starts_with("sideseat.")
        })
        .map(|(key, assets)| (*key, assets[0]))
        .collect();
    assert!(
        exclusive.len() > 30,
        "only {} framework-exclusive keys were derived, which cannot be right",
        exclusive.len()
    );

    exclusive
        .into_iter()
        .map(|(key, asset)| (key.clone(), asset.clone()))
        .collect()
}

/// A branch set's `fallback_if_primary_empty` asks whether the primaries produced anything of **its** kind.
///
/// Judged over every emission, a branch set whose primaries answer two different questions about one carrier
/// suppresses its own fallback. One dialect reads its serialised request as the conversation *and* reads the
/// tools it was offered out of the same attribute: a request carrying tools and no messages made the branch
/// non-empty, the fallback did not run, and the message path then filtered the tool definition out - so the
/// span reported **no message at all** and the tool call's arguments were lost.
///
/// Both directions are asserted, because the fix must not turn the fallback into an unconditional reading:
/// where the primaries *do* produce a message, the fallback must still stand down.
#[test]
fn a_branch_fallback_asks_about_its_own_kind_of_emission() {
    use crate::domain::rules::schema::EmitTarget;
    use std::collections::HashMap;

    let emitted = |attrs: &HashMap<String, String>| -> Vec<(String, EmitTarget)> {
        let plan = &crate::domain::rules::ruleset().messages;
        let ctx =
            crate::domain::rules::message_rules::MessageContext::for_span("call_llm", attrs, false);
        plan.run(&ctx)
            .into_iter()
            .map(|e| (e.rule_id.to_string(), e.target))
            .collect()
    };

    // Tools but no messages in the request, and a tool call's arguments beside it.
    let mut tools_only = HashMap::new();
    tools_only.insert(
        "gcp.vertex.agent.llm_request".to_string(),
        r#"{"tools":[{"functionDeclarations":[{"name":"search"}]}]}"#.to_string(),
    );
    tools_only.insert(
        "gcp.vertex.agent.tool_call_args".to_string(),
        r#"{"query":"weather"}"#.to_string(),
    );
    let answers = emitted(&tools_only);
    assert!(
        answers
            .iter()
            .any(|(id, target)| id == "google-adk.tool_call_args" && *target == EmitTarget::Message),
        "the primaries produced no *message*, so the fallback must read the tool call's arguments - \
         instead this span reported: {answers:?}"
    );

    // A request that does carry the conversation: the fallback must stand down, or a span would report its
    // arguments beside the turn that already contains them.
    let mut with_messages = HashMap::new();
    with_messages.insert(
        "gcp.vertex.agent.llm_request".to_string(),
        r#"{"contents":[{"role":"user","parts":[{"text":"what is the weather"}]}]}"#.to_string(),
    );
    with_messages.insert(
        "gcp.vertex.agent.tool_call_args".to_string(),
        r#"{"query":"weather"}"#.to_string(),
    );
    let answers = emitted(&with_messages);
    assert!(
        answers.iter().any(|(id, _)| id == "google-adk.llm_request"),
        "the request carries the conversation and must be read: {answers:?}"
    );
    assert!(
        !answers
            .iter()
            .any(|(id, _)| id == "google-adk.tool_call_args"),
        "a primary produced a message, so the fallback must stand down: {answers:?}"
    );
}

/// Every code name the architecture diagrams use resolves in this tree.
///
/// A diagram is a claim about the code, and a stale one is worse than none: a reader trusts it precisely
/// because they are not reading the code. So a rename breaks the build rather than the picture.
///
/// Which tokens count is decided **structurally**, not by a list of prose words to skip: a token containing
/// `::`, or an underscore between word characters, or an internal capital, is a code name; English words in a
/// label contain none of those. That direction is deliberate - a code name the discriminator fails to
/// recognise is missed silently, while a prose word it wrongly recognises **fails the test**, which is the
/// mistake that gets noticed.
#[test]
fn the_diagrams_name_things_that_exist() {
    let diagrams =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/architecture-diagrams.md");
    let text = std::fs::read_to_string(&diagrams)
        .unwrap_or_else(|e| panic!("the diagrams must be readable: {e}"));

    // The whole source tree, once, as the haystack every name is looked for in.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = String::new();
    walk_rust_sources(&root, &mut |_, source| sources.push_str(source));
    // Plus the asset section names, which are `RuleFile` members and appear in the assets themselves.
    for bytes in crate::domain::rules::schema::embedded_sources().values() {
        sources.push_str(&String::from_utf8_lossy(bytes));
    }

    let is_code_name = |token: &str| {
        let bytes = token.as_bytes();
        token.contains("::")
            || bytes.windows(3).any(|w| {
                w[1] == b'_' && w[0].is_ascii_alphanumeric() && w[2].is_ascii_alphanumeric()
            })
            || bytes
                .windows(2)
                .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
    };

    let mut checked = 0_usize;
    let mut missing: Vec<String> = Vec::new();
    // Only inside the diagrams: the prose around them is prose, and holding it to this would be the same
    // mistake as reading a doc comment for framework names.
    for block in text.split("```mermaid").skip(1) {
        let block = block.split("```").next().unwrap_or_default();
        for token in block.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')) {
            let token = token.trim_matches(':');
            if token.len() < 4 || !is_code_name(token) {
                continue;
            }
            // A path is checked by its last segment: the module structure is what the diagram groups by, and
            // an item's own name is what a rename changes.
            let needle = token.rsplit("::").next().unwrap_or(token);
            if needle.len() < 4 {
                continue;
            }
            checked += 1;
            if !sources.contains(needle) {
                missing.push(token.to_string());
            }
        }
    }

    assert!(
        checked > 60,
        "only {checked} code names were checked in the diagrams, which cannot be right - the discriminator \
         is probably no longer recognising them"
    );
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "the architecture diagrams name {} thing(s) this tree does not contain. A diagram is a claim about \
         the code, and a reader trusts it because they are not reading the code:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Predicates that hold when they should not, and declarations that could never hold.
///
/// Each of these compiled or held before the format review, and each is the same failure at predicate level:
/// a condition that reads as narrow and matches everything, or reads as a requirement and asks for nothing.
#[test]
fn a_predicate_that_could_never_mean_what_it_says_is_refused() {
    use crate::domain::rules::schema::RuleFile;

    // `KeyValue` was the one predicate type without strict decoding, so a flag on it was **discarded** and
    // the comparison an author asked to be case-insensitive stayed case-sensitive.
    let with_stray_member = serde_json::from_value::<RuleFile>(serde_json::json!({
        "id": "probe",
        "detect": [{
            "id": "probe.detect",
            "label": "probe",
            "legacy_rank": 1,
            "match": {"attr_equals": [{"key": "span.kind", "value": "TOOL", "ignore_case": true}]},
        }],
    }));
    assert!(
        with_stray_member.is_err(),
        "a stray member on a key/value predicate must be refused, or the flag is silently discarded"
    );

    // An empty substring: every present value contains it, so the rule matches every span carrying the key.
    // Refused for both detection and a gate, through one validator - they had drifted apart in both
    // directions, so this asserts the *same* answer from both callers.
    let contains_nothing = serde_json::json!({
        "span_attr_contains": [{"key": "metadata", "value": ""}],
    });
    let spec: crate::domain::rules::schema::DetectMatch =
        serde_json::from_value(contains_nothing).expect("the probe parses");
    assert!(
        crate::domain::rules::detect_rules::atom_literal_defect(&spec).is_some(),
        "an empty substring search must be refused: every present value contains it"
    );

    // An equality against the empty string stays legal - a producer can write an attribute that holds it.
    let equals_empty: crate::domain::rules::schema::DetectMatch =
        serde_json::from_value(serde_json::json!({"attr_equals": [{"key": "k", "value": ""}]}))
            .expect("the probe parses");
    assert!(
        crate::domain::rules::detect_rules::atom_literal_defect(&equals_empty).is_none(),
        "an attribute that holds the empty string is a thing a producer writes"
    );

    // A member requirement naming nothing asks for `<entry>.`, so the rule is dead; a requirement with no
    // entries holds for every entry, which is the opposite of "at least one of these".
    let probe_family = |require: serde_json::Value| {
        let asset = serde_json::json!({
            "id": "probe",
            "messages": [{
                "id": "probe.family",
                "legacy_rank": 1,
                "read": {"indexed_family": "probe.items"},
                "emit": "message",
                "require_members": require,
            }],
        });
        crate::domain::rules::message_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };
    assert!(
        probe_family(serde_json::json!({"all_of": [{"name": ""}]})).is_err(),
        "a requirement naming an empty member asks for `<entry>.` and can never hold"
    );
    assert!(
        probe_family(serde_json::json!({})).is_err(),
        "a requirement with no entries holds for every entry, which is not a requirement"
    );
    assert!(
        probe_family(serde_json::json!({"all_of": [{"name": "role"}]})).is_ok(),
        "a requirement naming a member must compile"
    );
}

/// `non_empty` asks a question a scalar cannot answer, so a scalar fails it either way.
///
/// `{"path": "$.content", "non_empty": true}` held for `{"content": 0}` - a number reported as filled
/// content. The compiler's refusal does not cover it: that one is about the *declaration*, and this is about
/// the value that arrives.
#[test]
fn a_scalar_is_neither_empty_nor_non_empty() {
    use crate::domain::rules::message_rules::predicate_holds_for_test as holds;

    for (subject, want) in [
        (serde_json::json!("text"), true),
        (serde_json::json!(""), false),
        (serde_json::json!([1]), true),
        (serde_json::json!([]), false),
        (serde_json::json!({"a": 1}), true),
        (serde_json::json!({}), false),
        // Neither, whichever way it is asked.
        (serde_json::json!(0), false),
        (serde_json::json!(7), false),
        (serde_json::json!(true), false),
        (serde_json::json!(serde_json::Value::Null), false),
    ] {
        let predicate: crate::domain::rules::schema::ValuePredicate =
            serde_json::from_value(serde_json::json!({"non_empty": true}))
                .expect("the probe parses");
        assert_eq!(
            holds(&predicate, &subject),
            want,
            "`non_empty: true` over {subject}"
        );
        let negated: crate::domain::rules::schema::ValuePredicate =
            serde_json::from_value(serde_json::json!({"non_empty": false}))
                .expect("the probe parses");
        // A string, array or object answers the negation; a scalar still fails, because the vocabulary has
        // nothing to say about it.
        let scalar = matches!(
            subject,
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) | serde_json::Value::Null
        );
        assert_eq!(
            holds(&negated, &subject),
            !scalar && !want,
            "`non_empty: false` over {subject}"
        );
    }
}

/// A **composed** reading cannot be starved by a lower-ranked rule.
///
/// Found by writing `specs/CarrierClaiming.tla`, not by reading the code. The spec's first form asserted
/// "the lowest-ranked rule that reads a carrier gets it", and TLC refuted it in seconds: a rule takes a
/// carrier only if it can take **every** carrier it reads, so a rank-1 rule reading one attribute leaves a
/// rank-4 rule that composes that attribute with another unable to take either - and the second attribute
/// then ends up owned by nobody even though a rule reads it.
///
/// Not reachable in today's ruleset: both composed readings have no lower-ranked competitor for any of their
/// attributes. That is exactly why this is worth pinning - the failure is silent (a whole dialect's response
/// reading disappears), it depends on a *rank relation between two rules in different assets*, and nothing
/// about either rule looks wrong on its own.
///
/// **Every spelling a member might select**, not only a member with one. My first version of this test scoped
/// it to single-spelling members on the reasoning that a member listing two survives losing one - which is
/// false, and Codex caught it: `composed()` takes the **first present** spelling with `find_map` and never
/// retries. So a span carrying `x`, `x_backup` and `y` where a lower-ranked rule reads `x` selects `x`, builds
/// an emission owning `x` and `y`, and loses the whole emission - the backup spelling is never tried. The
/// remediation the old message suggested ("give that member a second spelling") would not have worked either.
#[test]
fn a_composed_reading_cannot_be_starved_by_a_lower_ranked_rule() {
    use crate::domain::rules::schema::RuleFile;

    #[derive(Debug)]
    struct Reading {
        id: String,
        rank: i32,
        /// Attributes it takes when it wins, in any of its forms.
        reads: std::collections::BTreeSet<String>,
        /// Attributes it *must* have, one entry per compose member with a single spelling.
        requires: std::collections::BTreeSet<String>,
    }

    let mut readings: Vec<Reading> = Vec::new();
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.messages {
            let Some(rank) = rule.legacy_rank else {
                continue;
            };
            let mut reads = std::collections::BTreeSet::new();
            let mut requires = std::collections::BTreeSet::new();
            if let Some(attribute) = &rule.read.attribute {
                reads.insert(attribute.clone());
            }
            reads.extend(rule.read.attribute_any_of.iter().cloned());
            if let Some(compose) = &rule.compose {
                for member in &compose.members {
                    reads.extend(member.from_any_of.iter().cloned());
                    // Every spelling: whichever one the span happens to carry first is the one selected, and
                    // if that one is claimed the emission is lost whole.
                    requires.extend(member.from_any_of.iter().cloned());
                }
            }
            if reads.is_empty() {
                continue;
            }
            let _ = &path;
            readings.push(Reading {
                id: rule.id.clone(),
                rank,
                reads,
                requires,
            });
        }
    }
    assert!(
        readings.iter().any(|r| r.requires.len() > 1),
        "no composed reading was found, so this test is checking nothing"
    );

    let mut starvable: Vec<String> = Vec::new();
    for composed in readings.iter().filter(|r| r.requires.len() > 1) {
        for taker in &readings {
            if taker.id == composed.id || taker.rank >= composed.rank {
                continue;
            }
            let stolen: Vec<&String> = composed
                .requires
                .iter()
                .filter(|attribute| taker.reads.contains(*attribute))
                .collect();
            if !stolen.is_empty() {
                starvable.push(format!(
                    "  `{}` (rank {}) needs {:?}, and `{}` (rank {}) reads it first",
                    composed.id, composed.rank, stolen, taker.id, taker.rank
                ));
            }
        }
    }
    starvable.sort();
    assert!(
        starvable.is_empty(),
        "{} composed reading(s) can be starved. A compose selects the **first present** spelling of each \
         member and never retries, and an emission is accepted only if its whole ownership set is free - so \
         the composed rule loses its emission entirely and its other attributes end up owned by nobody. \
         Silently, and neither rule looks wrong on its own. Rank the composed reading **above** the rule that \
         takes its parts; a second spelling does not help, because the taken one is the one selected:\n{}",
        starvable.len(),
        starvable.join("\n")
    );
}

/// The boolean grammar answers exactly as the shell it replaces, over generated spans.
///
/// The migration's whole risk is that a translated condition means something slightly different, and the
/// difference shows only on an input nobody wrote a fixture for. So this compares the new expression against
/// the retired `DetectMatch` evaluator on **every** `DetectMatch` the shipped assets contain, over a generated
/// set of spans chosen to sit on each dimension's boundary: the attribute present with the value, present with
/// another value, absent entirely, and the span name matching or not.
///
/// `Unknown` is where they may legitimately differ, and the assertion is directional: where the old evaluator
/// said **true**, the new one must say true. The old one could not say "I cannot ask this", so it answered
/// false for an absent attribute - and the new one answers `Unknown`, which also does not hold. Requiring
/// equality of `holds()` is therefore the right comparison, and it is the one made here.
#[test]
fn the_boolean_grammar_answers_as_the_shell_it_replaces() {
    use crate::domain::rules::expr::{SpanSubject, span_expr_of};
    use crate::domain::rules::schema::{DetectMatch, RuleFile};
    use std::collections::HashMap;

    // Every `DetectMatch` the assets declare, wherever it sits.
    let mut specs: Vec<(String, DetectMatch)> = Vec::new();
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.detect {
            specs.push((format!("{path}:{}", rule.id), rule.match_spec.clone()));
        }
        for rule in file.observation_types.iter().chain(&file.span_categories) {
            for (index, conjunct) in rule.all_of.iter().enumerate() {
                specs.push((format!("{path}:{}#{index}", rule.id), conjunct.clone()));
            }
        }
        // The gates, wherever they are nested: `when` and `unless` on a message rule, a branch leaf, a field
        // source, a compose fallback. Walked over the raw JSON so a nesting nobody remembered is included.
        fn walk(value: &serde_json::Value, path: &str, out: &mut Vec<(String, DetectMatch)>) {
            match value {
                serde_json::Value::Object(members) => {
                    for (key, inner) in members {
                        if (key == "when" || key == "unless")
                            && let Ok(spec) = serde_json::from_value::<DetectMatch>(inner.clone())
                        {
                            out.push((format!("{path}/{key}"), spec));
                        }
                        walk(inner, path, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, path, out);
                    }
                }
                _ => {}
            }
        }
        walk(&value, &path, &mut specs);
    }
    assert!(
        specs.len() > 40,
        "only {} conditions were found in the assets, which cannot be right",
        specs.len()
    );

    // The keys and values every condition mentions, so the generated spans sit on their boundaries.
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut values: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_, spec) in &specs {
        keys.extend(spec.attr_exists.iter().cloned());
        keys.extend(spec.attr_prefix.iter().cloned());
        for pair in spec
            .attr_equals
            .iter()
            .chain(&spec.attr_equals_ignore_case)
            .chain(&spec.span_attr_contains)
        {
            keys.insert(pair.key.clone());
            values.insert(pair.value.clone());
        }
        names.extend(spec.span_name.iter().cloned());
        if let Some(text) = &spec.text_contains {
            for source in &text.sources {
                if let Some(key) = source.strip_prefix("attr:") {
                    keys.insert(key.to_string());
                }
            }
            values.extend(text.needles.iter().cloned());
        }
    }

    let mut compared = 0_usize;
    let mut per_condition: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut disagreements: Vec<String> = Vec::new();
    let mut untranslated: Vec<String> = Vec::new();
    for (id, spec) in &specs {
        // A condition that translates to nothing is **not** silently skipped. Skipping it was this test's own
        // version of the defect it exists to catch: dropping a dimension from the translation made every
        // condition that used only that dimension produce `None`, so it was skipped rather than compared, and
        // the mutation passed. A condition with any signal at all must translate.
        let has_signal = !spec.span_name.is_empty()
            || !spec.attr_prefix.is_empty()
            || !spec.attr_exists.is_empty()
            || !spec.attr_equals.is_empty()
            || !spec.attr_equals_ignore_case.is_empty()
            || !spec.span_attr_contains.is_empty()
            || spec
                .text_contains
                .as_ref()
                .is_some_and(|text| !text.needles.is_empty() && !text.sources.is_empty());
        let expr = match span_expr_of(spec) {
            Some(expr) => expr,
            None => {
                if has_signal {
                    untranslated.push(format!("  {id}: {spec:?}"));
                }
                continue;
            }
        };
        // Spans built from **this condition's own** literals, not from a global cross product truncated to a
        // budget. That was the first form and it hid two mutations: the targeted inputs were appended after a
        // large product and then cut off by the `take`, so a case-folding change and an ignored
        // `first_present` flag were never reached. A per-condition set is smaller *and* complete.
        let mut spans: Vec<(String, HashMap<String, String>)> =
            vec![("other.span".to_string(), HashMap::new())];
        let mut mentioned_keys: Vec<String> = spec
            .attr_exists
            .iter()
            .chain(&spec.attr_prefix)
            .cloned()
            .collect();
        // A key *under* each declared prefix, since that dimension is about the key rather than the value and
        // an exactly-equal key is not what it asks.
        for prefix in &spec.attr_prefix {
            mentioned_keys.push(format!("{prefix}something"));
        }
        let mut mentioned_values: Vec<String> = Vec::new();
        for pair in spec
            .attr_equals
            .iter()
            .chain(&spec.attr_equals_ignore_case)
            .chain(&spec.span_attr_contains)
        {
            mentioned_keys.push(pair.key.clone());
            mentioned_values.push(pair.value.clone());
        }
        if let Some(text) = &spec.text_contains {
            for source in &text.sources {
                if let Some(key) = source.strip_prefix("attr:") {
                    mentioned_keys.push(key.to_string());
                }
            }
            mentioned_values.extend(text.needles.iter().cloned());
        }
        mentioned_keys.sort();
        mentioned_keys.dedup();
        mentioned_values.sort();
        mentioned_values.dedup();
        // A condition may mention keys and no values at all - `attr_exists`, `attr_prefix`, a bare span-name
        // prefix. Those still need a span where the key is there and one where it is not, or the condition is
        // compared only against the empty span and the comparison shows nothing.
        if mentioned_values.is_empty() {
            mentioned_values.push("a value".to_string());
            mentioned_values.push(String::new());
        }
        if mentioned_keys.is_empty() {
            mentioned_keys.push("some.attribute".to_string());
        }

        let flip = |text: &str| -> String {
            text.chars()
                .map(|c| {
                    if c.is_ascii_lowercase() {
                        c.to_ascii_uppercase()
                    } else {
                        c.to_ascii_lowercase()
                    }
                })
                .collect()
        };

        let span_names: Vec<String> = spec
            .span_name
            .iter()
            .cloned()
            .chain(["other.span".to_string()])
            .collect();
        for name in &span_names {
            // The key absent altogether, which is where a value question has no answer.
            spans.push((name.clone(), HashMap::new()));
            for key in &mentioned_keys {
                for value in &mentioned_values {
                    for variant in [
                        value.clone(),
                        flip(value),
                        format!("prefix {value} suffix"),
                        "a value nothing mentions".to_string(),
                        String::new(),
                    ] {
                        let mut attrs = HashMap::new();
                        attrs.insert(key.clone(), variant);
                        spans.push((name.clone(), attrs));
                    }
                }
            }
            // The case that separates `first_present` from `any_present`, which needs *different* values
            // across the keys: exactly one source holds the matching value and the others hold something
            // unrelated. With the match on a later source, a first-present search says no and an any-present
            // search says yes - and with every key holding the same value the two agree, which is why the
            // same-value spans below could not see an ignored flag.
            if mentioned_keys.len() > 1 {
                for value in mentioned_values.iter().take(2) {
                    for matching in &mentioned_keys {
                        let mut attrs = HashMap::new();
                        for key in &mentioned_keys {
                            attrs.insert(
                                key.clone(),
                                if key == matching {
                                    value.clone()
                                } else {
                                    "unrelated".to_string()
                                },
                            );
                        }
                        spans.push((name.clone(), attrs));
                    }
                }
            }
            // Every key present at once, and each *single* key present with the others absent.
            if mentioned_keys.len() > 1 {
                for value in mentioned_values.iter().take(2) {
                    let mut all = HashMap::new();
                    for key in &mentioned_keys {
                        all.insert(key.clone(), value.clone());
                    }
                    spans.push((name.clone(), all));
                    for present in &mentioned_keys {
                        let mut one = HashMap::new();
                        one.insert(present.clone(), value.clone());
                        spans.push((name.clone(), one));
                        let mut one_other = HashMap::new();
                        one_other.insert(present.clone(), "unrelated".to_string());
                        spans.push((name.clone(), one_other));
                    }
                }
            }
        }
        for (span_name, attrs) in &spans {
            let old =
                crate::domain::rules::detect_rules::signals_hold_for_test(spec, span_name, attrs);
            let new = expr
                .eval(&mut |atom| atom.eval(&SpanSubject { span_name, attrs }))
                .holds();
            compared += 1;
            *per_condition.entry(id.clone()).or_default() += 1;
            if old != new {
                disagreements.push(format!(
                    "  {id}: span `{span_name}` attrs {attrs:?} - retired said {old}, the grammar says {new}"
                ));
            }
        }
    }
    assert!(
        untranslated.is_empty(),
        "{} condition(s) have a signal and translate to no expression, so they were never compared - which \
         is how a dropped dimension passes this test:\n{}",
        untranslated.len(),
        untranslated.join("\n")
    );
    // A floor per **condition**, not a total. A large total of arbitrary spans is what the first form had, and
    // it reached none of the boundaries that matter; what makes a comparison worth counting is that it was
    // built from the condition's own literals, so the requirement is that every condition got several.
    let thin: Vec<&String> = per_condition
        .iter()
        .filter(|(_, count)| **count < 3)
        .map(|(id, _)| id)
        .collect();
    assert!(
        thin.is_empty(),
        "{} condition(s) were compared on fewer than three spans, so the translation is barely exercised \
         for them: {:?}",
        thin.len(),
        thin.iter().take(8).collect::<Vec<_>>()
    );
    assert!(
        compared > 500,
        "only {compared} comparisons were made in total, which cannot exercise the translation"
    );
    disagreements.sort();
    disagreements.dedup();
    assert!(
        disagreements.is_empty(),
        "{} of {compared} comparisons disagree. The translation must preserve meaning exactly, or a \
         migrated condition means something the asset did not say:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The JSON half of the grammar answers as the shell it replaces, including the witness binding.
///
/// The binding is the finding this test exists for. A `ValuePredicate` carries a path and several conditions,
/// and the retired evaluator required **one selected value** to satisfy them all - so
/// `{path: "$.items[*]", starts_with: "a", one_of: ["apple","banana"]}` is false for `["avocado","banando"]`.
/// Translating each condition into its own atom under `all` makes that *true*, with different elements
/// witnessing the two clauses: a silent change of meaning in every multi-condition rule. So the first case
/// below is Codex's exact example, and the rest is every predicate the assets declare over generated payloads.
#[test]
fn the_json_grammar_answers_as_the_shell_it_replaces() {
    use crate::domain::rules::expr::json_expr_of_predicate;
    use crate::domain::rules::message_rules::predicate_holds_for_test as retired;
    use crate::domain::rules::schema::ValuePredicate;

    let evaluate = |predicate: &ValuePredicate, payload: &serde_json::Value| -> (bool, bool) {
        let old = retired(predicate, payload);
        let new = json_expr_of_predicate(predicate)
            .map(|expr| expr.eval(&mut |atom| atom.eval(payload)).holds())
            // A predicate stating nothing translates to nothing; the retired evaluator held for it.
            .unwrap_or(true);
        (old, new)
    };

    // The witness case, named explicitly so a regression says what broke.
    let witness: ValuePredicate = serde_json::from_value(serde_json::json!({
        "path": "$.items[*]",
        "starts_with": "a",
        "one_of": ["apple", "banana"],
    }))
    .expect("the probe parses");
    let split = serde_json::json!({"items": ["avocado", "banana"]});
    let (old, new) = evaluate(&witness, &split);
    assert!(
        !old,
        "the retired evaluator required one value to satisfy every condition"
    );
    assert_eq!(
        new, old,
        "two different elements must not witness two clauses of one predicate - the conditions share a \
         selection, which is why a predicate becomes one `some` holding a sub-expression rather than several \
         atoms under `all`"
    );
    // And a payload where one element does satisfy both must still hold.
    let together = serde_json::json!({"items": ["apple", "cherry"]});
    let (old, new) = evaluate(&witness, &together);
    assert!(old && new, "one element satisfying both must hold");

    // Every predicate the assets declare, over payloads generated from its own literals.
    let mut predicates: Vec<(String, ValuePredicate)> = Vec::new();
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        fn walk(value: &serde_json::Value, path: &str, out: &mut Vec<(String, ValuePredicate)>) {
            match value {
                serde_json::Value::Object(members) => {
                    // A predicate set sits under many member names, so it is recognised by *shape*: an object
                    // with `all` or `any` holding objects that parse as predicates.
                    for key in ["all", "any"] {
                        if let Some(serde_json::Value::Array(items)) = members.get(key) {
                            for item in items {
                                if let Ok(predicate) =
                                    serde_json::from_value::<ValuePredicate>(item.clone())
                                {
                                    out.push((format!("{path}/{key}"), predicate));
                                }
                            }
                        }
                    }
                    for inner in members.values() {
                        walk(inner, path, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, path, out);
                    }
                }
                _ => {}
            }
        }
        walk(&value, &path, &mut predicates);
    }
    assert!(
        predicates.len() > 30,
        "only {} predicates were found in the assets, which cannot be right",
        predicates.len()
    );

    let mut compared = 0_usize;
    let mut disagreements: Vec<String> = Vec::new();
    for (id, predicate) in &predicates {
        // Values this predicate mentions, plus the shapes that sit on its boundaries.
        let mut mentioned: Vec<serde_json::Value> = predicate
            .one_of
            .iter()
            .chain(&predicate.none_of)
            .map(|text| serde_json::json!(text))
            .collect();
        if let Some(prefix) = &predicate.starts_with {
            mentioned.push(serde_json::json!(prefix));
            mentioned.push(serde_json::json!(format!("{prefix}more")));
        }
        if let Some(prefix) = &predicate.lacks_prefix {
            mentioned.push(serde_json::json!(prefix));
            mentioned.push(serde_json::json!(format!("{prefix}more")));
        }
        mentioned.extend([
            serde_json::json!("something else"),
            serde_json::json!(""),
            serde_json::json!(0),
            serde_json::json!(7),
            serde_json::json!(true),
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(["a"]),
            serde_json::json!({}),
            serde_json::json!({"k": "v"}),
        ]);

        // The path's own leading member, so a payload can actually place a value where it looks.
        let member = predicate
            .path
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
            .trim_start_matches("$.")
            .trim_start_matches("$['")
            .split(['.', '[', '\''])
            .next()
            .unwrap_or("value")
            .to_string();

        let mut payloads: Vec<serde_json::Value> = vec![
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!("a bare string"),
        ];
        for value in &mentioned {
            payloads.push(serde_json::json!({member.clone(): value.clone()}));
            payloads.push(value.clone());
            // A list at the member, so a plural path selects more than one - which is where the witness
            // binding shows.
            payloads.push(serde_json::json!({member.clone(): [value.clone()]}));
            for other in mentioned.iter().take(3) {
                payloads.push(serde_json::json!({member.clone(): [value.clone(), other.clone()]}));
            }
        }

        for payload in &payloads {
            let (old, new) = evaluate(predicate, payload);
            compared += 1;
            if old != new {
                disagreements.push(format!(
                    "  {id}: {predicate:?} over {payload} - retired said {old}, the grammar says {new}"
                ));
            }
        }
    }
    assert!(
        compared > 2000,
        "only {compared} comparisons were made, which cannot exercise the translation"
    );
    disagreements.sort();
    disagreements.dedup();
    assert!(
        disagreements.is_empty(),
        "{} of {compared} comparisons disagree:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The third truth value is observable, and this is where.
///
/// The translation oracle cannot see it: no *translated* expression negates a `some`, so `Unknown` and `False`
/// are indistinguishable through it - both fail `holds()`. Removing the empty-selection case therefore passed
/// that oracle. The distinction exists for expressions an author writes directly, where the whole point is that
/// `not` over "there was nothing to ask about" does **not** become "yes".
///
/// That is the trap this repository knows from SQL: `NOT IN (…)` over a nullable column drops the rows with no
/// value, and `Filter::positive_twin` exists because negating "is this session" dropped every trace with no
/// session. `none_of` is the same mistake at predicate level.
#[test]
fn a_negation_over_nothing_does_not_hold() {
    use crate::domain::rules::expr::{Expr, JsonAtom, JsonSubjectAtom, Truth};

    // The constructors return `Option`, because an empty group is unconstructible by design. Every group here
    // is written with two children, so the invariant holds and these say so.
    fn two_all(children: Vec<Expr<JsonAtom>>) -> Expr<JsonAtom> {
        Expr::all(children).expect("two children were written out")
    }
    fn two_any(children: Vec<Expr<JsonAtom>>) -> Expr<JsonAtom> {
        Expr::any(children).expect("two children were written out")
    }
    use crate::domain::rules::schema::{JsonPath, ValueKind};

    let path = JsonPath::parse("$.name").expect("a path");
    let payload = serde_json::json!({"other": "value"});
    let present = serde_json::json!({"name": "alice"});

    let some_is = |values: Vec<&str>| {
        Expr::Atom(JsonAtom::Some {
            path: path.clone(),
            satisfies: Box::new(Expr::Atom(JsonSubjectAtom::OneOf {
                values: values.into_iter().map(str::to_string).collect(),
            })),
        })
    };
    let eval =
        |expr: &Expr<JsonAtom>, value: &serde_json::Value| expr.eval(&mut |atom| atom.eval(value));

    // The member is absent: the question cannot be asked.
    assert_eq!(eval(&some_is(vec!["bob"]), &payload), Truth::Unknown);
    // And negating it stays Unknown, so the condition does not hold. Two-valued, this would be `true` - a rule
    // saying "the name is not bob" would match a payload with no name at all.
    assert_eq!(
        eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &payload),
        Truth::Unknown
    );
    assert!(
        !eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &payload).holds(),
        "a negation over an absent value must not hold"
    );

    // Present and not in the set: a real no, so the negation is a real yes.
    assert_eq!(eval(&some_is(vec!["bob"]), &present), Truth::False);
    assert!(eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &present).holds());

    // Presence itself is **total**, which is how absence is stated: asking about presence always has an answer,
    // so its negation is a real yes.
    let exists = Expr::Atom(JsonAtom::Exists { path: path.clone() });
    assert_eq!(eval(&exists, &payload), Truth::False);
    assert!(eval(&Expr::Not(Box::new(exists)), &payload).holds());

    // `all` and `any` are strong Kleene: a False child settles a conjunction whatever else is Unknown, and a
    // True child settles a disjunction. Without that, one unanswerable atom would poison a whole condition.
    let unknown = some_is(vec!["bob"]);
    let definitely_false = Expr::Atom(JsonAtom::Some {
        path: JsonPath::parse("$.other").expect("a path"),
        satisfies: Box::new(Expr::Atom(JsonSubjectAtom::Kind {
            kind: ValueKind::Number,
        })),
    });
    assert_eq!(
        eval(
            &two_all(vec![unknown.clone(), definitely_false.clone()]),
            &payload
        ),
        Truth::False
    );
    assert_eq!(
        eval(&two_any(vec![unknown.clone(), definitely_false]), &payload),
        Truth::Unknown
    );
    let definitely_true = Expr::Atom(JsonAtom::Some {
        path: JsonPath::parse("$.other").expect("a path"),
        satisfies: Box::new(Expr::Atom(JsonSubjectAtom::Kind {
            kind: ValueKind::String,
        })),
    });
    assert_eq!(
        eval(
            &two_any(vec![unknown.clone(), definitely_true.clone()]),
            &payload
        ),
        Truth::True
    );
    assert_eq!(
        eval(&two_all(vec![unknown, definitely_true]), &payload),
        Truth::Unknown
    );
}

/// A shared rank is refused where the order is observable, and allowed where it is not.
///
/// The tie-break was the rule **id**, so renaming a rule changed which of two contenders read a carrier. A rule
/// id must not be a control-flow primitive - and classification and detection already refuse a shared rank
/// outright, so message rules were the one resolver where it was silently policy.
///
/// Not refused globally, and the second half of this test is why: the five shared ranks in the shipped assets
/// each pair a *message* rule with a *metadata* one, whose orders are independent. Refusing those would force
/// five renumberings that state nothing.
#[test]
fn a_shared_message_rank_is_refused_only_where_the_order_shows() {
    let compiled = |rules: serde_json::Value| {
        // The events the probes select on have to be declared, or compilation refuses them for that reason
        // instead - which is a different refusal and would make this test pass for the wrong reason.
        let asset = serde_json::json!({
            "id": "probe",
            "message_events": [
                {"name": "probe.first"},
                {"name": "probe.second"},
                {"name": "probe.shared"},
                {"name": "probe.other"},
            ],
            "messages": rules,
        });
        crate::domain::rules::message_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };

    // Codex's case: two conditional message rules reading one attribute at one rank. `a` wins today because
    // ids sort; renaming it to `zz` would hand the carrier to the other.
    let contending = serde_json::json!([
        {
            "id": "probe.a",
            "legacy_rank": 10,
            "read": {"attribute": "shared"},
            "parse": "text",
            "when": {"attr_exists": ["left"]},
            "emit": "message",
        },
        {
            "id": "probe.z",
            "legacy_rank": 10,
            "read": {"attribute": "other"},
            "parse": "text",
            "when": {"attr_exists": ["right"]},
            "emit": "message",
        },
    ]);
    assert!(
        compiled(contending).is_err(),
        "two message rules at one rank in the same stage must be refused: the tie-break is their ids"
    );

    // Different **axes**: a message and a tool definition are never read by the same path.
    let cross_axis = serde_json::json!([
        {
            "id": "probe.message",
            "legacy_rank": 10,
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.tools",
            "legacy_rank": 10,
            "read": {"attribute": "two"},
            "parse": "json",
            "emit": "tool_definitions",
        },
    ]);
    assert!(
        compiled(cross_axis).is_ok(),
        "a message rule and a metadata rule at one rank order nothing relative to each other"
    );

    // Different **event domains**: selected by name, and the names do not intersect.
    let disjoint_events = serde_json::json!([
        {
            "id": "probe.one",
            "legacy_rank": 20,
            "when_event": ["probe.first"],
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "legacy_rank": 20,
            "when_event": ["probe.second"],
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(disjoint_events).is_ok(),
        "two event rules at one rank whose names never coincide are never candidates together"
    );

    // An event rule beside a **span** rule: one is selected by event name and the other reads a span's
    // attributes, so they are never candidates in the same pass.
    let event_beside_span = serde_json::json!([
        {
            "id": "probe.event",
            "legacy_rank": 25,
            "when_event": ["probe.first"],
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.span",
            "legacy_rank": 25,
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(event_beside_span).is_ok(),
        "an event rule and a span rule at one rank are never candidates together"
    );

    // The same, where the names *do* intersect.
    let overlapping_events = serde_json::json!([
        {
            "id": "probe.one",
            "legacy_rank": 20,
            "when_event": ["probe.shared"],
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "legacy_rank": 20,
            "when_event": ["probe.shared", "probe.other"],
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(overlapping_events).is_err(),
        "two event rules at one rank sharing an event name contend on that event"
    );

    // Different **stages**: a fallback rule runs only where the dialect stage produced nothing.
    let cross_stage = serde_json::json!([
        {
            "id": "probe.dialect",
            "legacy_rank": 30,
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.fallback",
            "legacy_rank": 30,
            "stage": "fallback",
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(cross_stage).is_ok(),
        "a fallback rule's rank orders nothing against a dialect rule's"
    );
}

/// A `supersedes` edge that cannot take effect is refused.
///
/// The field waives the **overlap report** - the instrument that names which predicates are not yet
/// sufficient - and it does *not* order anything: `legacy_rank` still decides the winner, and the waiver is
/// read only from the rule that already won. So three shapes compiled silently and were inspected by nobody:
/// an edge naming a rule that does not exist, an edge to itself, and Codex's case - an edge from a
/// **higher**-ranked rule to a lower-ranked one, where the superseding rule never becomes the winner whose
/// waiver is consulted.
///
/// All eight edges in the shipped assets point from the rank-winner to the rank-loser, so today the field
/// documents why a rank is what it is. That is a fine thing for it to be; what it must not be is a statement
/// an author believes orders their rules.
#[test]
fn a_supersedes_edge_that_cannot_take_effect_is_refused() {
    let compiled = |first_rank: i32, second_rank: i32, supersedes: &str| {
        let asset = serde_json::json!({
            "id": "probe",
            "detect": [
                {
                    "id": "probe.specific",
                    "label": "strands",
                    "legacy_rank": first_rank,
                    "match": {"attr_prefix": ["probe.specific."]},
                    "supersedes": [supersedes],
                },
                {
                    "id": "probe.generic",
                    "label": "langchain",
                    "legacy_rank": second_rank,
                    "match": {"attr_prefix": ["probe."]},
                },
            ],
        });
        crate::domain::rules::detect_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("the probe serialises"),
        )]))
    };

    // The shape that works: the superseding rule outranks its target, so its waiver is the one read.
    assert!(
        compiled(10, 20, "probe.generic").is_ok(),
        "an edge from the rank-winner to the rank-loser is the shape the assets use"
    );
    // Codex's case: the edge points the wrong way down the ranks and is inspected by nobody.
    assert!(
        compiled(20, 10, "probe.generic").is_err(),
        "an edge from a rule its target outranks can never take effect"
    );
    // A target nothing declares.
    assert!(
        compiled(10, 20, "probe.absent").is_err(),
        "an edge naming a rule no asset declares can never take effect"
    );
    // An edge to itself.
    assert!(
        compiled(10, 20, "probe.specific").is_err(),
        "a rule cannot supersede itself"
    );

    // Named twice: one of the two says nothing.
    let repeated = serde_json::json!({
        "id": "probe",
        "detect": [
            {
                "id": "probe.specific",
                "label": "strands",
                "legacy_rank": 10,
                "match": {"attr_prefix": ["probe.specific."]},
                "supersedes": ["probe.generic", "probe.generic"],
            },
            {
                "id": "probe.generic",
                "label": "langchain",
                "legacy_rank": 20,
                "match": {"attr_prefix": ["probe."]},
            },
        ],
    });
    assert!(
        crate::domain::rules::detect_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&repeated).expect("serialises"),
        )]))
        .is_err(),
        "a target named twice by one rule must be refused"
    );

    // A **cycle** needs no test here, and that is a statement rather than a gap: every edge must point at a
    // rule its declarer outranks, so a cycle is unconstructible and the separate check written for it could
    // never fire. It comes back when `legacy_rank` does not.
}

/// `supersedes` is **transitive** where the overlap report reads it.
///
/// The report is the instrument for retiring `legacy_rank`: it names the spans where several rules match and no
/// ordering has been declared. Reading only *direct* edges made it report an overlap whose order **is**
/// declared - a rule superseding one that supersedes a third - which is noise in the one place that must be
/// signal.
#[test]
fn a_superseded_rule_is_dominated_transitively() {
    let asset = serde_json::json!({
        "id": "probe",
        "detect": [
            {
                "id": "probe.most_specific",
                "label": "strands",
                "legacy_rank": 10,
                "match": {"attr_exists": ["probe.marker"]},
                "supersedes": ["probe.middle"],
            },
            {
                "id": "probe.middle",
                "label": "langchain",
                "legacy_rank": 20,
                "match": {"attr_exists": ["probe.marker"]},
                "supersedes": ["probe.generic"],
            },
            {
                "id": "probe.generic",
                "label": "crewai",
                "legacy_rank": 30,
                "match": {"attr_exists": ["probe.marker"]},
            },
        ],
    });
    let plan = crate::domain::rules::detect_rules::compile(&std::collections::BTreeMap::from([(
        "probe.json".to_string(),
        serde_json::to_vec(&asset).expect("serialises"),
    )]))
    .expect("the probe compiles");

    let mut attrs = std::collections::HashMap::new();
    attrs.insert("probe.marker".to_string(), "1".to_string());
    let ctx = crate::domain::rules::detect_rules::DetectContext {
        span_name: "probe.span",
        span_attrs: &attrs,
        resource_attrs: &std::collections::HashMap::new(),
    };
    // All three match. The winner supersedes the middle directly and the generic *through* it, so nothing is
    // unresolved and the report must be empty.
    assert!(
        plan.overlapping_candidates(&ctx).is_empty(),
        "the winner dominates both others - directly and transitively - so no overlap is unresolved"
    );
}

/// Every clause id is non-empty and unique within the rule or fragment that holds it.
///
/// The ids exist so an emission can say **which** clause answered - a rule with four readings used to report
/// only the rule's id, and `doc` was standing in for an identity. That only works if an id identifies something:
/// two clauses sharing one inside a rule make a diagnostic ambiguous exactly where it is being read, and an
/// empty one names nothing.
///
/// Checked over the raw assets rather than per compiler, because the six clause types are compiled by three
/// different modules and this is one property about all of them - so a seventh type is covered the day it is
/// added, provided it is listed here.
#[test]
fn a_clause_id_is_unique_within_its_owner() {
    /// The array members that are answer-capable clauses, and therefore carry an id.
    const CLAUSE_ARRAYS: &[&str] = &[
        "alternatives",
        "also",
        "fallback",
        "extra_cases",
        "cases",
        "passes",
        "by",
        "routes",
        "sources",
        "signals",
    ];

    fn collect(
        value: &serde_json::Value,
        member: Option<&str>,
        found: &mut Vec<String>,
        problems: &mut Vec<String>,
        owner: &str,
    ) {
        match value {
            serde_json::Value::Object(members) => {
                for (key, inner) in members {
                    collect(inner, Some(key), found, problems, owner);
                }
            }
            serde_json::Value::Array(items) => {
                let is_clause = member.is_some_and(|name| CLAUSE_ARRAYS.contains(&name));
                for item in items {
                    if is_clause && item.is_object() {
                        match item.get("id").and_then(serde_json::Value::as_str) {
                            Some(id) if !id.is_empty() => found.push(id.to_string()),
                            Some(_) => problems
                                .push(format!("  {owner}: a clause declares an empty id")),
                            // The schema requires it, so this is a shape no asset can have - asserted so the
                            // list above staying in step with the schema is checked rather than assumed.
                            None => problems.push(format!(
                                "  {owner}: a clause in `{}` has no id, so the schema no longer requires one \
                                 there",
                                member.unwrap_or("?")
                            )),
                        }
                    }
                    collect(item, member, found, problems, owner);
                }
            }
            _ => {}
        }
    }

    let mut problems: Vec<String> = Vec::new();
    let mut total = 0_usize;
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let asset: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        let Some(members) = asset.as_object() else {
            continue;
        };
        // One id space per owner: a top-level entry of a rule section, or a named fragment.
        let mut owners: Vec<(String, &serde_json::Value)> = Vec::new();
        for (section, value) in members {
            match value {
                serde_json::Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        let name = item
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{section}[{index}]"));
                        owners.push((format!("{path}/{section}/{name}"), item));
                    }
                }
                // `fragments` is a map, and each entry is its own id space.
                serde_json::Value::Object(fragments) if section == "fragments" => {
                    for (name, fragment) in fragments {
                        owners.push((format!("{path}/fragments/{name}"), fragment));
                    }
                }
                _ => {}
            }
        }
        for (owner, value) in owners {
            let mut found = Vec::new();
            collect(value, None, &mut found, &mut problems, &owner);
            total += found.len();
            let mut seen: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
            for id in &found {
                if !seen.insert(id) {
                    problems.push(format!(
                        "  {owner}: two clauses share the id `{id}`, so a diagnostic naming it is ambiguous"
                    ));
                }
            }
        }
    }
    assert!(
        total > 190,
        "only {total} clause ids were found, and the assets declare over 200 - the list of clause arrays is \
         probably out of step with the schema"
    );
    problems.sort();
    problems.dedup();
    assert!(
        problems.is_empty(),
        "{} clause id problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
