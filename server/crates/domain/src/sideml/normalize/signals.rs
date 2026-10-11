//! A message written on both signals - as a span event and as a log record of the same span - is one message.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use super::Observed;
use crate::observations::{MessageSource, RawMessage};
use crate::rules::schema::{ReadingDirection, StreamMark};
use crate::sideml::provenance::PathSegment;

/// What makes two readings copies of one message: the carrier, its content exactly, and how it is read. Not the
/// instant, which a producer stamps on each signal apart.
#[derive(Hash, PartialEq, Eq)]
struct CopyKey {
    event: bool,
    carrier: String,
    content: String,
    rendering: bool,
    direction: Option<ReadingDirection>,
    stream: Option<StreamMark>,
}

impl CopyKey {
    fn of(raw: &RawMessage) -> Self {
        let (event, carrier) = match &raw.source {
            MessageSource::Event { name, .. } => (true, name.clone()),
            MessageSource::Attribute { key, .. } => (false, key.clone()),
        };
        Self {
            event,
            carrier,
            content: canonical(&raw.content),
            rendering: raw.rendering,
            direction: raw.direction,
            stream: raw.stream,
        }
    }
}

/// Where a span's stored messages end and its log records' begin, and when the span started.
#[derive(Clone, Copy)]
pub(crate) struct Signals {
    pub logged_from: usize,
    pub span_start: Option<DateTime<Utc>>,
}

