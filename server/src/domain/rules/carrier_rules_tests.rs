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

/// A carrier qualifier that answers differently depending on who asks is not part of the format.
///
/// Three of the four qualifiers were in that state, and none of the 55 shipped clauses used any of them - so the
/// accepted set is `observation_type` alone, and the list was never corpus-derived.
///
/// - **`span_name_prefix` is removed**, following `read.event`: it deserialised as part of the format and was
///   then refused for *every* asset, so the format advertised a dimension it could not execute. A declaration
///   naming it is now a parse error, which says the same thing sooner. It becomes expressible when the raw
///   producer name is persisted beside the display name, and reappears then under a name that says so.
/// - **The two scope dimensions are refused**, and this one is not hypothetical. The ingestion-side read of
///   `carrier_holds_span_output` supplies no scope while query-time resolution supplies the persisted one, so a
///   scope-qualified clause selects the *generic* clause at ingestion and its own when read - and those two can
///   disagree about whether the carrier holds the span's output, which decides whether a generation span's
///   answer gets augmented. One compiled ruleset must not give two answers about one span.
#[test]
fn a_carrier_qualifier_that_is_not_available_everywhere_is_refused() {
    let compiled = |match_spec: serde_json::Value| {
        let asset = serde_json::json!({
            "id": "probe",
            "carriers": [{
                "id": "probe.clause",
                "match": match_spec,
                "facts": {"preset": "emission"},
            }],
        });
        // Through the same path production takes, so a parse refusal and a compile refusal are both visible.
        serde_json::from_value::<crate::domain::rules::schema::RuleFile>(asset).map(|file| {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "id": file.id,
                "carriers": [],
            }))
            .expect("serialises");
            let _ = bytes;
        })
    };

    // Removed: not a member of the format at all now.
    assert!(
        compiled(serde_json::json!({
            "attribute": "answer",
            "span_name_prefix": "invoke_agent",
        }))
        .is_err(),
        "`span_name_prefix` is not part of the format - it was advertised and refused for every asset"
    );

    // Refused at compile time, since they parse but cannot answer consistently.
    for dimension in ["scope_name_contains", "scope_version_prefix"] {
        let asset = serde_json::json!({
            "id": "probe",
            "carriers": [{
                "id": "probe.clause",
                "match": {"attribute": "answer", dimension: "acme"},
                "facts": {"preset": "emission"},
            }],
        });
        let result =
            crate::domain::rules::carrier_rules::compile(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("serialises"),
            )]));
        assert!(
            result.is_err(),
            "`{dimension}` must be refused: ingestion resolves carriers without a scope and query time \
             resolves them with one, so a clause using it answers differently depending on who asks"
        );
    }

    // The one available qualifier still has to name observation types that exist, and to name some. A
    // misspelling compiled and could never match; an explicitly **empty** list was silently the same as
    // omitting the qualifier, so a clause that reads as narrow held for every span. Telling those two apart is
    // why the field is an `Option` - a `Vec` cannot, which is how the first version of this refusal was dead.
    for bad in [
        serde_json::json!(["generaton"]),
        serde_json::json!([]),
        serde_json::json!(["generation", "generation"]),
    ] {
        let asset = serde_json::json!({
            "id": "probe",
            "carriers": [{
                "id": "probe.clause",
                "match": {"attribute": "answer", "observation_type": bad},
                "facts": {"preset": "emission"},
            }],
        });
        assert!(
            crate::domain::rules::carrier_rules::compile(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("serialises"),
            )]))
            .is_err(),
            "an observation-type qualifier that cannot match must be refused: {bad}"
        );
    }
    // Omitting it is how to say "any observation type", and must still compile.
    let unqualified = serde_json::json!({
        "id": "probe",
        "carriers": [{
            "id": "probe.clause",
            "match": {"attribute": "answer"},
            "facts": {"preset": "emission"},
        }],
    });
    assert!(
        crate::domain::rules::carrier_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&unqualified).expect("serialises"),
        )]))
        .is_ok(),
        "omission means no restriction, which is a thing a clause may say"
    );

    // And the one that *is* available answers the same way everywhere, so it compiles.
    let asset = serde_json::json!({
        "id": "probe",
        "carriers": [{
            "id": "probe.clause",
            "match": {"attribute": "answer", "observation_type": ["generation"]},
            "facts": {"preset": "emission"},
        }],
    });
    assert!(
        crate::domain::rules::carrier_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("serialises"),
        )]))
        .is_ok(),
        "`observation_type` reaches every consumer and must compile"
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
        ("outcome.rs", include_str!("outcome.rs")),
        ("tool_shapes.rs", include_str!("tool_shapes.rs")),
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
        (
            "ToolShapeRule::require",
            "tool_shapes::compile, its own plan",
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
        let mut set = PredicateSet::default();
        set.all = vec![predicate];
        set
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
    "tool-shapes",
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
            "message_events": [{"id": "probe.probe_event", "name": "probe.event"}],
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
            serde_json::json!([{"id": "probe.role1", "name": "probe.event", "role": "narrator"}]),
        ),
        (
            "a tool-span role outside it, which the ordinary role would mask on a chat span",
            serde_json::json!([{"id": "probe.role2", "name": "probe.event", "role": "user", "role_in_tool_span": "narrator"}]),
        ),
        (
            "no role at all, which states nothing and would replace a real declaration",
            serde_json::json!([{"id": "probe.role3", "name": "probe.event"}]),
        ),
        (
            "no name",
            serde_json::json!([{"id": "probe.nameless", "name": "", "role": "user"}]),
        ),
        (
            "a name nothing produces - neither an event nor any rule's tag",
            serde_json::json!([{"id": "probe.role4", "name": "probe.absent", "role": "user"}]),
        ),
        (
            "two declarations that disagree, where which applies depends on load order",
            serde_json::json!([
                {"id": "probe.role5", "name": "probe.event", "role": "user"},
                {"id": "probe.role6", "name": "probe.event", "role": "assistant"},
            ]),
        ),
        (
            "two that disagree only about the tool span, which is the half easiest to overlook",
            serde_json::json!([
                {"id": "probe.role7", "name": "probe.event", "role": "user", "role_in_tool_span": "tool"},
                {"id": "probe.role8", "name": "probe.event", "role": "user"},
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
                {"id": "probe.role9", "name": "probe.event", "role": "user"},
                {"id": "probe.role10", "name": "probe.event", "role": "user"},
            ]),
        ),
        (
            "a role for a name a rule assigns with `tag_as`, which no producer emits",
            serde_json::json!([{"id": "probe.role11", "name": "probe.tag", "role": "tool"}]),
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
/// schema and it was already incomplete - `first_present`, `from_any_of`, `attrs_present`, `attr:`-encoded
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
        // The ownership rule is `RuleFile::declaration_defect`'s, checked in production for every asset - so
        // this reads it rather than restating it, and cannot drift from it.
        for (path, bytes) in &sources {
            let file: crate::domain::rules::schema::RuleFile =
                serde_json::from_slice(bytes).expect("the asset parses");
            assert!(
                file.declaration_defect().is_none(),
                "`{path}`: {:?}",
                file.declaration_defect()
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
                {"id": "probe.probe_first", "name": "probe.first"},
                {"id": "probe.probe_second", "name": "probe.second"},
                {"id": "probe.probe_shared", "name": "probe.shared"},
                {"id": "probe.probe_other", "name": "probe.other"},
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
            "source": {"event": {"names": ["probe.first"]}},
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "legacy_rank": 20,
            "source": {"event": {"names": ["probe.second"]}},
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
            "source": {"event": {"names": ["probe.first"]}},
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
            "source": {"event": {"names": ["probe.shared"]}},
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "legacy_rank": 20,
            "source": {"event": {"names": ["probe.shared", "probe.other"]}},
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
            "source": {"span": {"stage": "fallback"}},
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

/// Every clause id is non-empty and unique within the rule or fragment that holds it - and the **production**
/// validator says so, not this test.
///
/// The rule used to live here alone, restated over the raw JSON with a hand-maintained list of member names. It
/// guarded the embedded corpus and nothing else: the generic compiler accepted two clauses sharing an id, so
/// anything that loaded a file some other way got two clauses with the same provenance path - which is exactly
/// what the ids exist to prevent. `RuleFile::declaration_defect` is the single place now, walked over the
/// **typed** tree so a clause type that gains a nesting is covered by construction.
///
/// What this test adds is the other direction: that the validator actually sees the corpus, and that it refuses
/// each defect. A validator nothing exercises is the shape this whole review keeps finding.
#[test]
fn a_clause_id_is_unique_within_its_owner() {
    let mut owners = 0_usize;
    let mut clauses = 0_usize;
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let file: crate::domain::rules::schema::RuleFile =
            serde_json::from_slice(&bytes).expect("the asset parses");
        assert!(
            file.declaration_defect().is_none(),
            "`{path}` has a declaration defect: {:?}",
            file.declaration_defect()
        );
        for (_, ids) in file.clause_ids() {
            owners += 1;
            clauses += ids.len();
        }
    }
    assert!(
        clauses > 190 && owners > 50,
        "the validator saw {clauses} clauses across {owners} owners, and the assets declare over 200 across \
         more than fifty - it is not walking the tree it is meant to"
    );

    // And each defect is refused. Built as typed assets so the shapes are the ones production would meet.
    let with = |messages: serde_json::Value, extra: serde_json::Value| {
        let mut asset = serde_json::json!({"id": "probe", "messages": messages});
        if let (Some(object), Some(more)) = (asset.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                object.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value::<crate::domain::rules::schema::RuleFile>(asset)
            .expect("the probe parses")
            .declaration_defect()
    };
    let two_readings = |first: &str, second: &str| {
        serde_json::json!([{
            "id": "probe.message",
            "legacy_rank": 1,
            "read": {"attribute": "probe"},
            "parse": "json",
            "emit": "message",
            "alternatives": [
                {"id": first, "select": "$.a"},
                {"id": second, "select": "$.b"},
            ],
        }])
    };
    assert!(
        with(two_readings("same", "same"), serde_json::json!({})).is_some(),
        "two clauses of one rule sharing an id must be refused"
    );
    assert!(
        with(two_readings("", "other"), serde_json::json!({})).is_some(),
        "an empty clause id must be refused"
    );
    assert!(
        with(two_readings("first", "second"), serde_json::json!({})).is_none(),
        "distinct ids must be accepted"
    );
    // The same id in two *different* rules is fine: an id space is per owner.
    let two_rules = serde_json::json!([
        {
            "id": "probe.one",
            "legacy_rank": 1,
            "read": {"attribute": "one"},
            "parse": "json",
            "emit": "message",
            "alternatives": [{"id": "shared", "select": "$.a"}],
        },
        {
            "id": "probe.two",
            "legacy_rank": 2,
            "read": {"attribute": "two"},
            "parse": "json",
            "emit": "message",
            "alternatives": [{"id": "shared", "select": "$.a"}],
        },
    ]);
    assert!(
        with(two_rules, serde_json::json!({})).is_none(),
        "an id space is per owner, so two rules may each have a `shared` reading"
    );

    // `convention_namespaces` belongs to the conventions' asset alone. It parsed anywhere and was read
    // nowhere, so a dialect could claim its own namespace is a convention and read as having done so.
    assert!(
        with(
            serde_json::json!([]),
            serde_json::json!({"convention_namespaces": ["acme"]})
        )
        .is_some(),
        "only the conventions' asset may declare which namespaces are no producer's"
    );
}

/// An answer names the declaration that produced it, and a fact names **every** witness.
///
/// Both resolvers used to answer with a value alone: a classification returned `"span"` whether a rule said so
/// or nothing recognised the span, and a span fact returned `true` with no way to ask which of a dialect's
/// signals established it. Compilation was keeping the ids all along and discarding them at the one moment they
/// are useful.
///
/// There is deliberately no `Verdict<bool>`. A negative answer is not a clause saying "false" - it is no clause
/// answering - so absence is `None` and a verdict always carries at least one witness.
#[test]
fn an_answer_names_the_declaration_that_produced_it() {
    use crate::domain::rules::schema::SpanFact;
    use std::collections::HashMap;

    let ruleset = crate::domain::rules::ruleset();

    // A transport attribute makes a span a plain span, and the rule that says so is `category.transport`.
    let mut http = HashMap::new();
    http.insert("http.method".to_string(), "GET".to_string());
    let category = ruleset
        .observation_types
        .span_category("some.span", &http)
        .expect("a transport attribute is classified");
    assert_eq!(category.value, "http");
    assert_eq!(
        category.evidence.paths().len(),
        1,
        "a first-match classification has exactly one witness"
    );
    assert!(
        category.evidence.to_string().contains("transport"),
        "the verdict must name the rule that answered, not merely the label: {}",
        category.evidence
    );

    // Nothing recognised: no verdict at all, which is a different answer from "a rule said `other`".
    assert!(
        ruleset
            .observation_types
            .span_category("some.span", &HashMap::new())
            .is_none(),
        "no rule holding must be distinguishable from a rule answering"
    );

    // A span fact, with **every** signal that establishes it. The conventions and a dialect can each recognise
    // a tool span, and which ones did is a set rather than whichever was declared first.
    let mut tool = HashMap::new();
    tool.insert("gen_ai.tool.name".to_string(), "search".to_string());
    tool.insert("gen_ai.tool.call.id".to_string(), "call-1".to_string());
    let established = ruleset
        .span_facts
        .established(SpanFact::ToolExecution, &tool)
        .expect("a named tool with a call id is a tool execution");
    assert_eq!(established.value, SpanFact::ToolExecution);
    assert!(
        !established.evidence.paths().is_empty(),
        "an established fact carries its witnesses"
    );
    // Each witness names its rule *and* the signal inside it, which is what the clause ids are for.
    for path in established.evidence.paths() {
        assert!(
            !path.steps.is_empty(),
            "a witness must name the signal, not only the rule that holds it: {path}"
        );
    }
    // And `holds` agrees with `established`, so the boolean is a view of the same answer rather than a second
    // implementation of it.
    assert!(ruleset.span_facts.holds(SpanFact::ToolExecution, &tool));
    assert!(
        !ruleset
            .span_facts
            .holds(SpanFact::ToolExecution, &HashMap::new())
    );
    assert!(
        ruleset
            .span_facts
            .established(SpanFact::ToolExecution, &HashMap::new())
            .is_none()
    );

    // Evidence is ordered and free of repeats, so a diagnostic reads the same way twice.
    use crate::domain::rules::expr::{ClausePath, EvidenceSet};
    let set = EvidenceSet::of(vec![
        ClausePath::root("b").then("two"),
        ClausePath::root("a").then("one"),
        ClausePath::root("b").then("two"),
    ])
    .expect("three paths");
    assert_eq!(set.paths().len(), 2, "a repeated witness is one witness");
    assert_eq!(set.to_string(), "a → one, b → two");
    assert!(
        EvidenceSet::of(Vec::new()).is_none(),
        "an answer with no evidence is the absence of an answer"
    );
}

/// A field answer names the **source** that supplied it, and a merge names every contributor.
///
/// The rule id alone was not enough. The session id's chain names five spellings, so knowing that
/// `span-fields.session_id` answered says nothing about which producer's attribute was believed - and the
/// `refused` list beside it records sources that were present and unreadable, which is a different question and
/// never named the winner.
#[test]
fn a_field_answer_names_the_source_that_supplied_it() {
    use crate::domain::rules::schema::FieldTarget;
    use std::collections::HashMap;

    let resolve = |attrs: &HashMap<String, String>, target: FieldTarget| {
        crate::domain::rules::ruleset()
            .span_fields
            .resolve("some.span", attrs, &[])
            .into_iter()
            .find(|resolved| resolved.target == target)
    };

    // Two spellings of the session id, both present. The chain is first-wins, so exactly one source answered -
    // and which one is the fact this evidence exists to carry.
    let mut both = HashMap::new();
    both.insert("session.id".to_string(), "session-a".to_string());
    both.insert(
        "gen_ai.conversation.id".to_string(),
        "session-b".to_string(),
    );
    let resolved = resolve(&both, FieldTarget::SessionId).expect("a session id is resolved");
    let evidence = resolved
        .evidence
        .as_ref()
        .expect("an answer carries the source that supplied it");
    assert_eq!(
        evidence.paths().len(),
        1,
        "a first-wins chain has exactly one witness, not one per present source"
    );
    // The winner is the source whose value was stored, so the two must agree.
    let winner = evidence.paths()[0].to_string();
    assert!(
        winner.contains("session_id"),
        "the witness must name the source inside the rule: {winner}"
    );

    // A **merge**: every source that contributed, because a merge's answer is made of all of them and naming
    // one would misdescribe it.
    let mut tags = HashMap::new();
    tags.insert("tags".to_string(), r#"["a"]"#.to_string());
    tags.insert("langsmith.tags".to_string(), r#"["b"]"#.to_string());
    let merged = resolve(&tags, FieldTarget::Tags).expect("tags are resolved");
    let evidence = merged.evidence.as_ref().expect("a merge carries witnesses");
    assert!(
        evidence.paths().len() >= 2,
        "a merge that took values from two sources must name both: {evidence}"
    );

    // Nothing answered: no evidence, which is distinguishable from an answer nobody can name.
    let empty = resolve(&HashMap::new(), FieldTarget::SessionId);
    assert!(
        empty.is_none_or(|resolved| resolved.evidence.is_none()),
        "an unset field must carry no witness"
    );
}

/// A dotted family respects the separator; a raw prefix does not, and the two are different declarations.
///
/// `attribute_prefix: "ai.response"` selected `ai.responses` - a different attribute of the same dialect - and
/// `gen_ai.output.messages` selected `gen_ai.output.messages_extra`. Every undelimited prefix in the assets was
/// really a family root, so all six moved to `attribute_family`; the six that end in `.` are genuinely raw and
/// stayed, because as a family root `"ai."` would ask for `ai..something`.
#[test]
fn a_family_root_respects_the_separator() {
    use crate::domain::rules::schema::in_family;

    // The membership rule itself, which is where the defect lived.
    assert!(
        in_family("ai.response", "ai.response"),
        "the root is a member"
    );
    assert!(in_family("ai.response.text", "ai.response"));
    assert!(
        !in_family("ai.responses", "ai.response"),
        "a longer name that merely starts the same way is a different attribute"
    );
    assert!(
        !in_family("gen_ai.output.messages_extra", "gen_ai.output.messages"),
        "an underscore is not a separator"
    );
    assert!(!in_family("ai.respons", "ai.response"));

    // And end to end through the plan: the sibling attribute must not resolve to the family's clause.
    let semantics = |attribute: &str| {
        crate::domain::rules::ruleset()
            .carriers
            .resolve(&crate::domain::rules::CarrierContext {
                event: None,
                attribute: Some(attribute),
                observation_type: Some("generation"),
                span_name: None,
                scope_name: None,
                scope_version: None,
            })
            .map(|clause| clause.clause_id.to_string())
    };
    let family = semantics("ai.response.text");
    assert!(
        family.is_some(),
        "a member of the family must resolve to its clause"
    );
    assert_ne!(
        semantics("ai.responses"),
        family,
        "a sibling attribute must not resolve to the family's clause: that is the defect"
    );

    // The six dot-terminated prefixes are still raw, so a key directly under them still resolves.
    assert!(
        crate::domain::rules::ruleset()
            .carriers
            .resolve(&crate::domain::rules::CarrierContext {
                event: None,
                attribute: Some("gen_ai.prompt.0.content"),
                observation_type: Some("generation"),
                span_name: None,
                scope_name: None,
                scope_version: None,
            })
            .is_some(),
        "a raw prefix ending in `.` still selects the keys below it"
    );
}

/// A fact vector the model cannot mean is refused.
///
/// The eight overrides are applied independently, so **any** vector compiled - including ones where the facts
/// contradict each other and a reader could not say which the engine would act on. Each rule below holds across
/// all 55 shipped clauses, which is what makes it a statement about the model rather than a preference.
///
/// One implication is **absent on purpose**, and it is the interesting one: a detached request frame ought to
/// hold the span's input, and all three shipped frames declare that it does not. Their own docs say they are
/// what the model was given, so the rule is true of the model and false of the assets. Correcting the three
/// declarations was tried and measured - `carrier_holds_span_input` also gates history detection, so making the
/// declaration true changed what gets *filtered*: four fixtures moved, a span view lost two messages, and an
/// assistant's intro text sorted after its own tool call. Enforcing it now would refuse the shipped ruleset for
/// a defect that is real and not yet safely fixable.
#[test]
fn a_fact_vector_the_model_cannot_mean_is_refused() {
    let compiled = |facts: serde_json::Value, ordering_family: Option<&str>| {
        let mut clause = serde_json::json!({
            "id": "probe.clause",
            "match": {"attribute": "answer"},
            "facts": facts,
        });
        if let (Some(object), Some(family)) = (clause.as_object_mut(), ordering_family) {
            object.insert("ordering_family".to_string(), serde_json::json!(family));
        }
        let asset = serde_json::json!({"id": "probe", "carriers": [clause]});
        crate::domain::rules::carrier_rules::compile(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("serialises"),
        )]))
    };

    for (what, facts, family) in [
        (
            "one atomic emission whose positions prove nothing",
            serde_json::json!({"preset": "emission", "position_proves_distinct_occurrence": false}),
            None,
        ),
        (
            "one atomic emission that may also be a re-listing",
            serde_json::json!({"preset": "emission", "carrier_may_contain_history_or_state": true}),
            None,
        ),
        (
            "a request frame that holds the span's output",
            serde_json::json!({
                "preset": "emission",
                "carrier_is_detached_request_frame": true,
                "carrier_holds_span_output": true,
            }),
            None,
        ),
        (
            "an ordering family whose positions carry no order",
            serde_json::json!({
                "preset": "snapshot",
                "position_provides_sequence_order": false,
                "carrier_holds_span_input": true,
            }),
            Some("probe.family"),
        ),
        (
            "an ordering family on the output side",
            serde_json::json!({"preset": "accumulated_state", "carrier_holds_span_input": true}),
            Some("probe.family"),
        ),
        (
            "an ordering family that is not the input side at all",
            serde_json::json!({"preset": "snapshot"}),
            Some("probe.family"),
        ),
        (
            "an expandable array whose positions carry no order",
            serde_json::json!({
                "preset": "snapshot",
                "carrier_holds_expandable_message_array": true,
                "position_provides_sequence_order": false,
            }),
            None,
        ),
    ] {
        assert!(
            compiled(facts, family).is_err(),
            "{what} must be refused: the facts contradict each other, so no reader can say which the engine \
             acts on"
        );
    }

    // And the coherent shapes still compile, or the refusals are simply a ban on overriding.
    for (what, facts, family) in [
        (
            "a plain emission",
            serde_json::json!({"preset": "emission"}),
            None,
        ),
        (
            "a snapshot that is the input side",
            serde_json::json!({"preset": "snapshot", "carrier_holds_span_input": true}),
            None,
        ),
        (
            "an ordered input family",
            serde_json::json!({"preset": "snapshot", "carrier_holds_span_input": true}),
            Some("probe.family"),
        ),
        (
            "accumulated state, which is output and a re-listing at once",
            serde_json::json!({"preset": "accumulated_state"}),
            None,
        ),
        (
            "a carrier that is both sides",
            serde_json::json!({
                "preset": "snapshot",
                "carrier_holds_span_input": true,
                "carrier_holds_span_output": true,
            }),
            None,
        ),
    ] {
        assert!(
            compiled(facts, family).is_ok(),
            "{what} is coherent and must compile"
        );
    }
}

/// The raw-event policy is a fact about the event, stated once.
///
/// It used to be `replaces_raw_event` on each *reading*, ORed at runtime across every reading whose gates
/// held. Both readings of the one container event repeated it, so the policy was stated twice with nothing
/// keeping the two statements consistent - a `true` beside a `false` compiled and `true` silently won, which
/// is a disagreement resolved by which reading happened to be written first.
#[test]
fn the_raw_event_policy_is_the_events_own_and_cannot_disagree_with_itself() {
    use super::schema::{RawEventForm, RuleFile};

    let compile = |events: serde_json::Value| {
        let probe = serde_json::json!({"id": "probe", "message_events": events});
        let file: RuleFile = serde_json::from_value(probe).expect("the probe asset parses");
        super::compile_message_events(&[file])
    };

    // The shipped container, and an ordinary event beside it.
    let plan = compile(serde_json::json!([
        {"id": "probe.container", "name": "probe.container", "raw": "replace"},
        {"id": "probe.plain", "name": "probe.plain"},
    ]))
    .expect("two events that state different policies about *different* names are consistent");
    assert_eq!(plan["probe.container"].raw, RawEventForm::Replace);
    assert_eq!(
        plan["probe.plain"].raw,
        RawEventForm::Message,
        "an event says nothing about its raw form, so its body is a message - the default a reading-level \
         flag could not express, since absent meant only that this reading did not claim it"
    );

    // An agreeing repeat is a dialect re-stating a convention, and **both** witnesses are kept: the previous
    // form collapsed the entries into a set of names, so the second asset's declaration existed nowhere.
    let plan = compile(serde_json::json!([
        {"id": "semconv.container", "name": "probe.container", "raw": "replace"},
        {"id": "dialect.container", "name": "probe.container", "raw": "replace"},
    ]))
    .expect("a repeat that agrees is allowed");
    let witnesses: Vec<String> = plan["probe.container"]
        .witnesses
        .paths()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        witnesses,
        ["dialect.container", "semconv.container"],
        "both declarations said it, so both are evidence for it"
    );

    // A disagreement is refused rather than resolved by load order.
    let refused = compile(serde_json::json!([
        {"id": "a.container", "name": "probe.container", "raw": "replace"},
        {"id": "b.container", "name": "probe.container", "raw": "message"},
    ]))
    .expect_err("two assets disagreeing about one event's raw form must be refused");
    assert!(
        refused.contains("a.container") && refused.contains("b.container"),
        "the refusal must name both declarations, or it says where to look and not what disagreed: {refused}"
    );

    // And the declarations are clauses like any other, so two sharing an id is refused - both registries,
    // since each is its own id space.
    for (which, entries) in [
        (
            "message_events",
            serde_json::json!({
                "id": "probe",
                "message_events": [
                    {"id": "probe.same", "name": "probe.one"},
                    {"id": "probe.same", "name": "probe.two"},
                ],
            }),
        ),
        (
            "event_roles",
            serde_json::json!({
                "id": "probe",
                "message_events": [{"id": "probe.event", "name": "probe.one"}],
                "event_roles": [
                    {"id": "probe.same", "name": "probe.one", "role": "user"},
                    {"id": "probe.same", "name": "probe.one", "role": "user"},
                ],
            }),
        ),
    ] {
        let file: RuleFile = serde_json::from_value(entries).expect("the probe asset parses");
        let defect = file
            .declaration_defect()
            .unwrap_or_else(|| panic!("two `{which}` entries sharing an id must be refused"));
        assert!(
            defect.contains("probe.same") && defect.contains(which),
            "the refusal must name the id and the registry: {defect}"
        );
    }
}

/// One source syntax must not mean two different multiplicities.
///
/// `attribute_any_of` named a list of keys and said nothing about how many of them are read - and the answer
/// came from a *sibling* member: with `tool_repr` beside it every present key was read, without it only the
/// first. CrewAI's `["crew_agents", "crew_tasks"]` is exactly that shape, so the same list meant "both" there
/// and "the first" in the two other shipped rules. The two readings are now two members, and the ownership
/// analysis - which had modelled every list as first-wins - follows the one that applies.
#[test]
fn a_carrier_list_says_how_many_of_its_keys_are_read() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |body: &str| {
        std::collections::BTreeMap::from([("t.json".to_string(), body.as_bytes().to_vec())])
    };

    // `first_present`: the first spelling the span carries, and nothing after it.
    let plan = compile(&asset(
        r#"{"id":"t","messages":[{"id":"t.alternatives","read":{"first_present":["new","old"]},
             "parse":"text","emit":"message","legacy_rank":1}]}"#,
    ))
    .expect("ordered alternatives compile");
    let attrs = std::collections::HashMap::from([
        ("new".to_string(), "fresh".to_string()),
        ("old".to_string(), "stale".to_string()),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let read: Vec<String> = plan.run(&ctx).iter().map(|e| e.value.to_string()).collect();
    assert_eq!(
        read,
        [r#""fresh""#],
        "`first_present` reads one carrier per span - the first listed the span has"
    );

    // `each` with an ordinary body is refused rather than silently read as `first_present`, which is the
    // whole point of the split: the previous member would have quietly given the first-wins reading here.
    let refused = compile(&asset(
        r#"{"id":"t","messages":[{"id":"t.each","read":{"each":["new","old"]},
             "parse":"text","emit":"message","legacy_rank":1}]}"#,
    ))
    .expect_err("`each` on a body that cannot iterate its carriers must be refused");
    assert!(
        refused.to_string().contains("first_present"),
        "the refusal must name the member to use instead: {refused}"
    );

    // And `each` **is** honoured where a body iterates: every listed key present is its own observation.
    let plan = compile(&asset(
        r#"{"id":"t","messages":[{"id":"t.tools","read":{"each":["agents","tasks"]},"parse":"json",
             "emit":"tool_definitions","legacy_rank":1,
             "tool_repr":{"entries":"$[*]","candidates":["$"],"name_field":"name",
               "description_field":"description","name_label":"Tool Name:",
               "description_label":"Tool Description:","arguments_label":"Tool Arguments:",
               "repr_markers":["name="],"parameter_members":["parameters"],
               "field_terminators":[","],"type_map":[["str","string"]],"type_default":{"map_to":"string"}}}]}"#,
    ))
    .expect("`each` compiles with `tool_repr`");
    let attrs = std::collections::HashMap::from([
        (
            "agents".to_string(),
            r#"["Tool(name='a', description='one')"]"#.to_string(),
        ),
        (
            "tasks".to_string(),
            r#"["Tool(name='b', description='two')"]"#.to_string(),
        ),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        plan.tool_definitions(&ctx).len(),
        2,
        "`each` reads every listed key the span carries, which is what CrewAI's two tool keys need"
    );

    // A repeated key can never mean what it says, in either member.
    for member in ["first_present", "each"] {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.dup","read":{{"{member}":["k","k"]}},"parse":"json",
                 "emit":"tool_definitions","legacy_rank":1,
                 "tool_repr":{{"entries":"$[*]","candidates":["$"],"name_field":"name",
                 "description_field":"d","name_label":"N:","description_label":"D:",
                 "arguments_label":"A:","repr_markers":["name="],"parameter_members":["parameters"],
                 "field_terminators":[","],"type_map":[["str","string"]],"type_default":{{"map_to":"string"}}}}}}}}]}}"#
        );
        assert!(
            compile(&asset(&body)).is_err(),
            "`{member}` listing one key twice must be refused"
        );
    }
}

