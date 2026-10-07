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
    use crate::rules::span_fields::{Reading, compile};

    // Two sources with opposite witnesses on one carrier - the shape of a producer discriminator.
    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","span_fields":[{"id":"t.rule","target":"gen_ai_system","sources":[
             {"id":"t.anthropic","when_json":{"attribute":"request_data","path":"$.system"},
              "value":"anthropic"},
             {"id":"t.openai","when_json":{"attribute":"request_data","path":"$.messages"},
              "value":"openai"}]}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
        matches!(
            refused[0].cause,
            crate::rules::refusal::Unusable::UnanswerableGate { .. }
        ),
        // The cause this test's own name asserts. It checked `Malformed` before, because that was the only
        // word the vocabulary had: a witness is a *discriminator*, and what failed is the question rather than
        // the answer - which is what `UnanswerableGate` says and what a reader needs, since the two imply
        // different fixes (fix the producer's value, versus this branch could not be decided at all).
        "as an unanswerable gate: {:?}",
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
/// Without it: text for a compose, JSON for an ordinary read or a `tool_repr`, JSON-or-string for a (since
/// removed) named family. So one absent declaration was three different decisions, and which one applied was a property of a
/// **sibling** member - the same defect `attribute_any_of` had, where the multiplicity of a carrier list
/// depended on whether `tool_repr` sat beside it.
#[test]
fn a_reading_that_parses_a_scalar_declares_how() {
    use crate::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };

    // Each form that parses a scalar of its own, refused without a mode.
    for (what, read) in [
        ("an exact attribute", r#"{"attribute":"x"}"#),
        (
            "ordered alternatives",
            r#"{"attribute":{"first_of":["x", "y"]}}"#,
        ),
    ] {
        let rule = format!(r#"{{"id":"t.r","read":{read},"emit":"message","priority":1}}"#);
        assert!(
            asset(&rule).is_err(),
            "{what} parses a raw string and must say how"
        );
        let with_mode =
            format!(r#"{{"id":"t.r","read":{read},"parse":"json","emit":"message","priority":1}}"#);
        assert!(
            asset(&with_mode).is_ok(),
            "{what} compiles once the mode is stated"
        );
    }

    // A compose member naming carriers reads one of their strings, so the mode is the member's.
    assert!(
        asset(
            r#"{"id": "t.c", "compose": {"tag": "joined", "members": [{"as": "a", "from": "x"}]}, "emit": "message", "priority": 1}"#
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
            r#"{"id": "t.c", "compose": {"tag": "joined", "members": [{"as": "a", "from": "x", "parse": "text"}, {"sweep_prefix": "p.", "except": ["a"]}]}, "emit": "message", "priority": 1}"#,
        ),
        (
            "an indexed family, which assembles entries from keys rather than parsing one string",
            r#"{"id":"t.f","read":{"indexed_family":"fam"},"emit":"message","priority":1}"#,
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.compose", "priority": 1, "compose": {"tag": "canonical.response", "members": [{"as": "content", "from": "x", "parse": "text"}]}, "emit": "message"}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
/// content, post-content attachments - and every insert **overwrites**. The conflicting case compiled and produced a
/// message whose role is its content, with the two declarations before it silently discarded. Attachments could
/// overwrite literals, the content and each other; `compose.trailing` could overwrite a named or swept member.
///
/// The corpus-reachable one was `vercel-ai.response`: its sweep took `ai.response.role` into the `role` member
/// and `trailing.role` then overwrote it. Output-neutral to fix, because the overwrite always happened - but the
/// declaration was stating something untrue about what the rule reads.
#[test]
fn two_declarations_must_not_write_one_output_member() {
    use crate::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let read = r#""read":{"attribute":"x"},"parse":"json","emit":"message","priority":1"#;

    // A literal overwrites the declared role, then content overwrites both.
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
            r#"{"id": "t.c", "emit": "message", "priority": 1, "compose": {"tag": "joined", "trailing": {"role": "assistant"}, "members": [{"as": "role", "from": "x", "parse": "text"}]}}"#
        )
        .is_err(),
        "trailing is inserted last, so it discards the member's value"
    );

    // A sweep that does not exclude a fixed output name. A swept name is only known at read time, so excluding
    // them is the only way a sweep can state that it will not overwrite one.
    assert!(
        asset(
            r#"{"id": "t.c", "emit": "message", "priority": 1, "compose": {"tag": "joined", "trailing": {"role": "assistant"}, "members": [{"as": "content", "from": "x", "parse": "text"}, {"sweep_prefix": "p."}]}}"#
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
            r#"{"id": "t.c", "emit": "message", "priority": 1, "compose": {"tag": "joined", "trailing": {"role": "assistant"}, "members": [{"as": "content", "from": "x", "parse": "text"}, {"sweep_prefix": "p.", "except": ["content", "role"]}]}}"#
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
/// before. An indexed family accepted a `fallback`, a walk and `sections`.
///
/// And an element pass could state something other than what it did five different ways, the worst being a
/// decision table that answers for an element and has no tag for the answer - discarding a run the rule matched
/// on purpose.
#[test]
fn a_construction_branch_refuses_the_siblings_it_would_skip() {
    use crate::rules::message_rules::compile;

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
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
                 "emit":"message","priority":1}}"#
        );
        assert!(
            asset(&rule).is_err(),
            "`sections` returns before {what}, so declaring it is dead"
        );
    }
    // The branch itself still compiles, or the refusals are a ban on sections.
    let plain = format!(
        r#"{{"id":"t.s","read":{{"attribute":"x"}},"parse":"text",{sections},
             "emit":"message","priority":1}}"#
    );
    assert!(
        asset(&plain).is_ok(),
        "sections alone: {:?}",
        asset(&plain).err()
    );

    // The five element-pass shapes.
    let elements = |passes: &str| {
        format!(
            r#"{{"id":"t.e","read":{{"attribute":"x"}},"parse":"json",
                 "elements":{{"passes":{passes}}},"emit":"message","priority":1}}"#
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
            r#"[{"id":"p","tag_from":"$.n","group":{"by":[{"id":"c","value":"user"}],
               "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#,
        ),
        (
            "an empty decision table, so every run is empty",
            r#"[{"id":"p","group":{"by":[],"collect":"$.d","key_as":"role","tag_by_key":{}}}]"#,
        ),
        (
            "a derived value with no tag, so the run it matched is discarded",
            r#"[{"id":"p","group":{"by":[{"id":"c","value":"user"}],
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
                {"id":"grouped","group":{"by":[{"id":"c","value":"user"}],
                 "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#
        ))
        .is_ok(),
        "a tagging pass beside a grouping pass is the shipped shape: {:?}",
        asset(&elements(
            r#"[{"id":"named","tag_from":"$.n"},
                {"id":"grouped","group":{"by":[{"id":"c","value":"user"}],
                 "collect":"$.d","key_as":"role","tag_by_key":{"user":"gen_ai.user.message"}}}]"#
        ))
        .err()
    );
}

