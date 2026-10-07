use super::*;

fn plan_from(rule: serde_json::Value) -> ContentBlockPlan {
    plan_from_all(vec![rule])
}

fn plan_from_all(rules: Vec<serde_json::Value>) -> ContentBlockPlan {
    compiled(rules).expect("the probe rules compile")
}

fn probe(rules: Vec<serde_json::Value>) -> RuleFile {
    serde_json::from_value(serde_json::json!({
        "id": "probe",
        "content_blocks": rules,
    }))
    .expect("the probe asset parses")
}

fn compiled(rules: Vec<serde_json::Value>) -> Result<ContentBlockPlan, ContentBlockCompileError> {
    ContentBlockPlan::compile(&[probe(rules)])
}

/// The refusal a lone rule meets, which must name `expected`.
fn refused(rule: serde_json::Value, expected: &str) {
    refused_all(vec![rule], expected);
}

fn refused_all(rules: Vec<serde_json::Value>, expected: &str) {
    let error = compiled(rules).expect_err("the probe rules must be refused");
    assert!(
        error.to_string().contains(expected),
        "wrong refusal, expected `{expected}`: {error}"
    );
}

/// Two cases at one priority and one position: which answers a shape they both recognise would depend on
/// load order, which is nobody's statement.
#[test]
fn two_cases_sharing_a_priority_at_one_position_are_refused() {
    refused_all(
        vec![
            serde_json::json!({
                "id": "probe.a",
                "at": "after_provider_formats",
                "priority": 1,
                "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
                "text": {"text": ["$.value"]},
            }),
            serde_json::json!({
                "id": "probe.b",
                "at": "after_provider_formats",
                "priority": 1,
                "require": {"all": [{"path": "$.type", "one_of": ["prose"]}]},
                "text": {"text": ["$.value"]},
            }),
        ],
        "share priority 1 at the `after` position",
    );
}

/// The same rank at *different* positions means nothing: one runs before the provider formats and the
/// other after, so there is no contest to resolve.
#[test]
fn the_same_rank_at_different_positions_is_accepted() {
    let plan = plan_from_all(vec![
        serde_json::json!({
            "id": "probe.before",
            "at": "before_provider_formats",
            "priority": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
            "text": {"text": ["$.value"]},
        }),
        serde_json::json!({
            "id": "probe.after",
            "at": "after_provider_formats",
            "priority": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["prose"]}]},
            "text": {"text": ["$.value"]},
        }),
    ]);
    assert_eq!(plan.rule_count(), 2);
}

/// One target form, and the reason it must be exactly one.
#[test]
fn a_rule_declares_exactly_one_target_form() {
    // One is fine.
    let plan = plan_from(serde_json::json!({
        "id": "probe.text",
        "at": "after_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.type", "one_of": ["text"]}]},
        "text": {"text": ["$.value"]},
    }));
    assert_eq!(plan.rule_count(), 1);
}

/// Zero forms recognises a block and builds nothing - with an empty `require`, *every* block.
#[test]
fn a_rule_with_no_target_form_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.nothing",
            "at": "after_provider_formats",
            "priority": 1,
        }),
        "declares 0 target forms",
    );
}

/// Two forms means `built` takes whichever it checks first, which is an order nobody declared.
#[test]
fn a_rule_with_two_target_forms_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.both",
            "at": "after_provider_formats",
            "priority": 1,
            "text": {"text": ["$.value"]},
            "json": {"data": ["$.value"]},
        }),
        "declares 2 target forms",
    );
}

/// A required selector with no paths can never resolve, so the case recognises a block and then builds
/// nothing - dead, while reading as though it builds something.
#[test]
fn a_required_selector_with_no_paths_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.empty_text",
            "at": "after_provider_formats",
            "priority": 1,
            "text": {"text": []},
        }),
        "names no path for `text.text`",
    );
}

/// A form whose selectors are all empty always builds something, so with no condition it recognises
/// every block and swallows the rest of the chain.
#[test]
fn a_rule_that_builds_from_nothing_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.catch_all",
            "at": "before_provider_formats",
            "priority": 1,
            "json": {},
        }),
        "swallow the chain",
    );
}

/// The same, with a selector that names something: it resolves nothing and still builds a block, which
/// is why emptiness of the selector list was the wrong test.
#[test]
fn a_rule_whose_selector_may_resolve_nothing_still_needs_a_condition() {
    refused(
        serde_json::json!({
            "id": "probe.unresolved",
            "at": "before_provider_formats",
            "priority": 1,
            "json": {"data": ["$.missing"]},
        }),
        "swallow the chain",
    );
}