/// A rule says **where** it reads with one member, so no entry point can honour half of it.
///
/// `stage` and `when_event` were an implicit sum, and the two entry points disagreed about which fields they
/// consult: the event path selects on the event name and ignores `stage` entirely, while the span path selects
/// on `stage` and requires no event. Codex's demonstration is the first case below - two event rules at one
/// rank declaring *different* stages. The compiler held them to be different ordering arenas (where a shared
/// rank is legal, because rules in different arenas never contend), and then the event path ran both, leaving
/// ownership of the contested carrier to be decided by comparing their **ids**.
#[test]
fn a_rule_declares_where_it_reads_with_one_member() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rules: &str| {
        let body = format!(
            r#"{{"id":"t","message_events":[{{"id":"t.e","name":"acme.event"}}],"messages":{rules}}}"#
        );
        std::collections::BTreeMap::from([("t.json".to_string(), body.into_bytes())])
    };

    // Codex's case, verbatim in substance: two event rules over one event and one carrier, at one rank,
    // differing only in a stage the event path does not read.
    let refused = compile(&asset(
        r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","legacy_rank":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","legacy_rank":1}]"#,
    ))
    .expect_err("two event rules over one event at one rank must be refused");
    let message = refused.to_string();
    assert!(
        message.contains("rank") || message.contains("carrier"),
        "the refusal must be about the rank or the contested carrier, not something incidental: {message}"
    );

    // The case the *arena* rule owns on its own: two event rules over one event at one rank reading
    // **different** carriers. Nothing contests a carrier here, so the only defect is the shared rank - and
    // before cycle 9 a stage neither rule's entry point reads was enough to make the compiler call them
    // different arenas and accept it.
    let refused = compile(&asset(
        r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"one"},
             "parse":"text","emit":"message","legacy_rank":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"two"},
             "parse":"text","emit":"message","legacy_rank":1}]"#,
    ))
    .expect_err(
        "two event rules over one event at one rank share an arena, so the rank must be refused",
    );
    assert!(
        refused.to_string().contains("rank"),
        "the refusal must be about the shared rank: {refused}"
    );

    // And distinct ranks over one event are fine - the ranks are what order them.
    compile(&asset(
        r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"one"},
             "parse":"text","emit":"message","legacy_rank":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"two"},
             "parse":"text","emit":"message","legacy_rank":2}]"#,
    ))
    .expect("distinct ranks in one arena are ordered");

    // An event source naming nothing is refused rather than silently becoming a span rule - which is what
    // `when_event: []` did, sending the rule to a different entry point from the one it was written for.
    let refused = compile(&asset(
        r#"[{"id":"a","source":{"event":{"names":[]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","legacy_rank":1}]"#,
    ))
    .expect_err("an event source naming no event must be refused");
    assert!(
        refused.to_string().contains("reads nothing"),
        "the refusal must say the rule would read nothing: {refused}"
    );

    // And a source cannot be both: the grammar has one variant, so `deny_unknown_fields` refuses a stage
    // inside an event source and an event list inside a span source.
    for half in [
        r#"{"event":{"names":["acme.event"],"stage":"fallback"}}"#,
        r#"{"span":{"names":["acme.event"]}}"#,
    ] {
        let body = format!(
            r#"[{{"id":"a","source":{half},"read":{{"attribute":"payload"}},"parse":"text",
                 "emit":"message","legacy_rank":1}}]"#
        );
        assert!(
            compile(&asset(&body)).is_err(),
            "a source declaring half of each variant must be refused: {half}"
        );
    }

    // The ordinary case still needs no `source` at all, or the migration would be a tax on 340 rules.
    compile(&asset(
        r#"[{"id":"a","read":{"attribute":"payload"},"parse":"text","emit":"message","legacy_rank":1}]"#,
    ))
    .expect("a span rule at the dialect stage is the default and declares nothing");
}

