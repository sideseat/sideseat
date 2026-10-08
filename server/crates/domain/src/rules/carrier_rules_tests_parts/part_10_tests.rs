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
    use crate::rules::message_rules::{MessageContext, compile};
    use sideseat_core::constants::{RULE_MAX_EMISSIONS_PER_CARRIER, RULE_WALK_MAX_NODES};

    // A wide payload inside a shallow declared depth: 20,000 sibling objects at depth 1, well past the node
    // ceiling, with a walk that would otherwise visit every one.
    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.w", "read": {"attribute": "state"}, "parse": "json", "emit": "message", "priority": 1, "walk": {"max_depth": 3, "stop_on": ["as_message"]}, "also": [{"id": "as_message", "where": {"all": [{"path": "$.role"}, {"path": "$.content"}]}, "wrap": {"role_from": "$.role", "content_from": "$.content"}}]}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.each", "read": {"attribute": "turns"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "every", "select": "$[*]", "wrap": {"role": "user", "content_from": "$.text"}}]}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
/// A JSONPath is plural by nature: `$.*` matches every member, so a `tag_from: "$.*"` reads the *first* member
/// of several and the others are gone with nothing said. Fifteen sites took `query(...).into_iter().next()`,
/// and the same silent-first rule reaches a grouped `collect`, a `tag_from`, the indexed projections and
/// several constructors.
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.e","read":{"attribute":"x"},"parse":"json",
             "emit":"message","priority":1,
             "elements":{"passes":[{"id":"named","tag_from":"$.*"}]}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the probe compiles");
    let tags = |payload: &str| {
        let attrs = std::collections::HashMap::from([("x".to_string(), payload.to_string())]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .iter()
            .map(|e| e.carrier.name().to_string())
            .collect::<Vec<_>>()
    };

    // Two members sit in one element, and `$.*` matches both.
    assert_eq!(
        tags(r#"[{"a":"first","b":"second"}]"#),
        ["first".to_string()],
        "the first match is read and the second is not - which is the behaviour, reported now rather than \
         silent"
    );
    // One member is unambiguous, and answers the same way.
    assert_eq!(tags(r#"[{"a":"only"}]"#), ["only".to_string()]);
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
/// reaches a decision the shipped rules make. This remains open because completing it needs direct
/// `Expr` syntax at these fields, Logfire stating the absence it means explicitly, the bare-`none_of` branch
/// removed, and an explicitly empty group refused - which needs the schema to distinguish "declared empty" from
/// "not declared", and today it cannot.
///
/// Both assertions below are the **current** answers. Each is the opposite of what the completed migration
/// gives, so when that lands these flip, deliberately, rather than a claim quietly becoming true.
#[test]
fn the_predicate_semantics_have_not_migrated_and_here_is_what_still_answers_the_old_way() {
    use crate::rules::message_rules::predicates_hold;
    use crate::rules::schema::PredicateSet;

    // A bare `none_of` against a payload with no such member. Under the grammar's own rules the selection is
    // empty, so the question is `Unknown` and a negation over it does not hold - but the compatibility branch
    // makes absence an explicit `true`.
    let bare_none_of: PredicateSet = serde_json::from_value(serde_json::json!({
        "all": [{"path": "$.name", "none_of": ["bob"]}]
    }))
    .expect("a predicate set parses");
    assert!(
        predicates_hold(
            &serde_json::json!({}),
            &crate::rules::schema::ValueCondition::from_set(&bare_none_of)
        ),
        "today a missing value satisfies a bare `none_of`, because `logfire`'s element pass needs an event \
         with no name to pass a `none_of` on its name - stated as `any(not exists, some(not one_of))` rather \
         than left to a negation that quietly accepts absence"
    );
    // And it still answers `false` where the member *is* there and matches, or the branch would be a blanket
    // yes rather than a statement about absence.
    assert!(
        !predicates_hold(
            &serde_json::json!({"name": "bob"}),
            &crate::rules::schema::ValueCondition::from_set(&bare_none_of)
        ),
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
        predicates_hold(
            &serde_json::json!({}),
            &crate::rules::schema::ValueCondition::from_set(&explicit_empty)
        ),
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
        &crate::rules::schema::ValueCondition::from_set(&declared)
    ));
    assert!(!predicates_hold(
        &serde_json::json!({"role": "bot"}),
        &crate::rules::schema::ValueCondition::from_set(&declared)
    ));
    assert!(
        !predicates_hold(
            &serde_json::json!({}),
            &crate::rules::schema::ValueCondition::from_set(&declared)
        ),
        "an absent value does not satisfy a positive condition - which is the `Unknown` the grammar gives, \
         reaching a decision here"
    );
}

/// A presence coalesce answers an absent member and a wrong-shaped one through `else_element`.
///
/// A wrapper member is a list of declarations, so `{"function_declarations": {"name": "weather"}}` has not
/// declared its contents. With `else_element` the reading recovers from the enclosing object, which
/// independently describes a valid bare tool; without it the element is not this shape. Separate answers for
/// the two situations (`on_absent` / `on_malformed`) existed and no asset used them.
///
/// Also pinned: once presence has selected a representation, a later spelling is not tried. Presence chose;
/// falling through would answer from a representation the producer did not use.
#[test]
fn a_presence_coalesce_falls_back_to_the_element_only_where_declared() {
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |fallbacks: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.tools","read":{{"attribute":"tools"}},"parse":"json",
                 "emit":"tool_definitions","priority":1,
                 "alternatives":[{{"id":"decls","select":"$[*]",
                   "then_select":{{"first_of":["$.function_declarations", "$.functionDeclarations"]}}{fallbacks}}}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let read = |plan: &crate::rules::message_rules::MessagePlan, payload: serde_json::Value| {
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

    // Declared: both an absent member and a wrong-shaped one fall back to the element.
    let plan = asset(r#","else_element":true"#).expect("compiles");
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
    assert_eq!(read(&plan, serde_json::json!([{"name": "bare"}])).len(), 1);

    // Undeclared: neither falls back.
    let plan = asset("").expect("compiles");
    assert!(
        read(
            &plan,
            serde_json::json!([{"name": "bare", "function_declarations": {"name": "weather"}}])
        )
        .is_empty()
    );
    assert!(read(&plan, serde_json::json!([{"name": "bare"}])).is_empty());

    // A present list is still the contents, empty included: a producer writing `[]` has declared no tools.
    let plan = asset(r#","else_element":true"#).expect("compiles");
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

    // And the fallback is refused where there is no coalesce for it to answer.
    assert!(
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
                 "emit":"message","priority":1,
                 "alternatives":[{"id":"a","else_element":true}]}]}"#
                    .to_vec(),
            )]))
            .expect("the probe assets parse")
        )
        .is_err(),
        "`else_element` with no `then_present_any_of` names the fallback for a coalesce that is not there"
    );
}

