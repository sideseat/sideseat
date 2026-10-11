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
        frames,
        ..
    } = scope
    else {
        panic!("{label} / {view_name}: not a composed request scope");
    };
    for r in rows {
        let own = (&r.trace_id, &r.span_id) == (trace_id, span_id);
        let origin = (r.trace_id.clone(), r.span_id.clone());
        assert!(
            own || thread.contains(&origin) || calls.contains(&origin) || frames.contains(&origin),
            "{label} / {view_name}: a block from {origin:?} - neither this request, nor its thread, nor a tool \
             span it owns, nor a frame recorded for it - leaked into a composed span view"
        );
    }
}

/// ADK lists one request twice - its `call_llm` span's `llm_request` attribute and its `generate_content` span's
/// `gen_ai.user.message` log events - and quotes each earlier agent step behind the same preamble. Each step keeps
/// its own preamble, from one carrier, in the order the request sent them. Read as one occurrence in one carrier
/// and as several in the other, the copies were matched to the wrong messages: three preambles survived from the
/// attribute and the steps from the events, and the view grouped the preambles ahead of the instruction.
#[test]
fn a_request_listed_on_two_carriers_keeps_each_quoted_step_with_its_preamble() {
    let label = "_synthetic/quoted_steps_on_two_carriers";
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| l == label)
        .expect("the two-carrier request");
    let built = build_golden(label, &paths, &rows_for(&paths));
    let (_, _, rows) = built
        .invariants
        .iter()
        .find(|(_, scope, _)| matches!(scope, Scope::Trace { .. }))
        .expect("the trace view");
    let users: Vec<&InvariantRow> = rows.iter().filter(|r| r.role == "user").collect();
    let texts: Vec<&str> = users.iter().map(|r| r.content.as_str()).collect();
    let steps: Vec<usize> = (0..users.len())
        .filter(|&i| texts[i].starts_with("[researcher]"))
        .collect();
    assert_eq!(steps.len(), 4, "four quoted steps: {texts:#?}");
    for &at in &steps {
        let (step, preamble) = (users[at], at.checked_sub(1).map(|i| users[i]));
        assert!(
            preamble.is_some_and(|p| p.content.starts_with("For context:")
                && p.span_id == step.span_id
                && p.carrier == step.carrier),
            "step {at} is not preceded by its own preamble, from its own carrier: {texts:#?}"
        );
    }
    // Without the occurrence their carrier proves, four preambles in one trace are a duplicate: the check that
    // caught the split still holds the line.
    let carrier = users[steps[0]].carrier.clone();
    let mut unproven = rows.clone();
    for row in unproven.iter_mut().filter(|r| r.carrier == carrier) {
        row.carrier_proves_occurrence = false;
    }
    let caught = std::panic::catch_unwind(|| assert_no_duplicates(label, "trace", &unproven, None));
    assert!(
        caught.is_err(),
        "unproven repeats passed the duplicate check"
    );
}