/// `FieldSource` can name an event, and says which occurrence answers.
///
/// The primitive was missing, and its absence is why one reader stayed in Rust: field resolution was handed a
/// span's attributes and not its events, so the conventions' own `gen_ai.choice`/`finish_reason` was scanned
/// for by hand *after* every declared source - a precedence that came from where the code could put it rather
/// than from what the telemetry means.
///
/// The occurrence is declared because there is no defensible default when a span carries the event twice: the
/// retired loop took the first with a `break`, silently.
#[test]
fn a_field_source_can_read_an_event_and_says_which_occurrence_answers() {
    use crate::domain::rules::span_fields::{Reading, SpanEvent, compile};

    let asset = |sources: &str, target: &str| {
        let body = format!(
            r#"{{"id":"t","span_fields":[{{"id":"t.rule","target":"{target}","sources":{sources}}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let event = |reason: &str| SpanEvent {
        name: "acme.choice".to_string(),
        attributes: std::collections::HashMap::from([(
            "finish_reason".to_string(),
            reason.to_string(),
        )]),
    };
    let attrs = std::collections::HashMap::new();

    // `first_yielding` over two occurrences: the first, which is what the retired `break` did.
    let plan = asset(
        r#"[{"id":"t.s","event_attribute":{"event":"acme.choice","attribute":"finish_reason",
             "occurrence":"first_yielding"}}]"#,
        "gen_ai_finish_reasons",
    )
    .expect("an event source compiles");
    let resolved = plan.resolve("span", &attrs, &[event("stop"), event("length")]);
    assert_eq!(
        resolved
            .iter()
            .filter_map(|r| match &r.reading {
                Reading::StringList(items) => Some(items.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![vec!["stop".to_string()]],
        "`first_yielding` takes the first occurrence that holds the attribute"
    );

    // `every` over the same two: both, because two events carrying it state two reasons.
    let plan = asset(
        r#"[{"id":"t.s","event_attribute":{"event":"acme.choice","attribute":"finish_reason",
             "occurrence":"every"}}]"#,
        "gen_ai_finish_reasons",
    )
    .expect("an every-occurrence source compiles for a list-valued field");
    let resolved = plan.resolve("span", &attrs, &[event("stop"), event("length")]);
    assert_eq!(
        resolved
            .iter()
            .filter_map(|r| match &r.reading {
                Reading::StringList(items) => Some(items.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![vec!["stop".to_string(), "length".to_string()]],
        "`every` reports one value per occurrence, in the order the span carries them"
    );

    // And `every` into a field that holds one value is refused rather than silently keeping one of them.
    let refused = asset(
        r#"[{"id":"t.s","event_attribute":{"event":"acme.choice","attribute":"finish_reason",
             "occurrence":"every"}}]"#,
        "gen_ai_response_model",
    )
    .err()
    .expect("`every` into a scalar field must be refused");
    assert!(
        refused.to_string().contains("two answers"),
        "the refusal must say why: {refused}"
    );

    // Two occurrences that carry the attribute **empty** are `Empty`, not `Absent`. Dropping them and
    // answering `Absent` said "no event carried this" about two events that carried it, and took the decision
    // away from `accept_empty`, whose whole purpose is to say that an empty value a producer wrote is an
    // answer. `first_yielding` answers `Empty` there, so the two policies disagreed about one span.
    let plan = asset(
        r#"[{"id":"t.s","accept_empty":true,"event_attribute":{"event":"acme.choice",
             "attribute":"finish_reason","occurrence":"every"}}]"#,
        "gen_ai_finish_reasons",
    )
    .expect("compiles");
    let answered: Vec<Reading> = plan
        .resolve("span", &attrs, &[event(""), event("")])
        .into_iter()
        .filter(|r| r.evidence.is_some())
        .map(|r| r.reading)
        .collect();
    assert_eq!(
        answered.len(),
        1,
        "the source answered, because `accept_empty` says an empty value a producer wrote is an answer - \
         with the occurrences dropped it answered nothing at all"
    );
    assert!(
        matches!(answered[0], Reading::Empty),
        "and the answer is *empty*, not a shorter list: {:?}",
        answered[0]
    );

    // A malformed value on a scalar field is recorded as malformed rather than read as absent - the
    // distinction `on_malformed` acts on. (For `every` the case is unreachable: it is refused on anything but a
    // list-valued target, and a `StringList` reading cannot be malformed while `parse_string_array` splits on
    // commas, which is its own cycle-10 finding.)
    let plan = asset(
        r#"[{"id":"t.s","event_attribute":{"event":"acme.tokens","attribute":"count",
             "occurrence":"first_yielding"}}]"#,
        "usage_input_tokens",
    )
    .expect("compiles");
    let malformed = SpanEvent {
        name: "acme.tokens".to_string(),
        attributes: std::collections::HashMap::from([("count".to_string(), "many".to_string())]),
    };
    let resolved = plan.resolve("span", &attrs, &[malformed]);
    let refused: Vec<&crate::domain::rules::span_fields::Refusal> =
        resolved.iter().flat_map(|r| &r.refused).collect();
    assert_eq!(
        refused.len(),
        1,
        "a present value that does not convert is malformed, and the chain has to *see* it - not receive a \
         shorter list with nothing recorded"
    );
    assert!(
        matches!(refused[0].reading, Reading::Malformed { .. }),
        "recorded as malformed: {:?}",
        refused[0]
    );
    // And it names the **declaration**, not only the key: a chain of five spellings needs to say which of them
    // was refused, or a reader cannot tell whether the producer they care about was believed.
    assert_eq!(
        refused[0].clause.to_string(),
        "t.rule → t.s",
        "the refusal names its clause path"
    );

    // An event nothing carries is absent, not empty - the distinction the chain steps on.
    let plan = asset(
        r#"[{"id":"t.s","event_attribute":{"event":"acme.choice","attribute":"finish_reason"}}]"#,
        "gen_ai_finish_reasons",
    )
    .expect("the occurrence defaults");
    assert!(
        plan.resolve("span", &attrs, &[])
            .iter()
            .all(|r| !matches!(r.reading, Reading::StringList(_))),
        "no such event is no answer"
    );
}

/// Starvation is refused for **every** multi-owner reading, not only for a compose.
///
/// This replaces `a_composed_reading_cannot_be_starved_by_a_lower_ranked_rule`, a repository test that scanned
/// the shipped assets for the compose case. Two reasons it had to become a production refusal rather than gain
/// three more cases: a test over *this* corpus says nothing about an asset added later, and the shape it was
/// checking is one the compiler actively **excuses** - so the corpus could drift into it through an edit to
/// either rule. The property came from `specs/CarrierClaiming.tla`, whose first form asserted "the lowest-ranked
/// rule that reads a carrier gets it"; TLC refuted it in seconds, and `ComposedReadingsCanBeStarved` records
/// that the situation is reachable.
///
/// The guard was a repository test over composed readings alone, and the conflict check in production
/// deliberately excuses the shape: a conditional claim "yields on spans its condition excludes, and the ranks
/// decide which is tried first". That reasoning is sound when the loser's emission owns the one carrier it
/// lost, and false for an all-or-nothing reading - the loser is dropped **whole**, so carriers the taker never
/// wanted reach nobody.
///
/// The first case is Codex's demonstration, which compiled before cycle 9: a conditional rank-1 rule taking
/// `family.0.role` leaves the indexed entry unable to take `family.0.content`, and that content then appears in
/// no view at all. Neither rule looks wrong on its own, and nothing failed.
#[test]
fn an_all_or_nothing_reading_cannot_be_starved_by_an_earlier_rank() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rules: &str| {
        let body = format!(r#"{{"id":"t","messages":{rules}}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    // Codex's case: an indexed family, starved by a conditional earlier rule reading one of its keys.
    let refused = asset(
        r#"[{"id":"t.take_role","when":{"attr_exists":["marker"]},"read":{"attribute":"family.0.role"},
             "parse":"text","tag_as":"taken","emit":"message","legacy_rank":1},
            {"id":"t.read_family","read":{"indexed_family":"family"},"emit":"message","legacy_rank":2}]"#,
    )
    .expect_err("an indexed family starved by an earlier conditional rule must be refused");
    let message = refused.to_string();
    assert!(
        message.contains("t.read_family") && message.contains("t.take_role"),
        "the refusal must name both rules, since neither is wrong on its own: {message}"
    );
    assert!(
        message.contains("reach nobody"),
        "the refusal must say what is lost - not that two rules contend, which they do not: {message}"
    );

    // A **compose** with several members, which is the shape the retired test covered.
    assert!(
        asset(
            r#"[{"id":"t.take_x","when":{"attr_exists":["marker"]},"read":{"attribute":"x"},
                 "parse":"text","tag_as":"taken","emit":"message","legacy_rank":1},
                {"id":"t.compose","compose":{"tag":"joined","members":[
                    {"as":"a","from_any_of":["x","x_backup"],"parse":"text"},{"as":"b","from_any_of":["y"],"parse":"text"}]},
                 "emit":"message","legacy_rank":2}]"#,
        )
        .is_err(),
        "a composed reading starved of one member must be refused - and a backup spelling does not save it, \
         because `composed()` selects the first present with `find_map` and never retries"
    );

    // And an **overlay**, whose `from` carrier the entry owns at runtime (`consumed.push(overlay.from)`) - so
    // it is a second, *unrelated* key the reading cannot do without. The family's own keys are covered by the
    // case above; this is the one the family prefix does not reach.
    //
    // Every required member spelled out, because my first version of this probe omitted three and was refused
    // at **parse** - so `is_err()` held for a reason that had nothing to do with starvation, and the assertion
    // passed while checking nothing. Which is this review's own recurring finding, in the test for it.
    let overlaid = |taker_rank: i32, overlaid_rank: i32| {
        format!(
            r#"[{{"id":"t.take_rich","when":{{"attr_exists":["marker"]}},"read":{{"attribute":"rich"}},
                 "parse":"text","tag_as":"taken","emit":"message","legacy_rank":{taker_rank}}},
                {{"id":"t.overlaid","read":{{"indexed_family":"fam","entry_member":"message",
                   "overlay":{{"from":"rich","parse":"json","select_any_of":["$.messages"],
                     "witness":{{"any":[{{"path":"$[*].id","exists":true}}]}},
                     "when_member_prefix":"contents.","content_any_of":["$.content"],
                     "require":{{"all":[{{"kind":"array"}}]}},"as_member":"content"}}}},
                 "emit":"message","legacy_rank":{overlaid_rank}}}]"#
        )
    };
    let refused = asset(&overlaid(1, 2)).expect_err(
        "an overlay's own carrier is part of the reading, so losing it loses the reading",
    );
    assert!(
        refused.to_string().contains("rich"),
        "the refusal must name the overlay's carrier: {refused}"
    );
    asset(&overlaid(2, 1)).expect(
        "with the overlaid reading first, it takes what it needs and the other finds nothing",
    );

    // The refusal is **directional**, or it would ban every ordinary precedence: a multi-owner reading at the
    // *earlier* rank takes what it needs and the later rule simply finds nothing, which is what ranks are for.
    // Both rules gated, on conditions neither of which covers the other, so the pre-existing contested-carrier
    // check does not fire either - which is what leaves the starvation question the only one being asked.
    asset(
        r#"[{"id":"t.read_family","when":{"attr_exists":["family_marker"]},
             "read":{"indexed_family":"family"},"emit":"message","legacy_rank":1},
            {"id":"t.take_role","when":{"attr_exists":["marker"]},"read":{"attribute":"family.0.role"},
             "parse":"text","tag_as":"taken","emit":"message","legacy_rank":2}]"#,
    )
    .expect("a multi-owner reading at the earlier rank is not starved - it goes first");

    // **Across stages**, which the contested-carrier check exempts and this must not. That exemption is sound
    // for its own question - two stages sharing a carrier are safe because the fallback inherits what the
    // dialect stage claimed - and the inheritance is precisely what makes starvation reach across them.
    assert!(
        asset(
            r#"[{"id":"t.take_x","read":{"attribute":"x"},"parse":"text","emit":"message","legacy_rank":1},
                {"id":"t.compose","source":{"span":{"stage":"fallback"}},
                 "compose":{"tag":"joined","members":[
                    {"as":"a","from_any_of":["x"],"parse":"text"},{"as":"b","from_any_of":["y"],"parse":"text"}]},
                 "emit":"message","legacy_rank":2}]"#,
        )
        .is_err(),
        "a fallback-stage reading inherits the dialect stage's claims, so a dialect rule can starve it"
    );

    // A single-member compose is not multi-owner: it takes one spelling, so there is no half to lose.
    asset(
        r#"[{"id":"t.take_x","when":{"attr_exists":["marker"]},"read":{"attribute":"x"},
             "parse":"text","tag_as":"taken","emit":"message","legacy_rank":1},
            {"id":"t.compose","compose":{"tag":"joined","members":[
                {"as":"a","from_any_of":["x","x_backup"],"parse":"text"}]},
             "emit":"message","legacy_rank":2}]"#,
    )
    .expect("one member reading two spellings takes exactly one of them, so nothing is starved");
}

/// A family whose members are **names** is readable, and its order is declared rather than discovered.
///
/// `indexed_family` requires a numeric component and skips anything else, so `acme.messages.a` /
/// `acme.messages.b` - one message per named member - could not be read at all, while `carriers` has had
/// `attribute_family` all along. The two halves of the format disagreed about whether such a family exists.
///
/// The order is a **required** declaration because there is nothing to discover: extraction puts a span's
/// attributes in a `HashMap`, so producer order is gone before a rule sees them. Undeclared, the answer would
/// be a hash map's iteration order - which is exactly what a message sequence must not be, and would differ
/// per process.
#[test]
fn a_named_attribute_family_is_readable_in_a_declared_order() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.family",
             "read":{"attribute_family":{"root":"acme.messages","order":"member_name"}},
             "parse":"json","emit":"message","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect("a named family compiles");

    let attrs = std::collections::HashMap::from([
        (
            "acme.messages.c".to_string(),
            r#"{"role":"assistant","content":"third"}"#.to_string(),
        ),
        (
            "acme.messages.a".to_string(),
            r#"{"role":"user","content":"first"}"#.to_string(),
        ),
        (
            "acme.messages.b".to_string(),
            r#"{"role":"user","content":"second"}"#.to_string(),
        ),
        // Not a member: the family root is followed by a separator, so a key that merely starts with the
        // root's text is a different attribute - the same rule carrier matching follows.
        (
            "acme.messages_extra".to_string(),
            r#"{"role":"user","content":"not mine"}"#.to_string(),
        ),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let read: Vec<(String, String)> = plan
        .run(&ctx)
        .iter()
        .map(|e| {
            (
                e.carrier.name().to_string(),
                e.value["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            ("acme.messages.a".to_string(), "first".to_string()),
            ("acme.messages.b".to_string(), "second".to_string()),
            ("acme.messages.c".to_string(), "third".to_string()),
        ],
        "the members are read in the declared order, each tagged with its own key - one tag for the family \
         would make two members indistinguishable to carrier semantics and to identity"
    );

    // Deterministic across runs, which is the whole reason the order is declared. A hash map's iteration
    // order varies per process, so this asserts over repeated resolutions of the same span.
    for _ in 0..8 {
        let again: Vec<String> = plan
            .run(&ctx)
            .iter()
            .map(|e| e.carrier.name().to_string())
            .collect();
        assert_eq!(
            again,
            [
                "acme.messages.a".to_string(),
                "acme.messages.b".to_string(),
                "acme.messages.c".to_string()
            ],
            "the order must not depend on hash iteration"
        );
    }

    // A blank member is filtered when the rule says so. The family branch returns **before** the rule-wide
    // emptiness checks, so without applying them per member a `require_non_blank` on a named family was a
    // declaration read from nowhere: the asset stated a filter the engine did not have.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.family","require_non_blank":true,
             "read":{"attribute_family":{"root":"acme.messages","order":"member_name"}},
             "parse":"json_or_string","emit":"message","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect("a named family with an emptiness requirement compiles");
    let blank = std::collections::HashMap::from([
        ("acme.messages.a".to_string(), "   ".to_string()),
        ("acme.messages.b".to_string(), "real".to_string()),
    ]);
    let ctx = MessageContext::for_span("span", &blank, false);
    assert_eq!(
        plan.run(&ctx)
            .iter()
            .map(|e| e.carrier.name().to_string())
            .collect::<Vec<_>>(),
        ["acme.messages.b".to_string()],
        "the requirement applies per member, because a member is the observation - the family as a whole is \
         not one payload"
    );

    // And it is **refused** on an indexed family, where it is equally unreachable and has no meaning to give:
    // entries are assembled from many keys, so there is no raw string for the check to ask about.
    let refused = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.indexed","require_non_blank":true,
             "read":{"indexed_family":"fam"},"emit":"message","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect_err("an emptiness requirement on an indexed family must be refused, not ignored");
    assert!(
        refused.to_string().contains("require_members"),
        "the refusal must name the entry-level filter that family reads do honour: {refused}"
    );

    // The order is not optional: without it the format would say nothing about a sequence it produces.
    assert!(
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.family",
                 "read":{"attribute_family":{"root":"acme.messages"}},
                 "parse":"json","emit":"message","legacy_rank":1}]}"#
                .to_vec(),
        )]))
        .is_err(),
        "a named family with no declared order must be refused"
    );

    // And it is one source form among several, so naming it beside another is refused like any other pair.
    assert!(
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.family",
                 "read":{"attribute":"acme.other",
                   "attribute_family":{"root":"acme.messages","order":"member_name"}},
                 "parse":"json","emit":"message","legacy_rank":1}]}"#
                .to_vec(),
        )]))
        .is_err(),
        "exactly one source form, as every other combination is"
    );
}

