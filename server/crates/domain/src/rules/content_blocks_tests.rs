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
                "where": {
                    "path": "$.type",
                    "one_of": [
                        "text"
                    ]
                },
                "text": {
                    "text": "$.value"
                }
            }),
            serde_json::json!({
                "id": "probe.b",
                "at": "after_provider_formats",
                "priority": 1,
                "where": {
                    "path": "$.type",
                    "one_of": [
                        "prose"
                    ]
                },
                "text": {
                    "text": "$.value"
                }
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
            "where": {
                "path": "$.type",
                "one_of": [
                    "text"
                ]
            },
            "text": {
                "text": "$.value"
            }
        }),
        serde_json::json!({
            "id": "probe.after",
            "at": "after_provider_formats",
            "priority": 1,
            "where": {
                "path": "$.type",
                "one_of": [
                    "prose"
                ]
            },
            "text": {
                "text": "$.value"
            }
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
        "where": {
            "path": "$.type",
            "one_of": [
                "text"
            ]
        },
        "text": {
            "text": "$.value"
        }
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
            "text": {
                "text": "$.value"
            },
            "json": {
                "data": "$.value"
            }
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
            "text": {}
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
            "json": {
                "data": "$.missing"
            }
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
            "where": {
                "path": "$.type",
                "one_of": [
                    "tool-result"
                ]
            },
            "tool_result": {
                "content": "$"
            }
        }),
        "re-enters this plan",
    );
}

/// The same loop through a closed `map`: a literal the case itself recognises would be offered back to it as the
/// content's normalisation, for ever. As `blocks` the content is wrapped as data and never re-enters, so there
/// selecting the whole block is a statement, not a loop. And a literal the case recognises is not a loop by
/// itself: one that maps on to a literal the chain finishes with is two steps, and compiles.
#[test]
fn a_tool_result_content_that_rebuilds_its_own_block_is_refused_and_blocks_content_is_not() {
    let looping = |content_as: &str| {
        serde_json::json!({
            "id": "probe.loop",
            "at": "after_provider_formats",
            "priority": 1,
            "where": {"path": "$.type", "one_of": ["loop"]},
            "tool_result": {
                "content": {
                    "path": "$.payload",
                    "pipe": [{"map": {"again": {"type": "loop", "payload": "again"}}, "closed": true}]
                },
                "content_as": content_as
            }
        })
    };
    refused(looping("normalized"), "never finishes");
    refused(looping("value"), "never finishes");
    assert!(compiled(vec![looping("blocks")]).is_ok());
    let finite = plan_from(serde_json::json!({
        "id": "probe.finite",
        "at": "after_provider_formats",
        "priority": 1,
        "where": {"path": "$.kind", "one_of": ["result"]},
        "tool_result": {
            "content": {
                "path": "$.state",
                "pipe": [{"map": {
                    "outer": {"kind": "result", "state": "inner"},
                    "inner": {"type": "text", "text": "ok"}
                }, "closed": true}]
            }
        }
    }));
    let normalised = crate::sideml::content::normalize_block_in(
        &finite,
        &serde_json::json!({"kind": "result", "state": "outer"}),
        false,
    )
    .expect("the case recognises the block");
    assert!(
        normalised.to_string().contains(r#""text":"ok""#),
        "two steps through the table reach its last literal: {normalised}"
    );
    let whole = serde_json::json!({
        "id": "probe.whole",
        "at": "after_provider_formats",
        "priority": 1,
        "where": {"path": "$.type", "one_of": ["tool-result"]},
        "tool_result": {"content": "$", "content_as": "blocks"}
    });
    assert!(
        compiled(vec![whole]).is_ok(),
        "the whole block, kept as data, does not loop"
    );
}

/// **Normalisation is bounded however the telemetry nests.** A loop through two cases - each mapping its content
/// to the other's shape, which no single-case check can see - is refused when the plan is assembled, because the
/// assembled chain is run on every literal a table can hand it; and the deepest nesting of the shipped chain's own
/// wrapper a JSON text can hold finishes at the depth bound, on a thread with a small stack, so an unbounded
/// recursion fails here rather than passing on a large one.
#[test]
fn hostile_nesting_is_normalised_in_bounded_depth() {
    let case = |id: &str, kind: &str, other: &str, priority: i32| {
        serde_json::json!({
            "id": id,
            "at": "after_provider_formats",
            "priority": priority,
            "where": {"path": "$.type", "one_of": [kind]},
            "tool_result": {
                "content": {
                    "path": "$.payload",
                    "pipe": [{"map": {"x": {"type": other, "payload": "x"}}, "closed": true}]
                }
            }
        })
    };
    refused_all(
        vec![
            case("probe.ping", "ping", "pong", 1),
            case("probe.pong", "pong", "ping", 2),
        ],
        "never finishes",
    );
    // The deepest a JSON text can nest the wrapper the shipped chain unwraps: serde_json refuses deeper.
    let mut text = r#"{"type":"text","text":"bottom"}"#.to_string();
    let mut deepest = None;
    for _ in 0..1_000 {
        let wrapped = format!(r#"{{"value":{text}}}"#);
        match serde_json::from_str::<serde_json::Value>(&wrapped) {
            Ok(value) => {
                deepest = Some(value);
                text = wrapped;
            }
            Err(_) => break,
        }
    }
    let deepest = deepest.expect("some nesting parses");
    let answers = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let started = std::time::Instant::now();
            let (nested, bounded) = NormalisationDepth::observe(|| {
                crate::sideml::content::normalize_content_block(&deepest)
            });
            (nested.is_some(), bounded, started.elapsed())
        })
        .expect("the thread starts")
        .join()
        .expect("normalisation finishes within a small stack");
    assert!(
        answers.0,
        "the nesting degrades to a block rather than to nothing"
    );
    assert!(answers.1, "and it is the bound that stopped it");
    assert!(
        answers.2 < std::time::Duration::from_secs(5),
        "bounded time: {:?}",
        answers.2
    );
}