/// The duplicate invariant counts what the truth holds. ADK quotes each earlier agent step behind the same
/// preamble, and the truth holds one preamble per step: four copies no carrier proves pass because the truth holds
/// four, a fifth is a duplicate, one of the four taken away is the truth's to report missing, and a part repeated
/// still fires.
#[test]
fn the_duplicate_invariant_counts_what_the_truth_holds() {
    let label = "adk/native/multi_agent";
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| l == label)
        .expect("the ADK multi-agent fixture");
    let truths = message_truth::Truths::load();
    let (rows, spans) = message_truth::read(&paths);
    let built = build_golden(label, &paths, &rows);
    let (found, allowed) = truths.check(label, &paths, &built, spans);
    let allowed = allowed.expect("a truth describes the fixture");
    let preamble =
        |row: &InvariantRow| row.role == "user" && row.content.starts_with("For context:");
    let at = built
        .invariants
        .iter()
        .position(|(_, scope, rows)| {
            matches!(scope, Scope::Trace { .. }) && rows.iter().any(preamble)
        })
        .expect("the trace quoting the steps");
    let (name, _, rows) = &built.invariants[at];
    let fires = |rows: &[InvariantRow], allowed: Option<&[message_truth::Allowance]>| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_duplicates(label, name, rows, allowed)
        }))
        .is_err()
    };
    // As a carrier proving nothing by position lists them.
    let mut unproven = rows.clone();
    for row in &mut unproven {
        row.carrier_proves_occurrence = false;
    }
    let preambles: Vec<usize> = (0..rows.len()).filter(|&i| preamble(&rows[i])).collect();
    assert_eq!(preambles.len(), 4, "four quoted steps");
    assert!(
        fires(&unproven, None),
        "without the truth, four copies nothing proves are a duplicate"
    );
    assert!(
        !fires(&unproven, Some(&allowed[at])),
        "the truth holds four preambles"
    );
    // A copy at a place of its own in the payload, as a message the parser invented would be.
    let with_copy = |of: usize| {
        let mut rows = unproven.clone();
        let mut counts = allowed[at].clone();
        let mut copy = unproven[of].clone();
        copy.position = "999.0".to_string();
        rows.push(copy);
        counts.push(allowed[at][of]);
        (rows, counts)
    };
    let (fifth, counts) = with_copy(preambles[0]);
    assert!(
        fires(&fifth, Some(&counts)),
        "a fifth preamble the truth does not hold"
    );
    // Four copies, but the fourth is the first again: one observation twice, and one step without its preamble.
    let mut replaced = unproven.clone();
    replaced[preambles[3]] = unproven[preambles[0]].clone();
    assert!(
        fires(&replaced, Some(&allowed[at])),
        "a preamble shown twice from one place"
    );
    let question = (0..rows.len())
        .find(|&i| rows[i].role == "user" && allowed[at][i].facts == 1)
        .expect("a prompt the truth holds once");
    let (repeated, counts) = with_copy(question);
    assert!(fires(&repeated, Some(&counts)), "a part repeated");

    let (rows, spans) = message_truth::read(&paths);
    let mut fewer = build_golden(label, &paths, &rows);
    fewer.invariants[at].2.remove(preambles[3]);
    let (after, allowed) = truths.check(label, &paths, &fewer, spans);
    assert!(
        !fires(
            &fewer.invariants[at].2,
            Some(&allowed.expect("the truth")[at])
        ),
        "three preambles of four are no duplicate"
    );
    // Four identical facts and three blocks: the fact left without one is reported, as one whose only blocks the
    // others took.
    let lost: Vec<(&str, &str)> = after
        .iter()
        .filter(|v| !found.contains(v) && v.view == message_truth::ViolationView::Trace)
        .map(|v| (v.assertion.as_str(), v.subject.as_str()))
        .collect();
    assert!(
        matches!(lost[..], [("user_text.claimed_twice", fact)] if fact.starts_with("fact-")),
        "the preamble taken away is reported: {lost:?}"
    );
}

/// Two copies of one signature are one turn of reasoning, whatever the truth holds. pydantic-ai's agent span lists
/// the last call's withheld reasoning, and as its span exports no signature the view may show either of the
/// truth's two withheld turns there, each sealed with its own signature; a block carrying a signature shows only the
/// turn sealed with it, so two copies of the first signature are the first turn twice.
#[test]
fn two_copies_of_one_signature_are_not_two_turns_of_reasoning() {
    let label = "pydantic-ai/native/multi_turn";
    let (_, paths) = discover_fixtures()
        .into_iter()
        .find(|(l, _)| l == label)
        .expect("the pydantic-ai multi-turn fixture");
    let truth: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_root().join("../truth/pydantic-ai/multi_turn.json"))
            .expect("the truth"),
    )
    .expect("the truth parses");
    let seals: Vec<&str> = truth["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .filter(|f| f["kind"] == "reasoning")
        .filter_map(|f| f["seal"].as_str())
        .collect();
    assert_eq!(seals.len(), 2, "two withheld turns, each sealed");
    let truths = message_truth::Truths::load();
    // The agent span's reasoning, and a second copy of it at a place of its own, both carrying `seal`: what the
    // truth allows each, and whether the duplicate invariant fires.
    let check = |seal: Option<&str>| {
        let (rows, spans) = message_truth::read(&paths);
        let mut built = build_golden(label, &paths, &rows);
        let at = built
            .invariants
            .iter()
            .position(|(name, _, _)| name.ends_with("invoke_agent agent/span-6"))
            .expect("the last turn's agent span");
        let view = &mut built.invariants[at].2;
        let thinking = (0..view.len())
            .find(|&i| view[i].entry_type == "thinking")
            .expect("the reasoning");
        view[thinking].seal = seal.map(str::to_string);
        let mut copy = view[thinking].clone();
        copy.position = "999.0".to_string();
        view.push(copy);
        let (_, allowed) = truths.check(label, &paths, &built, spans);
        let allowed = allowed.expect("a truth describes the fixture");
        let (name, _, rows) = &built.invariants[at];
        let fires = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_duplicates(label, name, rows, Some(&allowed[at]))
        }))
        .is_err();
        (allowed[at][thinking].facts, fires)
    };
    assert_eq!(
        check(None),
        (2, false),
        "unsigned, either turn: two copies, two turns"
    );
    assert_eq!(
        check(Some(seals[0])),
        (1, true),
        "the first signature twice is the first turn twice"
    );
    assert_eq!(
        check(Some(seals[1])).0,
        1,
        "the second signature is the second turn"
    );
    assert_eq!(
        check(Some("00")).0,
        0,
        "a signature no turn was sealed with is neither"
    );
}