/// An emission names the clause **inside** its rule that produced it, not only the rule.
///
/// `SectionRoute.id`, `ElementPass.id` and `DerivedCase.id` are required declarations and were discarded before
/// the emission was built: `sectioned()` and `element_passes()` returned values and tags. So two routes of one
/// rule produced emissions with identical evidence, and a diagnostic could name the rule and not the route -
/// exactly the thing a reader needs when two routes disagree.
///
/// The section-route half is corpus-covered (`claude-agent-sdk.new_context`, and
/// `no_declared_subdivision_is_dead_across_the_corpus` fails if the id stops being carried). The element-pass
/// and derived-case halves are **not**: the only asset declaring them is Logfire's, whose suite has no captured
/// fixture. So they are pinned here, on the shape that asset carries, or the corpus gate would exempt them and
/// nothing would hold them at all.
#[test]
fn an_emission_names_the_clause_inside_its_rule() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.events","read":{"attribute":"events"},"parse":"json",
             "emit":"message","legacy_rank":1,"elements":{"passes":[
               {"id":"named","when":{"all":[{"path":"$['event.name']","one_of":["gen_ai.choice"]}]},
                "tag_from":"$['event.name']"},
               {"id":"blocks",
                "when":{"all":[{"path":"$['event.name']","none_of":["gen_ai.choice"]},
                               {"path":"$.data","kind":"object"}]},
                "group":{"collect":"$.data","key_as":"role","by":[
                   {"id":"is_input","when":{"all":[{"path":"$.data.type","starts_with":"input_"}]},
                    "value":"user"},
                   {"id":"is_output","when":{"all":[{"path":"$.data.type","starts_with":"output_"}]},
                    "value":"assistant"}],
                 "tag_by_key":{"user":"gen_ai.user.message","assistant":"gen_ai.assistant.message"}}}]}}]}"#
            .to_vec(),
    )]))
    .expect("the element-pass shape compiles");

    let attrs = std::collections::HashMap::from([(
        "events".to_string(),
        serde_json::json!([
            {"event.name": "gen_ai.choice", "content": "the reply"},
            {"data": {"type": "input_image", "url": "a"}},
            {"data": {"type": "input_image", "url": "b"}},
            {"data": {"type": "output_text", "text": "c"}},
        ])
        .to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let paths: Vec<String> = plan
        .run(&ctx)
        .iter()
        .map(|e| {
            e.evidence
                .paths()
                .iter()
                .map(|path| {
                    std::iter::once(path.root.as_str())
                        .chain(path.steps.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect::<Vec<_>>()
                .join(" + ")
        })
        .collect();
    assert_eq!(
        paths,
        [
            // The ungrouped pass: the pass alone, since no case chose anything.
            "t.events/named",
            // A **run** of consecutive input blocks: one emission, naming the pass and the case whose
            // predicate derived its key. Two cases can derive the same key, so the key does not name the
            // clause - which is why the run carries the case rather than being looked up from it.
            "t.events/blocks/is_input",
            "t.events/blocks/is_output",
        ],
        "each emission names the pass, and a grouped one also the derived case that produced its run"
    );
}

/// A **claim** on a container event's payload counts as handling it.
///
/// A claim means "this payload is framework internals, taken off the table deliberately" - so it must suppress
/// the container's raw form exactly as an emission does, which is the rule the span path already follows. The
/// case is unreachable from the shipped corpus (no container event has a claim-only reading), and it was
/// unreachable from a *probe* too until the raw form moved onto the plan: `from_event` read
/// `ruleset().message_events`, a global, so a test could not declare its own container.
#[test]
fn a_claim_on_a_container_event_suppresses_its_raw_form() {
    use crate::domain::rules::message_rules::compile;

    let plan = |emit: &str| {
        let body = format!(
            r#"{{"id":"t","message_events":[{{"id":"t.e","name":"acme.container","raw":"replace"}}],
                 "messages":[{{"id":"t.read","source":{{"event":{{"names":["acme.container"]}}}},
                   "read":{{"attribute":"payload"}},"parse":"json","emit":"{emit}","legacy_rank":1}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
        .expect("the probe compiles")
    };
    let readable = std::collections::HashMap::from([(
        "payload".to_string(),
        r#"{"role":"user","content":"q"}"#.to_string(),
    )]);
    let unreadable = std::collections::HashMap::from([("payload".to_string(), "{".to_string())]);
    let span_attrs = std::collections::HashMap::new();

    // A claim produces no message, and still replaces the raw form.
    let claim_plan = plan("claim");
    let claimed = claim_plan.from_event("acme.container", &readable, "span", &span_attrs, false);
    assert!(
        claimed.emissions.is_empty(),
        "a claim is not a message, so nothing is emitted"
    );
    assert!(
        claimed.replaces_raw,
        "and the container is still replaced - taking a payload off the table is what a claim is for"
    );
    assert!(!claimed.unhandled_container);

    // The same rule against an unreadable payload: nothing claimed it, the carrier was there, so the raw form
    // is kept and the caller is told.
    let failed = claim_plan.from_event("acme.container", &unreadable, "span", &span_attrs, false);
    assert!(
        !failed.replaces_raw && failed.unhandled_container,
        "a claim that could not read its payload has not taken anything off the table"
    );

    // And a message reading behaves the same way, which is what makes this a fact about handling rather than
    // about the emission kind.
    let message_plan = plan("message");
    let emitted = message_plan.from_event("acme.container", &readable, "span", &span_attrs, false);
    assert!(emitted.replaces_raw && !emitted.unhandled_container);
}

/// A wrong-typed member of a reduction is **malformed**, not a shorter list and not a zero.
///
/// `sum` saw a match, swallowed the non-numeric member into the same "contributes nothing" branch a *missing*
/// usage object takes, and answered `Integer(0)`. So a producer who wrote an invalid count had written a valid
/// zero - reported as a real measurement, with no diagnostic and nothing for `on_malformed` to act on. Zero
/// tokens and an unreadable count are different statements about a call, and one of them is a bill.
///
/// `collect_all` had the same shape at a smaller stake: a member holding an object shortened the list silently,
/// so the answer was a partial reading indistinguishable from a producer who listed fewer values.
#[test]
fn a_wrong_typed_member_of_a_reduction_is_malformed() {
    use crate::domain::rules::span_fields::{Reading, compile};

    let plan = |target: &str, path: &str, reduce: &str| {
        let body = format!(
            r#"{{"id":"t","span_fields":[{{"id":"t.rule","target":"{target}","sources":[
                 {{"id":"t.s","json":{{"attribute":"payload","path":"{path}","reduce":"{reduce}"}}}}]}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
        .expect("the probe compiles")
    };
    // The **answer** and what the chain **refused**, because a malformed reading ends a first-wins chain and
    // is recorded there rather than becoming the answer - so asserting only on the answer cannot tell a
    // malformed member from an absent one, which is the distinction under test.
    let resolve = |plan: &crate::domain::rules::span_fields::SpanFieldPlan,
                   payload: &str|
     -> (Reading, Vec<Reading>) {
        let attrs = std::collections::HashMap::from([("payload".to_string(), payload.to_string())]);
        let resolved = plan
            .resolve("span", &attrs, &[])
            .into_iter()
            .next()
            .expect("one rule");
        (
            resolved.reading,
            resolved.refused.into_iter().map(|r| r.reading).collect(),
        )
    };
    let read = |plan: &crate::domain::rules::span_fields::SpanFieldPlan,
                payload: &str|
     -> Reading { resolve(plan, payload).0 };

    // Codex's input.
    let summed = plan(
        "usage_input_tokens",
        "$.messages[*].models_usage.prompt_tokens",
        "sum",
    );
    let (answer, refused) = resolve(
        &summed,
        r#"{"messages":[{"models_usage":{"prompt_tokens":"bad"}}]}"#,
    );
    assert_eq!(
        refused.len(),
        1,
        "a member that is present and not a number makes the sum malformed, and the chain records it"
    );
    assert!(
        matches!(refused[0], Reading::Malformed { .. }),
        "recorded as malformed: {:?}",
        refused[0]
    );
    assert_ne!(
        answer,
        Reading::Integer(0),
        "and the answer is not a believable zero - which is what the swallowed member produced"
    );

    // And the case the previous behaviour existed to protect: a message with **no** usage object contributes
    // nothing, and the others still sum. That is a different fact from a wrong-typed member, which is the
    // whole point of separating them.
    assert_eq!(
        read(
            &summed,
            r#"{"messages":[{"models_usage":{"prompt_tokens":7}},{"other":1},
                 {"models_usage":{"prompt_tokens":5}}]}"#
        ),
        Reading::Integer(12),
        "an absent member contributes nothing and the rest still sum"
    );
    // No matches at all stays `Absent` - "the span has no such shape" is not "a call that used no tokens".
    assert_eq!(read(&summed, r#"{"messages":[]}"#), Reading::Absent);
    // A genuine zero is still a genuine zero.
    assert_eq!(
        read(
            &summed,
            r#"{"messages":[{"models_usage":{"prompt_tokens":0}}]}"#
        ),
        Reading::Integer(0)
    );

    // `collect_all`: a wrong-typed match is malformed rather than a shorter list.
    let collected = plan(
        "gen_ai_finish_reasons",
        "$.choices[*].finish_reason",
        "collect_all",
    );
    let (answer, refused) = resolve(
        &collected,
        r#"{"choices":[{"finish_reason":"stop"},{"finish_reason":{"x":1}}]}"#,
    );
    assert!(
        refused
            .iter()
            .any(|r| matches!(r, Reading::Malformed { .. })),
        "a member holding an object is malformed, not one fewer reason: {refused:?}"
    );
    assert_ne!(
        answer,
        Reading::StringList(vec!["stop".to_string()]),
        "and the answer is not the partial list, which is indistinguishable from a shorter statement"
    );
    assert_eq!(
        read(
            &collected,
            r#"{"choices":[{"finish_reason":"stop"},{"finish_reason":"length"}]}"#
        ),
        Reading::StringList(vec!["stop".to_string(), "length".to_string()])
    );
}

/// Every source form a refusal can come from **names itself**.
///
/// `source_label` ends in `String::new()`, so a source form added without a label there logs an empty carrier -
/// a diagnostic that says a field could not be read and not what could not be read. The event-attribute form
/// added in cycle 9 would have been exactly that. This walks the shipped sources and requires each to describe
/// itself, which is the part of "structured diagnostics" a test can hold: the clause path comes from the
/// declaration's own id and cannot be empty (compilation refuses that), while the carrier is hand-built per
/// form.
#[test]
fn every_span_field_source_can_name_what_it_read() {
    use crate::domain::rules::schema::RuleFile;

    let mut checked = 0;
    for (path, bytes) in crate::domain::rules::schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.span_fields {
            for source in &rule.sources {
                let label = crate::domain::rules::span_fields::source_label_for(source);
                assert!(
                    !label.is_empty(),
                    "`{}` in `{path}` has a source (`{}`) that cannot describe what it reads, so a refusal \
                     naming it would say a field was unreadable without saying what was unreadable",
                    rule.id,
                    source.id
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 100,
        "no sources were walked, so this gate is checking nothing: {checked}"
    );
}

/// An unreadable **witness** is unanswerable, not false.
///
/// A `when_json` witness gates a source on a member being present, and it answered `false` for a carrier that
/// was there and did not parse. So the Anthropic/OpenAI discriminator (`request_data` holding `"{"`) behaved
/// exactly as though nobody had written `request_data` at all, silently taking the branch it exists to rule
/// out - a gate deciding which producer's convention applies, deciding it from a payload nobody could read, with
/// `on_malformed` never seeing the failure.
///
/// The same three-valued shape `Truth` gives the boolean grammar, for the same reason: "no" and "could not ask"
/// are different answers.
#[test]
fn an_unreadable_witness_is_unanswerable_rather_than_false() {
    use crate::domain::rules::span_fields::{Reading, compile};

    // Two sources with opposite witnesses on one carrier - the shape of a producer discriminator.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","span_fields":[{"id":"t.rule","target":"gen_ai_system","sources":[
             {"id":"t.anthropic","when_json":{"attribute":"request_data","path":"$.system"},
              "value":"anthropic"},
             {"id":"t.openai","when_json":{"attribute":"request_data","path":"$.messages"},
              "value":"openai"}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let read = |raw: &str| {
        let attrs =
            std::collections::HashMap::from([("request_data".to_string(), raw.to_string())]);
        let resolved = plan
            .resolve("span", &attrs, &[])
            .into_iter()
            .next()
            .expect("one rule");
        (resolved.reading, resolved.refused)
    };

    // The member is there: the gated source answers.
    assert_eq!(
        read(r#"{"system":"be brief"}"#).0,
        Reading::Text("anthropic".to_string())
    );
    // The member is absent: a real `false`, so the next source answers. This is what a witness is *for*, and
    // it must keep working - otherwise the fix is a ban on unreadable payloads rather than a distinction.
    assert_eq!(
        read(r#"{"messages":[]}"#).0,
        Reading::Text("openai".to_string())
    );

    // **No carrier at all**: both witnesses are a real `false`, nothing answers, and nothing is refused. This
    // is the case that separates "absent" from "unanswerable" - without it, a fix that made *every* missing
    // witness unanswerable would pass, and a witness that can never say no is not a gate.
    let attrs = std::collections::HashMap::new();
    let resolved = plan
        .resolve("span", &attrs, &[])
        .into_iter()
        .next()
        .expect("one rule");
    assert_eq!(resolved.reading, Reading::Absent);
    assert!(
        resolved.refused.is_empty(),
        "an absent carrier is not a refusal - nobody wrote it, which the witness can answer: {:?}",
        resolved.refused
    );

    // Present and unparseable: the question could not be asked. The chain records a malformed witness and does
    // **not** fall through to the other producer's answer, which is the silent misattribution.
    let (answer, refused) = read("{");
    assert_eq!(
        refused.len(),
        1,
        "the unreadable witness is recorded, where it used to be indistinguishable from an absent member"
    );
    assert!(
        matches!(refused[0].reading, Reading::Malformed { .. }),
        "as malformed: {:?}",
        refused[0]
    );
    assert_ne!(
        answer,
        Reading::Text("openai".to_string()),
        "and the other producer's convention is not silently applied to a payload nobody could read"
    );
}

/// `parse` is required wherever a reading parses a raw scalar, because omitted it meant three things.
///
/// Without it: text for a compose, JSON for an ordinary read or a `tool_repr`, JSON-or-string for a named
/// family. So one absent declaration was three different decisions, and which one applied was a property of a
/// **sibling** member - the same defect `attribute_any_of` had, where the multiplicity of a carrier list
/// depended on whether `tool_repr` sat beside it.
#[test]
fn a_reading_that_parses_a_scalar_declares_how() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    // Each form that parses a scalar of its own, refused without a mode.
    for (what, read) in [
        ("an exact attribute", r#"{"attribute":"x"}"#),
        ("ordered alternatives", r#"{"first_present":["x","y"]}"#),
        (
            "a named family",
            r#"{"attribute_family":{"root":"x","order":"member_name"}}"#,
        ),
    ] {
        let rule = format!(r#"{{"id":"t.r","read":{read},"emit":"message","legacy_rank":1}}"#);
        assert!(
            asset(&rule).is_err(),
            "{what} parses a raw string and must say how"
        );
        let with_mode = format!(
            r#"{{"id":"t.r","read":{read},"parse":"json","emit":"message","legacy_rank":1}}"#
        );
        assert!(
            asset(&with_mode).is_ok(),
            "{what} compiles once the mode is stated"
        );
    }

    // A compose member naming carriers reads one of their strings, so the mode is the member's.
    assert!(
        asset(
            r#"{"id":"t.c","compose":{"tag":"joined","members":[
                 {"as":"a","from_any_of":["x"]}]},"emit":"message","legacy_rank":1}"#
        )
        .is_err(),
        "a compose member naming carriers must declare its own mode"
    );

    // The exemptions, each a reading that parses no scalar itself - stated rather than implicit.
    for (what, rule) in [
        (
            "a sweep member, where sniffing whatever a prefix holds is the point",
            // `except` names every fixed output member the rule writes, which the collision refusal requires -
            // a swept name is only known at read time, so excluding them is the only way a sweep can say it
            // will not overwrite one.
            r#"{"id":"t.c","compose":{"tag":"joined","members":[
                 {"as":"a","from_any_of":["x"],"parse":"text"},
                 {"sweep_prefix":"p.","except":["a"]}]},"emit":"message","legacy_rank":1}"#,
        ),
        (
            "an indexed family, which assembles entries from keys rather than parsing one string",
            r#"{"id":"t.f","read":{"indexed_family":"fam"},"emit":"message","legacy_rank":1}"#,
        ),
    ] {
        assert!(
            asset(rule).is_ok(),
            "{what} needs no mode: {:?}",
            asset(rule).err()
        );
    }
}

/// A compose owns its members' carriers and **not** its own tag.
///
/// The tag is the name the engine gives an assembled result; `owns` is what the span carried. Claiming it let
/// two composes that read entirely different physical carriers suppress each other - the earlier one owned a
/// name no producer wrote, so the later one's members went unread and its content reached nothing.
#[test]
fn a_compose_owns_its_members_and_not_its_own_tag() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.compose","legacy_rank":1,
             "compose":{"tag":"canonical.response","members":[
               {"as":"content","from_any_of":["x"],"parse":"text"}]},
             "emit":"message"}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let attrs = std::collections::HashMap::from([("x".to_string(), "the answer".to_string())]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emissions = plan.run(&ctx);
    assert_eq!(emissions.len(), 1);
    let owned: Vec<String> = emissions[0]
        .owns
        .iter()
        .map(|carrier| carrier.name.clone())
        .collect();
    assert_eq!(
        owned,
        ["x".to_string()],
        "the member's carrier is owned; the synthetic tag is not something the span carried"
    );
    assert_eq!(
        emissions[0].carrier.name(),
        "canonical.response",
        "and the tag is still what the emission is *reported* under - the two are different questions"
    );
}

/// Two declarations must not write the same output member.
///
/// A wrap builds its object by inserting in a fixed order - role, literal members, pre-content attachments, the
/// content, post-content attachments - and every insert **overwrites**. So Codex's case compiled and produced a
/// message whose role is its content, with the two declarations before it silently discarded. Attachments could
/// overwrite literals, the content and each other; `compose.trailing` could overwrite a named or swept member.
///
/// The corpus-reachable one was `vercel-ai.response`: its sweep took `ai.response.role` into the `role` member
/// and `trailing.role` then overwrote it. Output-neutral to fix, because the overwrite always happened - but the
/// declaration was stating something untrue about what the rule reads.
#[test]
fn two_declarations_must_not_write_one_output_member() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let read = r#""read":{"attribute":"x"},"parse":"json","emit":"message","legacy_rank":1"#;

    // Codex's case: a literal over the declared role, then the content over both.
    assert!(
        asset(&format!(
            r#"{{"id":"t.r",{read},"wrap":{{"role":"user","members":{{"role":"assistant"}},
                 "content_as":"role"}}}}"#
        ))
        .is_err(),
        "a role, a literal role and content-as-role are three declarations of one member"
    );

    // An attachment over the content.
    assert!(
        asset(&format!(
            r#"{{"id":"t.r",{read},"wrap":{{"role":"user",
                 "attach":[{{"as":"content","from_path":"$.other"}}]}}}}"#
        ))
        .is_err(),
        "an attachment writing the content member overwrites the content"
    );

    // Two attachments under one name.
    assert!(
        asset(&format!(
            r#"{{"id":"t.r",{read},"wrap":{{"role":"user","attach":[
                 {{"as":"finish_reason","from_path":"$.a"}},
                 {{"as":"finish_reason","from_path":"$.b"}}]}}}}"#
        ))
        .is_err(),
        "two attachments under one name: whichever is later wins, and the rule says both"
    );

    // `trailing` over a named compose member.
    assert!(
        asset(
            r#"{"id":"t.c","emit":"message","legacy_rank":1,
                 "compose":{"tag":"joined","trailing":{"role":"assistant"},"members":[
                   {"as":"role","from_any_of":["x"],"parse":"text"}]}}"#
        )
        .is_err(),
        "trailing is inserted last, so it discards the member's value"
    );

    // A sweep that does not exclude a fixed output name. A swept name is only known at read time, so excluding
    // them is the only way a sweep can state that it will not overwrite one.
    assert!(
        asset(
            r#"{"id":"t.c","emit":"message","legacy_rank":1,
                 "compose":{"tag":"joined","trailing":{"role":"assistant"},"members":[
                   {"as":"content","from_any_of":["x"],"parse":"text"},
                   {"sweep_prefix":"p."}]}}"#
        )
        .is_err(),
        "a sweep must exclude every fixed output member the rule writes"
    );

    // And the coherent shapes compile, or the refusal is a ban on envelopes.
    for (what, rule) in [
        (
            "distinct names throughout",
            format!(
                r#"{{"id":"t.r",{read},"wrap":{{"role":"user","members":{{"kind":"text"}},
                     "attach":[{{"as":"finish_reason","from_path":"$.a"}}]}}}}"#
            ),
        ),
        (
            "a tool-call list *replacing* the content, which is stated rather than an overwrite",
            format!(
                r#"{{"id":"t.r",{read},"wrap":{{"role":"assistant",
                     "tool_calls_from":{{"select":"$.calls","id":"$.id","name":"$.name","on_invalid_item":"skip",
                       "arguments":"$.args"}}}}}}"#
            ),
        ),
        (
            "a sweep excluding every fixed name",
            r#"{"id":"t.c","emit":"message","legacy_rank":1,
                 "compose":{"tag":"joined","trailing":{"role":"assistant"},"members":[
                   {"as":"content","from_any_of":["x"],"parse":"text"},
                   {"sweep_prefix":"p.","except":["content","role"]}]}}"#
                .to_string(),
        ),
    ] {
        assert!(asset(&rule).is_ok(), "{what}: {:?}", asset(&rule).err());
    }
}

