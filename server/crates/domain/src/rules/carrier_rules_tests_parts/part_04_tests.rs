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
            "message_events": [
                {"id": "probe.probe_event", "name": "probe.event"},
                // A second produced name, so the three tool-span authorities can be declared side by side.
                {"id": "probe.probe_event2", "name": "probe.event2"},
            ],
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
        (
            // Absence already means "the same role on both kinds of span", so this is a second spelling of one
            // fact - and it was legal, which is why one shipped declaration spelled it out while seven omitted it.
            "a tool-span role equal to the ordinary one, which absence already says",
            serde_json::json!([{"id": "probe.role11", "name": "probe.event", "role": "user", "role_in_tool_span": "user"}]),
        ),
        (
            "silence on tool spans beside a role for them, which is two answers about one span kind",
            serde_json::json!([{"id": "probe.role12", "name": "probe.event", "role": "user", "role_in_tool_span": "tool", "silent_in_tool_span": true}]),
        ),
        (
            "silence on tool spans and no other role, which states nothing at all",
            serde_json::json!([{"id": "probe.role13", "name": "probe.event", "silent_in_tool_span": true}]),
        ),
        (
            "two that disagree only about silence, the same load-order question one step further in",
            serde_json::json!([
                {"id": "probe.role14", "name": "probe.event", "role": "user", "silent_in_tool_span": true},
                {"id": "probe.role15", "name": "probe.event", "role": "user"},
            ]),
        ),
    ] {
        assert!(compiled(roles).is_err(), "{what} was accepted");
    }

    // The three authorities, each resolved from its own spelling. Without this the silent state is declarable
    // and untested: every other assertion about "no role here" is satisfied by a name no asset speaks for, which
    // is a different fact.
    {
        let compiled = compiled(serde_json::json!([
            {"id": "probe.same", "name": "probe.event", "role": "user"},
            {"id": "probe.other", "name": "probe.tag", "role": "assistant", "role_in_tool_span": "tool"},
            {"id": "probe.silent", "name": "probe.event2", "role": "system", "silent_in_tool_span": true},
        ]))
        .expect("three well-formed authorities compile");
        let role_on = |name: &str, tool| compiled.get(name).expect("declared").role_on(tool);
        use crate::sideml::ChatRole;
        // Absence: the same role on both kinds.
        assert_eq!(role_on("probe.event", false), Some(ChatRole::User));
        assert_eq!(role_on("probe.event", true), Some(ChatRole::User));
        // A role of its own on a tool span.
        assert_eq!(role_on("probe.tag", false), Some(ChatRole::Assistant));
        assert_eq!(role_on("probe.tag", true), Some(ChatRole::Tool));
        // Silence: it speaks for ordinary spans and says nothing on a tool span, which absence cannot express
        // because absence falls back to `role`.
        assert_eq!(role_on("probe.event2", false), Some(ChatRole::System));
        assert_eq!(
            role_on("probe.event2", true),
            None,
            "a silent declaration must not fall back to its ordinary role"
        );
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
        "server/crates/api/src/mcp/tools.rs",
        "generates integration documentation, which has to show the attribute names a framework writes",
    )];

    let inventory = producer_key_inventory();
    assert!(
        inventory.len() > 30,
        "only {} producer keys were derived, which cannot be right",
        inventory.len()
    );
    let exclusive: Vec<(&String, &String)> = inventory.iter().collect();

    let repository = repository_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut exempt_used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    walk_production_rust_sources(&mut |path, source| {
        let relative = path
            .strip_prefix(&repository)
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
        super::span_fields::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
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
        super::span_fields::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
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
        super::span_fields::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
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
    let sources = crate::rules::schema::embedded_sources();
    let mut per_asset: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for (path, bytes) in &sources {
        // Basename, for the same reason as above: the assets are grouped in subdirectories.
        let id = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".json")
            .to_string();
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
            let file: crate::rules::schema::RuleFile =
                serde_json::from_slice(bytes).expect("the asset parses");
            assert!(
                file.declaration_defect().is_none(),
                "`{path}`: {:?}",
                file.declaration_defect()
            );
        }
        let conventions: crate::rules::schema::RuleFile =
            serde_json::from_slice(&sources["conventions/semconv.json"])
                .expect("the conventions asset parses");
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
    use crate::rules::schema::EmitTarget;
    use std::collections::HashMap;

    let emitted = |attrs: &HashMap<String, String>| -> Vec<(String, EmitTarget)> {
        let plan = &crate::rules::ruleset().messages;
        let ctx = crate::rules::message_rules::MessageContext::for_span("call_llm", attrs, false);
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
/// The **numbers** in the diagrams are checked too, for the same reason the names are.
///
/// A count is a claim a reader trusts without verifying, and this one had already drifted: the asset diagram
/// said 41 assets and 347 rules while the tree held 43 and 382. Names were checked and numbers were not, so
/// the one that silently rots was the one nobody guarded. The diagram count is asserted here as well, because
/// a section added without a heading number reads as five diagrams when there are seven.
#[test]
fn the_diagrams_count_what_the_tree_holds() {
    let text = std::fs::read_to_string(
        repository_root().join("docs/engineering/architecture-diagrams.md"),
    )
    .expect("the diagrams are committed beside the code they describe");

    let sources = crate::rules::schema::embedded_sources();
    let clauses: usize = sources
        .values()
        .map(|bytes| {
            let file: serde_json::Value =
                serde_json::from_slice(bytes).expect("every asset parses");
            file.as_object().map_or(0, |members| {
                members
                    .values()
                    .map(|value| match value {
                        serde_json::Value::Array(items) => items.len(),
                        // `fragments` is a map of named clauses rather than a list.
                        serde_json::Value::Object(map) => map.len(),
                        _ => 0,
                    })
                    .sum()
            })
        })
        .sum();
    let expected = format!("{} assets · {clauses} clauses", sources.len());
    assert!(
        text.contains(&expected),
        "the asset diagram should say `{expected}`; a count nobody checks is the claim that rots first"
    );
    // The engine's own document states the same two numbers in prose, and *that* copy had rotted - the
    // diagram was guarded and the prose was not, which is the same asymmetry one level along.
    let prose = std::fs::read_to_string(
        repository_root().join("docs/engineering/framework-rules-engine.md"),
    )
    .expect("the engine's document is committed");
    let stated = format!("{} assets holding {clauses} clauses", sources.len());
    assert!(
        prose.contains(&stated),
        "docs/engineering/framework-rules-engine.md should say `{stated}`"
    );

    let sections = text.matches("\n## ").count();
    let stated = format!("{} diagrams, each answering a question", sections);
    assert!(
        text.contains(&stated),
        "the file has {sections} numbered sections and does not say so - it should open with `{stated}`"
    );
}

#[test]
fn the_diagrams_name_things_that_exist() {
    let diagrams = repository_root().join("docs/engineering/architecture-diagrams.md");
    let text = std::fs::read_to_string(&diagrams)
        .unwrap_or_else(|e| panic!("the diagrams must be readable: {e}"));

    // The whole source tree, once, as the haystack every name is looked for in.
    let mut sources = String::new();
    walk_production_rust_sources(&mut |_, source| sources.push_str(source));
    // Plus the asset section names, which are `RuleFile` members and appear in the assets themselves.
    for bytes in crate::rules::schema::embedded_sources().values() {
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
