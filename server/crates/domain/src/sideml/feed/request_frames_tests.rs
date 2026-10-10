use super::*;
use crate::observations::{MessageSource, RawMessage};

fn at(second: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + second, 0).expect("an instant")
}

fn system(text: &str) -> serde_json::Value {
    serde_json::json!({"role": "system", "content": text})
}

/// A stored message read from an attribute of the request span.
fn attribute(key: &str, content: serde_json::Value) -> RawMessage {
    RawMessage {
        source: MessageSource::Attribute {
            key: key.to_string(),
            time: at(0),
        },
        content,
        rendering: false,
        direction: None,
    }
}

/// A stored message read from an event, as a log record's messages are.
fn event(name: &str, content: serde_json::Value) -> RawMessage {
    RawMessage {
        source: MessageSource::Event {
            name: name.to_string(),
            time: at(-5),
        },
        content,
        rendering: false,
        direction: None,
    }
}

fn request_view(messages: Vec<RawMessage>) -> FeedResult {
    let mut row = frame_row(&RequestFrameRecord {
        trace_id: "trace".to_string(),
        span_id: "request".to_string(),
        timestamp: at(0),
        log_digest: String::new(),
        ordinal: 0,
        messages_json: None,
    });
    row.messages_json = serde_json::to_string(&messages).expect("messages serialise");
    row.observation_type = Some("generation".to_string());
    process_span(vec![row], &FeedOptions::new())
}

fn record(span: &str, digest: &str, messages: Vec<RawMessage>) -> RequestFrameRecord {
    RequestFrameRecord {
        trace_id: "trace".to_string(),
        span_id: span.to_string(),
        timestamp: at(-5),
        log_digest: digest.to_string(),
        ordinal: 0,
        messages_json: Some(serde_json::to_string(&messages).expect("messages serialise")),
    }
}

fn texts(view: &FeedResult) -> Vec<(String, String)> {
    view.messages
        .iter()
        .map(|block| {
            let text = match &block.content {
                crate::sideml::types::ContentBlock::Text { text, .. } => text.clone(),
                other => format!("{other:?}"),
            };
            (text, block.span_id.clone())
        })
        .collect()
}

/// The carriers this test frames with: the embedded corpus's detached-frame carriers, one an attribute of the
/// request span, the other an event a log record carries.
const PREVIEW: &str = "system_prompt_preview";
const LOGGED: &str = "system_prompt";

/// **A request that carries the first sections of its frame, cut short, is shown the whole frame once and in
/// order.** The request's preview holds the header and the preamble and lost the instruction to the cut; the
/// record joined to it holds all three. The view opens with the three, the two shared ones as the request's own
/// copies, the instruction from the record - and then the rest of what the request carried.
#[test]
fn a_cut_preview_and_its_whole_frame_merge_in_order_once() {
    let view = request_view(vec![
        attribute(PREVIEW, system("header")),
        attribute(PREVIEW, system("preamble")),
        attribute(
            "new_context",
            serde_json::json!({"role": "user", "content": "question"}),
        ),
    ]);
    let framed = frame(
        view,
        vec![record(
            "agent",
            "d1",
            vec![
                event(LOGGED, system("header")),
                event(LOGGED, system("preamble")),
                event(LOGGED, system("instruction")),
            ],
        )],
    );
    let pair = |text: &str, span: &str| (text.to_string(), span.to_string());
    assert_eq!(
        texts(&framed),
        [
            pair("header", "request"),
            pair("preamble", "request"),
            pair("instruction", "agent"),
            pair("question", "request"),
        ]
    );
    assert_eq!(framed.metadata.framed_by_records, 1);
    assert!(!framed.metadata.frames_truncated);
    assert_eq!(framed.metadata.block_count, 4);
    assert_eq!(
        framed.metadata.span_count, 2,
        "the request and the span its frame was recorded on"
    );
}

