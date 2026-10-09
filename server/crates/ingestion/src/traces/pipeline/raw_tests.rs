use super::*;
use base64::Engine;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

fn picture(seed: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        (0..900)
            .map(|i| (i as u8).wrapping_add(seed))
            .collect::<Vec<_>>(),
    )
}

fn hash_of(text: &str) -> String {
    hex::encode(blake3::hash(text.as_bytes()).as_bytes())
}

fn span(trace: u8, span: u8, text: Option<String>) -> Span {
    Span {
        trace_id: vec![trace; 16],
        span_id: vec![span; 8],
        name: "s".into(),
        start_time_unix_nano: 1_000 * u64::from(span),
        attributes: text
            .map(|text| {
                vec![KeyValue {
                    key: "input.value".into(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(text)),
                    }),
                }]
            })
            .unwrap_or_default(),
        ..Default::default()
    }
}

fn export(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource::default()),
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn key(trace: u8, span: u8) -> SpanKey {
    (hex::encode([trace; 16]), hex::encode([span; 8]))
}

fn decoded(row: &RawRecordRow, media: &[PendingFileWrite]) -> Vec<u8> {
    let objects: BTreeMap<[u8; 32], Vec<u8>> = media
        .iter()
        .map(|write| {
            let hash: [u8; 32] = hex::decode(&write.hash).unwrap().try_into().unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&write.data)
                .unwrap();
            (hash, bytes)
        })
        .collect();
    raw_payload::decode(&row.record, |hash| {
        objects
            .get(hash)
            .map(|bytes| std::borrow::Cow::Owned(bytes.clone()))
    })
    .unwrap()
    .1
}

/// Each object is owned by exactly the traces whose spans carry it, and the stored bytes are the
/// object's decoded content under the file store's address.
#[test]
fn media_is_owned_by_the_traces_that_carry_it() {
    let request = export(vec![
        span(1, 1, Some(format!("look: {}", picture(1)))),
        span(2, 2, Some(picture(2))),
        span(2, 3, Some(picture(1))),
    ]);
    let received = ReceivedPayload::new(
        request.encode_to_vec(),
        RawContent::Protobuf,
        chrono::Utc::now(),
    );
    let draft = RawDraft::new("project", &received, true);
    let writes = draft.media_writes(&request);
    let owners: BTreeSet<(String, String)> = writes
        .iter()
        .map(|w| (w.hash.clone(), w.trace_id.clone()))
        .collect();
    assert_eq!(
        owners,
        BTreeSet::from([
            (hash_of(&picture(1)), hex::encode([1u8; 16])),
            (hash_of(&picture(1)), hex::encode([2u8; 16])),
            (hash_of(&picture(2)), hex::encode([2u8; 16])),
        ])
    );
    for write in &writes {
        let expected = if write.hash == hash_of(&picture(2)) {
            picture(2)
        } else {
            picture(1)
        };
        assert_eq!(write.data, expected.into_bytes());
    }
}

/// The record of a whole export is the received body; one that lost a span to a fence is the *received*
/// export without it - not the request the pipeline mutated - so the raw store never keeps what a deletion
/// removed, and never keeps what the pipeline added.
#[test]
fn a_fenced_span_does_not_survive_in_the_raw_record() {
    let original = export(vec![span(1, 1, None), span(2, 2, None)]);
    let received = ReceivedPayload::new(
        original.encode_to_vec(),
        RawContent::Protobuf,
        chrono::Utc::now(),
    );
    let draft = RawDraft::new("project", &received, true);
    let mut mutated = original.clone();
    mutated.resource_spans[0]
        .resource
        .as_mut()
        .unwrap()
        .attributes
        .push(KeyValue {
            key: "sideseat.project_id".into(),
            value: None,
        });
    let now = DateTime::UNIX_EPOCH;

    let whole = draft
        .row(&mutated, &HashSet::from([key(1, 1), key(2, 2)]), now, None)
        .unwrap();
    assert_eq!(whole.origin, RawOrigin::Received);
    assert_eq!(decoded(&whole, &[]), received.bytes);
    assert_eq!(
        whole.trace_ids,
        vec![hex::encode([1u8; 16]), hex::encode([2u8; 16])]
    );

    let part = draft
        .row(&mutated, &HashSet::from([key(1, 1)]), now, None)
        .unwrap();
    assert_eq!(part.origin, RawOrigin::Fenced);
    assert_eq!(part.raw_id, whole.raw_id);
    assert_eq!(part.trace_ids, vec![hex::encode([1u8; 16])]);
    let kept = ExportTraceServiceRequest::decode(decoded(&part, &[]).as_slice()).unwrap();
    assert_eq!(kept, export(vec![span(1, 1, None)]));
    assert_eq!(
        part.signal_until,
        DateTime::from_timestamp_nanos(1_000),
        "the latest start of the spans kept"
    );
}

