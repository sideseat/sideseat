/// A rule says **where** it reads with one member, so no entry point can honour half of it.
///
/// `stage` and `when_event` were an implicit sum, and the two entry points disagreed about which fields they
/// consult: the event path selects on the event name and ignores `stage` entirely, while the span path selects
/// on `stage` and requires no event. The first case below uses two event rules at one
/// rank declaring *different* stages. The compiler held them to be different ordering arenas (where a shared
/// rank is legal, because rules in different arenas never contend), and then the event path ran both, leaving
/// ownership of the contested carrier to be decided by comparing their **ids**.
#[test]
fn a_rule_declares_where_it_reads_with_one_member() {
    use crate::rules::message_rules::compile;

    let asset = |rules: &str| {
        let body = format!(
            r#"{{"id":"t","message_events":[{{"id":"t.e","name":"acme.event"}}],"messages":{rules}}}"#
        );
        std::collections::BTreeMap::from([("t.json".to_string(), body.into_bytes())])
    };

    // Two event rules cover one event and one carrier at one rank,
    // differing only in a stage the event path does not read.
    let refused = compile(&ParsedAssets::parse(&asset(
        r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","priority":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","priority":1}]"#,
    )).expect("the probe assets parse"))
    .expect_err("two event rules over one event at one rank must be refused");
    let message = refused.to_string();
    assert!(
        message.contains("priority") || message.contains("carrier"),
        "the refusal must be about the priority or the contested carrier, not something incidental: {message}"
    );

    // The case the *arena* rule owns on its own: two event rules over one event at one rank reading
    // **different** carriers. Nothing contests a carrier here, so the only defect is the shared rank - and
    // Previously, a stage neither rule's entry point reads was enough to make the compiler call them
    // different arenas and accept it.
    let refused = compile(
        &ParsedAssets::parse(&asset(
            r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"one"},
             "parse":"text","emit":"message","priority":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"two"},
             "parse":"text","emit":"message","priority":1}]"#,
        ))
        .expect("the probe assets parse"),
    )
    .expect_err(
        "two event rules over one event at one rank share an arena, so the rank must be refused",
    );
    assert!(
        refused.to_string().contains("priority"),
        "the refusal must be about the shared priority: {refused}"
    );

    // And distinct ranks over one event are fine - the ranks are what order them.
    compile(
        &ParsedAssets::parse(&asset(
            r#"[{"id":"a","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"one"},
             "parse":"text","emit":"message","priority":1},
            {"id":"b","source":{"event":{"names":["acme.event"]}},"read":{"attribute":"two"},
             "parse":"text","emit":"message","priority":2}]"#,
        ))
        .expect("the probe assets parse"),
    )
    .expect("distinct ranks in one arena are ordered");

    // An event source naming nothing is refused rather than silently becoming a span rule - which is what
    // `when_event: []` did, sending the rule to a different entry point from the one it was written for.
    let refused = compile(
        &ParsedAssets::parse(&asset(
            r#"[{"id":"a","source":{"event":{"names":[]}},"read":{"attribute":"payload"},
             "parse":"text","emit":"message","priority":1}]"#,
        ))
        .expect("the probe assets parse"),
    )
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
                 "emit":"message","priority":1}}]"#
        );
        assert!(
            ParsedAssets::parse(&asset(&body)).map_or(true, |assets| compile(&assets).is_err()),
            "a source declaring half of each variant must be refused: {half}"
        );
    }

    // The ordinary case still needs no `source` at all, or the migration would be a tax on 340 rules.
    compile(&ParsedAssets::parse(&asset(
        r#"[{"id":"a","read":{"attribute":"payload"},"parse":"text","emit":"message","priority":1}]"#,
    )).expect("the probe assets parse"))
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
    use crate::rules::span_fields::{Reading, SpanEvent, compile};

    let asset = |sources: &str, target: &str| {
        let body = format!(
            r#"{{"id":"t","span_fields":[{{"id":"t.rule","target":"{target}","sources":{sources}}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
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

    // `first_yielding` is the only policy: an every-occurrence policy existed, no asset used it, and naming it
    // is now a parse refusal.
    assert!(
        ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","span_fields":[{"id":"t.rule","target":"gen_ai_finish_reasons","sources":[
                {"id":"t.s","event_attribute":{"event":"acme.choice","attribute":"finish_reason",
                 "occurrence":"every"}}]}]}"#
                .to_vec(),
        )]))
        .is_err()
    );

    // A malformed value on a scalar field is recorded as malformed rather than read as absent - the
    // distinction `on_malformed` acts on.
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
    let refused: Vec<&crate::rules::refusal::Refusal> =
        resolved.iter().flat_map(|r| &r.refused).collect();
    assert_eq!(
        refused.len(),
        1,
        "a present value that does not convert is malformed, and the chain has to *see* it - not receive a \
         shorter list with nothing recorded"
    );
    assert!(
        matches!(
            refused[0].cause,
            crate::rules::refusal::Unusable::Malformed { .. }
        ),
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

/// An event attribute holding JSON is read through a path, as a span attribute's `json` source is: the first
/// match that yields answers, an unparseable payload is malformed rather than absent, and `scalar_only` has the
/// same domain on both forms.
#[test]
fn an_event_source_reads_a_json_payload_through_a_path() {
    use crate::rules::span_fields::{Reading, SpanEvent, compile};

    let asset = |source: &str| {
        let body = format!(
            r#"{{"id":"t","span_fields":[{{"id":"t.rule","target":"gen_ai_finish_reasons","sources":[{source}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let details = |payload: &str| SpanEvent {
        name: "acme.details".to_string(),
        attributes: std::collections::HashMap::from([(
            "acme.output".to_string(),
            payload.to_string(),
        )]),
    };
    let attrs = std::collections::HashMap::new();
    let plan = asset(
        r#"{"id":"t.s","event_attribute":{"event":"acme.details","attribute":"acme.output",
             "path":"$[0:].finish_reason","scalar_only":true}}"#,
    )
    .expect("a path on an event source compiles");

    let reading = |events: &[SpanEvent]| {
        plan.resolve("span", &attrs, events)
            .into_iter()
            .map(|r| (r.reading, r.refused.len()))
            .next()
    };
    // The first message stating a reason answers; one without a reason is stepped over.
    assert!(matches!(
        reading(&[details(r#"[{"role":"assistant"},{"finish_reason":"tool_use"}]"#)]),
        Some((Reading::StringList(items), 0)) if items == vec!["tool_use".to_string()]
    ));
    // A payload that does not parse is malformed - the chain has to see it.
    assert!(matches!(reading(&[details("not json")]), Some((_, 1))));
    // `scalar_only` without a path has nothing to apply to, on the event form as on the `json` form.
    assert!(
        asset(
            r#"{"id":"t.s","event_attribute":{"event":"acme.details","attribute":"acme.output",
                 "scalar_only":true}}"#
        )
        .is_err()
    );
}

/// Starvation is refused for **every** multi-owner reading, not only for a compose.
///
/// This replaces `a_composed_reading_cannot_be_starved_by_a_lower_ranked_rule`, a repository test that scanned
/// the shipped assets for the compose case. Two reasons it had to become a production refusal rather than gain
/// three more cases: a test over *this* corpus says nothing about an asset added later, and the shape it was
/// checking is one the compiler actively **excuses** - so the corpus could drift into it through an edit to
/// either rule. The property came from `server/specs/CarrierClaiming.tla`, whose first form asserted "the lowest-ranked
/// rule that reads a carrier gets it"; TLC refuted it in seconds, and `ComposedReadingsCanBeStarved` records
/// that the situation is reachable.
///
/// The guard was a repository test over composed readings alone, and the conflict check in production
/// deliberately excuses the shape: a conditional claim "yields on spans its condition excludes, and the ranks
/// decide which is tried first". That reasoning is sound when the loser's emission owns the one carrier it
/// lost, and false for an all-or-nothing reading - the loser is dropped **whole**, so carriers the taker never
/// wanted reach nobody.
///
/// The first case previously compiled incorrectly: a conditional rank-1 rule taking
/// `family.0.role` leaves the indexed entry unable to take `family.0.content`, and that content then appears in
/// no view at all. Neither rule looks wrong on its own, and nothing failed.
#[test]
fn an_all_or_nothing_reading_cannot_be_starved_by_an_earlier_rank() {
    use crate::rules::message_rules::compile;

    let asset = |rules: &str| {
        let body = format!(r#"{{"id":"t","messages":{rules}}}"#);
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };

    // An indexed family is starved by an earlier conditional rule reading one of its keys.
    let refused = asset(
        r#"[{"id": "t.take_role", "where": {"source": "attr:marker", "exists": true}, "read": {"attribute": "family.0.role"}, "parse": "text", "tag_as": "taken", "emit": "message", "priority": 1}, {"id": "t.read_family", "read": {"indexed_family": "family"}, "emit": "message", "priority": 2}]"#,
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
            r#"[{"id": "t.take_x", "where": {"source": "attr:marker", "exists": true}, "read": {"attribute": "x"}, "parse": "text", "tag_as": "taken", "emit": "message", "priority": 1}, {"id": "t.compose", "compose": {"tag": "joined", "members": [{"as": "a", "from": {"first_of": ["x", "x_backup"]}, "parse": "text"}, {"as": "b", "from": "y", "parse": "text"}]}, "emit": "message", "priority": 2}]"#,
        )
        .err()
        .is_some_and(|error| matches!(
            error,
            crate::rules::message_rules::MessageCompileError::StarvedReading { .. }
        )),
        "a composed reading starved of one member must be refused **as starved** - and a backup spelling does \
         not save it, because `composed()` selects the first present with `find_map` and never retries. The \
         variant matters: a parse failure satisfies `is_err()` for an unrelated reason, so the refusal this \
         test is named for could be deleted with the assertion still holding"
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
            r#"[{{"id":"t.take_rich","where":{{"source":"attr:marker","exists":true}},"read":{{"attribute":"rich"}},
                 "parse":"text","tag_as":"taken","emit":"message","priority":{taker_rank}}},
                {{"id":"t.overlaid","read":{{"indexed_family":"fam","entry_member":"message",
                   "overlay":{{"from":"rich","parse":"json","select":"$.messages",
                     "witness": {{"path":"$[*].id","exists":true}},
                     "when_member_prefix":"contents.","content_from":"$.content",
                     "where": {{"kind":"array"}},"as_member":"content"}}}},
                 "emit":"message","priority":{overlaid_rank}}}]"#
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
        r#"[{"id": "t.read_family", "where": {"source": "attr:family_marker", "exists": true}, "read": {"indexed_family": "family"}, "emit": "message", "priority": 1}, {"id": "t.take_role", "where": {"source": "attr:marker", "exists": true}, "read": {"attribute": "family.0.role"}, "parse": "text", "tag_as": "taken", "emit": "message", "priority": 2}]"#,
    )
    .expect("a multi-owner reading at the earlier rank is not starved - it goes first");

    // **Across stages**, which the contested-carrier check exempts and this must not. That exemption is sound
    // for its own question - two stages sharing a carrier are safe because the fallback inherits what the
    // dialect stage claimed - and the inheritance is precisely what makes starvation reach across them.
    assert!(
        asset(
            r#"[{"id": "t.take_x", "read": {"attribute": "x"}, "parse": "text", "emit": "message", "priority": 1}, {"id": "t.compose", "source": {"span": {"stage": "fallback"}}, "compose": {"tag": "joined", "members": [{"as": "a", "from": "x", "parse": "text"}, {"as": "b", "from": "y", "parse": "text"}]}, "emit": "message", "priority": 2}]"#,
        )
        .is_err(),
        "a fallback-stage reading inherits the dialect stage's claims, so a dialect rule can starve it"
    );

    // A single-member compose is not multi-owner: it takes one spelling, so there is no half to lose.
    asset(
        r#"[{"id": "t.take_x", "where": {"source": "attr:marker", "exists": true}, "read": {"attribute": "x"}, "parse": "text", "tag_as": "taken", "emit": "message", "priority": 1}, {"id": "t.compose", "compose": {"tag": "joined", "members": [{"as": "a", "from": {"first_of": ["x", "x_backup"]}, "parse": "text"}]}, "emit": "message", "priority": 2}]"#,
    )
    .expect("one member reading two spellings takes exactly one of them, so nothing is starved");
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(&ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id": "t", "messages": [{"id": "t.events", "read": {"attribute": "events"}, "parse": "json", "emit": "message", "priority": 1, "elements": {"passes": [{"id": "named", "where": {"path": "$['event.name']", "one_of": ["gen_ai.choice"]}, "tag_from": "$['event.name']"}, {"id": "blocks", "where": {"all": [{"path": "$['event.name']", "none_of": ["gen_ai.choice"]}, {"path": "$.data", "kind": "object"}]}, "group": {"collect": "$.data", "key_as": "role", "by": [{"id": "is_input", "where": {"path": "$.data.type", "starts_with": "input_"}, "value": "user"}, {"id": "is_output", "where": {"path": "$.data.type", "starts_with": "output_"}, "value": "assistant"}], "tag_by_key": {"user": "gen_ai.user.message", "assistant": "gen_ai.assistant.message"}}}]}}]}"#
            .to_vec(),
    )])).expect("the probe assets parse"))
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
    use crate::rules::message_rules::compile;

    let plan = |emit: &str| {
        let body = format!(
            r#"{{"id":"t","message_events":[{{"id":"t.e","name":"acme.container","raw":"replace"}}],
                 "messages":[{{"id":"t.read","source":{{"event":{{"names":["acme.container"]}}}},
                   "read":{{"attribute":"payload"}},"parse":"json","emit":"{emit}","priority":1}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
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
    let claimed = claim_plan.from_event(
        "acme.container",
        &readable,
        "span",
        None,
        &span_attrs,
        false,
    );
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
    let failed = claim_plan.from_event(
        "acme.container",
        &unreadable,
        "span",
        None,
        &span_attrs,
        false,
    );
    assert!(
        !failed.replaces_raw && failed.unhandled_container,
        "a claim that could not read its payload has not taken anything off the table"
    );

    // And a message reading behaves the same way, which is what makes this a fact about handling rather than
    // about the emission kind.
    let message_plan = plan("message");
    let emitted = message_plan.from_event(
        "acme.container",
        &readable,
        "span",
        None,
        &span_attrs,
        false,
    );
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
    use crate::rules::span_fields::{Reading, compile};

    let plan = |target: &str, path: &str, reduce: &str| {
        let body = format!(
            r#"{{"id":"t","span_fields":[{{"id":"t.rule","target":"{target}","sources":[
                 {{"id":"t.s","json":{{"attribute":"payload","path":"{path}","reduce":"{reduce}"}}}}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
        .expect("the probe compiles")
    };
    // The **answer** and what the chain **refused**, because a malformed reading ends a first-wins chain and
    // is recorded there rather than becoming the answer - so asserting only on the answer cannot tell a
    // malformed member from an absent one, which is the distinction under test.
    let resolve = |plan: &crate::rules::span_fields::SpanFieldPlan,
                   payload: &str|
     -> (Reading, Vec<crate::rules::refusal::Unusable>) {
        let attrs = std::collections::HashMap::from([("payload".to_string(), payload.to_string())]);
        let resolved = plan
            .resolve("span", &attrs, &[])
            .into_iter()
            .next()
            .expect("one rule");
        (
            resolved.reading,
            resolved.refused.into_iter().map(|r| r.cause).collect(),
        )
    };
    let read = |plan: &crate::rules::span_fields::SpanFieldPlan, payload: &str| -> Reading {
        resolve(plan, payload).0
    };

    // The minimal malformed input.
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
        matches!(
            refused[0],
            crate::rules::refusal::Unusable::Malformed { .. }
        ),
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
            .any(|r| matches!(r, crate::rules::refusal::Unusable::Malformed { .. })),
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
/// would have been exactly that. This walks the shipped sources and requires each to describe
/// itself, which is the part of "structured diagnostics" a test can hold: the clause path comes from the
/// declaration's own id and cannot be empty (compilation refuses that), while the carrier is hand-built per
/// form.
#[test]
fn every_span_field_source_can_name_what_it_read() {
    use crate::rules::schema::RuleFile;

    let mut checked = 0;
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.span_fields {
            for source in &rule.sources {
                let label = crate::rules::span_fields::source_label_for(source);
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