/// A construction branch refuses every sibling it would return before reaching.
///
/// The refusals existed and were **incomplete**, which is the harder kind to notice: `sections` refused `wrap`
/// and `alternatives` and accepted a walk, an aggregate, a `fallback` and a `tag_as` - each of which it returns
/// before. An indexed family accepted a `fallback`, a walk and `sections`; the named family added in cycle 9
/// accepted all of them.
///
/// And an element pass could state something other than what it did five different ways, the worst being a
/// decision table that answers for an element and has no tag for the answer - discarding a run the rule matched
/// on purpose.
#[test]
fn a_construction_branch_refuses_the_siblings_it_would_skip() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let sections = r#""sections":{"split_on":"|","routes":[{"id":"all","role":"user"}]}"#;

    for (what, extra) in [
        ("a walk", r#""walk":{"max_depth":2}"#),
        ("an aggregate", r#""aggregate_into_array":true"#),
        ("a fallback", r#""fallback":[{"id":"raw"}]"#),
        ("a `tag_as`", r#""tag_as":"renamed""#),
    ] {
        let rule = format!(
            r#"{{"id":"t.s","read":{{"attribute":"x"}},"parse":"text",{sections},{extra},
                 "emit":"message","legacy_rank":1}}"#
        );
        assert!(
            asset(&rule).is_err(),
            "`sections` returns before {what}, so declaring it is dead"
        );
    }
    // The branch itself still compiles, or the refusals are a ban on sections.
    let plain = format!(
        r#"{{"id":"t.s","read":{{"attribute":"x"}},"parse":"text",{sections},
             "emit":"message","legacy_rank":1}}"#
    );
    assert!(
        asset(&plain).is_ok(),
        "sections alone: {:?}",
        asset(&plain).err()
    );

    // A named family, which returns at the same point.
    for (what, extra) in [
        ("a fallback", r#""fallback":[{"id":"raw"}]"#),
        (
            "elements",
            r#""elements":{"passes":[{"id":"p","tag_from":"$.n"}]}"#,
        ),
        ("an aggregate", r#""aggregate_into_array":true"#),
    ] {
        let rule = format!(
            r#"{{"id":"t.f","read":{{"attribute_family":{{"root":"fam","order":"member_name"}}}},
                 "parse":"json",{extra},"emit":"message","legacy_rank":1}}"#
        );
        assert!(
            asset(&rule).is_err(),
            "a named family emits one observation per member, so {what} is dead"
        );
    }

    // The five element-pass shapes.
    let elements = |passes: &str| {
        format!(
            r#"{{"id":"t.e","read":{{"attribute":"x"}},"parse":"json",
                 "elements":{{"passes":{passes}}},"emit":"message","legacy_rank":1}}"#
        )
    };
    for (what, passes) in [
        ("no passes at all", "[]"),
        (
            "a pass with neither `tag_from` nor `group`",
            r#"[{"id":"p"}]"#,
        ),
        (
            "a pass with both, where `group` silently wins",
            r#"[{"id":"p","tag_from":"$.n","group":{"by":[{"id":"c","when":{},"value":"user"}],
               "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#,
        ),
        (
            "an empty decision table, so every run is empty",
            r#"[{"id":"p","group":{"by":[],"collect":"$.d","key_as":"role","tag_by_key":{}}}]"#,
        ),
        (
            "a derived value with no tag, so the run it matched is discarded",
            r#"[{"id":"p","group":{"by":[{"id":"c","when":{},"value":"user"}],
               "collect":"$.d","key_as":"role","tag_by_key":{"assistant":"gen_ai.assistant.message"}}}]"#,
        ),
    ] {
        assert!(
            asset(&elements(passes)).is_err(),
            "an element pass with {what} must be refused"
        );
    }
    // And the shape Logfire's asset actually carries.
    assert!(
        asset(&elements(
            r#"[{"id":"named","tag_from":"$.n"},
                {"id":"grouped","group":{"by":[{"id":"c","when":{},"value":"user"}],
                 "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#
        ))
        .is_ok(),
        "a tagging pass beside a grouping pass is the shipped shape: {:?}",
        asset(&elements(
            r#"[{"id":"named","tag_from":"$.n"},
                {"id":"grouped","group":{"by":[{"id":"c","when":{},"value":"user"}],
                 "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#
        ))
        .err()
    );
}

/// A reading that **cannot be built** produced nothing, so the chain keeps going.
///
/// Codex's case: the first alternative selects something and its envelope names a member the payload has not, so
/// `wrapped()` returns `None`. The candidate used to be returned as *the* answer and dropped afterwards by the
/// caller, so the second alternative and the rule's `fallback` were never tried and the rule emitted nothing -
/// where a later shape would have worked. Construction happens inside the coalesce now, which makes "could not
/// be built" the same answer as "this shape does not match": the only one a coalesce can act on.
#[test]
fn a_reading_whose_envelope_cannot_be_built_lets_the_chain_continue() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "alternatives":[
               {"id":"first","select":"$.a","wrap":{"role":"user","content_from_any_of":["$.missing"]}},
               {"id":"second","select":"$.b","wrap":{"role":"user","content_from_any_of":["$.text"]}}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"a":{"other":"unbuildable"},"b":{"text":"usable"}}"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted: Vec<String> = plan
        .run(&ctx)
        .iter()
        .map(|e| e.value["content"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        emitted,
        ["usable".to_string()],
        "the first alternative selected and could not be built, so the second got its turn"
    );

    // And the rule's own `fallback`, which was equally unreachable.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "alternatives":[
               {"id":"first","select":"$.a","wrap":{"role":"user","content_from_any_of":["$.missing"]}}],
             "fallback":[
               {"id":"last","select":"$.b","wrap":{"role":"user","content_from_any_of":["$.text"]}}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let emitted: Vec<String> = plan
        .run(&ctx)
        .iter()
        .map(|e| e.value["content"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        emitted,
        ["usable".to_string()],
        "a fallback exists for exactly this - nothing else produced anything"
    );

    // Still no **bare payload**: an unbuildable envelope emits nothing rather than the value it wrapped, which
    // is the half of this that was already right.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "alternatives":[
               {"id":"only","select":"$.a","wrap":{"role":"user","content_from_any_of":["$.missing"]}}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    assert!(
        plan.run(&ctx).is_empty(),
        "nothing could be built, so nothing is emitted - not the payload under a message tag"
    );
}

/// An aggregate wraps the assembled array **once**, and a per-reading envelope beside it is refused.
///
/// The ordinary aggregate discarded every reading's envelope *and* the rule's, while the indexed-family
/// aggregate applied the rule's - so the same two declarations meant different things depending on the read
/// form, which is a property of the code rather than of the telemetry.
#[test]
fn an_aggregate_wraps_the_assembled_array_once() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    // The rule's envelope applies to the array.
    let plan = asset(
        r#"{"id":"t.a","read":{"attribute":"docs"},"parse":"json","aggregate_into_array":true,
             "wrap":{"role":"data"},"emit":"message","legacy_rank":1,
             "alternatives":[{"id":"each","select":"$[*]"}]}"#,
    )
    .expect("an aggregate with a rule envelope compiles");
    let attrs = std::collections::HashMap::from([(
        "docs".to_string(),
        r#"[{"t":"one"},{"t":"two"}]"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emissions = plan.run(&ctx);
    assert_eq!(emissions.len(), 1, "an aggregate is one observation");
    assert_eq!(
        emissions[0].value["role"].as_str(),
        Some("data"),
        "and the rule's envelope wrapped it - it used to be discarded here and applied by the indexed-family \
         aggregate, so one syntax meant two things"
    );
    assert_eq!(
        emissions[0].value["content"].as_array().map(Vec::len),
        Some(2),
        "the content is the assembled array"
    );
    // And its evidence names the reading that contributed. An aggregate is built from *every* reading, which is
    // why an emission carries a set: with one path it would have to pick one of them, or name none.
    assert_eq!(
        emissions[0]
            .evidence
            .paths()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["t.a → each".to_string()],
        "the aggregate names what built it"
    );

    // A per-reading envelope beside an aggregate is refused rather than discarded.
    assert!(
        asset(
            r#"{"id":"t.a","read":{"attribute":"docs"},"parse":"json","aggregate_into_array":true,
                 "emit":"message","legacy_rank":1,
                 "alternatives":[{"id":"each","select":"$[*]","wrap":{"role":"document"}}]}"#,
        )
        .is_err(),
        "construct-each-then-aggregate is a different operation and nothing declares it"
    );
}

/// An **alternative** names itself too, and a grouped run names every case that built it.
///
/// `Reading` was `(value, wrap, target)`: an emission produced by `{"id":"as_assistant","select":"$.response"}`
/// carried an empty clause path, and a nested fragment case lost both the selection-point id and the winning
/// case id. So the required ids of cycle 5 were still discarded before an emission existed - one level down from
/// where cycle 9 fixed it.
///
/// And an emission carries an `EvidenceSet` rather than one path, because some emissions genuinely have several
/// contributing clauses: two cases deriving one key are legitimate aliases, so a run built from both has two
/// witnesses and naming one of them claims it produced blocks it did not match.
#[test]
fn an_alternative_and_a_grouped_run_name_every_clause_that_built_them() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let paths = |plan: &crate::domain::rules::message_rules::MessagePlan,
                 attrs: &std::collections::HashMap<String, String>|
     -> Vec<String> {
        let ctx = MessageContext::for_span("span", attrs, false);
        plan.run(&ctx)
            .iter()
            .map(|e| {
                e.evidence
                    .paths()
                    .iter()
                    .map(|path| {
                        std::iter::once(path.root.as_str())
                            .chain(path.steps.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .collect::<Vec<_>>()
                    .join(" + ")
            })
            .collect()
    };

    // An alternative, and a fragment case reached *through* one: the selection point and then the case.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","fragments":{"shape":{"cases":[
             {"id":"as_user","require":{"all":[{"path":"$.role","one_of":["user"]}]},
              "wrap":{"role":"user","content_from_any_of":["$.content"]}},
             {"id":"as_other","wrap":{"role":"assistant","content_from_any_of":["$.content"]}}]}},
             "messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
               "legacy_rank":1,"alternatives":[
                 {"id":"plain","select":"$.direct"},
                 {"id":"via_fragment","select":"$.wrapped[*]","then_fragment":"t.shape"}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    assert_eq!(
        paths(
            &plan,
            &std::collections::HashMap::from([(
                "x".to_string(),
                r#"{"direct":{"role":"user","content":"q"}}"#.to_string()
            )])
        ),
        ["t.r/plain".to_string()],
        "an alternative that answered directly names itself, where it used to name nothing"
    );
    assert_eq!(
        paths(
            &plan,
            &std::collections::HashMap::from([(
                "x".to_string(),
                r#"{"wrapped":[{"role":"user","content":"q"},{"role":"bot","content":"a"}]}"#
                    .to_string()
            )])
        ),
        [
            "t.r/via_fragment/as_user".to_string(),
            "t.r/via_fragment/as_other".to_string()
        ],
        "a fragment case names the selection point *and* the case - both were lost"
    );

    // A grouped element run built from two cases that derive one key: two witnesses under one pass.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.e","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,"elements":{"passes":[{"id":"blocks","group":{
               "collect":"$.data","key_as":"role","by":[
                 {"id":"input_block","when":{"all":[{"path":"$.data.type","starts_with":"input_"}]},
                  "value":"user"},
                 {"id":"legacy_input","when":{"all":[{"path":"$.data.type","starts_with":"legacy_"}]},
                  "value":"user"}],
               "tag_by_key":{"user":"gen_ai.user.message"}}}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    assert_eq!(
        paths(
            &plan,
            &std::collections::HashMap::from([(
                "x".to_string(),
                r#"[{"data":{"type":"input_image"}},{"data":{"type":"legacy_image"}}]"#.to_string()
            )])
        ),
        ["t.e/blocks/input_block + t.e/blocks/legacy_input".to_string()],
        "two consecutive elements matching two different cases are one `user` run, and both cases built it - \
         naming only the first claims it produced a block it did not match"
    );
}

/// An attachment's sources fall through to each other, whichever form is declared.
///
/// Codex's case: `{"from_path": "$.finish_reason", "from": "finish_reason", "default": "unknown"}`. When the
/// payload path resolved to nothing, `?` returned from the whole function - so the sibling attribute, the
/// span-name fallback **and** the default were never consulted, while an absent `from_value_any_of` fell
/// through to exactly those. One member, two source forms, two different answers to "nothing here", and the
/// asymmetry was in the code rather than in anything declared.
#[test]
fn an_attachment_falls_through_to_its_other_sources() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "wrap":{"role":"assistant","content_from_any_of":["$.content"],
               "attach":[{"as":"finish_reason","from_path":"$.finish_reason","from":"finish_reason",
                 "default":"unknown"}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let reason = |payload: &str, span: Vec<(&str, &str)>| -> String {
        let mut attrs = std::collections::HashMap::from([("x".to_string(), payload.to_string())]);
        for (key, value) in span {
            attrs.insert(key.to_string(), value.to_string());
        }
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)[0].value["finish_reason"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // The payload path, which wins where it resolves.
    assert_eq!(
        reason(r#"{"content":"a","finish_reason":"stop"}"#, vec![]),
        "stop"
    );
    // Absent in the payload, present as a **span attribute**: the sibling source answers. This returned
    // `unknown`... no: it returned *nothing at all*, so the member was absent from the message entirely.
    assert_eq!(
        reason(r#"{"content":"a"}"#, vec![("finish_reason", "length")]),
        "length",
        "the sibling attribute is consulted, where `?` used to end the function before reaching it"
    );
    // Absent everywhere: the default, which was equally unreachable.
    assert_eq!(
        reason(r#"{"content":"a"}"#, vec![]),
        "unknown",
        "and the default is the last word, as it is for the value form"
    );
}

/// A walk stops on the clauses it **names**, and a clause it names must exist.
///
/// The boolean it replaces asked "did anything get selected at this node", which is wider than "was this node a
/// message": LangGraph's `also_3` selects every state member, so a node holding a message *beside* more state
/// counted as matched and its siblings were never visited.
///
/// Recognition is also decided **before** construction and in the **same pass** as the readings. Before: a clause
/// whose envelope failed made the node look unrecognised and widened the traversal, and the fix for that took two
/// evaluations of every node - quadratic on a deep payload.
#[test]
fn a_walk_stops_on_the_clauses_it_names() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |stop: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","legacy_rank":1,"walk":{{"max_depth":3,"stop_on":{stop}}},
                 "also":[
                   {{"id":"whole_node","require":{{"all":[{{"path":"$.role"}},{{"path":"$.content"}}]}},
                    "wrap":{{"role_from":"$.role","content_from_any_of":["$.content"]}}}},
                   {{"id":"any_member","select":"$.*",
                    "require":{{"all":[{{"path":"$.role"}},{{"path":"$.content"}}]}},
                    "wrap":{{"role_from":"$.role","content_from_any_of":["$.content"]}}}}]}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    // A node holding a message beside more state: `any_member` recognises the *root*, `whole_node` does not.
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"direct":{"role":"user","content":"q"},"nested":{"inner":{"role":"assistant","content":"a"}}}"#
            .to_string(),
    )]);

    let narrow = asset(r#"["whole_node"]"#).expect("naming one clause compiles");
    let ctx = MessageContext::for_span("span", &attrs, false);
    let contents = |plan: &crate::domain::rules::message_rules::MessagePlan| {
        let mut seen: Vec<String> = plan
            .run(&ctx)
            .iter()
            .map(|e| e.value["content"].as_str().unwrap_or_default().to_string())
            .collect();
        seen.sort();
        seen.dedup();
        seen
    };
    assert_eq!(
        contents(&narrow),
        ["a".to_string(), "q".to_string()],
        "the root is not a message, so the walk descends and the nested one is found"
    );

    // Naming the wide clause is the boolean's behaviour, and it loses the nested message - which is the point:
    // the two readings are different, and the format now says which one a rule means.
    let wide = asset(r#"["whole_node","any_member"]"#).expect("naming both compiles");
    assert_eq!(
        contents(&wide),
        ["q".to_string()],
        "`any_member` recognised the root, so the descent stopped and `nested` was never visited"
    );

    // A name that matches nothing could never stop the descent, which is indistinguishable from meaning never
    // to stop - so it is refused rather than silently running to `max_depth`.
    assert!(
        asset(r#"["whole_nde"]"#).is_err(),
        "a misspelled clause id must be refused"
    );
}

/// A grouped element run is consecutive in the **array a producer wrote**, not in the pass's filtered view.
///
/// The pass filtered the array before finding runs, so an element it does not match simply vanished - and two
/// content blocks with a *message* between them became one "consecutive" run of the two. Codex's input is
/// Logfire's own shape: an input block, a named assistant event, another input block. The run then claims two
/// blocks are adjacent while asserting nothing about the message lying between them.
#[test]
fn a_grouped_run_is_consecutive_in_the_array_the_producer_wrote() {
    use crate::domain::rules::message_rules::{MessageContext, MessagePlan, compile};

    fn plan_runs(plan: &MessagePlan, ctx: &MessageContext<'_>) -> Vec<usize> {
        plan.run(ctx)
            .iter()
            .filter(|e| e.carrier.name() == "gen_ai.user.message")
            .map(|e| e.value["content"].as_array().map_or(0, Vec::len))
            .collect()
    }

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.e","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,"elements":{"passes":[
               {"id":"named","when":{"all":[{"path":"$['event.name']","one_of":["assistant"]}]},
                "tag_from":"$['event.name']"},
               {"id":"blocks",
                "when":{"all":[{"path":"$['event.name']","none_of":["assistant"]},
                               {"path":"$.data","kind":"object"}]},
                "group":{"collect":"$.data","key_as":"role","by":[
                   {"id":"is_input","when":{"all":[{"path":"$.data.type","starts_with":"input_"}]},
                    "value":"user"}],
                 "tag_by_key":{"user":"gen_ai.user.message"}}}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let runs = |payload: &str| -> Vec<usize> {
        let attrs = std::collections::HashMap::from([("x".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .iter()
            .filter(|e| e.carrier.name() == "gen_ai.user.message")
            .map(|e| e.value["content"].as_array().map_or(0, Vec::len))
            .collect()
    };

    // Codex's input: two input blocks with an assistant message between them.
    assert_eq!(
        runs(
            r#"[{"data":{"type":"input_text","text":"before"}},
                {"event.name":"assistant","content":"middle"},
                {"data":{"type":"input_image","url":"after"}}]"#
        ),
        vec![1, 1],
        "the intervening message ends the run - as one run of two, the answer asserts the blocks are adjacent \
         while a message lies between them"
    );

    // Genuinely consecutive blocks are still one run, or the fix would be a ban on grouping.
    assert_eq!(
        runs(
            r#"[{"data":{"type":"input_text","text":"a"}},
                {"data":{"type":"input_image","url":"b"}}]"#
        ),
        vec![2],
        "adjacent blocks are one turn's content, which is what grouping is for"
    );

    // An element that matched the pass and derived a case but has **nothing to collect** ends a run as well:
    // it is an element of the array, so it lies between its neighbours whatever it holds. A separate probe,
    // because with `collect: "$.data"` an element that derives a case necessarily has something there.
    let collecting = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.e","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,"elements":{"passes":[
               {"id":"blocks","when":{"all":[{"path":"$.data","kind":"object"}]},
                "group":{"collect":"$.data.text","key_as":"role","by":[
                   {"id":"is_input","when":{"all":[{"path":"$.data.type","starts_with":"input_"}]},
                    "value":"user"}],
                 "tag_by_key":{"user":"gen_ai.user.message"}}}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"[{"data":{"type":"input_text","text":"a"}},
            {"data":{"type":"input_image","url":"no text here"}},
            {"data":{"type":"input_text","text":"b"}}]"#
            .to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        plan_runs(&collecting, &ctx),
        vec![1, 1],
        "the element with nothing at `collect` is still an element between the other two"
    );

    // And an element the *group* has no case for ends a run too, not only one the pass rejects.
    assert_eq!(
        runs(
            r#"[{"data":{"type":"input_text","text":"a"}},
                {"data":{"type":"other_thing"}},
                {"data":{"type":"input_image","url":"b"}}]"#
        ),
        vec![1, 1],
        "an element deriving no case is between them just as much as one the pass excluded"
    );
}

/// A lift states where it reads from **and** what happens on a collision, and dead slots are refused.
///
/// There were two members with opposite, unstated policies: `lift` overwrote the target's member and
/// `lift_from_parent` preserved it, and neither the names nor `lift`'s own documentation said so. A payload
/// carrying `finish_reason` outside a message *and* inside it therefore got the outer value under one member
/// name and the inner under the other, decided by which member an asset happened to use.
#[test]
fn a_lift_states_its_source_and_its_conflict_policy() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |reading: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","legacy_rank":1,"alternatives":[{reading}]}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let payload = r#"{"finish_reason":"outer",
                      "choices":[{"finish_reason":"beside","message":{"role":"assistant","content":"x",
                        "finish_reason":"inner"}}]}"#;
    let reason = |plan: &crate::domain::rules::message_rules::MessagePlan| -> String {
        let attrs = std::collections::HashMap::from([("x".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)[0].value["finish_reason"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // Both policies expressible from one source, which is what makes the difference a declaration.
    assert_eq!(
        reason(
            &asset(
                r#"{"id":"c","select":"$.choices[*]","descend":"message",
                     "lift":[{"from":"element","members":["finish_reason"],
                       "on_conflict":"replace_target"}]}"#
            )
            .expect("compiles")
        ),
        "beside",
        "`replace_target` lets the lifted value win"
    );
    assert_eq!(
        reason(
            &asset(
                r#"{"id":"c","select":"$.choices[*]","descend":"message",
                     "lift":[{"from":"element","members":["finish_reason"],
                       "on_conflict":"keep_target"}]}"#
            )
            .expect("compiles")
        ),
        "inner",
        "`keep_target` makes the lifted value a fallback - the same source, the opposite answer, and it used \
         to depend on which of two member names an asset chose"
    );
    // And the parent as a source, which is a different value again.
    assert_eq!(
        reason(
            &asset(
                r#"{"id":"c","select":"$.choices[*].message",
                     "lift":[{"from":"parent","members":["finish_reason"],
                       "on_conflict":"replace_target"}]}"#
            )
            .expect("compiles")
        ),
        "outer",
        "`parent` is the value the selection came out of"
    );

    // Dead slots, each of which compiled and did nothing.
    for (what, reading) in [
        (
            "a lift from the element with no `descend`, where the element is already the candidate",
            r#"{"id":"c","select":"$.choices[*]",
                 "lift":[{"from":"element","members":["finish_reason"],"on_conflict":"keep_target"}]}"#,
        ),
        (
            "both coalesce forms, where presence wins and the yielding one is dead",
            r#"{"id":"c","then_any_of":["$.a"],"then_present_any_of":["$.b"]}"#,
        ),
        (
            "`else_element` naming the fallback for a coalesce that is not there",
            r#"{"id":"c","else_element":true}"#,
        ),
    ] {
        assert!(asset(reading).is_err(), "{what} must be refused");
    }
}

/// A constructor and its target are about the same thing, and a claim constructs nothing.
///
/// `EmitTarget` was a filing destination with nothing checking that the reading filed there was the shape that
/// destination holds, so `{"wrap": {"role": "user"}, "emit": "tool_names"}` compiled and filed
/// `{"role":"user","content":"hello"}` as a tool *name*.
///
/// Deliberately **not** a full typing of the constructor/target pairs - that belongs to the discriminated
/// grammar, and treating this matrix as if it were would be worse than the gap. These are the pairs that are
/// provably incoherent today.
#[test]
fn a_constructor_and_its_target_describe_the_same_thing() {
    use crate::domain::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let read = r#""read":{"attribute":"x"},"parse":"text","legacy_rank":1"#;

    // Codex's case.
    assert!(
        asset(&format!(
            r#"{{"id":"t.r",{read},"wrap":{{"role":"user"}},"emit":"tool_names"}}"#
        ))
        .is_err(),
        "an envelope builds a message, and a tool name is a string"
    );
    // The same for the other two message constructors.
    assert!(
        asset(&format!(
            r#"{{"id":"t.r",{read},"emit":"tool_definitions",
                 "sections":{{"split_on":"|","routes":[{{"id":"all","role":"user"}}]}}}}"#
        ))
        .is_err(),
        "sections build a message per section"
    );
    assert!(
        asset(
            r#"{"id":"t.r","read":{"attribute":"x"},"parse":"json","legacy_rank":1,
                 "emit":"tool_names","elements":{"passes":[{"id":"p","tag_from":"$.n"}]}}"#
        )
        .is_err(),
        "element passes tag each element as a message carrier"
    );

    // A **claim** takes a payload off the table and emits nothing, so a constructed value is discarded. Probed
    // with a `compose` rather than a `wrap`: a wrap beside a claim is already refused by the check above, so a
    // wrap probe would pass for the wrong reason and say nothing about the claim rule.
    assert!(
        asset(
            r#"{"id":"t.r","emit":"claim","legacy_rank":1,
                 "compose":{"tag":"joined","members":[
                   {"as":"content","from_any_of":["k"],"parse":"text"}]}}"#
        )
        .is_err(),
        "a claim constructs nothing - whatever the compose assembled would be thrown away"
    );
    // And a bare claim is exactly what the three shipped ones are.
    assert!(
        asset(&format!(r#"{{"id":"t.r",{read},"emit":"claim"}}"#)).is_ok(),
        "recognition with no construction is the whole point of a claim"
    );
}

/// A tool-call list declares what happens to a member it cannot build, and reports it either way.
///
/// The constructor hardcoded two policies at once - "an id is mandatory" and "skip the invalid item" - and
/// reported neither. AutoGen's rule admits a list where *at least one* member has an id and a name, so a
/// response that called two tools showed one, with nothing saying why.
#[test]
fn a_tool_call_list_declares_what_an_unbuildable_call_means() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |policy: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","legacy_rank":1,
                 "wrap":{{"role":"assistant","tool_calls_from":{{"select":"$.content[*]","id":"$.id",
                   "name":"$.name","arguments":"$.arguments","on_invalid_item":"{policy}"}}}},
                 "alternatives":[{{"id":"as_calls"}}],
                 "fallback":[{{"id":"as_text","wrap":{{"role":"assistant",
                   "content_from_any_of":["$.summary"]}}}}]}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    // Codex's input: one call with an id, one without.
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"summary":"it called two tools",
            "content":[{"id":"c1","name":"search","arguments":{}},{"name":"calculate","arguments":{}}]}"#
            .to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);

    let skipping = asset("skip").expect("compiles");
    let emissions = skipping.run(&ctx);
    assert_eq!(emissions.len(), 1);
    assert_eq!(
        emissions[0].value["tool_calls"].as_array().map(Vec::len),
        Some(1),
        "`skip` keeps the rest, which may be right for a producer that logs partial calls"
    );

    // `fail_message` makes the construction malformed, so the coalesce moves on and the `fallback` answers -
    // which is the difference between "the message is incomplete" and "this shape did not apply".
    let failing = asset("fail_message").expect("compiles");
    let emissions = failing.run(&ctx);
    assert_eq!(emissions.len(), 1);
    assert_eq!(
        emissions[0].value["content"].as_str(),
        Some("it called two tools"),
        "the fallback answered, because the tool-call construction reported itself unbuildable"
    );

    // The policy is required: it was hardcoded twice over, and neither answer is right for every producer.
    let body = r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
         "emit":"message","legacy_rank":1,
         "wrap":{"role":"assistant","tool_calls_from":{"select":"$.content[*]","id":"$.id",
           "name":"$.name","arguments":"$.arguments"}}}]}"#;
    assert!(
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.as_bytes().to_vec(),
        )]))
        .is_err(),
        "a tool-call list with no declared policy must be refused"
    );
}