/// A probe corpus with the shipped role vocabulary beside it: which spellings mean a role is declared, so a probe
/// stating an alias needs the declaration a real corpus carries.
fn with_role_vocabulary(
    mut probe: std::collections::BTreeMap<String, Vec<u8>>,
) -> std::collections::BTreeMap<String, Vec<u8>> {
    let (path, bytes) = crate::rules::schema::embedded_sources()
        .into_iter()
        .find(|(path, _)| path.ends_with("role-authority.json"))
        .expect("the role vocabulary ships");
    probe.insert(path, bytes);
    probe
}

/// A role a rule **states** must be a role.
///
/// `role_map: {"model": "assisstant"}` compiled, and the typo became **User** - because an unrecognised role
/// folds to User rather than being refused. So a rule could say "assistant" and mean "user", with nothing
/// anywhere saying otherwise. Compared against the canonical spellings and the corpus's declared meanings, which is
/// the question `ChatRole::try_from_str` asks every reader, rather than a second list that would drift from it.
///
/// The corpus had three: `openinference`'s retrieval and reranker rules said `role: "documents"`, which is not a
/// role - it folded to User through the unknown-role *default* rather than through any declaration. They say
/// `context` now, which is the declared vocabulary for retrieved material and folds to User by declaration. No
/// reader sees a difference; the retired extractor's oracle records the divergence.
#[test]
fn a_role_a_rule_states_must_be_a_role() {
    use crate::rules::message_rules::compile;

    let asset = |wrap: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","priority":1,"wrap":{wrap}}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&with_role_vocabulary(std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )])))
            .expect("the probe assets parse"),
        )
    };

    // Preserve the misspelling found in the captured corpus.
    for (what, wrap) in [
        (
            "a misspelled mapped role",
            r#"{"role_from":{"path":"$.speaker","pipe":[{"map":{"model":"assisstant"}}]}}"#,
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
        asset(r#"{"role_from":{"path":"$.speaker","pipe":[{"map":{"planner":"assistant"}}]}}"#)
            .is_ok(),
        "a mapping to a real role is fine - the map's *keys* are the producer's vocabulary, not ours"
    );
    assert!(
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                br#"{"id": "t", "messages": [{"id": "t.c", "emit": "message", "priority": 1, "compose": {"tag": "joined", "trailing": {"role": "assisstant"}, "members": [{"as": "content", "from": "x", "parse": "text"}]}}]}"#
                    .to_vec(),
            )]))
            .expect("the probe assets parse")
        )
        .is_err(),
        "a compose's trailing role folds the same way"
    );
}