#[test]
fn a_tool_result_keeps_its_declared_name() {
    let plan = plan_from(serde_json::json!({
        "id": "probe.named_result",
        "at": "before_provider_formats",
        "priority": 1,
        "where": {
            "path": "$.result"
        },
        "tool_result": {
            "tool_use_id": "$.id",
            "name": "$.name",
            "content": "$.result"
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
            "where": {"exists": true},
            "json": {}
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
            "where": {
                "path": "$",
                "exists": true
            },
            "json": {}
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
        "where": {
            "path": "$.value"
        },
        "json": {
            "data": "$.value"
        }
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
            "where": {
                "path": "$.type",
                "kind": "number",
                "identifier_like": true
            },
            "text": {
                "text": "$.value"
            }
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
        "where": {
            "path": "$.content"
        },
        "media": {
            "media_type": "$.mime_type",
            "data": "$.content"
        }
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
        "where": {
            "path": "$.uri"
        },
        "media": {
            "media_type": "$.mime_type",
            "data": "$.uri"
        }
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
        "where": {
            "path": "$.content",
            "starts_with": "{"
        },
        "unwrap": {
            "from": "$.content",
            "parse_json": true
        }
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
        "where": {"all": [{"path": "$.type", "one_of": ["text"]}, {"path": "$.content", "kind": "array"}]},
        "splice": {"from": "$.content"},
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
    whole["splice"]["from"] = serde_json::json!("$");
    refused(whole, "unwraps the whole block");
}

/// A splice answers with the member list, and never through the single-block chain - where it leaves the
/// block to the cases after it rather than claiming it.
#[test]
fn a_splice_yields_its_member_list_and_nothing_to_the_single_block_chain() {
    let plan = plan_from(splice_rule("message_envelope"));
    let block = serde_json::json!({"type": "text", "content": [{"text": "a"}, {"toolUse": {}}]});
    let Some(crate::rules::content_blocks::Expansion::Members(members)) = plan.expand(&block)
    else {
        panic!("a splice expands into its members");
    };
    assert_eq!(
        members,
        &vec![
            serde_json::json!({"text": "a"}),
            serde_json::json!({"toolUse": {}})
        ]
    );
    assert!(
        plan.normalize(&block, ChainPosition::MessageEnvelope)
            .is_none()
    );
    assert!(
        plan.expand(&serde_json::json!({"type": "text", "content": "prose"}))
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
        "where": {
            "all": [
                {
                    "path": "$.media_type"
                },
                {
                    "path": "$.data"
                }
            ]
        },
        "media": {
            "media_type": "$.media_type",
            "data": "$.data"
        }
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

fn provider_run_rule(at: &str, kind: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "probe.run",
        "at": at,
        "priority": 1,
        "where": {"path": "$.type", "one_of": [kind]},
        "provider_run": {"id": "$.id", "name": "$.name", "input": "$.input", "result": "$.found"},
    })
}

/// A provider-run item answers with its call and its result, both run by the provider, in a message's content.
#[test]
fn a_provider_run_is_its_call_and_result_in_the_message_content() {
    let plan = plan_from(provider_run_rule("message_envelope", "search_call"));
    let item = serde_json::json!({"type": "search_call", "id": "s1", "name": "search",
        "input": {"q": "louvre"}, "found": ["https://a"]});
    for content in [serde_json::json!([item.clone()]), item.clone()] {
        let blocks = crate::sideml::content::normalize_content_in(&plan, Some(&content));
        assert_eq!(
            blocks,
            serde_json::json!([
                {"type": "tool_use", "id": "s1", "name": "search", "input": {"q": "louvre"},
                 "provider_executed": true},
                {"type": "tool_result", "tool_use_id": "s1", "content": [{"type": "json", "data": ["https://a"]}],
                 "is_error": false, "provider_executed": true}
            ])
        );
    }
    // Without a result yet, the call alone.
    let pending =
        serde_json::json!({"type": "search_call", "id": "s2", "name": "search", "input": {}});
    let blocks =
        crate::sideml::content::normalize_content_in(&plan, Some(&serde_json::json!([pending])));
    assert_eq!(blocks.as_array().map(Vec::len), Some(1));
    // Never through the single-block chain.
    assert!(
        plan.normalize(&item, ChainPosition::MessageEnvelope)
            .is_none()
    );
}

/// The blocks a provider run builds are canonical and never expanded again, so a run whose condition its own
/// output satisfies answers once instead of recursing without end.
#[test]
fn a_provider_run_that_matches_its_own_output_is_expanded_once() {
    let plan = plan_from(provider_run_rule("message_envelope", "tool_use"));
    let call = serde_json::json!({"type": "tool_use", "id": "c1", "name": "lookup", "input": {"a": 1}, "found": "x"});
    for content in [serde_json::json!([call.clone()]), call.clone()] {
        let blocks = crate::sideml::content::normalize_content_in(&plan, Some(&content));
        assert_eq!(blocks.as_array().map(Vec::len), Some(2), "{blocks}");
    }
}

#[test]
fn a_provider_run_outside_the_message_envelope_is_refused() {
    refused(
        provider_run_rule("provider_formats", "search_call"),
        "reads a provider-run item at `provider_formats`",
    );
}

fn cited_rule(cases: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "probe.cited",
        "at": "provider_formats",
        "priority": 1,
        "where": {"path": "$.type", "one_of": ["answer"]},
        "text": {"text": "$.text", "citations": {"from": "$.notes", "cases": cases}},
    })
}

/// Each citation is read by the first case that recognises it; one no case reads, or one naming no source, is
/// kept as `unknown` with the provider's item, so a new shape is seen arriving.
#[test]
fn citations_are_read_by_their_case_and_an_unread_one_is_kept_unknown() {
    let plan = plan_from(cited_rule(serde_json::json!([
        {"where": {"path": "$.type", "one_of": ["page"]}, "kind": "url", "source": "$.url",
         "title": "$.title", "text_start": "$.from", "text_end": "$.to"}
    ])));
    let block = serde_json::json!({"type": "answer", "text": "Open at nine.", "notes": [
        {"type": "page", "url": "https://a", "title": "A", "from": 0, "to": 13},
        {"type": "page", "title": "no url"},
        {"type": "shelf", "row": 3}
    ]});
    let read = plan
        .normalize(&block, ChainPosition::ProviderFormats)
        .expect("the text reads");
    assert_eq!(
        read["citations"],
        serde_json::json!([
            {"kind": "url", "source": "https://a", "title": "A", "text_start": 0, "text_end": 13},
            {"kind": "unknown", "raw": {"type": "page", "title": "no url"}},
            {"kind": "unknown", "raw": {"type": "shelf", "row": 3}}
        ])
    );
    let block: crate::sideml::ContentBlock = serde_json::from_value(read).expect("canonical");
    let crate::sideml::ContentBlock::Text { citations, .. } = block else {
        panic!("a text block");
    };
    assert_eq!(citations.len(), 3);
}

#[test]
fn citation_cases_must_exist_and_only_the_last_may_omit_its_condition() {
    refused(cited_rule(serde_json::json!([])), "text.citations");
    refused(
        cited_rule(serde_json::json!([
            {"kind": "url", "source": "$.url"},
            {"where": {"path": "$.type", "one_of": ["page"]}, "kind": "document", "source": "$.title"}
        ])),
        "a case after it never answers",
    );
}

/// SideML's own citations pass through with their text; a provider's own list under the same member does not
/// pass as canonical, and the text is kept where no case reads it.
#[test]
fn only_canonical_citations_pass_through_and_a_provider_list_keeps_its_text() {
    let canonical = serde_json::json!({"type": "text", "text": "t",
        "citations": [{"kind": "url", "source": "https://a"}]});
    assert_eq!(
        crate::sideml::content::normalize_content_block(&canonical),
        Some(canonical.clone())
    );
    let provider = serde_json::json!({"type": "text", "text": "t",
        "citations": [{"type": "char_location", "document_index": 0}]});
    assert_eq!(
        crate::sideml::content::normalize_content_block(&provider),
        Some(serde_json::json!({"type": "text", "text": "t"}))
    );
}

/// A citation breaking SideML's invariants is not a canonical one: a known kind without a source, or one carrying
/// a provider's item.
#[test]
fn a_canonical_citation_names_its_source_and_only_an_unknown_one_carries_raw() {
    let read = |citation: serde_json::Value| {
        serde_json::from_value::<crate::sideml::Citation>(citation).is_ok()
    };
    assert!(read(
        serde_json::json!({"kind": "url", "source": "https://a"})
    ));
    assert!(read(
        serde_json::json!({"kind": "unknown", "raw": {"type": "x"}})
    ));
    assert!(!read(serde_json::json!({"kind": "url"})));
    assert!(!read(
        serde_json::json!({"kind": "url", "source": "https://a", "raw": {}})
    ));
    assert!(!read(
        serde_json::json!({"kind": "url", "source": "https://a", "page": 2})
    ));
}

fn named_media_rule(extra: serde_json::Value) -> serde_json::Value {
    let mut media = serde_json::json!({
        "kind_of": {"path": "$.modality", "map": {"image": "image"}},
        "media_type": "$.mime_type",
        "missing_media_type": "omit",
        "data": "$.uri",
        "source": {"literal": "url"},
    });
    media
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::json!({
        "id": "probe.named",
        "at": "provider_formats",
        "priority": 1,
        "where": {"path": "$.type", "one_of": ["uri"]},
        "media": media,
    })
}

/// A part that states no media type is of the kind it names; a stated media type is the stronger fact.
#[test]
fn a_named_kind_answers_only_where_no_media_type_is_stated() {
    let plan = plan_from(named_media_rule(serde_json::json!({})));
    let read = |block: serde_json::Value| plan.normalize(&block, ChainPosition::ProviderFormats);
    let named = read(serde_json::json!({"type": "uri", "modality": "image", "uri": "https://a/b"}));
    assert_eq!(
        named.as_ref().map(|b| b["type"].clone()),
        Some(serde_json::json!("image"))
    );
    let typed = read(
        serde_json::json!({"type": "uri", "modality": "image", "uri": "https://a/b.pdf",
        "mime_type": "application/pdf"}),
    );
    assert_eq!(
        typed.as_ref().map(|b| b["type"].clone()),
        Some(serde_json::json!("document"))
    );
    let unmapped =
        read(serde_json::json!({"type": "uri", "modality": "hologram", "uri": "https://a/b"}));
    assert_eq!(
        unmapped.as_ref().map(|b| b["type"].clone()),
        Some(serde_json::json!("file"))
    );
    refused(
        named_media_rule(serde_json::json!({"kind": "image"})),
        "both `kind` and `kind_of`",
    );
    refused(
        named_media_rule(serde_json::json!({"kind_of": {"path": "$.modality", "map": {}}})),
        "media.kind_of.map",
    );
}
