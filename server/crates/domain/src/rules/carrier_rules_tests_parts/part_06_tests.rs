/// A `supersedes` edge that cannot mean what it says is refused.
///
/// The field documents an overlap the priorities resolve, and waives the overlap report - the instrument that
/// names which predicates are not yet sufficient. It once *ordered*, ahead of rank, and that made detection a
/// preference relation no total order could state; now it is checked against the priorities and never executed.
/// So the refusals are: an edge pointing at a rule tried first, a target nothing declares, an edge to a rule
/// itself, and the same target twice.
#[test]
fn a_supersedes_edge_that_cannot_take_effect_is_refused() {
    let compiled = |first_rank: i32, second_rank: i32, supersedes: &str| {
        let asset = serde_json::json!({
            "id": "probe",
            "detect": [
                {
                    "id": "probe.specific",
                    "label": "strands",
                    "priority": first_rank,
                    "where": {"source": "attr_keys", "starts_with": "probe.specific."},
                    "supersedes": [supersedes],
                },
                {
                    "id": "probe.generic",
                    "label": "langchain",
                    "priority": second_rank,
                    "where": {"source": "attr_keys", "starts_with": "probe."},
                },
            ],
        });
        crate::rules::detect_rules::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };

    // The shape the shipped assets use: the superseding rule is tried first, and it wins by priority.
    let plan = compiled(10, 20, "probe.generic")
        .expect("an edge from the earlier rule to the later one is the shape the assets use");
    let attrs: std::collections::HashMap<String, String> =
        [("probe.specific.marker".to_string(), "1".to_string())]
            .into_iter()
            .collect();
    let empty = std::collections::HashMap::new();
    let found = plan
        .resolve(&crate::rules::detect_rules::DetectContext {
            span_name: "chat",
            scope_name: None,
            span_attrs: &attrs,
            resource_attrs: &empty,
        })
        .expect("both rules match this span");
    assert_eq!(found.label, "strands", "the earlier priority wins");
    // The shape that once *ordered* - an edge against the priorities - now contradicts them and is refused, with
    // the two priorities named.
    let refused = compiled(20, 10, "probe.generic").expect_err(
        "an edge pointing at a rule tried first states an order the priorities contradict",
    );
    assert!(
        refused.to_string().contains("priority 10"),
        "the refusal names the priorities it disagrees with: {refused}"
    );
    // Equal priorities are refused by the tie refusal first; an edge never decides a tie.
    assert!(compiled(10, 10, "probe.generic").is_err());

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
                "priority": 10,
                "where": {
                    "source": "attr_keys",
                    "starts_with": "probe.specific."
                },
                "supersedes": [
                    "probe.generic",
                    "probe.generic"
                ]
            },
            {
                "id": "probe.generic",
                "label": "langchain",
                "priority": 20,
                "where": {
                    "source": "attr_keys",
                    "starts_with": "probe."
                }
            }
        ]
    });
    assert!(
        crate::rules::detect_rules::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&repeated).expect("serialises"),
            )]))
            .expect("the probe assets parse")
        )
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
                "priority": 10,
                "where": {
                    "source": "attr_keys",
                    "starts_with": "probe.mid.deep."
                },
                "supersedes": [
                    "probe.middle"
                ]
            },
            {
                "id": "probe.middle",
                "label": "langchain",
                "priority": 20,
                "where": {
                    "source": "attr_keys",
                    "starts_with": "probe.mid."
                },
                "supersedes": [
                    "probe.generic"
                ]
            },
            {
                "id": "probe.generic",
                "label": "crewai",
                "priority": 30,
                "where": {
                    "source": "attr_keys",
                    "starts_with": "probe."
                }
            }
        ]
    });
    let plan = crate::rules::detect_rules::compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "probe.json".to_string(),
            serde_json::to_vec(&asset).expect("serialises"),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the probe compiles");

    // Nested prefixes rather than one identical condition in all three: identical conditions make the two later
    // rules genuinely unreachable, which the shadowing refusal now rejects at compile time - correctly, since
    // nothing could ever answer with their labels. Nested prefixes keep all three matching one span while leaving
    // each of them reachable on its own.
    let mut attrs = std::collections::HashMap::new();
    attrs.insert("probe.mid.deep.marker".to_string(), "1".to_string());
    let ctx = crate::rules::detect_rules::DetectContext {
        span_name: "probe.span",
        scope_name: None,
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
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let file: crate::rules::schema::RuleFile =
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
        serde_json::from_value::<crate::rules::schema::RuleFile>(asset)
            .expect("the probe parses")
            .declaration_defect()
    };
    let two_readings = |first: &str, second: &str| {
        serde_json::json!([{
            "id": "probe.message",
            "priority": 1,
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
            "priority": 1,
            "read": {"attribute": "one"},
            "parse": "json",
            "emit": "message",
            "alternatives": [{"id": "shared", "select": "$.a"}],
        },
        {
            "id": "probe.two",
            "priority": 2,
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
    use crate::rules::schema::SpanFact;
    use std::collections::HashMap;

    let ruleset = crate::rules::ruleset();

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
    use crate::rules::expr::{ClausePath, EvidenceSet};
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
    use crate::rules::schema::FieldTarget;
    use std::collections::HashMap;

    let resolve = |attrs: &HashMap<String, String>, target: FieldTarget| {
        crate::rules::ruleset()
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
    use crate::rules::schema::in_family;

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
        crate::rules::ruleset()
            .carriers
            .resolve(&crate::rules::CarrierContext {
                event: None,
                attribute: Some(attribute),
                observation_type: Some("generation"),
                span_name: None,
                direction: None,
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
        crate::rules::ruleset()
            .carriers
            .resolve(&crate::rules::CarrierContext {
                event: None,
                attribute: Some("gen_ai.prompt.0.content"),
                observation_type: Some("generation"),
                span_name: None,
                direction: None,
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
        crate::rules::carrier_rules::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };

    for (what, facts, family) in [
        (
            "one atomic emission whose positions prove nothing",
            serde_json::json!({"preset": "emission", "position_proves_distinct_occurrence": false}),
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
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |body: &str| {
        std::collections::BTreeMap::from([("t.json".to_string(), body.as_bytes().to_vec())])
    };

    // `first_present`: the first spelling the span carries, and nothing after it.
    let plan = compile(
        &ParsedAssets::parse(&asset(
            r#"{"id": "t", "messages": [{"id": "t.alternatives", "read": {"attribute": {"first_of": ["new", "old"]}}, "parse": "text", "emit": "message", "priority": 1}]}"#,
        ))
        .expect("the probe assets parse"),
    )
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
    let refused = compile(
        &ParsedAssets::parse(&asset(
            r#"{"id": "t", "messages": [{"id": "t.each", "read": {"every": ["new", "old"]}, "parse": "text", "emit": "message", "priority": 1}]}"#,
        ))
        .expect("the probe assets parse"),
    )
    .expect_err("`each` on a body that cannot iterate its carriers must be refused");
    assert!(
        refused.to_string().contains("first_of"),
        "the refusal must name the member to use instead: {refused}"
    );

    // And `each` **is** honoured where a body iterates: every listed key present is its own observation.
    let plan = compile(&ParsedAssets::parse(&asset(
        r#"{"id": "t", "messages": [{"id": "t.tools", "read": {"every": ["agents", "tasks"]}, "parse": "json", "emit": "tool_definitions", "priority": 1, "tool_repr": {"entries": "$[*]", "candidates": ["$"], "name_field": "name", "description_field": "description", "name_label": "Tool Name:", "description_label": "Tool Description:", "arguments_label": "Tool Arguments:", "repr_markers": ["name="], "parameter_members": ["parameters"], "field_terminators": [","], "type_map": [["str", "string"]], "type_default": {"map_to": "string"}}}]}"#,
    )).expect("the probe assets parse"))
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
    for member in [
        r#""attribute":{"first_of":["k","k"]}"#,
        r#""every":["k","k"]"#,
    ] {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.dup","read":{{{member}}},"parse":"json",
                 "emit":"tool_definitions","priority":1,
                 "tool_repr":{{"entries":"$[*]","candidates":["$"],"name_field":"name",
                 "description_field":"d","name_label":"N:","description_label":"D:",
                 "arguments_label":"A:","repr_markers":["name="],"parameter_members":["parameters"],
                 "field_terminators":[","],"type_map":[["str","string"]],"type_default":{{"map_to":"string"}}}}}}]}}"#
        );
        assert!(
            compile(&ParsedAssets::parse(&asset(&body)).expect("the probe assets parse")).is_err(),
            "`{member}` listing one key twice must be refused"
        );
    }
}