/// The merge keeps both sides' order and multiplicity: what only the request holds stays in its place, ahead of
/// what only the frame holds between the same shared blocks; a block twice on both sides is twice; and a request
/// with nothing joined, or a frame with nothing in common, keeps every block.
#[test]
fn the_merge_keeps_both_orders_and_their_multiplicity() {
    let merge = |own: &[&str], joined: &[&str]| {
        let view = request_view(
            own.iter()
                .map(|text| attribute(PREVIEW, system(text)))
                .collect(),
        );
        let records = match joined.is_empty() {
            true => Vec::new(),
            false => vec![record(
                "agent",
                "d1",
                joined
                    .iter()
                    .map(|text| event(LOGGED, system(text)))
                    .collect(),
            )],
        };
        texts(&frame(view, records))
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        merge(&["mine", "a", "b"], &["a", "b", "c"]),
        ["mine", "a", "b", "c"]
    );
    assert_eq!(merge(&["a", "a"], &["a", "a", "c"]), ["a", "a", "c"]);
    assert_eq!(
        merge(&["a", "x", "b"], &["a", "y", "b"]),
        ["a", "x", "y", "b"]
    );
    assert_eq!(merge(&["x"], &["a"]), ["x", "a"]);
    assert_eq!(merge(&["a", "b"], &[]), ["a", "b"]);
    assert_eq!(merge(&[], &["a", "b"]), ["a", "b"]);
}

/// The texts of a request's own frame and of one joined record, merged.
fn merge_texts(own: &[String], joined: &[String]) -> FeedResult {
    let view = request_view(
        own.iter()
            .map(|text| attribute(PREVIEW, system(text)))
            .collect(),
    );
    frame(
        view,
        vec![record(
            "agent",
            "d1",
            joined
                .iter()
                .map(|text| event(LOGGED, system(text)))
                .collect(),
        )],
    )
}

/// Whether `part` is a subsequence of `whole`.
fn in_order(part: &[String], whole: &[String]) -> bool {
    let mut rest = whole.iter();
    part.iter().all(|wanted| rest.any(|text| text == wanted))
}

/// **Past the merge's table bound both orders and every block survive, and the view says the merge was not
/// exact.** Two frames of over a thousand sections each, sharing a section and nothing at either end, would
/// tabulate more cells than the bound allows. The shared blocks are matched in order on both sides - so a frame
/// that holds two in the order the request holds them the other way round keeps both orders - and a block the
/// frame holds twice and the request once survives once more.
#[test]
fn past_the_table_bound_the_merge_keeps_both_orders_and_says_it_is_inexact() {
    let named = |prefix: &str, range: std::ops::Range<usize>| -> Vec<String> {
        range.map(|n| format!("{prefix} {n}")).collect()
    };
    let own: Vec<String> = named("own", 0..550)
        .into_iter()
        .chain(["shared".to_string()])
        .chain(named("own", 550..1100))
        .collect();
    let joined: Vec<String> = named("frame", 0..500)
        .into_iter()
        .chain(["shared".to_string(), "shared".to_string()])
        .chain(named("frame", 500..1000))
        .collect();
    assert!(
        (own.len() + 1) * (joined.len() + 1)
            > sideseat_core::constants::REQUEST_FRAMES_MERGE_MAX_CELLS,
        "the case must exceed the bound"
    );
    let framed = merge_texts(&own, &joined);
    let pair = |text: &String, span: &str| (text.clone(), span.to_string());
    let expected: Vec<(String, String)> = own[..550]
        .iter()
        .map(|text| pair(text, "request"))
        .chain(joined[..500].iter().map(|text| pair(text, "agent")))
        .chain(own[550..].iter().map(|text| pair(text, "request")))
        .chain(joined[501..].iter().map(|text| pair(text, "agent")))
        .collect();
    assert_eq!(texts(&framed), expected);
    assert!(framed.metadata.frames_truncated, "an inexact merge says so");

    // Two shared blocks the frame holds in the other order: both orders survive.
    let own: Vec<String> = ["a".to_string(), "b".to_string()]
        .into_iter()
        .chain(named("own", 0..1100))
        .collect();
    let joined: Vec<String> = ["b".to_string(), "a".to_string()]
        .into_iter()
        .chain(named("frame", 0..1000))
        .collect();
    let merged: Vec<String> = texts(&merge_texts(&own, &joined))
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert!(in_order(&own, &merged), "the request's order");
    assert!(in_order(&joined, &merged), "the frame's order");
    assert_eq!(
        merged.len(),
        2 + 1100 + 1 + 1000,
        "one shared block kept once"
    );
}

