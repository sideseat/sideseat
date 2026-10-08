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
                        Some((_, disagreement)) => self.disagreements.push(disagreement),
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
