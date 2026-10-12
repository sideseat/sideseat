// Absence proofs over texts exported in pieces, and reasoning a flattened family marks.

/// A text exported cut into pieces - each part its own string, none holding a fragment the search looks for - is
/// still exported: a gap claiming it absent is refused.
#[test]
fn a_text_cut_into_pieces_is_not_absent() {
    let pieces: Vec<String> = TEXT
        .chars()
        .collect::<Vec<_>>()
        .chunks(16)
        .map(|chunk| chunk.iter().collect())
        .collect();
    let segmented = span(|s| {
        s.attributes = pieces
            .iter()
            .enumerate()
            .map(|(i, piece)| kv(&format!("parts.{i}.text"), string(piece)))
            .collect();
    });
    assert!(matches!(prove(&text_fact(), &segmented), Proof::Partial(_)));
    // However finely cut: four characters at a time, whitespace and all.
    let fine = span(|s| {
        s.attributes = TEXT
            .chars()
            .collect::<Vec<_>>()
            .chunks(4)
            .enumerate()
            .map(|(i, chunk)| {
                kv(
                    &format!("parts.{i}.text"),
                    string(&chunk.iter().collect::<String>()),
                )
            })
            .collect();
    });
    assert!(matches!(prove(&text_fact(), &fine), Proof::Partial(_)));
    // A text too short for a fragment is covered whole.
    let short = fact_with_text("Weigh the alternatives.");
    let halves = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Weigh the")),
            kv("parts.1.text", string(" alternatives.")),
        ];
    });
    assert!(matches!(prove(&short, &halves), Proof::Partial(_)));
    // Words the text holds, scattered through other content, are not its pieces.
    let scattered = span(|s| {
        s.attributes = vec![
            kv("a", string("Paris")),
            kv("b", string("sunny")),
            kv("c", string("umbrella")),
        ];
    });
    assert_eq!(prove(&text_fact(), &scattered), Proof::Absent);
    // Nor are words of it that other content holds in its order, short of a fragment.
    let forecast =
        fact_with_text("Paris weather forecasts predict clear skies throughout tomorrow.");
    let metadata = span(|s| {
        s.attributes = vec![
            kv("city", string("Paris")),
            kv("topic", string("weather")),
            kv("kind", string("forecasts")),
        ];
    });
    assert_eq!(prove(&forecast, &metadata), Proof::Absent);
    // Pieces of the text that do not continue one another - a word here, a phrase further on - are not its run.
    let apart = span(|s| {
        s.attributes = vec![
            kv("a", string("Paris will be")),
            kv("b", string("leave the umbrella")),
            kv("c", string("at home.")),
        ];
    });
    assert_eq!(prove(&text_fact(), &apart), Proof::Absent);
    // Metadata the text quotes in its middle - a path - is not the text, though its parts continue each other:
    // a run must cover the text's head, its tail or a sentence.
    let read = fact_with_text(
        "Read the forecast file in /private/tmp/sideseat-cli/workdir and summarise what it says",
    );
    let path = span(|s| {
        s.attributes = vec![
            kv("process.cwd", string("/private/tmp/sideseat")),
            kv("process.dir", string("-cli/workdir")),
        ];
    });
    assert_eq!(prove(&read, &path), Proof::Absent);
    // A string between the pieces that could continue the run is passed over when it does not lead on.
    let print = fact_with_text("Print text exactly.");
    let typed = span(|s| {
        s.attributes = vec![
            kv("parts.0.type", string("text")),
            kv("parts.0.text", string("Print ")),
            kv("parts.1.type", string("text")),
            kv("parts.1.text", string("text exactly.")),
        ];
    });
    assert!(matches!(prove(&print, &typed), Proof::Partial(_)));
    // A run may start where the first piece occurs a second time, overlapping the first.
    let repeated = fact_with_text(&format!("aaaa{}", "X".repeat(21)));
    let overlapping = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("aaa")),
            kv("parts.1.text", string(&"X".repeat(21))),
        ];
    });
    assert!(matches!(prove(&repeated, &overlapping), Proof::Partial(_)));
    // The members' indices order them, whatever order they were written in.
    let reversed = span(|s| {
        s.attributes = vec![
            kv("parts.1.text", string(" alternatives.")),
            kv("parts.0.text", string("Weigh the")),
        ];
    });
    assert!(matches!(prove(&short, &reversed), Proof::Partial(_)));
    // A fragment the text holds twice counts where the pieces hold it.
    let twice = fact_with_text("abcdefghijklmnopqrstuvwx and then UNIQUE abcdefghijklmnopqrstuvwx");
    let tail = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("UNIQUE abcdefgh")),
            kv("parts.1.text", string("ijklmnopqrstuvwx")),
        ];
    });
    assert!(matches!(prove(&twice, &tail), Proof::Partial(_)));
    // Pieces that hold the text's head between them, spaces and all.
    let fox = fact_with_text("The quick brown fox jumps over the lazy dog.");
    let head = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("The quick")),
            kv("parts.1.text", string(" brown fox jump")),
        ];
    });
    assert!(matches!(prove(&fox, &head), Proof::Partial(_)));
    // A piece whose edge lost the space a fragment keeps still holds its share of the text.
    let sky = fact_with_text("The sky is clear today. Carry a coat for later.");
    let edged = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("The sky is clear today. ")),
            kv("parts.1.text", string("Carry a coat for later.")),
        ];
    });
    assert!(matches!(prove(&sky, &edged), Proof::Partial(_)));
    // A member exported encoded is still that member, beside its plain siblings.
    let encoded = span(|s| {
        s.attributes = vec![
            kv(
                "parts.0.text",
                string(&base64::engine::general_purpose::STANDARD.encode("Weigh the ")),
            ),
            kv("parts.1.text", string("alternatives.")),
        ];
    });
    assert!(matches!(prove(&short, &encoded), Proof::Partial(_)));
    // And one in the middle of its siblings: its encoded form is an alternative, not a piece.
    let read = fact_with_text("Read this text exactly.");
    let middle = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Read ")),
            kv(
                "parts.1.text",
                string(&base64::engine::general_purpose::STANDARD.encode("this text ")),
            ),
            kv("parts.2.text", string("exactly.")),
        ];
    });
    assert!(matches!(prove(&read, &middle), Proof::Partial(_)));
    // A member exported as a data URL is still that member.
    let uri = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Read ")),
            kv("parts.1.text", string("data:text/plain,this%20text%20")),
            kv("parts.2.text", string("exactly.")),
        ];
    });
    assert!(matches!(prove(&read, &uri), Proof::Partial(_)));
    // Where both readings of a member occur in the text, the one the join needs is the one tried.
    let say = fact_with_text("Say \"foo\": foo now.");
    let quoted = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Say \"foo\": ")),
            kv("parts.1.text", string("\"foo\"")),
            kv("parts.2.text", string(" now.")),
        ];
    });
    assert!(matches!(prove(&say, &quoted), Proof::Partial(_)));
    // And where the text holds a member as written - quotes and all - that reading is tried too.
    let quote = fact_with_text("Use \"x\" now, please.");
    let written = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Use ")),
            kv("parts.1.text", string("\"x\"")),
            kv("parts.2.text", string(" now, please.")),
        ];
    });
    assert!(matches!(prove(&quote, &written), Proof::Partial(_)));
    // Each member's reading is its own: the join may need one member as written and the next as decoded.
    let mixed = fact_with_text("Use \"x\"; say x.");
    let both = span(|s| {
        s.attributes = vec![
            kv("parts.0.text", string("Use ")),
            kv("parts.1.text", string("\"x\"")),
            kv("parts.2.text", string("; say ")),
            kv("parts.3.text", string("\"x\"")),
            kv("parts.4.text", string(".")),
        ];
    });
    assert!(matches!(prove(&mixed, &both), Proof::Partial(_)));
    // Pieces each encoded on their own are pieces however short: none is long enough to be read as base64 alone.
    let short_encoded = span(|s| {
        s.attributes = "Read this text exactly."
            .chars()
            .collect::<Vec<_>>()
            .chunks(4)
            .enumerate()
            .map(|(i, chunk)| {
                kv(
                    &format!("parts.{i}.text"),
                    string(
                        &base64::engine::general_purpose::STANDARD
                            .encode(chunk.iter().collect::<String>()),
                    ),
                )
            })
            .collect();
    });
    assert!(matches!(prove(&read, &short_encoded), Proof::Partial(_)));
    // A text streamed one event at a time, each event's body holding its own one-part list, is a family by the
    // events' index.
    let streamed = span(|s| {
        s.events = ["Read ", "this text ", "exactly."]
            .iter()
            .map(|chunk| Event {
                name: "chunk".to_string(),
                attributes: vec![kv(
                    "output",
                    string(&serde_json::json!({"parts": [{"text": chunk}]}).to_string()),
                )],
                ..Default::default()
            })
            .collect();
    });
    assert!(matches!(prove(&read, &streamed), Proof::Partial(_)));
    // Pieces streamed several parts an event join across the events as well as within each.
    let grid = span(|s| {
        s.events = [["Read ", "this "], ["text ", "exactly."]]
            .iter()
            .map(|chunks| Event {
                name: "chunk".to_string(),
                attributes: vec![kv(
                    "output",
                    string(
                        &serde_json::json!({"parts": chunks.iter().map(|c| serde_json::json!({"text": c})).collect::<Vec<_>>()})
                            .to_string(),
                    ),
                )],
                ..Default::default()
            })
            .collect();
    });
    assert!(matches!(prove(&read, &grid), Proof::Partial(_)));
    // Pieces encoded one character each, unpadded, are still pieces.
    let per_character = span(|s| {
        s.attributes = "Read this text exactly."
            .chars()
            .enumerate()
            .map(|(i, c)| {
                kv(
                    &format!("parts.{i}.text"),
                    string(&base64::engine::general_purpose::STANDARD_NO_PAD.encode(c.to_string())),
                )
            })
            .collect();
    });
    assert!(matches!(prove(&read, &per_character), Proof::Partial(_)));
    // A text exported one log record at a time, every record its span's, is that span's text in pieces.
    let records = {
        let record = |chunk: &str| LogRecord {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            body: Some(string(chunk)),
            ..Default::default()
        };
        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: ["Read ", "this text ", "exactly."].map(record).to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        Haystack::from_requests(&[], &[("logs-001.pb".to_string(), request)])
    };
    assert!(matches!(prove(&read, &records), Proof::Partial(_)));
    // One span's records in several exports are still that span's records.
    let exports: Vec<(String, ExportLogsServiceRequest)> = ["Read ", "this text ", "exactly."]
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let request = ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    scope_logs: vec![ScopeLogs {
                        log_records: vec![LogRecord {
                            trace_id: vec![1; 16],
                            span_id: vec![2; 8],
                            body: Some(string(chunk)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            };
            (format!("logs-00{}.pb", i + 1), request)
        })
        .collect();
    let exported = Haystack::from_requests(&[], &exports);
    assert!(matches!(prove(&read, &exported), Proof::Partial(_)));
    // One choice's parts across events, beside another choice's: the family holds the choice and opens the rest.
    let choices = span(|s| {
        s.events = [
            serde_json::json!({"choices": [
                {"parts": [{"text": "Read "}, {"text": "this "}]},
                {"parts": [{"text": "OTHER"}]}
            ]}),
            serde_json::json!({"choices": [{"parts": [{"text": "text "}, {"text": "exactly."}]}]}),
        ]
        .iter()
        .map(|body| Event {
            name: "chunk".to_string(),
            attributes: vec![kv("output", string(&body.to_string()))],
            ..Default::default()
        })
        .collect();
    });
    assert!(matches!(prove(&read, &choices), Proof::Partial(_)));
    // A short member still encoded where the search stops is not vouched for: deeper than the search reads, the
    // proof is unprovable rather than absent.
    let nested = |layers: usize| {
        span(|s| {
            s.attributes = "Read this text exactly."
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let mut piece = c.to_string();
                    for _ in 0..layers {
                        piece = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&piece);
                    }
                    kv(&format!("parts.{i}.text"), string(&piece))
                })
                .collect();
        })
    };
    assert!(matches!(prove(&read, &nested(8)), Proof::Partial(_)));
    assert!(matches!(prove(&read, &nested(9)), Proof::Unprovable(_)));
    // An index just before a decoded list still makes its strings pieces: `output.0|json[1]`.
    let lists = span(|s| {
        s.attributes = vec![
            kv(
                "output.0",
                string(&serde_json::json!(["Read ", "this "]).to_string()),
            ),
            kv(
                "output.1",
                string(&serde_json::json!(["text ", "exactly."]).to_string()),
            ),
        ];
    });
    assert!(matches!(prove(&read, &lists), Proof::Partial(_)));
    // However deeply the pieces are nested, one decoding layer down.
    let deep = span(|s| {
        let nested = |chunks: [&str; 2]| {
            let mut value = serde_json::json!({"parts": chunks.iter().map(|c| serde_json::json!({"text": c})).collect::<Vec<_>>()});
            for _ in 0..7 {
                value = serde_json::json!([value]);
            }
            value.to_string()
        };
        s.events = [nested(["Read ", "this "]), nested(["text ", "exactly."])]
            .into_iter()
            .map(|body| Event {
                name: "chunk".to_string(),
                attributes: vec![kv("output", string(&body))],
                ..Default::default()
            })
            .collect();
    });
    assert!(matches!(prove(&read, &deep), Proof::Partial(_)));
    // Records captured out of their timestamps' order are still the text's pieces: no order decides.
    let shuffled = {
        let record = |(chunk, time): (&str, u64)| LogRecord {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            time_unix_nano: time,
            body: Some(string(chunk)),
            ..Default::default()
        };
        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: [("text exactly.", 300), ("this ", 200), ("Read ", 100)]
                        .map(record)
                        .to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        Haystack::from_requests(&[], &[("logs-001.pb".to_string(), request)])
    };
    assert!(matches!(prove(&read, &shuffled), Proof::Partial(_)));
    // A body nested deeper than the decoder reads is unprovable, never absent.
    let deepest = span(|s| {
        let mut value = serde_json::json!(["Read ", "this text ", "exactly."]);
        for _ in 0..128 {
            value = serde_json::json!([value]);
        }
        s.attributes = vec![kv("output", string(&value.to_string()))];
    });
    assert!(matches!(prove(&read, &deepest), Proof::Unprovable(_)));
    // A text in a wider script, each of its bytes encoded as a piece of its own: no piece is text alone, but the
    // pieces joined are.
    let script = fact_with_text("甲乙丙丁戊己庚辛壬癸子丑寅卯辰");
    let byte_wise = span(|s| {
        s.attributes = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰"
            .bytes()
            .enumerate()
            .map(|(i, byte)| {
                kv(
                    &format!("parts.{i}.text"),
                    string(&base64::engine::general_purpose::STANDARD.encode([byte])),
                )
            })
            .collect();
    });
    assert!(matches!(prove(&script, &byte_wise), Proof::Partial(_)));
    // Its characters joined by a wider space, the bytes cut mid-character - the space too - and each piece encoded.
    let spaced = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰"
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join("\u{a0}");
    let bytes = spaced.as_bytes();
    let mut cuts = vec![0, 1];
    while cuts.last().is_some_and(|&cut| cut < bytes.len()) {
        cuts.push((cuts[cuts.len() - 1] + 5).min(bytes.len()));
    }
    let nbsp = span(|s| {
        s.attributes = cuts
            .windows(2)
            .enumerate()
            .map(|(i, cut)| {
                kv(
                    &format!("parts.{i}.text"),
                    string(
                        &base64::engine::general_purpose::STANDARD.encode(&bytes[cut[0]..cut[1]]),
                    ),
                )
            })
            .collect();
    });
    assert!(matches!(
        prove(&fact_with_text(&spaced), &nbsp),
        Proof::Partial(_)
    ));
    // A family inside a decoded body keeps its child path.
    let body = span(|s| {
        s.attributes = vec![kv(
            "output",
            string(
                &serde_json::json!({"parts": [{"text": "Read "}, {"text": "this text "}, {"text": "exactly."}]})
                    .to_string(),
            ),
        )];
    });
    assert!(matches!(prove(&read, &body), Proof::Partial(_)));
    // Coverage is counted in characters: half of a short text in a wider script is not all of it.
    let wide = fact_with_text("甲乙丙丁戊己庚辛壬癸子丑寅卯辰");
    let half = span(|s| {
        s.attributes = vec![kv("a", string("甲乙丙丁")), kv("b", string("戊己庚辛"))];
    });
    assert_eq!(prove(&wide, &half), Proof::Absent);
}

