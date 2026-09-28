
/// The *extraction* layer names no framework either.
///
/// `the_engine_names_no_framework` reads the rules engine; this reads the code that calls it. That was the
/// remaining gap: every framework message and tool-definition carrier had moved into the assets, and
/// nothing held the file to it - a new hardcoded carrier key would compile, pass, and quietly re-open the
/// hole. Measured on the source, ignoring `#[cfg(test)]` items, which are the retired reference
/// implementations the equivalence oracles compare against and legitimately name every dialect.
#[test]
fn message_extraction_names_no_framework() {
    // Both extraction files. `attributes.rs` reached zero the same way `messages.rs` did - every chain, table
    // and sweep moved to an asset - and a *measured* zero decays, so it is gated by the same instrument.
    const SOURCES: &[(&str, &str)] = &[
        (
            "messages.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../ingestion/src/traces/extract/messages.rs"
            )),
        ),
        (
            "attributes.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../ingestion/src/traces/extract/attributes.rs"
            )),
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
        for (number, token) in production_names(source, file) {
            let token = token.trim();
            if STATED_EXCEPTIONS.contains(&token) {
                continue;
            }
            // Case-insensitively, because a constant is upper case and a key is lower, and the same fact must
            // not evade the gate by how it happens to be spelled.
            let folded = token.to_ascii_lowercase();
            if let Some(marker) = FRAMEWORK_MARKERS
                .iter()
                .find(|m| names_marker(&folded, &m.to_ascii_lowercase()))
            {
                offenders.push(format!("  {file}:{}: {} <- `{marker}`", number, token));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "extraction names {} framework fact(s) in production code. Every carrier, counter, spelling and \
         precedence a framework writes belongs in `server/assets/rules/*.json`:\n{}",
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
    fn append_rust_sources(directory: &std::path::Path, source: &mut String) {
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
            .expect("schema module directory")
            .map(|entry| entry.expect("schema module entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                append_rust_sources(&path, source);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                source.push('\n');
                source.push_str(&std::fs::read_to_string(path).expect("schema module source"));
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rules/schema.rs");
    let mut schema = std::fs::read_to_string(&root).expect("schema module root");
    append_rust_sources(&root.with_extension(""), &mut schema);

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
    for line in schema.lines() {
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
    use crate::rules::schema::{PredicateSet, ValuePredicate};

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
            with(|p| p.kind = Some(crate::rules::schema::ValueKind::String)),
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
            crate::rules::message_rules::predicate_defect(&set).is_some(),
            "`exists: false` beside `{name}` compiled, and the condition is ignored at runtime"
        );
    }

    // `none_of` is the exception, and it is documented: its reading accepts absence, which is how a
    // dialect's unnamed events fall through to the reading that handles them.
    let none_of = with(|p| p.none_of = vec!["a".to_string()]);
    assert!(
        crate::rules::message_rules::predicate_defect(&none_of).is_none(),
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
    use crate::rules::message_rules::predicate_defect;
    use crate::rules::schema::PredicateSet;
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
    "role-authority",
];

/// Assets naming a **provider** rather than a framework. The pricing catalogue is entitled to those names,
/// and each is checked against the catalogue's own table - so a *framework* cannot hide here either.
///
/// Exclusions rather than a list of frameworks, because the frameworks are the part that grows: a new asset
/// is a new name the sweep must know, and deriving it means adding one cannot be forgotten. And exclusions
/// need a property each, or the list is a way to make the sweep quiet.
const PROVIDERS: &[&str] = &["bedrock", "azure-openai", "vertex-ai"];
