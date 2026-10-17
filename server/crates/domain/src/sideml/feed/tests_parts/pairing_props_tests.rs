// The pairing generator: a trace's tool executions as ground truth, recorded the way an agent loop records them,
// and the law the execution table must keep (`server/specs/CallPairing.tla`, `Conservation` and `Matching`).
//
// Each turn's generation is sent the whole history so far - every earlier call and every result, re-sent - and
// emits the turn's calls; each execution's tool span records its own result. Every copy restates an execution
// whose emission is in the trace, and a request re-sends the whole history, so no copy is ambiguous: the trace
// shows each call once and each result once, its own call's, whatever the ids, shapes and contents.

/// One execution of the truth: its call id (`None`: id-less), its tool and arguments, and its result, if any.
#[derive(Debug, Clone)]
struct TruthExecution {
    id: Option<String>,
    shape: u8,
    result: Option<u8>,
    turn: u8,
}

fn pairing_call(execution: &TruthExecution) -> serde_json::Value {
    let mut call = json!({"type": "tool_use", "name": format!("tool_{}", execution.shape % 2), "input": {"arg": execution.shape / 2}});
    if let Some(id) = &execution.id {
        call["id"] = json!(id);
    }
    call
}

fn pairing_result(execution: &TruthExecution, content: u8) -> serde_json::Value {
    let mut result =
        json!({"type": "tool_result", "content": format!("said {content}"), "is_error": false});
    match &execution.id {
        Some(id) => result["tool_use_id"] = json!(id),
        None => result["name"] = json!(format!("tool_{}", execution.shape % 2)),
    }
    result
}

/// The rows of a trace whose executions are `truth`, in turn order.
fn pairing_trace(truth: &[TruthExecution]) -> Vec<MessageSpanRow> {
    let t0 = pairing_t0();
    let time = |ms: i64| t0 + chrono::Duration::milliseconds(ms);
    let at = |key: &str, ms: i64| json!({"attribute": {"key": key, "time": time(ms).to_rfc3339()}});
    let turns = truth.iter().map(|e| e.turn).max().map_or(0, |t| t + 1);
    let mut rows = Vec::new();
    for turn in 0..=turns {
        let start = i64::from(turn) * 100;
        let mut messages = vec![
            json!({"source": at("llm.input_messages", start), "content": {"role": "user", "content": "Go."}}),
        ];
        for earlier in truth.iter().filter(|e| e.turn < turn) {
            messages.push(json!({"source": at("llm.input_messages", start), "content": {"role": "assistant", "content": [pairing_call(earlier)]}}));
            if let Some(content) = earlier.result {
                messages.push(json!({"source": at("llm.input_messages", start), "content": {"role": "tool", "content": [pairing_result(earlier, content)]}}));
            }
        }
        let emitted: Vec<serde_json::Value> = truth
            .iter()
            .filter(|e| e.turn == turn)
            .map(pairing_call)
            .collect();
        let output = if emitted.is_empty() {
            json!("Done.")
        } else {
            json!(emitted)
        };
        messages.push(json!({"source": at("llm.output_messages", start + 10), "content": {"role": "assistant", "content": output}}));
        rows.push(make_span_row_full(
            "trace-pairing",
            &format!("gen-{turn}"),
            Some("root"),
            &serde_json::Value::Array(messages).to_string(),
            time(start),
            Some(time(start + 10)),
            Some("generation"),
        ));
        for (i, execution) in truth.iter().enumerate().filter(|(_, e)| e.turn == turn) {
            let Some(content) = execution.result else {
                continue;
            };
            let ms = start + 20 + i64::try_from(i).unwrap_or(0);
            rows.push(make_span_row_full(
                "trace-pairing",
                &format!("tool-{i}"),
                Some("root"),
                &json!([{"source": at("output.value", ms), "content": {"role": "tool", "content": [pairing_result(execution, content)]}}]).to_string(),
                time(ms),
                Some(time(ms)),
                Some("tool"),
            ));
        }
    }
    rows
}

/// Executions with ids drawn from `ids` (`None` among them: id-less) over two tools with two arguments each.
fn pairing_truth(ids: Vec<Option<&'static str>>) -> impl Strategy<Value = Vec<TruthExecution>> {
    let id = proptest::sample::select(ids);
    proptest::collection::vec((id, 0u8..4, proptest::option::of(0u8..2), 0u8..3), 1..5).prop_map(
        |drawn| {
            let mut truth: Vec<TruthExecution> = drawn
                .into_iter()
                .map(|(id, shape, result, turn)| TruthExecution {
                    id: id.map(String::from),
                    shape,
                    result,
                    turn,
                })
                .collect();
            truth.sort_by_key(|e| e.turn);
            truth
        },
    )
}

fn distinct<T: Ord>(values: impl Iterator<Item = T>) -> bool {
    let mut values: Vec<T> = values.collect();
    let all = values.len();
    values.sort();
    values.dedup();
    values.len() == all
}

/// Every call shown once, every result once and after its own call.
fn shown_once(truth: &[TruthExecution]) -> Result<(), TestCaseError> {
    let blocks = process_trace_spans(pairing_trace(truth), &FeedOptions::new()).messages;
    let calls = blocks
        .iter()
        .filter(|b| matches!(b.content, ContentBlock::ToolUse { .. }))
        .count();
    let results = blocks
        .iter()
        .filter(|b| matches!(b.content, ContentBlock::ToolResult { .. }))
        .count();
    let answered = truth.iter().filter(|e| e.result.is_some()).count();
    prop_assert_eq!(calls, truth.len(), "calls of {:?}", truth);
    prop_assert_eq!(results, answered, "results of {:?}", truth);
    for (at, block) in blocks.iter().enumerate() {
        if let ContentBlock::ToolResult {
            tool_use_id: Some(id),
            ..
        } = &block.content
        {
            let called_before = blocks[..at]
                .iter()
                .any(|b| matches!(&b.content, ContentBlock::ToolUse { id: Some(call), .. } if call == id));
            prop_assert!(
                called_before,
                "result {} before its call in {:?}",
                id,
                truth
            );
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x51de_5ea7),
        ..ProptestConfig::default()
    })]

    /// The control: distinct ids and distinct shapes, so nothing is a retry and nothing alike. The passes keep it
    /// today, which shows the generator records what the feed reads.
    #[test]
    fn distinct_executions_are_shown_once(truth in pairing_truth(vec![Some("a"), Some("b"), Some("c"), Some("d")])
        .prop_filter("nothing alike", |t| {
            distinct(t.iter().map(|e| e.id.clone())) && distinct(t.iter().map(|e| e.shape))
        })) {
        shown_once(&truth)?;
    }

    /// Distinct ids, alike in tool and arguments: two parallel calls after an earlier alike one lose the first,
    /// ranked as the call the generation was sent.
    #[test]
    #[ignore = "pairing: generator: alike calls under distinct ids merge across turns by position rank"]
    fn alike_executions_under_distinct_ids_are_shown_once(truth in pairing_truth(vec![Some("a"), Some("b"), Some("c"), Some("d")])
        .prop_filter("ids distinct", |t| distinct(t.iter().map(|e| e.id.clone())))) {
        shown_once(&truth)?;
    }

    /// Reused ids, id-less calls, retries alike in tool and arguments: the execution table's law.
    #[test]
    #[ignore = "pairing: generator: reused and id-less executions are merged or duplicated"]
    fn every_execution_is_shown_once(truth in pairing_truth(vec![Some("a"), Some("b"), None])) {
        shown_once(&truth)?;
    }
}