/// **The bound is the table's real size.** Two disjoint frames of 1,024 sections each have a product of exactly the
/// bound, and a table of one more row and column than that: past the bound, so the merge says it was not exact.
#[test]
fn the_table_bound_counts_the_table_s_own_cells() {
    let side = 1024;
    assert_eq!(
        side * side,
        sideseat_core::constants::REQUEST_FRAMES_MERGE_MAX_CELLS
    );
    let named =
        |prefix: &str| -> Vec<String> { (0..side).map(|n| format!("{prefix} {n}")).collect() };
    let framed = merge_texts(&named("own"), &named("frame"));
    assert_eq!(framed.messages.len(), 2 * side);
    assert!(framed.metadata.frames_truncated);
}

/// **Within the bound the merge is exact, and exactly equal blocks are the same block.** Blocks that differ by
/// whitespace alone are two blocks - what a frame said and what a request said are compared byte for byte - and
/// a merge within the table's bound does not say it was inexact.
#[test]
fn only_exactly_equal_blocks_merge() {
    let strings = |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
    let framed = merge_texts(
        &strings(&["header", "preamble"]),
        &strings(&["header ", "preamble"]),
    );
    assert_eq!(
        texts(&framed)
            .into_iter()
            .map(|(text, span)| format!("{text}|{span}"))
            .collect::<Vec<_>>(),
        ["header|request", "header |agent", "preamble|request"]
    );
    assert!(!framed.metadata.frames_truncated);

    // Two thoughts of one text under different signatures are two thoughts.
    let thought = |signature: &str| {
        serde_json::json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "consider", "signature": signature}
        ]})
    };
    let view = request_view(vec![attribute(PREVIEW, thought("one"))]);
    let framed = frame(
        view,
        vec![record("agent", "d1", vec![event(LOGGED, thought("two"))])],
    );
    assert_eq!(framed.messages.len(), 2, "{:?}", texts(&framed));
}

