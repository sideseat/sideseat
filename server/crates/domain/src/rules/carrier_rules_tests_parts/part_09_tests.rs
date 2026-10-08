/// A grouped element run is consecutive in the **array a producer wrote**, not in the pass's filtered view.
///
/// The pass filtered the array before finding runs, so an element it does not match simply vanished - and two
/// content blocks with a *message* between them became one "consecutive" run of the two. The input is
/// Logfire's own shape: an input block, a named assistant event, another input block. The run then claims two
/// blocks are adjacent while asserting nothing about the message lying between them.
#[test]
fn a_grouped_run_is_consecutive_in_the_array_the_producer_wrote() {
    use crate::rules::message_rules::{MessageContext, MessagePlan, compile};

    fn plan_runs(plan: &MessagePlan, ctx: &MessageContext<'_>) -> Vec<usize> {
        plan.run(ctx)
            .iter()
            .filter(|e| e.carrier.name() == "gen_ai.user.message")
            .map(|e| e.value["content"].as_array().map_or(0, Vec::len))
            .collect()
    }

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.e", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "elements": {"passes": [{"id": "named", "where": {"path": "$['event.name']", "one_of": ["assistant"]}, "tag_from": "$['event.name']"}, {"id": "blocks", "where": {"all": [{"path": "$['event.name']", "none_of": ["assistant"]}, {"path": "$.data", "kind": "object"}]}, "group": {"collect": "$.data", "key_as": "role", "by": [{"id": "is_input", "where": {"path": "$.data.type", "starts_with": "input_"}, "value": "user"}], "tag_by_key": {"user": "gen_ai.user.message"}}}]}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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

    // Two input blocks have an assistant message between them.
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
    let collecting = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.e", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "elements": {"passes": [{"id": "blocks", "where": {"path": "$.data", "kind": "object"}, "group": {"collect": "$.data.text", "key_as": "role", "by": [{"id": "is_input", "where": {"path": "$.data.type", "starts_with": "input_"}, "value": "user"}], "tag_by_key": {"user": "gen_ai.user.message"}}}]}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |reading: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","priority":1,"alternatives":[{reading}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    let payload = r#"{"finish_reason":"outer",
                      "choices":[{"finish_reason":"beside","message":{"role":"assistant","content":"x",
                        "finish_reason":"inner"}}]}"#;
    let reason = |plan: &crate::rules::message_rules::MessagePlan| -> String {
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
    let read = r#""read":{"attribute":"x"},"parse":"text","priority":1"#;

    // Minimal counterexample.
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
            r#"{"id":"t.r","read":{"attribute":"x"},"parse":"json","priority":1,
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
            r#"{"id": "t.r", "emit": "claim", "priority": 1, "compose": {"tag": "joined", "members": [{"as": "content", "from": "k", "parse": "text"}]}}"#
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
    use crate::rules::message_rules::{MessageContext, compile};

    let asset = |policy: &str| {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                 "emit":"message","priority":1,
                 "wrap":{{"role":"assistant","tool_calls_from":{{"select":"$.content[*]","id":"$.id",
                   "name":"$.name","arguments":"$.arguments","on_invalid_item":"{policy}"}}}},
                 "alternatives":[{{"id":"as_calls"}}],
                 "fallback":[{{"id":"as_text","wrap":{{"role":"assistant",
                   "content_from":"$.summary"}}}}]}}]}}"#
        );
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    // One call has an id and the other does not.
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
         "emit":"message","priority":1,
         "wrap":{"role":"assistant","tool_calls_from":{"select":"$.content[*]","id":"$.id",
           "name":"$.name","arguments":"$.arguments"}}}]}"#;
    assert!(
        ParsedAssets::parse(&std::collections::BTreeMap::from([(
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.f","read":{"indexed_family":"fam"},
             "require_members":{"all_of":[{"name":"role"},{"name":"content"}]},
             "emit":"message","priority":1}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.compose", "priority": 1, "emit": "message", "compose": {"tag": "joined", "members": [{"as": "content", "from": "text", "parse": "text"}, {"as": "extra", "from": "structured", "parse": "json"}]}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.tools","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_definitions","priority":1,
             "tool_repr":{"entries":"$[*]","candidates":["$"],
               "name_field":"name","description_field":"description",
               "name_label":"Tool Name:","description_label":"Tool Description:",
               "arguments_label":"Tool Arguments:","repr_markers":["name=","CrewStructuredTool("],
               "parameter_members":["args"],"field_terminators":["env_vars"],
               "type_map":[["str","string"]],"type_default":{"map_to":"string"}}}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    // A non-marker string is accepted as a name whatever it says; tightening that
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

    // The earliest boundary: a quoted field *after* the description must not be swallowed by it. The
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

    // A field's name written inside another field's string is that string's text, not the field.
    let quoted = tools(serde_json::json!([
        "CrewStructuredTool(name=\"real\", description=\"name='fake'\")"
    ]));
    assert_eq!(quoted.len(), 1, "{quoted:?}");
    assert_eq!(
        quoted[0]["function"]["name"].as_str(),
        Some("real"),
        "the name was read out of the description's text"
    );
}

/// A field name that is not ASCII is found past a longer identifier ending in it, without slicing the input
/// inside a character.
#[test]
fn a_repr_field_name_that_is_not_ascii_is_found_without_slicing_a_character() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            r#"{"id":"t","messages":[{"id":"t.tools","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_definitions","priority":1,
             "tool_repr":{"entries":"$[*]","candidates":["$"],
               "name_field":"é","description_field":"description",
               "name_label":"Tool Name:","description_label":"Tool Description:",
               "arguments_label":"Tool Arguments:","repr_markers":["Tool("],
               "parameter_members":["args"],"field_terminators":["env_vars"],
               "type_map":[["str","string"]],"type_default":{"map_to":"string"}}}]}"#
                .as_bytes()
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the probe compiles");
    let attrs = std::collections::HashMap::from([(
        "tools".to_string(),
        serde_json::json!(["Tool(xé='bad', é='real')"]).to_string(),
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let names: Vec<String> = plan
        .tool_definitions(&ctx)
        .iter()
        .flat_map(|e| e.value.as_array().cloned().unwrap_or_default())
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, ["real"]);
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
    use crate::rules::message_rules::{MessageContext, compile};

    // The overrides come **last**, and every default an override names is left out of the base rather than
    // written twice - `deny_unknown_fields` refuses a duplicate member, so a base carrying `arguments_label` and
    // an override supplying another would fail at parse and say nothing about the check under test. Parsing
    // panics in `asset`, so a probe refused for that reason fails loudly instead of passing.
    let base = |overrides: &str| {
        let defaults = [
            ("candidates", r#""candidates":["$"]"#),
            ("description_field", r#""description_field":"description""#),
            ("name_label", r#""name_label":"N:""#),
            ("description_label", r#""description_label":"D:""#),
            ("arguments_label", r#""arguments_label":"A:""#),
            ("repr_markers", r#""repr_markers":["name="]"#),
            ("parameter_members", r#""parameter_members":["args"]"#),
            ("type_map", r#""type_map":[["str","string"]]"#),
            ("type_default", r#""type_default":{"map_to":"string"}"#),
        ];
        let kept: String = defaults
            .iter()
            .filter(|(key, _)| !overrides.contains(&format!("\"{key}\"")))
            .map(|(_, member)| format!(",{member}"))
            .collect();
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.tools","read":{{"attribute":"tools"}},"parse":"json",
                 "emit":"tool_definitions","priority":1,
                 "tool_repr":{{"entries":"$[*]","name_field":"name"{kept}{overrides}}}}}]}}"#
        )
    };
    let asset = |body: String| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                body.into_bytes(),
            )]))
            .expect("the probe assets parse"),
        )
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
/// Input: `{"gen_ai.agent.tools": "[\"search\", 7]"}`. The rule emitted and persisted the list as
/// written, and the read side deserialised the whole column as `Vec<String>` - so the number failed that and
/// took the valid `"search"` with it. A malformed item poisoning its siblings at the last possible moment,
/// after storage had already accepted it.
///
/// Both ends are fixed and both are needed: emission keeps a non-string out of storage, and the read keeps the
/// ones **already stored** from costing their neighbours.
#[test]
fn a_tool_name_is_a_non_blank_string_and_a_bad_one_costs_only_itself() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.names","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_names","priority":1}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
    // perfectly good `get_weather`: provider shapes live in Rust and must
    // move into the assets before a definition can be validated where it is produced.
    let definitions = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","messages":[{"id":"t.defs","read":{"attribute":"tools"},"parse":"json",
             "emit":"tool_definitions","priority":1}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
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
/// *alone*, so the pair below compiled: two rules reading one carrier and both emitting tool definitions.
/// The metadata path then did no claiming, so both survived and their rank became precedence somewhere
/// downstream - which is a rule id deciding an answer.
#[test]
fn metadata_contends_on_the_axis_it_emits_on() {
    use crate::rules::message_rules::{MessageContext, compile};

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

    // Both unconditional rules use the same carrier and axis.
    assert!(
        asset(
            r#"[{"id":"t.raw","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
                 "priority":1},
                {"id":"t.projected","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
                 "priority":2,"alternatives":[{"id":"inner","select":"$.definitions"}]}]"#
        )
        .is_err(),
        "two definition readings of one carrier contend, as two message readings of it do"
    );

    // **Different** axes on one carrier still stand: that is the case the message-axis check was written for.
    let plan = asset(
        r#"[{"id":"t.defs","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions",
             "priority":1},
            {"id":"t.names","read":{"attribute":"tools"},"parse":"json","emit":"tool_names",
             "priority":2,"alternatives":[{"id":"each","select":"$[*].name"}]}]"#,
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
        r#"[{"id": "t.first", "where": {"source": "attr:marker", "exists": true}, "read": {"attribute": "tools"}, "parse": "json", "emit": "tool_definitions", "priority": 1}, {"id": "t.second", "read": {"attribute": "tools"}, "parse": "json", "emit": "tool_definitions", "priority": 2, "alternatives": [{"id": "inner", "select": "$[*]"}]}]"#,
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
             "priority":1},
            {"id":"t.tools","read":{"attribute":"payload"},"parse":"json","emit":"tool_definitions",
             "priority":2}]"#,
    )
    .expect("one carrier holding a conversation and the tools it was offered is two statements");
}