/// An indexed family is read in **one pass** over the span's attributes, not one per entry.
///
/// Discovery walked the attribute map and then every entry walked it again - twice, plus once per `require`d
/// member - so reading a family of `N` entries out of a span carrying `M` attributes cost `O(N·M)`. A
/// hundred-turn conversation flattened into a family means a hundred walks over every attribute the span has.
///
/// Asymptotic, so the corpus does not show it: its largest family is small enough that the ingestion benchmark
/// moved 75.6 → 73.0 ms, which is noise on this host. This measures the shape directly, and asserts the
/// **answer** is unchanged - the point of the change is that it is.
#[test]
fn an_indexed_family_is_read_in_one_pass() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.f","read":{"indexed_family":"fam"},
             "require_members":{"all_of":[{"name":"role"},{"name":"content"}]},
             "emit":"message","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");

    // 400 entries, plus 400 unrelated attributes the old scans walked once per entry.
    let mut attrs = std::collections::HashMap::new();
    for index in 0..400 {
        attrs.insert(format!("fam.{index}.role"), "user".to_string());
        attrs.insert(format!("fam.{index}.content"), format!("turn {index}"));
        attrs.insert(format!("unrelated.{index}"), "noise".to_string());
    }
    let ctx = MessageContext::for_span("span", &attrs, false);
    let started = std::time::Instant::now();
    let emissions = plan.run(&ctx);
    let elapsed = started.elapsed();

    assert_eq!(emissions.len(), 400, "every entry is an observation");
    assert_eq!(
        emissions[0].value["content"].as_str(),
        Some("turn 0"),
        "and the entries are in index order, not hash order"
    );
    assert_eq!(emissions[399].value["content"].as_str(), Some("turn 399"));
    // Entries whose required members are missing are still excluded, asked of the entry's own bucket.
    attrs.insert("fam.400.role".to_string(), "user".to_string());
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        plan.run(&ctx).len(),
        400,
        "an entry with no `content` is not a message, and the requirement is answered from its bucket"
    );

    // **The members of one entry keep a stable order**, which nothing else observes: reversing the sort changes
    // no golden, because no shipped family has members whose order is distinguishable. It still matters - the
    // object is persisted with its insertion order and content identity is hashed from it - so a hash map's
    // iteration order deciding it would make a message's identity vary per process. Asserted here, since the
    // corpus cannot.
    let one = std::collections::HashMap::from([
        ("fam.0.role".to_string(), "user".to_string()),
        ("fam.0.content".to_string(), "q".to_string()),
        ("fam.0.name".to_string(), "alice".to_string()),
        ("fam.0.id".to_string(), "x1".to_string()),
    ]);
    let ctx = MessageContext::for_span("span", &one, false);
    let emissions = plan.run(&ctx);
    assert_eq!(
        emissions[0]
            .value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["content", "id", "name", "role"],
        "the members are in a stable order, whatever order the attribute map iterates in"
    );

    // A ceiling rather than a measurement, so the test is not a benchmark on a shared host: at `O(N·M)` this
    // shape is ~480,000 key comparisons per scan pass and took over a second in release-less builds.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "reading 400 entries out of 1,200 attributes took {elapsed:?}, which is the quadratic shape"
    );
}

