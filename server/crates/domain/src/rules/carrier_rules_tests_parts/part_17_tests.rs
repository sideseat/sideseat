/// **A declaration the engine would ignore is refused** - cycle 2's compile-time findings, one probe each.
#[test]
fn a_declaration_the_engine_would_ignore_is_refused() {
    // `sections` splits raw text, so any other `parse` is ignored.
    let sections = |parse: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.s","read":{{"attribute":"x"}}{parse},"emit":"message",
                "priority":1,"sections":{{"split_on":"|","routes":[{{"id":"r","role":"system"}}]}}}}]}}"#
        ))
    };
    assert!(sections(r#","parse":"text""#).is_ok());
    assert!(sections(r#","parse":"json""#).is_err());

    // A wrap with neither `role` nor `role_from` emits a roleless message - allowed only where the rule's
    // name carries a declared event role, which decides the role at read time.
    let roleless = |tag: &str, roles: &str| {
        probe_compile(&format!(
            r#"{{"id":"t"{roles},"messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1{tag},"wrap":{{"content_from":"$.text"}}}}]}}"#
        ))
    };
    assert!(roleless("", "").is_err(), "a roleless wrap is refused");
    assert!(
        roleless(
            r#","tag_as":"t.result""#,
            r#","event_roles":[{"id":"t.role","name":"t.result","role":"tool"}]"#
        )
        .is_ok(),
        "a declared event role decides it at read time"
    );
    // A `content_default` with no `content_from` is never read.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"user","content_default":"d"}}]}"#,
        )
        .is_err()
    );
    // Content construction beside `tool_calls_from` is discarded.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"assistant","content_from":"$.text",
                "tool_calls_from":{"select":"$.calls","id":"$.id","name":"$.name","arguments":"$.args",
                "on_invalid_item":"skip"}}}]}"#,
        )
        .is_err()
    );
    // A nested block constructor writing one member twice.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"assistant","block":{"type":"tool_use","content_as":"type"}}}]}"#,
        )
        .is_err()
    );
    // A reading filing what the rule's envelope wraps under a target that holds no messages.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"user","content_from":"$.text"},
                "alternatives":[{"id":"a","where":{"path":"$.tools"},"emit":"tool_definitions"}]}]}"#,
        )
        .is_err()
    );
    // A per-reading `direction` beside an aggregate is discarded.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
                "emit":"tool_definitions","priority":1,"aggregate_into_array":true,
                "alternatives":[{"id":"a","select":"$.tools","each":true,"direction":"input"}]}]}"#,
        )
        .is_err()
    );
    // A compose sweep's `parse`/`fallback`, and a named member's `except`, are never read.
    let compose = |member: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.c","emit":"message","priority":1,
                "compose":{{"tag":"t.composed","members":[{member}]}}}}]}}"#
        ))
    };
    assert!(compose(r#"{"sweep_prefix":"p."}"#).is_ok());
    assert!(compose(r#"{"sweep_prefix":"p.","parse":"json"}"#).is_err());
    assert!(compose(r#"{"as":"m","from":"k","except":["x"]}"#).is_err());

    // A span condition asserting absence and testing the value.
    let detect = |condition: &str| {
        super::detect_rules::compile(&probe_assets(&format!(
            r#"{{"id":"t","detect":[{{"id":"t.d","label":"Probe","priority":1,"where":{condition}}}]}}"#
        )))
    };
    assert!(detect(r#"{"source":"attr:k","exists":false,"equals":"x"}"#).is_err());
    assert!(
        detect(r#"{"not":{"source":"attr:k","exists":false,"equals":"x"}}"#).is_ok(),
        "under a not it is a condition"
    );
}

/// **A version suffix past u64 is a version this scheme cannot say**: saturated, two distinct bounds compared
/// equal.
#[test]
fn an_oversized_version_suffix_fails_the_parse() {
    use crate::rules::versions::Version;
    use crate::rules::versions::VersionScheme;
    assert!(Version::parse(VersionScheme::Pep440, "1.0rc1").is_some());
    assert!(Version::parse(VersionScheme::Pep440, "1.0rc99999999999999999999999").is_none());
}

/// **A structured tool output is its data, whole**: `{"type":"json","data":{"text":"answer","confidence":0.9}}`
/// is the object, not its `text` member - and a merged result keeps the `name` it pairs by and the
/// `provider_executed` that says whose execution it was.
#[test]
fn a_structured_tool_output_keeps_its_members() {
    use crate::sideml::content::create_inner_content;
    let structured =
        serde_json::json!({"type": "json", "data": {"text": "answer", "confidence": 0.9}});
    assert_eq!(
        create_inner_content(std::slice::from_ref(&structured)),
        serde_json::json!({"text": "answer", "confidence": 0.9})
    );
    let text = serde_json::json!({"type": "text", "text": "answer"});
    assert_eq!(
        create_inner_content(std::slice::from_ref(&text)),
        serde_json::json!("answer")
    );
}
