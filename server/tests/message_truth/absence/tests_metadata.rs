// Absence proofs over usage counts a producer states wrongly.

/// A usage gap is accepted only where no payload carries the wire's count, in any form a count takes: an
/// integer attribute, an integer written as text, an integer inside a decoded body. Any payload carrying it
/// refuses the gap - reading it is a parser fix, not a limitation.
#[test]
fn a_usage_count_is_absent_only_where_no_payload_carries_it() {
    let input = Claim::Metadata("usage.input", vec!["515".to_string()]);
    let int = |n: i64| AnyValue {
        value: Some(any_value::Value::IntValue(n)),
    };
    // The producer's own doubled count is not the wire's.
    let doubled = span(|s| s.attributes = vec![kv("gen_ai.usage.input_tokens", int(1030))]);
    assert_eq!(prove_claim(&input, &doubled), Proof::Absent);
    // The wire's count anywhere refuses the gap.
    let beside = span(|s| {
        s.attributes = vec![
            kv("gen_ai.usage.input_tokens", int(1030)),
            kv("llm.usage.prompt_tokens", int(515)),
        ]
    });
    assert!(present(prove_claim(&input, &beside)));
    let as_text = attribute(string("515"));
    assert!(present(prove_claim(&input, &as_text)));
    let in_body = attribute(string(
        &serde_json::json!({"usage": {"input_tokens": 515, "output_tokens": 71}}).to_string(),
    ));
    assert!(present(prove_claim(&input, &in_body)));
    let in_log = log(|r| r.attributes.push(kv("input_tokens", int(515))));
    assert!(present(prove_claim(&input, &in_log)));
    // A count only quoted inside prose is no count, and a larger number holding its digits is another.
    let prose = attribute(string("515 tokens were sent"));
    assert_eq!(prove_claim(&input, &prose), Proof::Absent);
    let longer = span(|s| s.attributes = vec![kv("n", int(5150))]);
    assert_eq!(prove_claim(&input, &longer), Proof::Absent);
}

/// A count beyond what a float holds exactly is still found, as an integer and as integer text, in a decoded body:
/// the search reads integers exactly, so a large wire count in a payload refuses the gap.
#[test]
fn a_large_usage_count_is_found_exactly() {
    const LARGE: i64 = 9_000_000_000_000_001;
    let claim = Claim::Metadata("usage.output", vec![LARGE.to_string()]);
    let as_integer = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {LARGE}}}}}"
    )));
    assert!(present(prove_claim(&claim, &as_integer)));
    let as_text = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": \"{LARGE}\"}}}}"
    )));
    assert!(present(prove_claim(&claim, &as_text)));
    // Integral floats are read too: a small one as its count, a large one as a figure that may be it - never
    // absent.
    let small = Claim::Metadata("usage.output", vec!["515".to_string()]);
    let as_small_float = attribute(string("{\"usage\": {\"output_tokens\": 515.0}}"));
    assert!(present(prove_claim(&small, &as_small_float)));
    let as_float = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {LARGE}.0}}}}"
    )));
    assert_ne!(prove_claim(&claim, &as_float), Proof::Absent);
    // Past 2^53 an integer is read exactly, and its neighbour - what a float conversion rounds it to - is not it.
    const PAST: i64 = 9_007_199_254_740_993;
    let past = Claim::Metadata("usage.output", vec![PAST.to_string()]);
    let exact = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {PAST}}}}}"
    )));
    assert!(present(prove_claim(&past, &exact)));
    let neighbour = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {}}}}}",
        PAST - 1
    )));
    assert_eq!(prove_claim(&past, &neighbour), Proof::Absent);
    let quoted = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": \"{PAST}\"}}}}"
    )));
    assert!(present(prove_claim(&past, &quoted)));
    let quoted_neighbour = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": \"{}\"}}}}",
        PAST - 1
    )));
    assert_eq!(prove_claim(&past, &quoted_neighbour), Proof::Absent);
    // Between 2^51 and 2^52 a float's last unit is a half, so the decoder may read it as either neighbour: the
    // count it states is unknown, never absent.
    const HALF: i64 = 4_503_599_627_370_494;
    let half = Claim::Metadata("usage.output", vec![HALF.to_string()]);
    let half_float = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {HALF}.0}}}}"
    )));
    assert_ne!(prove_claim(&half, &half_float), Proof::Absent);
    // A float that large states no integer exactly: whether it is the count is unknown, never absent.
    let rounded = attribute(string(&format!(
        "{{\"usage\": {{\"output_tokens\": {PAST}.0}}}}"
    )));
    assert!(matches!(prove_claim(&past, &rounded), Proof::Unprovable(_)));
}