/// A reading that **cannot be built** produced nothing, so the chain keeps going.
///
/// Counterexample: the first alternative selects something and its envelope names a member the payload has not, so
/// `wrapped()` returns `None`. The candidate used to be returned as *the* answer and dropped afterwards by the
/// caller, so the second alternative and the rule's `fallback` were never tried and the rule emitted nothing -
/// where a later shape would have worked. Construction happens inside the coalesce now, which makes "could not
/// be built" the same answer as "this shape does not match": the only one a coalesce can act on.
#[test]
fn a_reading_whose_envelope_cannot_be_built_lets_the_chain_continue() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id": "t", "messages": [{"id": "t.r", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "first", "select": "$.a", "wrap": {"role": "user", "content_from": "$.missing"}}, {"id": "second", "select": "$.b", "wrap": {"role": "user", "content_from": "$.text"}}]}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
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
    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id": "t", "messages": [{"id": "t.r", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "first", "select": "$.a", "wrap": {"role": "user", "content_from": "$.missing"}}], "fallback": [{"id": "last", "select": "$.b", "wrap": {"role": "user", "content_from": "$.text"}}]}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
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
    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id": "t", "messages": [{"id": "t.r", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "only", "select": "$.a", "wrap": {"role": "user", "content_from": "$.missing"}}]}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
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
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |rule: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rule}]}}"#);
        compile(
            &ParsedAssets::parse(&with_role_vocabulary(std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )])))
            .expect("the probe assets parse"),
        )
    };

    // The rule's envelope applies to the array.
    let plan = asset(
        r#"{"id":"t.a","read":{"attribute":"docs"},"parse":"json","aggregate_into_array":true,
             "wrap":{"role":"data"},"emit":"message","priority":1,
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
                 "emit":"message","priority":1,
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
/// case id. Required ids were therefore still discarded before an emission existed, one level below the
/// earlier fix.
///
/// And an emission carries an `EvidenceSet` rather than one path, because some emissions genuinely have several
/// contributing clauses: two cases deriving one key are legitimate aliases, so a run built from both has two
/// witnesses and naming one of them claims it produced blocks it did not match.
#[test]
fn an_alternative_and_a_grouped_run_name_every_clause_that_built_them() {
    use crate::rules::message_rules::{MessageContext, compile};

    let paths = |plan: &crate::rules::message_rules::MessagePlan,
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
    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "fragments": {"shape": {"cases": [{"id": "as_user", "where": {"path": "$.role", "one_of": ["user"]}, "wrap": {"role": "user", "content_from": "$.content"}}, {"id": "as_other", "wrap": {"role": "assistant", "content_from": "$.content"}}]}}, "messages": [{"id": "t.r", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "plain", "select": "$.direct"}, {"id": "via_fragment", "select": "$.wrapped[*]", "then_fragment": "t.shape"}]}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id": "t", "messages": [{"id": "t.e", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "elements": {"passes": [{"id": "blocks", "group": {"collect": "$.data", "key_as": "role", "by": [{"id": "input_block", "where": {"path": "$.data.type", "starts_with": "input_"}, "value": "user"}, {"id": "legacy_input", "where": {"path": "$.data.type", "starts_with": "legacy_"}, "value": "user"}], "tag_by_key": {"user": "gen_ai.user.message"}}}]}}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
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
/// Counterexample: `{"from_path": "$.finish_reason", "from": "finish_reason", "default": "unknown"}`. When the
/// payload path resolved to nothing, `?` returned from the whole function - so the sibling attribute, the
/// span-name fallback **and** the default were never consulted, while an absent `from_value_any_of` fell
/// through to exactly those. One member, two source forms, two different answers to "nothing here", and the
/// asymmetry was in the code rather than in anything declared.
#[test]
fn an_attachment_falls_through_to_its_other_sources() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.r", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "wrap": {"role": "assistant", "content_from": "$.content", "attach": [{"as": "finish_reason", "from_path": "$.finish_reason", "from": "finish_reason", "default": "unknown"}]}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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

/// A tool exporter may use the complete span name as the tool name.
///
/// `or_span_name_after` used to be reachable only as a fallback from another source. Declaring it on its own
/// therefore looked valid but emitted no name because the literal-only fast path returned first. An empty prefix
/// is intentional here: stripping it yields the complete span name.
#[test]
fn a_span_name_can_be_an_attachments_only_source() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"text",
             "emit":"message","priority":1,
             "wrap":{"role":"tool","block":{"type":"tool_result","attach":[
               {"as":"name","or_span_name_after":""}]}}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the probe compiles");
    let attrs = std::collections::HashMap::from([("x".to_string(), "result".to_string())]);
    let ctx = MessageContext::for_span("done", &attrs, false);
    assert_eq!(
        plan.run(&ctx)[0].value["content"][0]["name"],
        "done",
        "an exporter that names its span exactly after the tool keeps that name"
    );
}