/// **The merge, within the bound and past it, over thousands of generated pairs.** Sequences of up to 64 blocks
/// a side, over alphabets of one symbol (every block equal, the most ambiguous case), two and four, so repeats and
/// crossings are the rule. Within the bound the merge is a shortest common supersequence - its length checked
/// against a longest common subsequence computed independently here. With the table's bound made small, the
/// greedy fallback runs instead, and is held to what it promises. Both keep the request's blocks, in order, as the
/// request's own; keep the frame's in order; keep every block either side holds; answer the same twice; and say
/// whether they were exact. The seed is fixed and printed with any failure.
#[test]
fn the_merge_keeps_both_orders_and_every_block_over_generated_pairs() {
    fn lcs(a: &[String], b: &[String]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                table[i][j] = if a[i - 1] == b[j - 1] {
                    table[i - 1][j - 1] + 1
                } else {
                    table[i - 1][j].max(table[i][j - 1])
                };
            }
        }
        table[a.len()][b.len()]
    }
    let template = request_view(vec![attribute(PREVIEW, system("template"))]).messages[0].clone();
    let block = |text: &str, span: &str| {
        let mut block = template.clone();
        block.content = serde_json::from_value(serde_json::json!({"type": "text", "text": text}))
            .expect("a text block");
        block.span_id = span.to_string();
        block
    };
    let shown = |blocks: &[BlockEntry]| -> Vec<(String, String)> {
        blocks
            .iter()
            .map(|block| match &block.content {
                crate::sideml::types::ContentBlock::Text { text, .. } => {
                    (text.clone(), block.span_id.clone())
                }
                other => panic!("not text: {other:?}"),
            })
            .collect()
    };
    const SEED: u64 = 0x5eed_f4a3;
    let mut state = SEED;
    let mut next = move |bound: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % bound
    };
    let mut inexact = 0;
    for case in 0..2_400 {
        let symbols = [1u64, 2, 4][case % 3];
        let own: Vec<String> = (0..next(65))
            .map(|_| format!("s{}", next(symbols)))
            .collect();
        let joined: Vec<String> = (0..next(65))
            .map(|_| format!("s{}", next(symbols)))
            .collect();
        let cap = if case % 2 == 0 {
            sideseat_core::constants::REQUEST_FRAMES_MERGE_MAX_CELLS
        } else {
            4
        };
        let why = format!("seed {SEED:#x}, case {case}, cap {cap}: {own:?} + {joined:?}");
        let run = || {
            merged_within(
                own.iter().map(|text| block(text, "request")).collect(),
                joined.iter().map(|text| block(text, "agent")).collect(),
                cap,
            )
        };
        let (merged, exact) = run();
        let out = shown(&merged);
        let (again, again_exact) = run();
        assert_eq!(
            (out.clone(), exact),
            (shown(&again), again_exact),
            "deterministic: {why}"
        );
        let texts: Vec<String> = out.iter().map(|(text, _)| text.clone()).collect();
        let mine: Vec<String> = out
            .iter()
            .filter(|(_, span)| span == "request")
            .map(|(text, _)| text.clone())
            .collect();
        let theirs: Vec<String> = out
            .iter()
            .filter(|(_, span)| span == "agent")
            .map(|(text, _)| text.clone())
            .collect();
        assert_eq!(
            mine, own,
            "the request's blocks, in order, as its own: {why}"
        );
        assert!(
            in_order(&joined, &texts),
            "the frame's order: {why} -> {texts:?}"
        );
        assert!(
            in_order(&theirs, &joined),
            "only the frame's own blocks: {why} -> {texts:?}"
        );
        let shortest = own.len() + joined.len() - lcs(&own, &joined);
        assert!(
            texts.len() >= shortest && texts.len() <= own.len() + joined.len(),
            "every block kept, none invented: {why} -> {texts:?}"
        );
        if exact {
            assert_eq!(
                texts.len(),
                shortest,
                "an exact merge is shortest: {why} -> {texts:?}"
            );
        } else {
            assert_eq!(cap, 4, "only a small table is inexact here: {why}");
            inexact += 1;
        }
        if cap == sideseat_core::constants::REQUEST_FRAMES_MERGE_MAX_CELLS {
            assert!(exact, "these sizes are within the real bound: {why}");
        }
    }
    assert!(
        inexact > 500,
        "the fallback ran in {inexact} cases, seed {SEED:#x}"
    );
}

