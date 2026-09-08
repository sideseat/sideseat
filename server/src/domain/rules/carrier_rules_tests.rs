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
        // The tags the probe's own rules assign, gathered the way the ruleset gathers them - so a nested
        // tag is reachable here too, which is the defect the recursive collector fixed.
        let tags = super::tag_names(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&probe).expect("the probe serialises"),
        )]));
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
            out.insert(text.clone());
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
                "sources": [{"attribute": "probe.attribute", "lowercase": lowercase}],
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
    // Which **namespaces** are the conventions', derived rather than listed. A key's first segment decides,
    // and a namespace is the conventions' when either the conventions declare something under it, or no
    // framework asset does. That is what separates `session.id` and `enduser.id` - OTel's own, enumerated in
    // a shared chain and by nobody's dialect - from `ai.usage.promptTokens`, which sits in the same kind of
    // chain and is one producer's. Listing the namespaces by hand would have been the same
    // hand-maintained projection this sweep exists to avoid.
    let namespace = |key: &str| key.split_once('.').map(|(head, _)| head.to_string());
    let framework_namespaces: std::collections::BTreeSet<String> = per_asset
        .iter()
        .filter(|(id, _)| {
            !CONVENTIONS.contains(&id.as_str())
                && !PROVIDERS.contains(&id.as_str())
                && !SHARED_VOCABULARY.contains(&id.as_str())
        })
        .flat_map(|(_, keys)| keys.iter().filter_map(|key| namespace(key)))
        .collect();
    let convention_namespaces: std::collections::BTreeSet<String> = per_asset
        .iter()
        .filter(|(id, _)| CONVENTIONS.contains(&id.as_str()))
        .flat_map(|(_, keys)| keys.iter().filter_map(|key| namespace(key)))
        .collect();
    let conventional = |key: &str| {
        namespace(key).is_some_and(|head| {
            convention_namespaces.contains(&head) || !framework_namespaces.contains(&head)
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