/// Selecting the block itself as tool-result content re-enters this plan with the same value.
#[test]
fn self_selecting_tool_result_content_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.recursive",
            "at": "after_provider_formats",
            "priority": 1,
            "require": {"all": [{"path": "$.type", "one_of": ["tool-result"]}]},
            "tool_result": {"content": ["$"]},
        }),
        "re-enters this plan",
    );
}

#[test]
fn a_tool_result_keeps_its_declared_name() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.named_result",
        "at": "before_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.result"}]},
        "tool_result": {
            "tool_use_id": ["$.id"],
            "name": ["$.name"],
            "content": ["$.result"]
        }
    }));

    assert_eq!(
        plan.normalize(
            &serde_json::json!({
                "id": "call-1",
                "name": "weather",
                "result": "sunny"
            }),
            ChainPosition::BeforeProviderFormats,
        ),
        Some(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "name": "weather",
            "content": "sunny",
            "is_error": false
        }))
    );
}

/// A condition that is a tautology is not a condition. `{}`, `{"path": "$"}` and `{"exists": true}` are
/// the same statement about the root, which always exists - so requiring one recognises everything, and
/// the "must name a condition" rule was bypassable by writing a syntactically non-empty one.
#[test]
fn a_tautological_condition_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.tautology",
            "at": "before_provider_formats",
            "priority": 1,
            "require": {"all": [{}]},
            "json": {},
        }),
        "tautology",
    );
}

/// The same statement written as an explicit root path.
#[test]
fn a_root_path_with_no_condition_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.root_path",
            "at": "before_provider_formats",
            "priority": 1,
            "require": {"all": [{"path": "$", "exists": true}]},
            "json": {},
        }),
        "tautology",
    );
}

/// And a *member* path with no conditions stays legal: it asserts the member is there.
#[test]
fn a_member_path_with_no_condition_is_legal() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.member",
        "at": "before_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.value"}]},
        "json": {"data": ["$.value"]},
    }));
    assert_eq!(plan.rule_count(), 1);
}

/// A predicate that can never hold decides which shape a block is read as, so it is refused here too -
/// this plan compiles separately from the message rules and had no validation at all.
#[test]
fn a_contradictory_predicate_is_refused() {
    refused(
        serde_json::json!({
            "id": "probe.contradiction",
            "at": "after_provider_formats",
            "priority": 1,
            "require": {"all": [{"path": "$.type", "kind": "number", "identifier_like": true}]},
            "text": {"text": ["$.value"]},
        }),
        "can never hold",
    );
}

/// A blob that names no media type is identified by its bytes.
///
/// The conventions make a binary part's MIME type optional and Logfire's Anthropic instrumentation
/// leaves it out, so an image a user sent was kept as an unknown block. A blob with neither a declared
/// type nor recognisable bytes is still not media.
#[test]
fn an_undeclared_media_type_comes_from_the_bytes() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.blob",
        "at": "after_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.content"}]},
        "media": {"media_type": ["$.mime_type"], "data": ["$.content"]},
    }));
    let normalize =
        |block: serde_json::Value| plan.normalize(&block, ChainPosition::AfterProviderFormats);

    let jpeg = normalize(serde_json::json!({"content": "/9j/4AAQSkZJRgABAQAAAQABAAD"}))
        .expect("JPEG bytes name their type");
    assert_eq!(jpeg["media_type"].as_str(), Some("image/jpeg"));
    assert_eq!(jpeg["type"].as_str(), Some("image"));
    assert!(normalize(serde_json::json!({"content": "aGVsbG8gd29ybGQgaGVsbG8"})).is_none());
}

/// A `data:` URI names its media type in its header and carries the payload after the comma; the
/// block holds the payload alone, as one read from a base64 member would.
#[test]
fn a_data_uri_reads_as_its_media_type_and_payload() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.uri",
        "at": "after_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.uri"}]},
        "media": {"media_type": ["$.mime_type"], "data": ["$.uri"]},
    }));

    let block = plan
        .normalize(
            &serde_json::json!({"uri": "data:image/jpeg;base64,/9j/4AAQSkZJRg"}),
            ChainPosition::AfterProviderFormats,
        )
        .expect("a data URI is media");

    assert_eq!(block["type"].as_str(), Some("image"));
    assert_eq!(block["media_type"].as_str(), Some("image/jpeg"));
    assert_eq!(block["data"].as_str(), Some("/9j/4AAQSkZJRg"));
}