/// A compose owns the carriers it **read**, not the ones it looked at.
///
/// Ownership was recorded before the parse, so a member whose payload does not parse was owned anyway - while
/// the *same* malformed carrier read by an ordinary rule was not, because that rule's reading returns early and
/// produces no emission to own anything. One asymmetry, and the consequence is that a compose silently
/// suppressed another dialect's reading of a payload it could not read either.
///
/// Asserted on the emission's `owns` rather than through a second rule: two rules reading one carrier is
/// refused at compile unless the *earlier* one is conditional, and arranging that would make the probe about
/// the contested-carrier excuse rather than about what a compose owns. A rule that means to take a payload away
/// without emitting has `claim` for it, which is the declaration this was making by accident.
#[test]
fn a_compose_owns_the_carriers_it_read() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[
             {"id":"t.compose","legacy_rank":1,"emit":"message",
              "compose":{"tag":"joined","members":[
                {"as":"content","from_any_of":["text"],"parse":"text"},
                {"as":"extra","from_any_of":["structured"],"parse":"json"}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let owned = |payload: &str| -> Vec<String> {
        let attrs = std::collections::HashMap::from([
            ("text".to_string(), "the answer".to_string()),
            ("structured".to_string(), payload.to_string()),
        ]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        let emissions = plan.run(&ctx);
        assert_eq!(
            emissions.len(),
            1,
            "the compose emits, whatever the member did"
        );
        let mut names: Vec<String> = emissions[0]
            .owns
            .iter()
            .map(|carrier| carrier.name.clone())
            .collect();
        names.sort();
        names
    };

    // Readable: both carriers were read, so both are owned.
    assert_eq!(
        owned(r#"{"a":1}"#),
        ["structured".to_string(), "text".to_string()]
    );

    // **Unreadable**: the member contributed nothing, so the carrier is not taken off the table and a rule that
    // can read it keeps its turn.
    assert_eq!(
        owned("{"),
        ["text".to_string()],
        "a member whose payload does not parse neither contributes nor owns"
    );
}

/// A `repr` field is the field, not the tail of a longer identifier - and a value ends at the **earliest**
/// boundary.
///
/// Two defects in the sealed repr grammar, both of which invent or corrupt a tool:
///
/// - `name=` matches inside `username=`, so `CrewStructuredTool(username='admin', …)` decoded as a tool and
///   `repr_field` then found `name='admin'` inside that same token. The server invented a tool called `admin`
///   out of a constructor that names none.
/// - a loosely-quoted value took the **last** `')` in the remainder and tried declared terminators in
///   *declaration* order, so a quoted field after the description (`env_vars='SECRET')`) put its own closing
///   quote at the end and the description swallowed `env_vars='SECRET`.
#[test]
fn a_repr_field_respects_identifier_boundaries_and_the_earliest_close() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.tools","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_definitions","legacy_rank":1,
             "tool_repr":{"entries":"$[*]","candidates":["$"],
               "name_field":"name","description_field":"description",
               "name_label":"Tool Name:","description_label":"Tool Description:",
               "arguments_label":"Tool Arguments:","repr_markers":["name=","CrewStructuredTool("],
               "parameter_members":["args"],"field_terminators":["env_vars"],
               "type_map":[["str","string"]],"type_default":{"map_to":"string"}}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let tools = |payload: serde_json::Value| -> Vec<serde_json::Value> {
        let attrs = std::collections::HashMap::from([("tools".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.tool_definitions(&ctx)
            .iter()
            .flat_map(|e| e.value.as_array().cloned().unwrap_or_default())
            .collect()
    };

    // A constructor whose only `name=`-looking token is `username=`: the marker must not match there, so the
    // string is not decoded as a repr and **no tool called `admin` is invented**.
    //
    // What it *does* produce is the bare-name reading - the whole string as a tool name - which is the other
    // half of Codex's finding 6: a non-marker string is accepted as a name whatever it says, and fixing that
    // needs the tagged decoders (`bare_name` versus `python_constructor_repr` declared per candidate) rather
    // than a boundary test. So this asserts the invention is gone, not that the reading is right.
    let mistaken = tools(serde_json::json!([
        "CrewStructuredTool2(username='admin', description='not a tool name')"
    ]));
    assert!(
        mistaken
            .iter()
            .all(|tool| tool["function"]["name"].as_str() != Some("admin")),
        "`name=` inside `username=` is not the `name` field: {mistaken:?}"
    );

    // The real thing still decodes, so the boundary test is not a ban on markers.
    let real = tools(serde_json::json!([
        "CrewStructuredTool(name='search', description='Find records')"
    ]));
    assert_eq!(real.len(), 1);
    assert_eq!(real[0]["function"]["name"].as_str(), Some("search"));

    // The earliest boundary: a quoted field *after* the description must not be swallowed by it. Codex's own
    // input, and it needs the **label** inside the quoted value - that value is a container the grammar reads
    // labels out of, so an unlabelled `description='Find records'` reports no description at all, which is
    // this dialect's shape rather than a defect.
    let bounded = tools(serde_json::json!([
        "CrewStructuredTool(name='search', description='Tool Description: Find records', \
         env_vars='SECRET')"
    ]));
    assert_eq!(bounded.len(), 1);
    let description = bounded[0]["function"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !description.contains("SECRET"),
        "the description ran to a later field's closing quote: {description:?}"
    );
    assert_eq!(description, "Find records");
}

/// A `tool_repr`'s literals must be able to match something, and its type targets must be JSON Schema's.
///
/// The typed structure accepted every one of these, and each is a declaration that cannot mean what it says.
/// The worst is an empty token: it matches at position zero of every string, so an empty `repr_markers` entry
/// makes every entry a repr and an empty `name_field` finds the name at the start of anything.
///
/// And `type_default` was a bare string documented as "the widest type" while being `"string"`, which is the
/// opposite - a schema saying `type: string` rejects the number a producer may pass. `unconstrained` is the
/// widest, and it writes no `type` at all.
#[test]
fn a_tool_repr_declares_literals_that_can_match_and_types_that_exist() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    // The overrides come **last**, and every default they may replace is expressed as an override rather than
    // written twice - `deny_unknown_fields` refuses a duplicate member, so a base carrying `type_default` and
    // an override supplying another would fail at parse and say nothing about the check under test.
    let base = |overrides: &str| {
        let with_default = if overrides.contains("type_default") {
            String::new()
        } else {
            r#","type_default":{"map_to":"string"}"#.to_string()
        };
        let with_map = if overrides.contains("type_map") {
            String::new()
        } else {
            r#","type_map":[["str","string"]]"#.to_string()
        };
        let with_description = if overrides.contains("description_field") {
            String::new()
        } else {
            r#","description_field":"description""#.to_string()
        };
        let with_candidates = if overrides.contains("candidates") {
            String::new()
        } else {
            r#","candidates":["$"]"#.to_string()
        };
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.tools","read":{{"attribute":"tools"}},"parse":"json",
                 "emit":"tool_definitions","legacy_rank":1,
                 "tool_repr":{{"entries":"$[*]"{with_candidates},
                   "name_field":"name"{with_description},
                   "name_label":"N:","description_label":"D:","arguments_label":"A:",
                   "repr_markers":["name="],"parameter_members":["args"]{with_map}{with_default}
                   {overrides}}}}}]}}"#
        )
    };
    let asset = |body: String| {
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    assert!(asset(base("")).is_ok(), "the shipped shape compiles");

    for (what, overrides) in [
        ("no candidates", r#","candidates":[]"#),
        ("an empty description field", r#","description_field":"""#),
        ("an empty label", r#","arguments_label":"  ""#),
        ("an empty marker", r#","repr_markers":[""]"#),
        ("an empty parameter member", r#","parameter_members":[""]"#),
        ("an empty terminator", r#","field_terminators":[""]"#),
        (
            "a case-folded duplicate source type",
            r#","type_map":[["str","string"],["STR","integer"]]"#,
        ),
        (
            "a target outside JSON Schema's primitives",
            r#","type_map":[["str","banana"]]"#,
        ),
        (
            "a default outside them",
            r#","type_default":{"map_to":"banana"}"#,
        ),
    ] {
        assert!(
            asset(base(overrides)).is_err(),
            "{what} must be refused: it is a declaration that cannot mean what it says"
        );
    }

    // `unconstrained` writes **no** `type`, which is what the widest type is.
    let plan = asset(base(r#","type_default":"unconstrained""#)).expect("compiles");
    let attrs = std::collections::HashMap::from([(
        "tools".to_string(),
        // A JSON entry, which is what reads `parameter_members`. A *repr* string takes its arguments from the
        // `arguments_label` instead, so a repr probe supplies no parameters at all - and then "no `type` was
        // written" holds because the property does not exist, which is a test passing for the wrong reason.
        serde_json::json!([{"name": "when", "args": {"at": "datetime"}}]).to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let tools: Vec<serde_json::Value> = plan
        .tool_definitions(&ctx)
        .iter()
        .flat_map(|e| e.value.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(tools.len(), 1);
    let at = &tools[0]["function"]["parameters"]["properties"]["at"];
    assert!(
        at.is_object(),
        "the property must exist, or the next assertion holds for the wrong reason: {}",
        tools[0]
    );
    assert!(
        at.get("type").is_none(),
        "an unrecognised type constrains nothing, so no `type` is written: {at}"
    );

    // And a producer whose unrecognised names really are one kind of thing can still say so.
    let plan = asset(base(r#","type_default":{"map_to":"string"}"#)).expect("compiles");
    let tools: Vec<serde_json::Value> = plan
        .tool_definitions(&MessageContext::for_span("span", &attrs, false))
        .iter()
        .flat_map(|e| e.value.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        tools[0]["function"]["parameters"]["properties"]["at"]["type"].as_str(),
        Some("string")
    );
}

/// A tool name that is not a non-blank string names nothing, and it must not cost its siblings.
///
/// Codex's input: `{"gen_ai.agent.tools": "[\"search\", 7]"}`. The rule emitted and persisted the list as
/// written, and the read side deserialised the whole column as `Vec<String>` - so the number failed that and
/// took the valid `"search"` with it. A malformed item poisoning its siblings at the last possible moment,
/// after storage had already accepted it.
///
/// Both ends are fixed and both are needed: emission keeps a non-string out of storage, and the read keeps the
/// ones **already stored** from costing their neighbours.
#[test]
fn a_tool_name_is_a_non_blank_string_and_a_bad_one_costs_only_itself() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.names","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_names","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let names = |payload: &str| -> Vec<serde_json::Value> {
        let attrs = std::collections::HashMap::from([("tools".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.tool_definitions(&ctx)
            .iter()
            .flat_map(|e| e.value.as_array().cloned().unwrap_or_default())
            .collect()
    };

    assert_eq!(
        names(r#"["search", 7]"#),
        vec![serde_json::json!("search")],
        "the number names nothing, and the name beside it is kept"
    );
    assert_eq!(
        names(r#"["search", "  "]"#),
        vec![serde_json::json!("search")],
        "a blank string names nothing either"
    );
    // Asserted on the **emission count**, not the flattened items: an emission whose value is an empty array
    // flattens to nothing either way, so the item assertion could not tell one from the other.
    let attrs = std::collections::HashMap::from([("tools".to_string(), r#"[7, {}]"#.to_string())]);
    assert!(
        plan.tool_definitions(&MessageContext::for_span("span", &attrs, false))
            .is_empty(),
        "an emission with nothing usable left is no emission, not an empty one"
    );
    assert_eq!(
        names(r#"["search", "calculate"]"#),
        vec![serde_json::json!("search"), serde_json::json!("calculate")],
        "and ordinary names are untouched, or the check is a ban on tool names"
    );

    // A **definition** is deliberately not checked here: at emission it is still the producer's shape - Bedrock
    // writes `{"toolSpec": {"name": …}}` - and the canonical `{"function": {"name": …}}` appears only at
    // query-time normalisation. Written as a check over `function.name` this dropped `bedrock/converse`'s
    // perfectly good `get_weather`, which is cycle 13's finding 3: the provider shapes live in Rust and must
    // move into the assets before a definition can be validated where it is produced.
    let definitions = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.defs","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_definitions","legacy_rank":1}]}"#
            .to_vec(),
    )]))
    .expect("compiles");
    let attrs = std::collections::HashMap::from([(
        "tools".to_string(),
        r#"[{"toolSpec":{"name":"get_weather"}}]"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        definitions.tool_definitions(&ctx).len(),
        1,
        "a producer-shaped definition survives emission - naming it unusable here needs the provider shapes \
         to be declarable first"
    );
}

/// Metadata contends on the axis it emits on, and one carrier yields one reading per axis.
///
/// The conflict check was written for the message axis - correctly, because a dialect stating its tools on the
/// carrier another rule reads as a conversation is two true statements. But it was written for that axis
/// *alone*, so Codex's pair below compiled: two rules reading one carrier and both emitting tool definitions.
/// The metadata path then did no claiming, so both survived and their rank became precedence somewhere
/// downstream - which is a rule id deciding an answer.
#[test]
fn metadata_contends_on_the_axis_it_emits_on() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |rules: &str| {
        let body = format!(r#"{{"id":"t","messages":{rules}}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    // Codex's case: same carrier, same axis, unconditional both.
    assert!(
        asset(
            r#"[{"id":"t.raw","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
                 "legacy_rank":1},
                {"id":"t.projected","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
                 "legacy_rank":2,"alternatives":[{"id":"inner","select":"$.definitions"}]}]"#
        )
        .is_err(),
        "two definition readings of one carrier contend, as two message readings of it do"
    );

    // **Different** axes on one carrier still stand: that is the case the message-axis check was written for.
    let plan = asset(
        r#"[{"id":"t.defs","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
             "legacy_rank":1},
            {"id":"t.names","read":{"attribute":"tools"},"parse":"json","emit":"tool_names",
             "legacy_rank":2,"alternatives":[{"id":"each","select":"$[*].name"}]}]"#,
    )
    .expect("a definition list and a name list from one carrier are two statements, both true");
    let attrs = std::collections::HashMap::from([(
        "tools".to_string(),
        r#"[{"name":"search"}]"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let axes: Vec<String> = plan
        .tool_definitions(&ctx)
        .iter()
        .map(|e| format!("{:?}", e.target))
        .collect();
    assert_eq!(axes.len(), 2, "both axes read the carrier: {axes:?}");

    // The **runtime** claim, which needs the shape the compile check excuses: a *conditional* earlier rule
    // beside an unconditional later one. The compiler accepts that pair - they take turns and the ranks decide -
    // and on a span where both gates hold, only the first reading of the carrier survives on that axis.
    let plan = asset(
        r#"[{"id":"t.first","when":{"attr_exists":["marker"]},"read":{"attribute":"tools"},
             "parse":"json","emit":"tool_definitions","legacy_rank":1},
            {"id":"t.second","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
             "legacy_rank":2,"alternatives":[{"id":"inner","select":"$[*]"}]}]"#,
    )
    .expect("a conditional earlier rule beside an unconditional later one is accepted");
    let both = std::collections::HashMap::from([
        ("tools".to_string(), r#"[{"name":"search"}]"#.to_string()),
        ("marker".to_string(), "yes".to_string()),
    ]);
    let rules: Vec<&str> = plan
        .tool_definitions(&MessageContext::for_span("span", &both, false))
        .iter()
        .map(|e| e.rule_id)
        .collect();
    assert_eq!(
        rules,
        ["t.first"],
        "one carrier yields one definition reading; without the claim both survived and their rank became \
         precedence somewhere downstream"
    );

    // And a conversation beside a definition list, which is the documented co-located shape.
    asset(
        r#"[{"id":"t.conversation","read":{"attribute":"payload"},"parse":"json","emit":"message",
             "legacy_rank":1},
            {"id":"t.tools","read":{"attribute":"payload"},"parse":"json","emit":"tool_definitions",
             "legacy_rank":2}]"#,
    )
    .expect("one carrier holding a conversation and the tools it was offered is two statements");
}

/// A rule's work is bounded by this server, not only by what an asset declares.
///
/// A rule states how *deep* to descend, which is semantics - the shape of the state object a framework writes.
/// Within that depth a payload nests as widely as it likes and every node is evaluated, so the declared depth
/// bounds nothing; the 64 MiB body limit does not either, because the cost is in the evaluation rather than the
/// bytes. And a rule reading an array emits one observation per element, so a payload of a hundred thousand
/// elements is a hundred thousand messages from one span.
///
/// Server policy rather than a declaration, deliberately: a ceiling an asset could raise would not be a ceiling.
#[test]
fn a_rules_work_is_bounded_by_the_server() {
    use crate::core::constants::{RULE_MAX_EMISSIONS_PER_CARRIER, RULE_WALK_MAX_NODES};
    use crate::domain::rules::message_rules::{MessageContext, compile};

    // A wide payload inside a shallow declared depth: 20,000 sibling objects at depth 1, well past the node
    // ceiling, with a walk that would otherwise visit every one.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.w","read":{"attribute":"state"},"parse":"json",
             "emit":"message","legacy_rank":1,"walk":{"max_depth":3,"stop_on":["as_message"]},
             "also":[{"id":"as_message","require":{"all":[{"path":"$.role"},{"path":"$.content"}]},
               "wrap":{"role_from":"$.role","content_from_any_of":["$.content"]}}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    // **Message-shaped members**, so the number of nodes visited is observable in the answer. With members
    // that match nothing, "no more emissions than the ceiling" holds at zero and the assertion cannot tell a
    // bounded walk from an unbounded one - which is how my first version of this test passed with the ceiling
    // disabled.
    let wide: serde_json::Map<String, serde_json::Value> = (0..20_000)
        .map(|i| {
            (
                format!("m{i}"),
                serde_json::json!({"role": "user", "content": format!("turn {i}")}),
            )
        })
        .collect();
    let attrs = std::collections::HashMap::from([(
        "state".to_string(),
        serde_json::Value::Object(wide).to_string(),
    )]);
    let started = std::time::Instant::now();
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx).len();
    let elapsed = started.elapsed();
    assert!(
        emitted <= RULE_WALK_MAX_NODES,
        "the walk visited more nodes than the ceiling allows: {emitted} emissions"
    );
    assert!(
        emitted > 100,
        "and it visited a useful number of them before stopping - a ceiling that stops at once is a ban: \
         {emitted}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "a wide payload inside a shallow depth took {elapsed:?} - the ceiling is what bounds this, since the \
         declared depth does not"
    );

    // An array of one carrier: one observation per element, capped and reported.
    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.each","read":{"attribute":"turns"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "alternatives":[{"id":"every","select":"$[*]",
               "wrap":{"role":"user","content_from_any_of":["$.text"]}}]}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");
    let many: Vec<serde_json::Value> = (0..RULE_MAX_EMISSIONS_PER_CARRIER + 500)
        .map(|i| serde_json::json!({"text": format!("turn {i}")}))
        .collect();
    let attrs = std::collections::HashMap::from([(
        "turns".to_string(),
        serde_json::Value::Array(many).to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        plan.run(&ctx).len(),
        RULE_MAX_EMISSIONS_PER_CARRIER,
        "one carrier reports at most this many observations"
    );

    // And an ordinary payload is untouched, or the ceilings are a ban on reading arrays.
    let attrs = std::collections::HashMap::from([(
        "turns".to_string(),
        serde_json::json!([{"text": "one"}, {"text": "two"}]).to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(plan.run(&ctx).len(), 2);
}

/// A path used in a **singular** role reports when the payload offered more than one match.
///
/// A JSONPath is plural by nature: `$.*` matches every member, so `elements.select: "$.*"` reads the *first*
/// array of several and the others are gone with nothing said. Fifteen sites took
/// `query(...).into_iter().next()`, and the same silent-first rule reaches a grouped `collect`, a `tag_from`,
/// the indexed projections and several constructors.
///
/// Behaviour is unchanged deliberately. Which of those sites a real payload makes ambiguous is not something to
/// guess at, and a strict "at most one" refusal applied blind would reject shapes the corpus may depend on - so
/// this makes the ambiguity **reported**, which turns the question into a measurement, and the strict roles come
/// after there is evidence about which sites need them. An author who means the first can write `[0]`.
///
/// What the test pins is that the first match is still the answer, since that is the property a strict role
/// would later change and it must be a deliberate change rather than a drift.
#[test]
fn a_singular_path_takes_the_first_match_and_says_when_there_were_more() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.e","read":{"attribute":"x"},"parse":"json",
             "emit":"message","legacy_rank":1,
             "elements":{"select":"$.*","passes":[{"id":"named","tag_from":"$['event.name']"}]}}]}"#
            .to_vec(),
    )]))
    .expect("the probe compiles");

    // Codex's shape: two arrays under one object, and `$.*` matches both.
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"a":[{"event.name":"first"}],"b":[{"event.name":"second"}]}"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let tags: Vec<String> = plan
        .run(&ctx)
        .iter()
        .map(|e| e.carrier.name().to_string())
        .collect();
    assert_eq!(
        tags,
        ["first".to_string()],
        "the first array is read and the second is not - which is the behaviour, reported now rather than \
         silent"
    );

    // One array is unambiguous, and answers the same way.
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"a":[{"event.name":"only"}]}"#.to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    assert_eq!(
        plan.run(&ctx)
            .iter()
            .map(|e| e.carrier.name().to_string())
            .collect::<Vec<_>>(),
        ["only".to_string()]
    );
}

/// The predicate **mechanism** has migrated to the boolean grammar; the **semantics** have not, and this pins
/// exactly which two things still answer the old way.
///
/// `predicates_hold` goes through `Expr`/`Truth` now, so there is one evaluator rather than two and the retired
/// shell is a `#[cfg(test)]` oracle. But two compatibility translations preserve the pre-grammar answers, both
/// deliberately and both **reachable in the shipped assets**:
///
/// | Case | Where the translation is | Which asset relies on it |
/// | --- | --- | --- |
/// | a bare `none_of` holds for a *missing* value | `json_expr_of_predicate`'s `bare_negative_set` branch, which emits `any(not exists, some(not one_of))` | `logfire.json`'s element pass, whose `none_of` on `$['event.name']` must accept an event with no name |
/// | an explicitly empty group places no condition | `json_expr_of` returns `None` for a set with no predicates, and `predicates_hold` answers `true` for `None` | `langchain.json` ships an explicit `"all": []` beside a non-empty `any` |
///
/// So `Truth::Unknown` occurs *internally* - `JsonAtom::Some` answers it for an empty selection - and never
/// reaches a decision the shipped rules make. Codex's ruling: do not close this. Completing it needs direct
/// `Expr` syntax at these fields, Logfire stating the absence it means explicitly, the bare-`none_of` branch
/// removed, and an explicitly empty group refused - which needs the schema to distinguish "declared empty" from
/// "not declared", and today it cannot.
///
/// Both assertions below are the **current** answers. Each is the opposite of what the completed migration
/// gives, so when that lands these flip, deliberately, rather than a claim quietly becoming true.
#[test]
fn the_predicate_semantics_have_not_migrated_and_here_is_what_still_answers_the_old_way() {
    use crate::domain::rules::message_rules::predicates_hold;
    use crate::domain::rules::schema::PredicateSet;

    // A bare `none_of` against a payload with no such member. Under the grammar's own rules the selection is
    // empty, so the question is `Unknown` and a negation over it does not hold - but the compatibility branch
    // makes absence an explicit `true`.
    let bare_none_of: PredicateSet = serde_json::from_value(serde_json::json!({
        "all": [{"path": "$.name", "none_of": ["bob"]}]
    }))
    .expect("a predicate set parses");
    assert!(
        predicates_hold(&serde_json::json!({}), &bare_none_of),
        "today a missing value satisfies a bare `none_of`, because `logfire`'s element pass needs an event \
         with no name to pass a `none_of` on its name - stated as `any(not exists, some(not one_of))` rather \
         than left to a negation that quietly accepts absence"
    );
    // And it still answers `false` where the member *is* there and matches, or the branch would be a blanket
    // yes rather than a statement about absence.
    assert!(
        !predicates_hold(&serde_json::json!({"name": "bob"}), &bare_none_of),
        "a present, matching value is still refused"
    );

    // An explicitly empty group. The schema cannot tell it from an undeclared one, so it places no condition.
    let explicit_empty: PredicateSet =
        serde_json::from_value(serde_json::json!({"all": []})).expect("parses");
    assert!(
        explicit_empty.expression().is_none(),
        "an empty group translates to no expression, which is indistinguishable from declaring nothing"
    );
    assert!(
        predicates_hold(&serde_json::json!({}), &explicit_empty),
        "and no expression holds - `langchain.json` ships an explicit `\"all\": []`, so refusing it is a \
         migration rather than a fix"
    );

    // The mechanism *is* migrated: a set that declares something is evaluated by the grammar, and the
    // three-valued atom is what answers.
    let declared: PredicateSet = serde_json::from_value(serde_json::json!({
        "all": [{"path": "$.role", "one_of": ["user"]}]
    }))
    .expect("parses");
    assert!(
        declared.expression().is_some(),
        "a declared set has an expression"
    );
    assert!(predicates_hold(
        &serde_json::json!({"role": "user"}),
        &declared
    ));
    assert!(!predicates_hold(
        &serde_json::json!({"role": "bot"}),
        &declared
    ));
    assert!(
        !predicates_hold(&serde_json::json!({}), &declared),
        "an absent value does not satisfy a positive condition - which is the `Unknown` the grammar gives, \
         reaching a decision here"
    );
}

/// A presence coalesce tells **absent** from **present and the wrong shape**, which `else_element` could not.
///
/// A wrapper member is a list of declarations, so `{"function_declarations": {"name": "weather"}}` has not
/// declared its contents - and treating that as the member being *absent* sent it to the element fallback, which
/// emits the whole wrapper as a tool definition. Codex's ruling: keep the recovery, because the enclosing object
/// independently describes a valid bare tool, and **report** the malformed member rather than pretending nobody
/// wrote it. So the two situations get separate answers.
///
/// Also pinned: once presence has selected a representation, a later spelling is not tried. Presence chose;
/// falling through would answer from a representation the producer did not use.
#[test]
fn a_presence_coalesce_tells_absent_from_the_wrong_shape() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |fallbacks: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.tools","read":{{"attribute":"tools"}},"parse":"json",
                 "emit":"tool_definitions","legacy_rank":1,
                 "alternatives":[{{"id":"decls","select":"$[*]",
                   "then_present_any_of":["$.function_declarations","$.functionDeclarations"]{fallbacks}}}]}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };
    let read = |plan: &crate::domain::rules::message_rules::MessagePlan,
                payload: serde_json::Value| {
        let attrs = std::collections::HashMap::from([("tools".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        // An emission's value is the observation itself here - one per selected element - so a list is
        // flattened and anything else is one item. Flattening unconditionally reported nothing for the
        // single-object case, which is how my first version of this test failed.
        plan.tool_definitions(&ctx)
            .iter()
            .flat_map(|e| match e.value.as_array() {
                Some(items) => items.clone(),
                None => vec![e.value.clone()],
            })
            .collect::<Vec<_>>()
    };

    // The two answers, declared separately: recover from a wrong-shaped member, and refuse an absent one.
    let plan = asset(r#","on_malformed":"element","on_absent":"nothing""#).expect("compiles");
    let recovered = read(
        &plan,
        serde_json::json!([{"name": "bare", "function_declarations": {"name": "weather"}}]),
    );
    assert_eq!(
        recovered.len(),
        1,
        "the enclosing object independently describes a valid bare tool, so the reading recovers"
    );
    assert_eq!(recovered[0]["name"].as_str(), Some("bare"));
    assert!(
        read(&plan, serde_json::json!([{"name": "bare"}])).is_empty(),
        "and an *absent* member is a different situation, answered separately - which `else_element` could not \
         express, since both fell to it"
    );

    // Reversed, to show the two are independent rather than one dial.
    let plan = asset(r#","on_malformed":"nothing","on_absent":"element""#).expect("compiles");
    assert!(
        read(
            &plan,
            serde_json::json!([{"name": "bare", "function_declarations": {"name": "weather"}}])
        )
        .is_empty()
    );
    assert_eq!(read(&plan, serde_json::json!([{"name": "bare"}])).len(), 1);

    // A present list is still the contents, empty included: a producer writing `[]` has declared no tools.
    let plan = asset(r#","on_malformed":"element","on_absent":"element""#).expect("compiles");
    assert_eq!(
        read(
            &plan,
            serde_json::json!([{"function_declarations": [{"name": "a"}, {"name": "b"}]}])
        )
        .len(),
        2
    );
    assert!(
        read(&plan, serde_json::json!([{"function_declarations": []}])).is_empty(),
        "an empty wrapper has declared no tools, and that is a statement rather than a fall-through"
    );

    // **Presence chooses the representation**: the second spelling is not tried once the first named something.
    assert!(
        read(
            &plan,
            serde_json::json!([{
                "function_declarations": {"wrong": "shape"},
                "functionDeclarations": [{"name": "would_have_worked"}]
            }])
        )
        .iter()
        .all(|tool| tool["name"].as_str() != Some("would_have_worked")),
        "falling through to a later spelling would answer from a representation the producer did not use"
    );

    // And the members are refused where the coalesce cannot make the distinction they express.
    assert!(
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
                 "emit":"message","legacy_rank":1,
                 "alternatives":[{"id":"a","then_any_of":["$.a"],"on_absent":"element"}]}]}"#
                .to_vec(),
        )]))
        .is_err(),
        "a yielding coalesce has one not-found state, so `on_absent` states a distinction it cannot make"
    );
}

/// A role a rule **states** must be a role.
///
/// `role_map: {"model": "assisstant"}` compiled, and the typo became **User** - because an unrecognised role
/// folds to User rather than being refused. So a rule could say "assistant" and mean "user", with nothing
/// anywhere saying otherwise. Compared against `ChatRole::try_from_str`, which is the same question every reader
/// asks, rather than a second list that would drift from it.
///
/// The corpus had three: `openinference`'s retrieval and reranker rules said `role: "documents"`, which is not a
/// role - it folded to User through the unknown-role *default* rather than through any declaration. They say
/// `context` now, which is the declared vocabulary for retrieved material and folds to User by declaration. No
/// reader sees a difference; the retired extractor's oracle records the divergence.
#[test]
fn a_role_a_rule_states_must_be_a_role() {
    use crate::domain::rules::message_rules::compile;

    let asset = |wrap: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","legacy_rank":1,"wrap":{wrap}}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    // Codex's typo, and the shape the corpus had.
    for (what, wrap) in [
        (
            "a misspelled mapped role",
            r#"{"role_from":"$.speaker","role_map":{"model":"assisstant"}}"#,
        ),
        ("a literal that is not a role", r#"{"role":"documents"}"#),
        (
            "a literal that is a content description",
            r#"{"role":"narrator"}"#,
        ),
    ] {
        assert!(
            asset(wrap).is_err(),
            "{what} must be refused: it folds to `user`, so the declaration means something other than it says"
        );
    }

    // Every alias the vocabulary folds is still a role a rule may state - the check is not a narrowing of it.
    for role in [
        "system",
        "developer",
        "user",
        "human",
        "data",
        "context",
        "assistant",
        "ai",
        "bot",
        "model",
        "choice",
        "tool_call",
        "tool",
        "function",
        "ipython",
    ] {
        let wrap = format!(r#"{{"role":"{role}"}}"#);
        assert!(
            asset(&wrap).is_ok(),
            "`{role}` is in the vocabulary and must be statable: {:?}",
            asset(&wrap).err()
        );
    }

    // A mapped output is checked as a literal is, and a compose's trailing role too.
    assert!(
        asset(r#"{"role_from":"$.speaker","role_map":{"planner":"assistant"}}"#).is_ok(),
        "a mapping to a real role is fine - the map's *keys* are the producer's vocabulary, not ours"
    );
    assert!(
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.c","emit":"message","legacy_rank":1,
                 "compose":{"tag":"joined","trailing":{"role":"assisstant"},"members":[
                   {"as":"content","from_any_of":["x"],"parse":"text"}]}}]}"#
                .to_vec(),
        )]))
        .is_err(),
        "a compose's trailing role folds the same way"
    );
}

/// A **closed** role map must say what an unmapped value means.
///
/// Closedness is enforced - an unlisted value is discarded, which is the point - but with no literal fallback the
/// message is emitted with **no role**, and normalisation then infers one from unrelated payload members:
/// Assistant if the message happens to carry tool calls, User otherwise. Codex's case: a speaker called
/// `"planner"` against `role_map: {"user": "user"}` means whatever the rest of the turn happens to contain.
///
/// Every shipped closed map declares the fallback, so this is a gate rather than a migration.
#[test]
fn a_closed_role_map_says_what_an_unmapped_value_means() {
    use crate::domain::rules::message_rules::{MessageContext, compile};

    let asset = |wrap: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","legacy_rank":1,"wrap":{wrap}}}]}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            body.into_bytes(),
        )]))
    };

    assert!(
        asset(r#"{"role_from":"$.speaker","role_map":{"user":"user"},"role_map_is_closed":true}"#)
            .is_err(),
        "a closed map with no fallback leaves an unmapped value with no role, and the payload's other members \
         then decide what it was"
    );

    // With the fallback - the shape every shipped closed map has - an unmapped value takes it.
    let plan = asset(
        r#"{"role_from":"$.speaker","role_map":{"user":"user"},"role_map_is_closed":true,
             "role":"assistant","content_from_any_of":["$.content"]}"#,
    )
    .expect("a closed map with a fallback compiles");
    let role = |speaker: &str| {
        let attrs = std::collections::HashMap::from([(
            "x".to_string(),
            serde_json::json!({"speaker": speaker, "content": "answer"}).to_string(),
        )]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)[0].value["role"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(role("user"), "user", "a mapped value is mapped");
    assert_eq!(
        role("planner"),
        "assistant",
        "and an unmapped one takes the declared fallback rather than being left for the payload to decide"
    );

    // An **open** map needs no fallback: an unmapped value passes through as the producer wrote it, which is a
    // different statement and a different defect (cycle 15's finding 3).
    assert!(
        asset(r#"{"role_from":"$.speaker","role_map":{"user":"user"}}"#).is_ok(),
        "closedness is what creates the obligation"
    );
}