/// A **closed** role map must say what an unmapped value means.
///
/// Closedness is enforced - an unlisted value is discarded, which is the point - but with no literal fallback the
/// message is emitted with **no role**, and normalisation then infers one from unrelated payload members:
/// Assistant if the message happens to carry tool calls, User otherwise. Counterexample: a speaker called
/// `"planner"` against `role_map: {"user": "user"}` means whatever the rest of the turn happens to contain.
///
/// Every shipped closed map declares the fallback, so this is a gate rather than a migration.
#[test]
fn a_closed_role_map_says_what_an_unmapped_value_means() {
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |wrap: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","priority":1,"wrap":{wrap}}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };

    assert!(
        asset(
            r#"{"role_from":{"path":"$.speaker","pipe":[{"map":{"user":"user"},"closed":true}]}}"#
        )
        .is_err(),
        "a closed map with no fallback leaves an unmapped value with no role, and the payload's other members \
         then decide what it was"
    );

    // With the fallback - the shape every shipped closed map has - an unmapped value takes it.
    let plan = asset(
        r#"{"role_from": {"path": "$.speaker", "pipe": [{"map": {"user": "user"}, "closed": true}]}, "role": "assistant", "content_from": "$.content"}"#,
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
    // different statement and a different defect.
    assert!(
        asset(r#"{"role_from":{"path":"$.speaker","pipe":[{"map":{"user":"user"}}]}}"#).is_ok(),
        "closedness is what creates the obligation"
    );
}

/// Global authority is owned by the asset entitled to it, and asset ids are unique.
///
/// Three rules about a declaration that would change a *shared* answer from a place with no standing to change it.
///
/// `role_authority` is this engine's own vocabulary - which stated roles outrank the name a reading was found
/// under - and it compiles into one global plan, so a producer's asset could add an authoritative spelling and
/// change role resolution for every unrelated producer. Exactly the shape `convention_namespaces` already had, one
/// section over, and it went unnoticed when that section was added.
///
/// And an asset id is the provenance every diagnostic reports, while `declaration_defect` is per file - so two
/// files could declare the same one and their clauses would be indistinguishable precisely where a reader looks to
/// tell them apart.
#[test]
fn a_shared_answer_is_declared_only_by_the_asset_that_owns_it() {
    use schema::RuleFile;
    let parse =
        |asset: &str| -> RuleFile { serde_json::from_str(asset).expect("the probe parses") };

    // A producer's asset may not grant role authority.
    let intruder = parse(
        r#"{"id":"acme","role_authority":[{"id":"acme.r","role":"narrator","outranks_a_tag":true}]}"#,
    );
    let defect = intruder
        .declaration_defect()
        .expect("a producer's asset may not declare role authority");
    assert!(
        defect.contains("role_authority") && defect.contains("role-authority"),
        "the refusal should name the section and the asset entitled to it: {defect}"
    );

    // The owning asset may.
    assert!(
        parse(
            r#"{"id":"role-authority","role_authority":[{"id":"r.r","role":"user","outranks_a_tag":true}]}"#
        )
        .declaration_defect()
        .is_none()
    );

    // The same rule the conventions' asset already had, asserted beside it so the pair cannot drift.
    assert!(
        parse(r#"{"id":"acme","convention_namespaces":["acme."]}"#)
            .declaration_defect()
            .is_some()
    );
    assert!(
        parse(r#"{"id":"semconv","convention_namespaces":["gen_ai."]}"#)
            .declaration_defect()
            .is_none()
    );

    // Two assets declaring one id, which `declaration_defect` cannot see because it is per file.
    let corpus = |ids: [&str; 2]| {
        ParsedAssets::parse(&std::collections::BTreeMap::from([
            (
                "a.json".to_string(),
                format!(r#"{{"id":"{}"}}"#, ids[0]).into_bytes(),
            ),
            (
                "b.json".to_string(),
                format!(r#"{{"id":"{}"}}"#, ids[1]).into_bytes(),
            ),
        ]))
    };
    let refused =
        corpus(["acme", "acme"]).expect_err("two assets sharing an id share a provenance path");
    assert!(refused.to_string().contains("`acme`"), "{refused}");
    assert!(corpus(["acme", "other"]).is_ok());

    // And the shipped assets satisfy all of it, which is what makes these rules statements about them rather than
    // only about future edits.
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        assert!(
            file.declaration_defect().is_none(),
            "{path}: {}",
            file.declaration_defect().unwrap_or_default()
        );
    }
}

/// Every classification refusal fires.
///
/// Three of the eight were exercised by nothing, so each was a claim rather than a guard. Matched on the variant
/// rather than the message, so rewording a diagnostic does not quietly stop testing it.
#[test]
fn every_classification_refusal_fires() {
    use crate::rules::classify::{ClassifyCompileError as E, compile};
    let compiled = |asset: &str| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                asset.as_bytes().to_vec(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        (
            "a rule whose condition reads the resource, which classification is never given",
            r#"{"id": "t", "observation_types": [{"id": "r", "priority": 1, "where": {"source": "resource:service.name", "contains": "x"}, "result": "tool"}]}"#,
            |e| matches!(e, E::DeadCondition { .. }),
        ),
        (
            "a rule whose result is not one of the answers this classification may give",
            r#"{"id": "t", "observation_types": [{"id": "r", "priority": 1, "where": {"source": "attr:k", "exists": true}, "result": "narrator"}]}"#,
            |e| matches!(e, E::UnknownResult { .. }),
        ),
        (
            "a rule with no result at all",
            r#"{"id": "t", "observation_types": [{"id": "r", "priority": 1, "where": {"source": "attr:k", "exists": true}, "result": ""}]}"#,
            |e| matches!(e, E::NoResult { .. }),
        ),
        (
            "a condition that can never hold",
            r#"{"id": "t", "observation_types": [{"id": "r", "priority": 1, "where": {"source": "attr_keys", "starts_with": ""}, "result": "tool"}]}"#,
            |e| matches!(e, E::DeadCondition { .. }),
        ),
        (
            "a condition naming a resource dimension classification is never given",
            r#"{"id": "t", "observation_types": [{"id": "r", "priority": 1, "where": {"source": "resource:service.name", "contains": "x"}, "result": "tool"}]}"#,
            |e| matches!(e, E::DeadCondition { .. }),
        ),
        (
            "two rules of one classification sharing a rank",
            r#"{"id": "t", "observation_types": [{"id": "a", "priority": 1, "where": {"source": "attr:k", "exists": true}, "result": "tool"}, {"id": "b", "priority": 1, "where": {"source": "attr:j", "exists": true}, "result": "agent"}]}"#,
            |e| matches!(e, E::SharedPriority { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id": "t", "observation_types": [{"id": "a", "priority": 1, "where": {"source": "attr:k", "exists": true}, "result": "tool"}, {"id": "a", "priority": 2, "where": {"source": "attr:j", "exists": true}, "result": "agent"}]}"#,
            |e| matches!(e, E::DuplicateId { .. }),
        ),
        (
            "a rule an earlier rule always satisfies, whose result can never be reached",
            r#"{"id": "t", "observation_types": [{"id": "a", "priority": 1, "where": {"source": "attr:k", "exists": true}, "result": "tool"}, {"id": "b", "priority": 2, "where": {"source": "attr:k", "equals": "v"}, "result": "agent"}]}"#,
            |e| matches!(e, E::ShadowedRule { .. }),
        ),
    ];
    for (what, asset, expected) in cases {
        let error = compiled(asset)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }
    // A pair of ordinary rules compiles, or the refusals are simply a ban.
    assert!(
        compiled(
            r#"{"id": "t", "observation_types": [{"id": "a", "priority": 1, "where": {"source": "attr:one", "exists": true}, "result": "tool"}, {"id": "b", "priority": 2, "where": {"source": "attr:two", "exists": true}, "result": "agent"}]}"#
        )
        .is_ok()
    );
}

/// Every tool-shape refusal fires.
///
/// Five of the six were exercised by nothing. `Inexpressible` is the shared predicate validator, which
/// `every_predicate_set_in_the_schema_is_validated` requires every section to run - so a section that stopped
/// running it would pass that test and lose the refusal, which is why it is asked here directly.
#[test]
fn every_tool_shape_refusal_fires() {
    use crate::rules::tool_shapes::{ToolShapeError as E, ToolShapePlan};
    let compiled = |shapes: &str| {
        let asset = format!(r#"{{"id":"t","tool_shapes":{shapes}}}"#);
        let file: schema::RuleFile = serde_json::from_str(&asset).expect("the probe parses");
        ToolShapePlan::compile(std::slice::from_ref(&file))
    };
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        (
            "a shape handing over a canonical object *and* saying where each part is",
            r#"[{"id":"s","priority":1,"function":"$.function","name":"$.name"}]"#,
            |e| matches!(e, E::TwoAnswers { .. }),
        ),
        (
            "a shape with no name, which is not a definition",
            r#"[{"id":"s","priority":1,"description":"$.d"}]"#,
            |e| matches!(e, E::NoName { .. }),
        ),
        (
            "two shapes sharing a rank, where load order would decide",
            r#"[{"id":"a","priority":1,"name":"$.name"},{"id":"b","priority":1,"name":"$.n"}]"#,
            |e| matches!(e, E::SharedPriority { .. }),
        ),
        (
            "an empty carried member name, which names nothing",
            r#"[{"id":"s","priority":1,"name":"$.name","carry":[""]}]"#,
            |e| matches!(e, E::EmptyCarry { .. }),
        ),
        (
            "parameters with no path to read them from",
            r#"[{"id": "s", "priority": 1, "name": "$.name", "parameters": {"encoding": "json_schema"}}]"#,
            |e| matches!(e, E::NoParameterPath { .. }),
        ),
        (
            "a requirement that could never mean what it says",
            r#"[{"id": "s", "priority": 1, "name": "$.name", "where": {"all": [{"not_null": true}, {"not_null": false}]}}]"#,
            |e| matches!(e, E::Inexpressible { .. }),
        ),
    ];
    for (what, shapes, expected) in cases {
        let error = compiled(shapes)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }
    assert!(compiled(r#"[{"id":"s","priority":1,"name":"$.name","carry":["strict"]}]"#).is_ok());
}

/// Every carrier refusal fires.
///
/// Four of the nine were exercised by nothing. `IncoherentFacts` is the one that matters most: it is what refuses
/// an impossible fact vector, and the presets are constructors over those facts rather than a vocabulary - so a
/// preset override producing a combination the model cannot mean is the shape it exists for.
#[test]
fn every_carrier_refusal_fires() {
    let compiled = |carriers: serde_json::Value| {
        let asset = serde_json::json!({"id": "probe", "doc": "d", "carriers": carriers});
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                serde_json::to_vec(&asset).expect("serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let clause = |id: &str, extra: serde_json::Value, facts: serde_json::Value| {
        let mut entry = serde_json::json!({
            "id": id, "doc": "d", "match": {"attribute": format!("probe.{id}")}, "facts": facts,
        });
        if let (Some(object), Some(more)) = (entry.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                object.insert(key.clone(), value.clone());
            }
        }
        entry
    };
    type Case = (&'static str, serde_json::Value, fn(&CompileError) -> bool);
    let cases: Vec<Case> = vec![
        (
            "an impossible fact vector",
            serde_json::json!([clause(
                "a",
                serde_json::json!({}),
                serde_json::json!({"preset": "emission", "carrier_is_atomic_emission": false,
                                   "position_proves_distinct_occurrence": true,
                                   "position_provides_sequence_order": false,
                                   "carrier_holds_expandable_message_array": true})
            )]),
            |e| matches!(e, CompileError::IncoherentFacts { .. }),
        ),
        (
            "an observation type that is not one",
            serde_json::json!([{
                "id": "a", "doc": "d",
                "match": {"attribute": "probe.a", "observation_type": ["narrator"]},
                "facts": {"preset": "emission"},
            }]),
            |e| matches!(e, CompileError::UnknownObservationType { .. }),
        ),
        (
            "an empty attribute, which would claim every observation of a span",
            serde_json::json!([{
                "id": "a", "doc": "d", "match": {"attribute": ""}, "facts": {"preset": "emission"},
            }]),
            |e| matches!(e, CompileError::EmptyLiteral { .. }),
        ),
        (
            "two clauses sharing an id",
            serde_json::json!([
                clause("a", serde_json::json!({}), serde_json::json!({"preset": "emission"})),
                {"id": "a", "doc": "d", "match": {"attribute": "probe.b"},
                 "facts": {"preset": "emission"}},
            ]),
            |e| matches!(e, CompileError::DuplicateClauseId { .. }),
        ),
        (
            "two clauses that could claim one observation with equal specificity",
            serde_json::json!([
                clause("a", serde_json::json!({}), serde_json::json!({"preset": "emission"})),
                {"id": "b", "doc": "d", "match": {"attribute": "probe.a"},
                 "facts": {"preset": "snapshot"}},
            ]),
            |e| matches!(e, CompileError::Ambiguous { .. }),
        ),
    ];
    for (what, carriers, expected) in cases {
        let error = compiled(carriers)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }
    // An ordinary pair that must compile. Malformed input is refused by `ParsedAssets::parse`, before any section.
    assert!(
        compiled(serde_json::json!([
            clause(
                "a",
                serde_json::json!({}),
                serde_json::json!({"preset": "emission"})
            ),
            clause(
                "b",
                serde_json::json!({}),
                serde_json::json!({"preset": "snapshot"})
            ),
        ]))
        .is_ok()
    );
}

/// Every message-rule refusal fires.
///
/// Six of the eight were exercised by nothing, `StarvedReading` among them - the one that refuses a lower-ranked
/// rule taking part of an all-or-nothing reading, so the reading is lost *whole* and the carriers the taker never
/// wanted reach nobody.
#[test]
fn every_message_rule_refusal_fires() {
    use crate::rules::message_rules::{MessageCompileError as E, compile};
    let compiled = |asset: &str| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                asset.as_bytes().to_vec(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        (
            "a rule naming no carrier",
            r#"{"id":"t","messages":[{"id":"r","read":{},"parse":"json","emit":"message","priority":1}]}"#,
            |e| matches!(e, E::NotExactlyOneCarrier { .. }),
        ),
        (
            "a rule naming an empty carrier, which names nothing",
            r#"{"id":"t","messages":[{"id":"r","read":{"attribute":""},"parse":"json","emit":"message","priority":2}]}"#,
            |e| matches!(e, E::EmptyCarrier { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id":"t","messages":[
               {"id":"r","read":{"attribute":"a"},"parse":"json","emit":"message","priority":3},
               {"id":"r","read":{"attribute":"b"},"parse":"json","emit":"message","priority":4}]}"#,
            |e| matches!(e, E::DuplicateRuleId { .. }),
        ),
        (
            "a reading referencing a fragment nobody declares",
            r#"{"id":"t","messages":[{"id":"r","read":{"attribute":"a"},"parse":"json","emit":"message","priority":5,
               "alternatives":[{"id":"r.alt","then_fragment":"absent.fragment"}]}]}"#,
            |e| matches!(e, E::UnknownFragment { .. }),
        ),
        (
            // The engine accepts the construct and cannot execute it, which from a reader's point of view is the
            // same defect as a field silently ignored: the asset says something and nothing happens.
            "a fragment declaring no cases",
            r#"{"id":"t","messages":[],"fragments":{"probe.frag":{"cases":[]}}}"#,
            |e| matches!(e, E::Inexpressible { .. }),
        ),
        (
            "two rules reading one carrier, where the later can never be reached",
            r#"{"id":"t","messages":[
               {"id":"a","read":{"attribute":"same"},"parse":"json","emit":"message","priority":1},
               {"id":"b","read":{"attribute":"same"},"parse":"json","emit":"message","priority":2}]}"#,
            |e| matches!(e, E::ContestedCarrier { .. }),
        ),
    ];
    for (what, asset, expected) in cases {
        let error = compiled(asset)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }
    assert!(
        compiled(
            r#"{"id":"t","messages":[
               {"id":"a","read":{"attribute":"one"},"parse":"json","emit":"message","priority":7},
               {"id":"b","read":{"attribute":"two"},"parse":"json","emit":"message","priority":8}]}"#
        )
        .is_ok()
    );
}
