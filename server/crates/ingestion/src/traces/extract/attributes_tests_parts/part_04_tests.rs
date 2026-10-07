/// The `gen_ai.choice` **event** is the *first* finish-reason source, which is where the retired chain had it.
///
/// This test asserted the opposite for one commit's worth of reasons, and both states were honest at the time.
/// When the four attribute sources moved into the declared chain (`ff0d3cdf`) the event could not follow -
/// `FieldSource` had no way to name an event - so it stayed as a hand-written scan in `extract/mod.rs` that
/// necessarily ran *after* resolution. That made the event last, which is a precedence **no producer states**:
/// it came from where the code could put it, not from what the telemetry means.
///
/// `event_attribute` makes the source declarative like every other field source and keeps it where
/// the retired order had it - first. A reader who believes the intermediate order would expect `length` here.
///
/// Still end to end through `extract_attributes_batch`, because that is what supplies the events.
#[test]
fn the_choice_event_is_the_first_finish_reason_source() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span::Event};

    let kv = |key: &str, value: &str| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    };

    let span_with = |attrs: Vec<KeyValue>, with_event: bool| {
        let events = if with_event {
            vec![Event {
                time_unix_nano: 1_700_000_000_000_000_000,
                name: "gen_ai.choice".to_string(),
                attributes: vec![kv("finish_reason", "stop")],
                dropped_attributes_count: 0,
            }]
        } else {
            Vec::new()
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: None,
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![Span {
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        name: "chat".to_string(),
                        kind: 1,
                        start_time_unix_nano: 1_700_000_000_000_000_000,
                        end_time_unix_nano: 1_700_000_000_100_000_000,
                        attributes: attrs,
                        events,
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        crate::traces::extract::extract_attributes_batch(&request)
            .into_iter()
            .next()
            .expect("one span")
            .gen_ai_finish_reasons
    };

    // An attribute source and the event disagreeing: the **event** wins, because it is the first declared
    // source - the conventions' own spelling, ahead of four dialects' serialised payloads.
    assert_eq!(
        span_with(
            vec![kv("response_data", r#"{"finish_reason":"length"}"#)],
            true
        ),
        vec!["stop".to_string()],
        "the event is declared first, so the conventions' own spelling answers"
    );
    // The event alone answers, and an attribute alone answers - so neither is dead.
    assert_eq!(
        span_with(Vec::new(), true),
        vec!["stop".to_string()],
        "with no attribute source the event must answer"
    );
    assert_eq!(
        span_with(
            vec![kv("response_data", r#"{"finish_reason":"length"}"#)],
            false
        ),
        vec!["length".to_string()],
        "with no event the attribute chain must answer"
    );
    // And neither: no reason at all, rather than an empty string.
    assert!(span_with(Vec::new(), false).is_empty());
}

/// Every field target is listed in `FieldTarget::ALL`.
///
/// The list is hand-written, so it is the one thing a new variant can escape - and the two tests below need it to
/// be complete or they check a subset while claiming to check the ontology.
#[test]
fn every_field_target_is_listed() {
    let module_file =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../domain/src/rules/schema.rs");
    let mut pending = vec![module_file.with_extension(""), module_file];
    let mut source = String::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
                    .map(|entry| entry.expect("schema module entry").path()),
            );
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            source.push_str(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
            );
            source.push('\n');
        }
    }
    let start = source
        .find("pub enum FieldTarget {")
        .expect("the enum is declared in the schema module");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the enum body ends");
    let declared = body[..end]
        .lines()
        .filter(|line| {
            let trimmed = line.trim_end();
            trimmed.starts_with("    ")
                && trimmed.ends_with(',')
                && !trimmed.trim_start().starts_with("//")
                && trimmed
                    .trim_start()
                    .trim_end_matches(',')
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                && trimmed
                    .trim_start()
                    .trim_end_matches(',')
                    .chars()
                    .all(char::is_alphanumeric)
        })
        .count();
    assert_eq!(
        sideseat_domain::rules::schema::FieldTarget::ALL.len(),
        declared,
        "`FieldTarget::ALL` lists {} of the {declared} declared variants",
        sideseat_domain::rules::schema::FieldTarget::ALL.len()
    );
}

/// Every target's declared **type** reaches the sink that writes it.
///
/// `apply_field` chooses a setter per target - `text()`, `integer()`, `float()`, `list()` - and each returns
/// `None`/empty when the reading is a different variant. So a target whose `field_type()` says `Float` while its
/// arm calls `integer()` resolves perfectly, converts perfectly, and writes **nothing**: the column stays unset
/// and no diagnostic says why. Nothing checked the correspondence, and it is exactly the kind of pair that drifts
/// when a column's type changes.
///
/// Asked by feeding each target a reading of its own declared type and requiring the write to be observable, in
/// either sink - the counters go to `TokenReadings` rather than to a column.
#[test]
fn every_target_writes_what_its_declared_type_produces() {
    use sideseat_domain::rules::schema::{FieldTarget, FieldType};
    use sideseat_domain::rules::span_fields::{Reading, Resolved};

    for target in FieldTarget::ALL {
        // A value of the target's own type that is inside whatever range it admits, so this measures the sink and
        // not the configured bound.
        let reading = match target.field_type() {
            // Text that is also a JSON object, so a text target storing what the text encodes (`metadata`) is
            // measured on a value it can hold rather than one it rightly drops.
            FieldType::Text => Reading::Text(r#"{"probe":true}"#.to_string()),
            FieldType::Integer => Reading::Integer(1),
            FieldType::Float => Reading::Float(0.5),
            FieldType::StringList => Reading::StringList(vec!["probe".to_string()]),
        };
        let resolved = Resolved {
            target: *target,
            reading,
            rule_id: "probe.rule".to_string(),
            evidence: None,
            refused: Vec::new(),
        };
        let mut span = SpanData::default();
        let mut tokens = crate::traces::extract::attributes::TokenReadings::default();
        let before = format!("{span:?}{tokens:?}");
        crate::traces::extract::attributes::apply_field_for_test(&mut span, &resolved, &mut tokens);
        assert_ne!(
            format!("{span:?}{tokens:?}"),
            before,
            "`{target:?}` declares {:?} and its sink wrote nothing, so a value of its own type is silently lost",
            target.field_type()
        );
    }
}

/// **Every** span-field refusal fires, because none of them did.
///
/// Seventeen refusals, each a statement about a declaration that cannot mean what it says, and not one was
/// exercised anywhere - so each was a claim rather than a guard, and any of them could have been deleted or
/// narrowed with the suite still green. Several are one edit from being unreachable: the exclusivity check *counts*
/// the seven reader forms (it used to pattern-match a pair, which stopped covering the forms as they were added),
/// and a count that drifted to six would silently admit the form it forgot.
///
/// One probe per refusal, matched on the variant rather than on the message, so rewording a diagnostic does not
/// quietly stop testing it.
#[test]
fn every_span_field_refusal_fires() {
    use sideseat_domain::rules::span_fields::{FieldCompileError as E, compile};

    let compiled = |asset: &str| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                asset.as_bytes().to_vec(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    /// A probe asset, and the refusal it must produce.
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        (
            "a rule with no source",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","sources":[]}]}"#,
            |e| matches!(e, E::NoSources { .. }),
        ),
        (
            "a source naming nowhere to read from",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","sources":[{"id":"s"}]}]}"#,
            |e| matches!(e, E::SourceReadsNothing { .. }),
        ),
        (
            "a source naming two places, where the reader's branch order would decide",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","raw_span_name":true}]}]}"#,
            |e| matches!(e, E::SourceReadsTwoThings { .. }),
        ),
        (
            "a literal with no gate, which answers on every span",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","value":"x"}]}]}"#,
            |e| matches!(e, E::UngatedLiteral { .. }),
        ),
        (
            "a JSON source naming neither a path nor a first-present group",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a"}}]}]}"#,
            |e| matches!(e, E::JsonNamesNoMember { .. }),
        ),
        (
            "a sum into a field that holds text",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a","path":"$.n","reduce":"sum"}}]}]}"#,
            |e| matches!(e, E::ReductionThatCannotYield { .. }),
        ),
        (
            "a reduction over a first-present group, which selects one path rather than combining matches",
            r#"{"id":"t","span_fields":[{"id":"f","target":"usage_input_tokens",
               "sources":[{"id":"s","json":{"attribute":"a","first_present_of":["$.a","$.b"],"reduce":"sum"}}]}]}"#,
            |e| matches!(e, E::ReductionWithoutAPath { .. }),
        ),
        (
            "folding a field that holds no text",
            r#"{"id":"t","span_fields":[{"id":"f","target":"usage_input_tokens",
               "sources":[{"id":"s","attribute":"k","lowercase":true}]}]}"#,
            |e| matches!(e, E::FoldWithoutText { .. }),
        ),
        (
            "`scalar_only` where it cannot apply",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a","first_present_of":["$.a"],"scalar_only":true}}]}]}"#,
            |e| matches!(e, E::ScalarOnlyWithoutAPath { .. }),
        ),
        (
            "an empty attribute name",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":""}]}]}"#,
            |e| matches!(e, E::EmptyAttribute { .. }),
        ),
        (
            "a merge into a field that holds one value",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","combine":"merge_all",
               "sources":[{"id":"s","attribute":"k"}]}]}"#,
            |e| matches!(e, E::MergeIntoScalar { .. }),
        ),
        (
            "two rules resolving one target",
            r#"{"id":"t","span_fields":[
               {"id":"f","target":"user_id","sources":[{"id":"s","attribute":"k"}]},
               {"id":"g","target":"user_id","sources":[{"id":"s","attribute":"j"}]}]}"#,
            |e| matches!(e, E::DuplicateTarget { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id":"t","span_fields":[
               {"id":"f","target":"user_id","sources":[{"id":"s","attribute":"k"}]},
               {"id":"f","target":"http_method","sources":[{"id":"s","attribute":"j"}]}]}"#,
            |e| matches!(e, E::DuplicateId { .. }),
        ),
        (
            "a gate that can never hold",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"attr_prefix":[""]}}]}]}"#,
            |e| matches!(e, E::DeadGate { .. }),
        ),
        (
            "a gate naming a resource dimension field resolution is never given",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"service_name":["x"]}}]}]}"#,
            |e| matches!(e, E::UnavailableGate { .. }),
        ),
    ];

    for (what, asset, expected) in cases {
        let error = compiled(asset)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }

    // Two *sources* of one rule sharing an id is a declaration defect, refused where the assets are parsed and
    // so before any section compiler can see the file.
    assert!(
        ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k"},{"id":"s","attribute":"j"}]}]}"#
                .to_vec(),
        )]))
        .is_err()
    );

    // And a rule stating one reader with nothing dead about it compiles, or the refusals are simply a ban.
    assert!(
        compiled(
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k"},{"id":"t","json":{"attribute":"a","path":"$.u"}}]}]}"#
        )
        .is_ok()
    );
}