/// **A frame's block names the record it was read from**, and its message's place in that record - not a place in
/// the list this read joined, which is not where the span's stored list holds it when the span recorded anything
/// before its frame.
#[test]
fn a_frame_block_names_its_record_and_message() {
    let view = request_view(vec![attribute(
        "new_context",
        serde_json::json!({"role": "user", "content": "question"}),
    )]);
    let framed = frame(
        view,
        vec![
            record(
                "a",
                "d1",
                vec![
                    event(LOGGED, system("first")),
                    event(LOGGED, system("second")),
                ],
            ),
            record("a", "d2", vec![event(LOGGED, system("third"))]),
        ],
    );
    let roots: Vec<String> = framed
        .messages
        .iter()
        .filter(|block| block.span_id == "a")
        .map(|block| {
            block
                .position
                .to_string()
                .splitn(3, '.')
                .take(2)
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect();
    assert_eq!(roots, ["frame:d1:0.0", "frame:d1:0.1", "frame:d2:0.0"]);
    // Numbered below zero, in order, so they repeat no index anything else of the span shows in this view.
    let indices: Vec<i32> = framed
        .messages
        .iter()
        .filter(|block| block.span_id == "a")
        .map(|block| block.message_index)
        .collect();
    assert_eq!(indices, [-3, -2, -1]);
    assert!(
        framed
            .messages
            .iter()
            .filter(|block| block.span_id != "a")
            .all(|block| block.message_index >= 0),
        "the request's own keep theirs"
    );
}

/// A block of the request view's shape with `content`, on `span`, at `position`.
fn synthetic(content: serde_json::Value, span: &str) -> BlockEntry {
    let mut block = request_view(vec![attribute(PREVIEW, system("template"))]).messages[0].clone();
    block.content = serde_json::from_value(content).expect("a content block");
    block.span_id = span.to_string();
    block
}

/// **Equal content is the same block whatever order its members were written in**, and different content is not.
#[test]
fn equal_content_merges_whatever_its_member_order() {
    let json = |data: serde_json::Value| serde_json::json!({"type": "json", "data": data});
    let (merged, exact) = merged_within(
        vec![synthetic(
            json(serde_json::json!({"a": 1, "b": {"c": 2, "d": [3, {"e": 4, "f": 5}]}})),
            "request",
        )],
        vec![
            synthetic(
                json(serde_json::json!({"b": {"d": [3, {"f": 5, "e": 4}], "c": 2}, "a": 1})),
                "agent",
            ),
            synthetic(json(serde_json::json!({"a": 1, "b": 2})), "agent"),
        ],
        sideseat_core::constants::REQUEST_FRAMES_MERGE_MAX_CELLS,
    );
    assert!(exact);
    assert_eq!(
        merged
            .iter()
            .map(|block| block.span_id.as_str())
            .collect::<Vec<_>>(),
        ["request", "agent"],
        "the reordered copy is the request's own; the different one is the frame's"
    );
}

/// **One observation read as several messages keeps them several.** Two messages of one frame record's observation
/// are numbered apart, so a client that groups a message's blocks by span and index does not fold them into one.
#[test]
fn messages_of_one_observation_are_numbered_apart() {
    use crate::sideml::provenance::PathSegment;
    let at = |message_index: i32, entry: usize| {
        let mut block = synthetic(serde_json::json!({"type": "text", "text": "part"}), "a");
        block.position = crate::sideml::provenance::PositionPath::root(0)
            .child_index(entry)
            .with_root([
                PathSegment::Key("frame:d1:0".to_string()),
                PathSegment::Index(0),
            ]);
        block.message_index = message_index;
        block
    };
    let mut blocks = vec![at(0, 0), at(0, 1), at(1, 0)];
    numbered_below_zero(&mut blocks);
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.message_index)
            .collect::<Vec<_>>(),
        [-2, -2, -1],
        "the first message's two blocks together, the second message apart"
    );
}

/// **Records of several spans keep their order.** Two records of one span with another span's record between
/// them read as the three records were written, while the two of one span are still distinct occurrences.
#[test]
fn frame_records_of_interleaved_spans_keep_their_order() {
    let view = request_view(vec![attribute(
        "new_context",
        serde_json::json!({"role": "user", "content": "question"}),
    )]);
    let framed = frame(
        view,
        vec![
            record("a", "d1", vec![event(LOGGED, system("first"))]),
            record("b", "d2", vec![event(LOGGED, system("second"))]),
            record("a", "d3", vec![event(LOGGED, system("third"))]),
        ],
    );
    let pair = |text: &str, span: &str| (text.to_string(), span.to_string());
    assert_eq!(
        texts(&framed),
        [
            pair("first", "a"),
            pair("second", "b"),
            pair("third", "a"),
            pair("question", "request"),
        ]
    );
}