/// The goldens read rows as the store returns them, marks included: a row built without the marks the ingest
/// decided is one no mark-asking projection ever matches, so the projection would pass every golden unseen.
#[test]
fn the_goldens_read_the_marks_the_ingest_decided() {
    // Discovery honours `MESSAGE_FIXTURES`, so a run narrowed to other producers has nothing to assert.
    let Some((_, paths)) = discover_fixtures()
        .into_iter()
        .find(|(label, _)| label == "logfire/native@4.18.0/streaming")
    else {
        assert!(
            std::env::var_os("MESSAGE_FIXTURES").is_some(),
            "the capture whose streamed requests are marked is missing from an unrestricted discovery"
        );
        return;
    };
    let rows = rows_for(&paths);
    let marked = rows.iter().filter(|(_, row)| row.span_marks != 0).count();
    assert!(marked > 0, "no row carries the streamed-request mark");

    // A withdrawal demanding restatement (`only_restated`) is honoured only beside the kept row that
    // restates it. Read alone - a span-scoped query, or a trace whose completed streaming record never
    // arrived - the marked request span's copy is the only one, so it stays. The request span closes
    // successfully before the stream completes, so `successful_only` cannot tell this case apart.
    let label = "logfire/native@4.18.0/streaming";
    let lifetime: Vec<(String, MessageSpanRow)> = rows
        .iter()
        .filter(|(_, row)| {
            row.span_name
                .as_deref()
                .is_some_and(|name| name.starts_with("Responses API with "))
        })
        .cloned()
        .collect();
    assert!(
        !lifetime.is_empty(),
        "the lifetime spans are in the capture"
    );
    let alone = build_golden(label, &paths, &lifetime);
    let kept: usize = alone
        .invariants
        .iter()
        .filter(|(_, scope, _)| matches!(scope, Scope::Trace { .. }))
        .map(|(_, _, rows)| rows.len())
        .sum();
    assert!(
        kept > 0,
        "with its completed companion absent, the request span's only copy must stay in the conversation"
    );

    // A witness the collapsed view hides is no witness. Push every companion's span start past its own
    // messages: on a child generation span, a message older than the span start is history and the view
    // drops it - so if the withdrawal had been licensed by the raw copy rather than by the survivor, the
    // conversation would keep neither copy. The withdrawal must fail its proof and the marked span's own
    // copy must stay visible.
    let hidden_companions: Vec<(String, MessageSpanRow)> = rows
        .iter()
        .map(|(label, row)| {
            let mut row = row.clone();
            if row
                .span_name
                .as_deref()
                .is_some_and(|name| name.starts_with("streaming response"))
            {
                row.observation_type = Some("generation".to_string());
                row.span_timestamp += chrono::Duration::days(365);
            }
            (label.clone(), row)
        })
        .collect();
    let starved = build_golden(label, &paths, &hidden_companions);
    let user_turn_shown = starved
        .invariants
        .iter()
        .filter(|(_, scope, _)| matches!(scope, Scope::Trace { .. }))
        .flat_map(|(_, _, rows)| rows.iter())
        .any(|row| row.role == "user" && row.content.contains("weather in Rome"));
    assert!(
        user_turn_shown,
        "with every companion copy hidden as history, the marked span's own copy is the conversation's \
         only one and must stay"
    );
}