/// An attachment can take one member of a structured sibling attribute.
///
/// A tracer that reports no call id still records which span ran the tool: Laminar writes a span's ancestry
/// as an array ending with the span's own id. Without that id two identical executions on two spans had the
/// same identity, and deduplication left one call and one result where the agent had run the tool twice.
#[test]
fn an_attachment_can_select_a_member_of_its_attribute() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"text",
             "emit":"message","priority":1,
             "wrap":{"role":"tool","block":{"type":"tool_result","attach":[
               {"as":"tool_use_id","from":"path","parse":"json","select":"$[-1]","default":"none"}]}}}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
    .expect("the probe compiles");
    let id = |path: Option<&str>| {
        let mut attrs = std::collections::HashMap::from([("x".to_string(), "result".to_string())]);
        if let Some(path) = path {
            attrs.insert("path".to_string(), path.to_string());
        }
        let ctx = MessageContext::for_span("tool", &attrs, false);
        plan.run(&ctx)[0].value["content"][0]["tool_use_id"].clone()
    };

    assert_eq!(id(Some(r#"["root","step","own"]"#)), "own");
    assert_eq!(
        id(Some("[]")),
        "none",
        "a path that selects nothing falls through to the default, like an absent attribute"
    );
    assert_eq!(id(None), "none");
}

/// `select` reads inside a parsed attribute, so it is refused where there is no attribute or no structure.
#[test]
fn a_selection_needs_a_structured_attribute() {
    use crate::rules::message_rules::compile;

    let asset = |attach: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"text",
                 "emit":"message","priority":1,"alternatives":[{{"id":"a",
                 "wrap":{{"role":"tool","block":{{"type":"tool_result","attach":[{attach}]}}}}}}]}}]}}"#
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
        asset(r#"{"as":"id","from_path":"$.ids","select":"$[-1]"}"#).is_err(),
        "a selection with no `from` attribute selects from nothing"
    );
    assert!(
        asset(r#"{"as":"id","from":"ids","select":"$[-1]"}"#).is_err(),
        "an attribute read as text has no members to select"
    );
    assert!(asset(r#"{"as":"id","from":"ids","parse":"json","select":"$[-1]"}"#).is_ok());
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
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |stop: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","priority":1,"walk":{{"max_depth":3,"stop_on":{stop}}},
                 "also":[
                   {{"id":"whole_node","where": {{"all":[{{"path":"$.role"}},{{"path":"$.content"}}]}},
                    "wrap":{{"role_from":"$.role","content_from":"$.content"}}}},
                   {{"id":"any_member","select":"$.*",
                    "where": {{"all":[{{"path":"$.role"}},{{"path":"$.content"}}]}},
                    "wrap":{{"role_from":"$.role","content_from":"$.content"}}}}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    // A node holding a message beside more state: `any_member` recognises the *root*, `whole_node` does not.
    let attrs = std::collections::HashMap::from([(
        "x".to_string(),
        r#"{"direct":{"role":"user","content":"q"},"nested":{"inner":{"role":"assistant","content":"a"}}}"#
            .to_string(),
    )]);

    let narrow = asset(r#"["whole_node"]"#).expect("naming one clause compiles");
    let ctx = MessageContext::for_span("span", &attrs, false);
    let contents = |plan: &crate::rules::message_rules::MessagePlan| {
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