/// A flattened family marks its text as reasoning by an attribute's own name or by a sibling flag, as a JSON part
/// does by its members: the text is then not reasoning exported as ordinary text.
#[test]
fn a_flattened_part_marks_its_text_as_reasoning() {
    const THOUGHT: &str = "The two slowest should cross together so the 10 absorbs the 5.";
    let fact = fact("reasoning", serde_json::json!({ "text": THOUGHT }));
    let kind = |haystack: &Haystack| prove_claim(&Claim::Kind(&fact), haystack);
    let flag = |value: bool| AnyValue {
        value: Some(any_value::Value::BoolValue(value)),
    };
    let flagged = span(|s| {
        s.attributes = vec![
            kv("gen_ai.output.parts.0.text", string(THOUGHT)),
            kv("gen_ai.output.parts.0.thought", flag(true)),
        ];
    });
    assert!(present(kind(&flagged)));
    let unflagged = span(|s| {
        s.attributes = vec![
            kv("gen_ai.output.parts.0.text", string(THOUGHT)),
            kv("gen_ai.output.parts.0.thought", flag(false)),
        ];
    });
    assert_eq!(kind(&unflagged), Proof::Absent);
    let named = span(|s| {
        s.attributes = vec![kv("gen_ai.response.reasoning_content", string(THOUGHT))];
    });
    assert!(present(kind(&named)));
    let plain = span(|s| {
        s.attributes = vec![kv("gen_ai.output.parts.0.text", string(THOUGHT))];
    });
    assert_eq!(kind(&plain), Proof::Absent);
    // A reasoning name anywhere in the key, not only at its end.
    let deeper = span(|s| {
        s.attributes = vec![kv(
            "gen_ai.response.reasoning_content.0.text",
            string(THOUGHT),
        )];
    });
    assert!(present(kind(&deeper)));
    // A sibling type exported encoded types the part as much as one written plainly.
    let encoded_type = span(|s| {
        s.attributes = vec![
            kv("gen_ai.output.parts.0.text", string(THOUGHT)),
            kv(
                "gen_ai.output.parts.0.type",
                string(&base64::engine::general_purpose::STANDARD.encode("thinking")),
            ),
        ];
    });
    assert!(present(kind(&encoded_type)));
    // A flag inside a decoded body marks its sibling text, however that text is encoded.
    let body_flag = span(|s| {
        s.attributes = vec![kv(
            "output",
            string(
                &serde_json::json!({
                    "thought": true,
                    "text": base64::engine::general_purpose::STANDARD.encode(THOUGHT),
                })
                .to_string(),
            ),
        )];
    });
    assert!(present(kind(&body_flag)));
    // A flag on a body enclosing the part the encoded text sits in marks it as the plain text's would.
    let enclosing = span(|s| {
        s.attributes = vec![kv(
            "output",
            string(
                &serde_json::json!({
                    "thought": true,
                    "content": [{"text": base64::engine::general_purpose::STANDARD.encode(THOUGHT)}],
                })
                .to_string(),
            ),
        )];
    });
    assert!(present(kind(&enclosing)));
    // Another part's flag says nothing of this part: `parts.10` is not `parts.1`.
    let other_part = span(|s| {
        s.attributes = vec![
            kv("gen_ai.output.parts.1.text", string(THOUGHT)),
            kv("gen_ai.output.parts.10.thought", flag(true)),
        ];
    });
    assert_eq!(kind(&other_part), Proof::Absent);
    // A span named for the scenario does not name its attributes' text.
    let named_span = span(|s| {
        s.name = "reasoning".to_string();
        s.attributes = vec![kv("output", string(THOUGHT))];
    });
    assert_eq!(kind(&named_span), Proof::Absent);
}

/// A text whose remainder after a cut is exported in pieces too short to find alone was not cut: it is all carried.
#[test]
fn a_remainder_exported_in_pieces_is_no_truncation() {
    let chars: Vec<char> = TEXT.chars().collect();
    let pieces = span(|s| {
        s.attributes = std::iter::once(kv(
            "parts.0.text",
            string(&chars[..27].iter().collect::<String>()),
        ))
        .chain(
            chars[27..]
                .iter()
                .enumerate()
                .map(|(i, c)| kv(&format!("parts.{}.text", i + 1), string(&c.to_string()))),
        )
        .collect();
    });
    assert_eq!(super::truncated_at(TEXT, &pieces), None);
}

/// Withheld reasoning is refused where a flattened part is flagged as reasoning, as where a JSON part is.
#[test]
fn a_flattened_thought_flag_is_a_withheld_reasoning_part() {
    let flag = AnyValue {
        value: Some(any_value::Value::BoolValue(true)),
    };
    let flagged = span(|s| {
        s.attributes = vec![
            kv("gen_ai.output.parts.0.thought", flag),
            kv("gen_ai.output.parts.0.text", string("")),
        ];
    });
    assert!(present(prove(&withheld_fact(), &flagged)));
}