/// Media the file store refused stays in the record, which therefore still decodes to the received body with
/// no file at all for that object.
#[test]
fn media_the_store_refused_stays_inline() {
    let request = export(vec![
        span(1, 1, Some(picture(1))),
        span(1, 2, Some(picture(2))),
    ]);
    let received = ReceivedPayload::new(
        request.encode_to_vec(),
        RawContent::Protobuf,
        chrono::Utc::now(),
    );
    let mut draft = RawDraft::new("project", &received, true);
    let writes = draft.media_writes(&request);
    let refused = format!("#!B64!#image/png::{}", hash_of(&picture(1)));
    draft.keep_inline(&[
        ("project".to_string(), refused.clone()),
        (
            "elsewhere".to_string(),
            format!("#!B64!#::{}", hash_of(&picture(2))),
        ),
    ]);
    let row = draft
        .row(
            &request,
            &HashSet::from([key(1, 1), key(1, 2)]),
            DateTime::UNIX_EPOCH,
            None,
        )
        .unwrap();
    let stored: Vec<PendingFileWrite> = writes
        .into_iter()
        .filter(|write| write.hash != hash_of(&picture(1)))
        .collect();
    assert_eq!(decoded(&row, &stored), received.bytes);
    let referenced: Vec<String> = raw_payload::media_hashes(&row.record)
        .unwrap()
        .iter()
        .map(hex::encode)
        .collect();
    assert_eq!(referenced, vec![hash_of(&picture(2))]);
}

/// A repair holds the latest version's spans and the ingest's own, at least two versions above the latest.
#[test]
fn a_repair_is_the_union_two_versions_up() {
    let request = export(vec![span(1, 1, None), span(1, 2, None), span(1, 3, None)]);
    let received = ReceivedPayload::new(
        request.encode_to_vec(),
        RawContent::Protobuf,
        chrono::Utc::now(),
    );
    let draft = RawDraft::new("project", &received, true);
    let at = DateTime::from_timestamp_micros(5).unwrap();
    let mut latest = draft
        .row(&request, &HashSet::from([key(1, 1)]), at, None)
        .unwrap();
    latest.version = 1_000;
    let written = HashSet::from([key(1, 2)]);
    assert!(!draft.covers(&latest, &written));

    // Repaired by an ingest received a month later: still the record received when it was.
    let later = at + chrono::TimeDelta::days(31);
    let repair = draft
        .repair_row(&request, Some(&latest), &written, later, None)
        .unwrap();
    assert_eq!(repair.version, later.timestamp_micros());
    assert_eq!(
        repair.received_at, latest.received_at,
        "a repair keeps the record's receipt, so its versions share one partition and one replay position"
    );
    let repair = draft
        .repair_row(&request, Some(&latest), &written, at, None)
        .unwrap();
    assert_eq!(repair.version, 1_002);
    assert_eq!(repair.origin, RawOrigin::Fenced);
    assert_eq!(
        record_identities(&repair.record).unwrap(),
        HashSet::from([key(1, 1), key(1, 2)])
    );
    assert!(draft.covers(&repair, &written));

    let whole = draft
        .repair_row(
            &request,
            Some(&repair),
            &HashSet::from([key(1, 3)]),
            at,
            None,
        )
        .unwrap();
    assert_eq!(
        whole.origin,
        RawOrigin::Received,
        "the union is the whole export"
    );
    assert_eq!(decoded(&whole, &[]), received.bytes);
}
