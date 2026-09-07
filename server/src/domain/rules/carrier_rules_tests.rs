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
    const SOURCE: &str = include_str!("../traces/extract/messages.rs");

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
    let mut in_test_item = false;
    let mut seen_open = false;
    let mut test_depth: i32 = 0;
    let mut pending_test = false;
    for (number, line) in SOURCE.lines().enumerate() {
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
            offenders.push(format!("  {}: {} <- `{marker}`", number + 1, trimmed));
        }
    }

    assert!(
        offenders.is_empty(),
        "message extraction names {} framework fact(s) in production code. Every carrier a framework \
         writes belongs in `server/rules/*.json`:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}