/// Frame records are joined in the order read, up to the record bound and then the byte bound, and a cut by
/// either says so.
#[test]
fn frame_records_are_joined_within_their_bounds_and_a_cut_says_so() {
    let records: Vec<RequestFrameRecord> = (0..=REQUEST_FRAMES_MAX_RECORDS)
        .map(|index| {
            record(
                "agent",
                &format!("d{index:02}"),
                vec![event(LOGGED, system(&format!("frame {index}")))],
            )
        })
        .collect();
    let framed = frame(request_view(Vec::new()), records.clone());
    assert_eq!(
        framed.metadata.framed_by_records,
        REQUEST_FRAMES_MAX_RECORDS
    );
    assert!(
        framed.metadata.frames_truncated,
        "one past the bound was read, so the view was cut"
    );
    assert_eq!(
        texts(&framed)[0].0,
        "frame 0",
        "the first records in record order are kept"
    );

    let exactly = frame(
        request_view(Vec::new()),
        records[..REQUEST_FRAMES_MAX_RECORDS].to_vec(),
    );
    assert!(
        !exactly.metadata.frames_truncated,
        "exactly as many as the bound is no cut"
    );

    let mut large = record("agent", "big", vec![event(LOGGED, system(&"x".repeat(64)))]);
    large.messages_json = Some(format!(
        "[{}]",
        std::iter::repeat_n(
            serde_json::to_string(&event(LOGGED, system("piece"))).expect("serialises"),
            REQUEST_FRAMES_MAX_BYTES / 64
        )
        .collect::<Vec<_>>()
        .join(",")
    ));
    let over = frame(
        request_view(Vec::new()),
        vec![
            record("agent", "a", vec![event(LOGGED, system("small"))]),
            large,
        ],
    );
    assert_eq!(
        over.metadata.framed_by_records, 1,
        "the record past the byte bound is left out"
    );
    assert!(over.metadata.frames_truncated);

    // And a record the statement returned without its bytes, because the ones before it reached the bound.
    let mut sentinel = record("agent", "z", Vec::new());
    sentinel.messages_json = None;
    let cut = frame(
        request_view(Vec::new()),
        vec![
            record("agent", "a", vec![event(LOGGED, system("small"))]),
            sentinel,
        ],
    );
    assert_eq!(cut.metadata.framed_by_records, 1);
    assert!(
        cut.metadata.frames_truncated,
        "a record returned without its bytes is a cut"
    );
}

/// Two records of one span are two occurrences of it: their blocks follow on from each other in record order,
/// each at a position and index of its own, as the span's own log join places them.
#[test]
fn two_records_of_one_span_are_distinct_occurrences() {
    let framed = frame(
        request_view(Vec::new()),
        vec![
            record("agent", "d1", vec![event(LOGGED, system("first"))]),
            record("agent", "d2", vec![event(LOGGED, system("second"))]),
        ],
    );
    assert_eq!(
        texts(&framed)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let identities: std::collections::BTreeSet<(String, i32, String)> = framed
        .messages
        .iter()
        .map(|block| {
            (
                block.span_id.clone(),
                block.message_index,
                block.position.to_string(),
            )
        })
        .collect();
    assert_eq!(
        identities.len(),
        2,
        "two occurrences, not one position twice: {identities:?}"
    );
}

/// Only frame blocks join: a record's message from a carrier that frames nothing is not put at a request's head.
#[test]
fn only_a_frame_carriers_blocks_join() {
    let framed = frame(
        request_view(vec![attribute(PREVIEW, system("header"))]),
        vec![record(
            "agent",
            "d1",
            vec![
                event(LOGGED, system("instruction")),
                event(
                    "user_prompt",
                    serde_json::json!({"role": "user", "content": "not a frame"}),
                ),
            ],
        )],
    );
    let joined: Vec<String> = texts(&framed).into_iter().map(|(text, _)| text).collect();
    assert_eq!(joined, ["header", "instruction"]);
}