/// A failed span that names its `error.type` reports its own status message as the error.
///
/// OpenTelemetry's Google Gen AI instrumentation records a tool that raised as `error.type` plus an
/// ERROR status carrying the exception message, with no `exception` event. The status alone is not
/// evidence - frameworks copy it onto every ancestor - so the tool's error used to vanish from the
/// conversation. `error.type` is set on the failed operation only, which makes the message this span's.
#[test]
fn an_error_type_makes_the_status_message_the_spans_own_error() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};

    let kv = |key: &str, value: &str| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    };
    let failed_with = |attributes: Vec<KeyValue>, message: &str| {
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: None,
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![Span {
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        name: "execute_tool book_flight".to_string(),
                        kind: 1,
                        start_time_unix_nano: 1_700_000_000_000_000_000,
                        end_time_unix_nano: 1_700_000_000_100_000_000,
                        attributes,
                        status: Some(Status {
                            message: message.to_string(),
                            code: 2,
                        }),
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let span = crate::traces::extract::extract_attributes_batch(&request)
            .into_iter()
            .next()
            .expect("one span");
        (span.exception_type, span.exception_message)
    };
    let failed = |attributes| failed_with(attributes, "No seats: the booking system is offline.");

    assert_eq!(
        failed(vec![kv("error.type", "BookingUnavailable")]),
        (
            Some("BookingUnavailable".to_string()),
            Some("No seats: the booking system is offline.".to_string())
        )
    );
    assert_eq!(
        failed(Vec::new()),
        (None, None),
        "a bare ERROR status may be inherited from a child, so it stays out of the conversation"
    );
    assert_eq!(
        failed_with(vec![kv("error.type", "tool_error")], ""),
        (None, None),
        "a class with no message says less than the tool result beside it"
    );
    assert_eq!(
        failed_with(vec![kv("error.type", "TOOL_ERROR")], "TOOL_ERROR"),
        (None, None),
        "a status that repeats the class explains no more than the class alone"
    );
}

/// A producer's own price reaches the span through the declared fields, and a negative one is refused rather
/// than cancelling real spend in a total.
#[test]
fn a_reported_cost_is_read_from_the_declared_fields() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("llm.cost.total", "0.0125"),
        ("llm.cost.prompt", "0.01"),
        ("llm.cost.completion", "-0.0025"),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "ChatCompletion");
    assert_eq!(span.extracted_cost_total, Some(0.0125));
    assert_eq!(span.extracted_cost_input, Some(0.01));
    assert_eq!(span.extracted_cost_output, None);
}

/// The declared `metadata` field stores exactly what the retired read did: the JSON the bare attribute encodes,
/// and nothing for text that encodes none or an attribute that is absent.
#[test]
fn the_declared_metadata_field_reads_as_the_constant_it_replaced() {
    for value in [
        Some(r#"{"run_id": "r1", "tags": ["a"]}"#),
        Some("[1, 2]"),
        Some("\"a string\""),
        Some("42"),
        Some("not json"),
        Some(""),
        None,
    ] {
        let attrs = match value {
            Some(value) => make_attrs(&[("metadata", value)]),
            None => make_attrs(&[]),
        };
        let retired = attrs
            .get("metadata")
            .and_then(|m| serde_json::from_str::<JsonValue>(m).ok())
            .unwrap_or(JsonValue::Null);
        let mut span = SpanData::default();
        apply_span_fields(&mut span, "", &attrs, &[]);
        assert_eq!(span.metadata, retired, "{value:?}");
    }
}