/// The observations with each log record's copy of a message the span holds left out: the stored messages
/// before `logged_from` are the span's, the rest its log records'. Compared once expanded, so a span event
/// bundling several messages and records holding one each are matched message by message.
///
/// Copies are matched one to one, so a message the span holds twice and the records three times is three
/// messages. The copy kept keeps its instant - and with it its place among the others - unless the two copies
/// disagree on whether it was sent with this call: stamped before the span started on one signal and after on
/// the other, the later instant is the one that says so, and the copy kept takes it. A streamed response is
/// matched whole - each segment of a stream, the readings of its chunks and of the terminal that ends them,
/// against an equal segment - so records holding a response the span does not hold keep it intact; its
/// instants move onto the records' clock only where the records matched the whole stream.
pub(super) fn one_copy_each(mut observed: Vec<Observed>, signals: Signals) -> Vec<Observed> {
    let root = |(_, position): &Observed| match position.segments().first() {
        Some(PathSegment::Index(root)) => *root,
        _ => usize::MAX,
    };
    let logged = |observation: &Observed| root(observation) >= signals.logged_from;
    if !observed.iter().any(logged) {
        return observed;
    }
    let mut dropped = vec![false; observed.len()];
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut taken_segments: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();

    // Messages: each span copy taken by one record at most, in order.
    let mut span_copies: HashMap<CopyKey, (Vec<usize>, usize)> = HashMap::new();
    for (index, observation) in observed.iter().enumerate() {
        if !logged(observation) && observation.0.stream.is_none() {
            span_copies
                .entry(CopyKey::of(&observation.0))
                .or_default()
                .0
                .push(index);
        }
    }
    for (index, observation) in observed.iter().enumerate() {
        if !logged(observation) || observation.0.stream.is_some() {
            continue;
        }
        let Some((copies, used)) = span_copies.get_mut(&CopyKey::of(&observation.0)) else {
            continue;
        };
        if let Some(&span) = copies.get(*used) {
            *used += 1;
            dropped[index] = true;
            taken.push((span, index));
        }
    }

    // A span event ingest enriched from its span's own attributes - a tool span's call id and tool name - holds
    // members its log record's copy never receives. A span copy exceeding a record copy only by those members,
    // every shared member equal, is the same message; any other extra member keeps both, since matching on a
    // subset would merge genuinely distinct messages. Asked only of what exact matching left, within a carrier.
    let mut kept_taken = vec![false; observed.len()];
    for &(span, _) in &taken {
        kept_taken[span] = true;
    }
    let mut left: HashMap<CarrierKey, Vec<usize>> = HashMap::new();
    for (index, observation) in observed.iter().enumerate() {
        if !logged(observation) && observation.0.stream.is_none() && !kept_taken[index] {
            left.entry(CarrierKey::of(&observation.0))
                .or_default()
                .push(index);
        }
    }
    for (index, observation) in observed.iter().enumerate() {
        if dropped[index] || !logged(observation) || observation.0.stream.is_some() {
            continue;
        }
        let Some(spans) = left.get_mut(&CarrierKey::of(&observation.0)) else {
            continue;
        };
        if let Some(at) = spans
            .iter()
            .position(|&span| enriched(&observed[span].0.content, &observation.0.content))
        {
            let span = spans.remove(at);
            dropped[index] = true;
            taken.push((span, index));
        }
    }

    // Streams: segment against segment, a segment being the readings - each stored message whole, in instant
    // then position order - from one terminal to the next.
    let segments = |logged_side: bool| -> Vec<Vec<usize>> {
        let mut readings: BTreeMap<(DateTime<Utc>, usize), Vec<usize>> = BTreeMap::new();
        for (index, observation) in observed.iter().enumerate() {
            if observation.0.stream.is_some() && logged(observation) == logged_side {
                readings
                    .entry((time(&observation.0), root(observation)))
                    .or_default()
                    .push(index);
            }
        }
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for reading in readings.into_values() {
            let terminal = observed[reading[0]]
                .0
                .stream
                .is_some_and(StreamMark::is_terminal);
            open.extend(reading);
            if terminal {
                out.push(std::mem::take(&mut open));
            }
        }
        if !open.is_empty() {
            out.push(open);
        }
        out
    };
    let key = |segment: &[usize]| -> Vec<CopyKey> {
        segment
            .iter()
            .map(|&index| CopyKey::of(&observed[index].0))
            .collect()
    };
    let mut span_segments: HashMap<Vec<CopyKey>, (Vec<Vec<usize>>, usize)> = HashMap::new();
    for segment in segments(false) {
        span_segments
            .entry(key(&segment))
            .or_default()
            .0
            .push(segment);
    }
    for segment in segments(true) {
        let Some((copies, used)) = span_segments.get_mut(&key(&segment)) else {
            continue;
        };
        if let Some(span) = copies.get(*used) {
            *used += 1;
            for &index in &segment {
                dropped[index] = true;
            }
            taken_segments.push((span.clone(), segment));
        }
    }

    // Each kept copy's instant, where its record's says it was sent with this call and its own does not. A stream
    // segment moves whole, so its readings keep the order they are joined in.
    if let Some(start) = signals.span_start {
        let disagree = |kept: usize, record: usize| {
            time(&observed[kept].0) < start && time(&observed[record].0) >= start
        };
        let mut moves: Vec<(usize, DateTime<Utc>)> = Vec::new();
        for &(kept, record) in &taken {
            if disagree(kept, record) {
                moves.push((kept, time(&observed[record].0)));
            }
        }
        // A stream's readings are ordered by their instants, and its segments are mutually consistent only on
        // one clock: moving one segment onto the records' clock while another keeps the span's interleaves
        // them. So the stream moves whole or not at all - every span-side reading matched, and some pair
        // disagreeing on whether it came before the span started - and partial matches keep the span's clock.
        let span_stream_readings = observed
            .iter()
            .enumerate()
            .filter(|(_, observation)| !logged(observation) && observation.0.stream.is_some())
            .count();
        let matched_readings: usize = taken_segments.iter().map(|(span, _)| span.len()).sum();
        let stream_disagrees = taken_segments.iter().any(|(span, logged)| {
            span.iter()
                .zip(logged)
                .any(|(&kept, &record)| disagree(kept, record))
        });
        if matched_readings == span_stream_readings && stream_disagrees {
            let stream_moves: Vec<(usize, DateTime<Utc>)> = taken_segments
                .iter()
                .flat_map(|(span, logged)| {
                    span.iter()
                        .zip(logged)
                        .map(|(&kept, &record)| (kept, time(&observed[record].0)))
                })
                .collect();
            // The move serves only the history rule; the readings' order is positional evidence. The records'
            // clock may tie where the span's did not, and a tie falls back to the span's storage positions -
            // so where the moved instants would reorder the readings, the span's clock stays.
            let moved: HashMap<usize, DateTime<Utc>> = stream_moves.iter().copied().collect();
            let ordered = |clock: &dyn Fn(usize) -> DateTime<Utc>| -> Vec<usize> {
                let mut readings: Vec<usize> = (0..observed.len())
                    .filter(|&index| {
                        !logged(&observed[index]) && observed[index].0.stream.is_some()
                    })
                    .collect();
                readings.sort_by_key(|&index| (clock(index), root(&observed[index]), index));
                readings
            };
            let before = ordered(&|index| time(&observed[index].0));
            let after = ordered(&|index| {
                moved
                    .get(&index)
                    .copied()
                    .unwrap_or_else(|| time(&observed[index].0))
            });
            if before == after {
                moves.extend(stream_moves);
            }
        }
        for (kept, instant) in moves {
            set_time(&mut observed[kept].0, instant);
        }
    }

    observed
        .into_iter()
        .zip(dropped)
        .filter_map(|(observation, dropped)| (!dropped).then_some(observation))
        .collect()
}