/// A block serialised into another block's text is decoded and read as itself; text that is not JSON
/// is left to the rest of the chain.
#[test]
fn an_unwrap_can_decode_a_serialised_block() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.serialised",
        "at": "before_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.content", "starts_with": "{"}]},
        "unwrap": {"from": ["$.content"], "parse_json": true},
    }));
    let normalize = |content: &str| {
        plan.normalize(
            &serde_json::json!({"type": "text", "content": content}),
            ChainPosition::BeforeProviderFormats,
        )
    };

    let decoded = normalize(r#"{"type": "text", "text": "inside"}"#).expect("decodes");
    assert_eq!(decoded["text"].as_str(), Some("inside"));
    assert!(normalize("{not json").is_none());
}

fn splice_rule(at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "probe.splice",
        "at": at,
        "priority": 1,
        "require": {"all": [{"path": "$.type", "one_of": ["text"]}, {"path": "$.content", "kind": "array"}]},
        "splice": {"from": ["$.content"]},
    })
}

/// Only a message's content is a list a block can be spliced into; every other caller of the chain asks
/// for one block, so a splice anywhere else could never answer.
#[test]
fn a_splice_outside_the_message_envelope_is_refused() {
    refused(
        splice_rule("before_provider_formats"),
        "splices at `before_provider_formats`",
    );
    refused(
        splice_rule("after_provider_formats"),
        "splices at `after_provider_formats`",
    );
    let mut whole = splice_rule("message_envelope");
    whole["splice"]["from"] = serde_json::json!(["$"]);
    refused(whole, "unwraps the whole block");
}

/// A splice answers with the member list, and never through the single-block chain - where it leaves the
/// block to the cases after it rather than claiming it.
#[test]
fn a_splice_yields_its_member_list_and_nothing_to_the_single_block_chain() {
    let plan = plan_from(splice_rule("message_envelope"));
    let block = serde_json::json!({"type": "text", "content": [{"text": "a"}, {"toolUse": {}}]});
    assert_eq!(
        plan.splice(&block),
        Some(&vec![
            serde_json::json!({"text": "a"}),
            serde_json::json!({"toolUse": {}})
        ])
    );
    assert!(
        plan.normalize(&block, ChainPosition::MessageEnvelope)
            .is_none()
    );
    assert!(
        plan.splice(&serde_json::json!({"type": "text", "content": "prose"}))
            .is_none()
    );
}

/// A stored reference is the authority on its own media type.
///
/// `media_type: image/png` beside `#!B64!#application/pdf::HASH` produced an *image* block whose bytes a
/// reader fetches as a PDF - two statements about one datum, and the wrong one won because it was the
/// declared one. The reference wins now: it is the stored fact and what a fetch returns.
///
/// Reported rather than refused, deliberately. Refusing drops content over metadata, and the block is
/// perfectly usable once the two agree about what it is.
#[test]
fn a_stored_reference_is_the_authority_on_its_media_type() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.media",
        "at": "after_provider_formats",
        "priority": 1,
        "require": {"all": [{"path": "$.media_type"}, {"path": "$.data"}]},
        "media": {"media_type": ["$.media_type"], "data": ["$.data"]},
    }));
    let normalize = |media_type: &str, data: &str| {
        plan.normalize(
            &serde_json::json!({"media_type": media_type, "data": data}),
            ChainPosition::AfterProviderFormats,
        )
        .expect("the rule recognises the block")
    };

    // The declaration says image, while the stored reference identifies a PDF.
    let block = normalize("image/png", "#!B64!#application/pdf::abc123");
    assert_eq!(
        block["media_type"].as_str(),
        Some("application/pdf"),
        "the stored reference is what a reader will fetch"
    );
    assert_eq!(
        block["type"].as_str(),
        Some("document"),
        "and the block's kind follows the media type that won, or the two disagree again one level up"
    );
    assert_eq!(block["source"].as_str(), Some("file"));

    // Agreement is unremarkable, and inline bytes have only the declaration to go on.
    assert_eq!(
        normalize("image/png", "#!B64!#image/png::abc123")["media_type"].as_str(),
        Some("image/png")
    );
    let inline = normalize("image/png", "iVBORw0KGgo");
    assert_eq!(inline["media_type"].as_str(), Some("image/png"));
    assert_eq!(inline["source"].as_str(), Some("base64"));
    // And a URL is a fetch, which is the half of this that used to be called `base64`.
    assert_eq!(
        normalize("image/png", "https://example.com/a.png")["source"].as_str(),
        Some("url")
    );
}
