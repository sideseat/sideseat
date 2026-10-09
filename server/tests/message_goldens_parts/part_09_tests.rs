// ============================================================================
// Equivalence oracle: the declared content chain against the retired Rust readers, over the corpus
// ============================================================================

/// Every JSON object nested anywhere in the captured corpus - span and resource attributes, event and log
/// payloads, and the model requests the recording proxy saw - each read through the content chain by the
/// declared rules and by the retired Rust readers, at all three ways into the chain.
///
/// A superset of the blocks the goldens normalise, which is the point: an object no fixture happens to
/// normalise today is one a producer could send tomorrow, and agreement on it is evidence the goldens cannot
/// give. Encoded values are decoded the way the chain decodes them - JSON text, Python renderings, base64
/// request bodies - so a block serialised inside an attribute is compared as the block it is.
#[test]
fn the_declared_content_chain_matches_the_readers_it_replaced_over_the_corpus() {
    use base64::Engine as _;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value::Value as Otlp};

    fn otlp_json(value: &AnyValue) -> serde_json::Value {
        match &value.value {
            Some(Otlp::StringValue(text)) => serde_json::Value::String(text.clone()),
            Some(Otlp::BoolValue(flag)) => json!(flag),
            Some(Otlp::IntValue(number)) => json!(number),
            Some(Otlp::DoubleValue(number)) => json!(number),
            Some(Otlp::ArrayValue(items)) => {
                serde_json::Value::Array(items.values.iter().map(otlp_json).collect())
            }
            Some(Otlp::KvlistValue(members)) => kv_json(&members.values),
            Some(Otlp::BytesValue(_)) | None => serde_json::Value::Null,
        }
    }
    fn kv_json(members: &[KeyValue]) -> serde_json::Value {
        serde_json::Value::Object(
            members
                .iter()
                .map(|kv| {
                    (
                        kv.key.clone(),
                        kv.value.as_ref().map(otlp_json).unwrap_or_default(),
                    )
                })
                .collect(),
        )
    }

    struct Oracle {
        seen: HashSet<u64>,
        compared: usize,
        reordered: usize,
        reasoning_items: usize,
        cited_texts: usize,
        provider_runs: usize,
        disagreements: Vec<String>,
    }
    impl Oracle {
        fn visit(&mut self, value: &serde_json::Value, depth: usize) {
            if depth > 64 {
                return;
            }
            match value {
                serde_json::Value::String(text) => {
                    let decoded = serde_json::from_str::<serde_json::Value>(text)
                        .ok()
                        .filter(|v| v.is_object() || v.is_array())
                        .or_else(|| {
                            sideseat_domain::sideml::test_support::parse_python_rendering(text)
                        });
                    if let Some(decoded) = decoded {
                        self.visit(&decoded, depth + 1);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        self.visit(item, depth + 1);
                    }
                }
                serde_json::Value::Object(members) => {
                    let digest = {
                        use std::hash::{Hash, Hasher};
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        serde_json::to_string(value)
                            .expect("a value serialises")
                            .hash(&mut hasher);
                        hasher.finish()
                    };
                    if !self.seen.insert(digest) {
                        return;
                    }
                    self.compared += 1;
                    match sideseat_domain::sideml::test_support::content_chain_disagreement(value) {
                        // The one stated difference: a file part's members in the order every other media
                        // block uses. The retired reader wrote the media type last for this part alone.
                        Some((true, _))
                            if matches!(
                                value.get("type").and_then(serde_json::Value::as_str),
                                Some("input_file" | "file")
                            ) =>
                        {
                            self.reordered += 1;
                        }
                        // The second: a Responses API reasoning item, which the retired readers dropped as an
                        // unknown block, is reasoning - with no text and signed where it is withheld.
                        Some(_) if is_responses_reasoning_item(value) => {
                            self.reasoning_items += 1;
                        }
                        // The third and fourth, each checked way by way rather than by the value's shape: a cited
                        // text that is the same text with its citations added, and a provider-run block the
                        // retired readers kept as unknown that is now its own provider-executed call or result.
                        // Any other difference on those shapes is still a disagreement.
                        Some((_, disagreement)) => match stated_difference(value) {
                            Some(Stated::CitedText) => self.cited_texts += 1,
                            Some(Stated::ProviderRun) => self.provider_runs += 1,
                            None => self.disagreements.push(disagreement),
                        },
                        None => {}
                    }
                    for member in members.values() {
                        self.visit(member, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// Which stated difference every way the value reads differently shows, if each does.
    fn stated_difference(value: &serde_json::Value) -> Option<Stated> {
        let mut stated = None;
        for (_, declared, retired) in
            sideseat_domain::sideml::test_support::content_chain_answers(value)
        {
            if declared == retired {
                continue;
            }
            let declared = declared?;
            let this = if cites_what_it_read(value, &declared, retired.as_ref()) {
                Stated::CitedText
            } else if runs_what_it_named(value, &declared, retired.as_ref()) {
                Stated::ProviderRun
            } else {
                return None;
            };
            if stated.is_some_and(|seen| seen != this) {
                return None;
            }
            stated = Some(this);
        }
        stated
    }

    /// The same text with its citations added: a Responses part reads as before plus `citations`, and a Converse
    /// cited answer, which the retired readers kept as one opaque block or did not read, is the text it generated,
    /// joined.
    fn cites_what_it_read(
        value: &serde_json::Value,
        declared: &serde_json::Value,
        retired: Option<&serde_json::Value>,
    ) -> bool {
        let Some(cited) = declared
            .get("citations")
            .and_then(serde_json::Value::as_array)
        else {
            return false;
        };
        if cited.is_empty()
            || declared.get("type").and_then(serde_json::Value::as_str) != Some("text")
        {
            return false;
        }
        let mut without = declared.clone();
        without
            .as_object_mut()
            .expect("a block")
            .remove("citations");
        if retired == Some(&without) {
            return true;
        }
        let generated: Option<String> = value
            .pointer("/citationsContent/content")
            .and_then(serde_json::Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                    .collect()
            });
        generated.is_some_and(|text| {
            without == serde_json::json!({"type": "text", "text": text})
                && retired.is_none_or(|retired| {
                    retired.get("type").and_then(serde_json::Value::as_str) != Some("text")
                })
        })
    }

    /// A block the retired readers kept as unknown - or, read as a singleton result, did not read at all - now the
    /// provider-executed call it names, by its own id and name, or the result of the call its own id answers.
    fn runs_what_it_named(
        value: &serde_json::Value,
        declared: &serde_json::Value,
        retired: Option<&serde_json::Value>,
    ) -> bool {
        let field = |v: &serde_json::Value, key: &str| v.get(key).cloned();
        retired.is_none_or(|retired| {
            retired.get("type").and_then(serde_json::Value::as_str) == Some("unknown")
        }) && declared.get("provider_executed") == Some(&serde_json::json!(true))
            && match declared.get("type").and_then(serde_json::Value::as_str) {
                Some("tool_use") => {
                    field(declared, "id") == field(value, "id")
                        && field(declared, "name") == field(value, "name")
                }
                Some("tool_result") => {
                    field(declared, "tool_use_id") == field(value, "tool_use_id")
                }
                _ => false,
            }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stated {
        CitedText,
        ProviderRun,
    }

    // The excuses see as much as the oracle: a citation that changes the text, or a provider run read under
    // another call's id, is still a disagreement.
    let page = serde_json::json!({"type": "output_text", "text": "Open at nine.",
        "annotations": [{"type": "url_citation", "url": "https://a", "title": "A", "start_index": 0, "end_index": 4}]});
    let cited = serde_json::json!({"type": "text", "text": "Open at nine.",
        "citations": [{"kind": "url", "source": "https://a"}]});
    let plain = serde_json::json!({"type": "text", "text": "Open at nine."});
    assert!(cites_what_it_read(&page, &cited, Some(&plain)));
    // A Responses part the retired readers did not read is not excused: only a Converse cited answer may be.
    assert!(!cites_what_it_read(&page, &cited, None));
    let mut dropped = cited.clone();
    dropped["text"] = serde_json::json!("Open at");
    assert!(!cites_what_it_read(&page, &dropped, Some(&plain)));
    let converse = serde_json::json!({"citationsContent": {"content": [{"text": "write "}, {"text": "a poem"}]}});
    let joined = serde_json::json!({"type": "text", "text": "write a poem",
        "citations": [{"kind": "document", "source": "task"}]});
    assert!(cites_what_it_read(&converse, &joined, None));
    let mut short = joined.clone();
    short["text"] = serde_json::json!("write a");
    assert!(!cites_what_it_read(&converse, &short, None));
    let call = serde_json::json!({"type": "server_tool_use", "id": "s1", "name": "web_search", "input": {}});
    let unknown = serde_json::json!({"type": "unknown", "raw": call.clone()});
    let run = serde_json::json!({"type": "tool_use", "id": "s1", "name": "web_search", "input": {},
        "provider_executed": true});
    assert!(runs_what_it_named(&call, &run, Some(&unknown)));
    assert!(runs_what_it_named(&call, &run, None));
    let mut other_id = run.clone();
    other_id["id"] = serde_json::json!("s2");
    assert!(!runs_what_it_named(&call, &other_id, Some(&unknown)));
    let mut client_run = run.clone();
    client_run
        .as_object_mut()
        .expect("a block")
        .remove("provider_executed");
    assert!(!runs_what_it_named(&call, &client_run, Some(&unknown)));
    // A block the retired readers read as something else is not excused as a provider run.
    assert!(!runs_what_it_named(&call, &run, Some(&plain)));

    /// A Responses API reasoning item: `type` reasoning beside the summary list the API always writes.
    fn is_responses_reasoning_item(value: &serde_json::Value) -> bool {
        value.get("type").and_then(serde_json::Value::as_str) == Some("reasoning")
            && value
                .get("summary")
                .is_some_and(serde_json::Value::is_array)
    }

    let mut oracle = Oracle {
        seen: HashSet::new(),
        compared: 0,
        reordered: 0,
        reasoning_items: 0,
        cited_texts: 0,
        provider_runs: 0,
        disagreements: Vec::new(),
    };
    let mut files = 0_usize;
    let mut pending = vec![fixture_root()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|e| panic!("read {}: {e}", directory.display()));
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if is_captured_request(name) {
                files += 1;
                let request = decode_request(&path);
                for resource in &request.resource_spans {
                    if let Some(attributes) = resource.resource.as_ref().map(|r| &r.attributes) {
                        oracle.visit(&kv_json(attributes), 0);
                    }
                    for scope in &resource.scope_spans {
                        for span in &scope.spans {
                            oracle.visit(&kv_json(&span.attributes), 0);
                            for event in &span.events {
                                oracle.visit(&kv_json(&event.attributes), 0);
                            }
                        }
                    }
                }
            } else if is_captured_logs(name) {
                files += 1;
                for resource in &decode_logs(&path).resource_logs {
                    for scope in &resource.scope_logs {
                        for record in &scope.log_records {
                            oracle.visit(&kv_json(&record.attributes), 0);
                            if let Some(body) = &record.body {
                                oracle.visit(&otlp_json(body), 0);
                            }
                        }
                    }
                }
            } else if name == "model-requests.json" {
                files += 1;
                let transcript: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&path)
                        .unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
                )
                .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
                for interaction in transcript["interactions"].as_array().into_iter().flatten() {
                    let Some(body) = interaction["body"].as_str() else {
                        continue;
                    };
                    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body) else {
                        continue;
                    };
                    if let Ok(request) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        oracle.visit(&request, 0);
                    }
                }
            }
        }
    }

    // Said aloud, because an oracle that compared nothing agrees with everything.
    assert!(files > 1000, "the oracle read only {files} corpus files");
    assert!(
        oracle.compared > 10_000,
        "the oracle compared only {} distinct objects",
        oracle.compared
    );
    eprintln!(
        "content-chain oracle: {} distinct objects from {files} files, {} file parts reordered, {} \
         reasoning items read",
        oracle.compared, oracle.reordered, oracle.reasoning_items
    );
    // Each stated difference is one the corpus holds, so the allowance cannot outlive what it allows.
    assert!(
        oracle.reasoning_items > 0,
        "no captured Responses reasoning item: the stated difference covers nothing"
    );
    assert!(
        oracle.cited_texts > 0,
        "no captured cited text: the stated difference covers nothing"
    );
    assert!(
        oracle.provider_runs > 0,
        "no captured provider-run block: the stated difference covers nothing"
    );
    oracle.disagreements.sort();
    oracle.disagreements.dedup();
    assert!(
        oracle.disagreements.is_empty(),
        "{} corpus value(s) are read differently by the declared chain and the retired readers:\n{}",
        oracle.disagreements.len(),
        oracle
            .disagreements
            .iter()
            .take(40)
            .map(|d| d.chars().take(600).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Every block of a composed request's view belongs to it: the request itself, its thread's earlier requests, or a
/// tool span whose call one of those requests answered.
///
/// A composed view holds what the call was *sent*, which is more than the span's own payload - so the span-scope
/// check cannot apply, and this is what replaces it rather than an exemption. Checkable at all because a composed
/// block keeps the span it came from, which is also the provenance the rubric reads.
fn assert_composed_scope(label: &str, view_name: &str, scope: &Scope, rows: &[InvariantRow]) {
    let Scope::RequestSpan {
        trace_id,
        span_id,
        thread,
        calls,
    } = scope
    else {
        panic!("{label} / {view_name}: not a composed request scope");
    };
    for r in rows {
        let own = (&r.trace_id, &r.span_id) == (trace_id, span_id);
        let origin = (r.trace_id.clone(), r.span_id.clone());
        assert!(
            own || thread.contains(&origin) || calls.contains(&origin),
            "{label} / {view_name}: a block from {origin:?} - neither this request, nor its thread, nor a tool \
             span it owns - leaked into a composed span view"
        );
    }
}