/// The carrier a copy came from and how it is read: a [`CopyKey`] without its content.
#[derive(Hash, PartialEq, Eq)]
struct CarrierKey {
    event: bool,
    carrier: String,
    rendering: bool,
    direction: Option<ReadingDirection>,
}

impl CarrierKey {
    fn of(raw: &RawMessage) -> Self {
        let CopyKey {
            event,
            carrier,
            rendering,
            direction,
            ..
        } = CopyKey::of(raw);
        Self {
            event,
            carrier,
            rendering,
            direction,
        }
    }
}

/// The members ingest's tool-span enrichment writes onto a span event's copy from the span's own attributes,
/// which a log record's copy never receives: the canonical role, call id and tool name.
const ENRICHED_MEMBERS: [&str; 3] = ["role", "tool_call_id", "name"];

/// Whether `span` is `logged` with the enrichment members added: an object holding every member the record's
/// holds, equal, and more - all of them from [`ENRICHED_MEMBERS`].
fn enriched(span: &serde_json::Value, logged: &serde_json::Value) -> bool {
    match (span.as_object(), logged.as_object()) {
        (Some(span), Some(logged)) => {
            span.len() > logged.len()
                && logged
                    .iter()
                    .all(|(key, value)| span.get(key) == Some(value))
                && span
                    .keys()
                    .all(|key| logged.contains_key(key) || ENRICHED_MEMBERS.contains(&key.as_str()))
        }
        _ => false,
    }
}

/// A JSON value as text with every object's members in key order: two copies are one whatever order a producer
/// wrote their members in on each signal.
fn canonical(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(members) => {
                let mut sorted: Vec<(&String, &serde_json::Value)> = members.iter().collect();
                sorted.sort_unstable_by(|a, b| a.0.cmp(b.0));
                out.push('{');
                for (at, (key, member)) in sorted.into_iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(key.clone()).to_string());
                    out.push(':');
                    write(member, out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            scalar => out.push_str(&scalar.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn time(raw: &RawMessage) -> DateTime<Utc> {
    match &raw.source {
        MessageSource::Event { time, .. } | MessageSource::Attribute { time, .. } => *time,
    }
}

fn set_time(raw: &mut RawMessage, instant: DateTime<Utc>) {
    match &mut raw.source {
        MessageSource::Event { time, .. } | MessageSource::Attribute { time, .. } => {
            *time = instant
        }
    }
}
