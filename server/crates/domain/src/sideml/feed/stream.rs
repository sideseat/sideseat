//! Streamed responses: a producer that reports a response chunk by chunk writes one reading per chunk and one
//! that ends it (`MessageRule::stream`). A chunk is a piece of the response, never a response of its own, so every
//! view shows a stream's chunks as the one response they are pieces of; the raw telemetry keeps each reading.
//!
//! A **stream** is the stream readings one signal carries for one span's row - its span events, or its log
//! records - in event-time order and then the order they were stored in. A producer that writes a stream on both
//! signals delivers it twice, so each delivery is read on its own, as any message written on both is. Each
//! terminal closes a **segment** - the chunks since the previous terminal, and it; chunks after the last terminal
//! are an **open** segment, a stream that was interrupted. A closed segment is read by what its terminal holds:
//!
//! - `aggregate`: the terminal is the response, and its chunks restate it;
//! - `delta`: the response is the chunks and the terminal, joined;
//! - `unknown`: resolved only by evidence - the response the span's nearest generation ancestor recorded. Equal
//!   to the terminal, and the terminal begins with its chunks, it is an aggregate; equal to the join, a delta.
//!   A terminal holding nothing can only be the chunks' end, so it is a delta without evidence. Otherwise - or
//!   with no such ancestor, as in a span view, which sees its span alone - the chunks are shown joined and the
//!   terminal beside them as it was recorded: nothing is lost, and nothing is assumed. The record is taken as
//!   the finished response, since the terminal says the call finished; one a cancelled call left holding only
//!   its last piece would read as an aggregate of that piece.
//!
//! An open segment is its join, with no finish reason. A chunk's tool call may be one whose arguments have not
//! finished arriving, so it is a call only where the producer marks which are (`partial_calls`) and this one is
//! not: a settled chunk's call is in every join, once - the terminal's copy and each chunk's are one call, shown
//! where it first appears with the id any copy names - and alike calls of one reading are separate calls. The
//! mark is the chunk's, so one holding a finished call beside one still arriving keeps both out. A producer
//! that marks none has every chunk's call left out, and a complete call it sends only in a chunk is lost.
//!
//! Joining happens between normalisation's two stages (`read_sideml`, `finish_sideml`), on one message per
//! reading: where a response's calls split it, and which of its whitespace is blank beside visible content,
//! are facts about the whole response, so they are settled on it exactly as if one reading had held it.

use super::*;
use crate::rules::schema::StreamMark;
use crate::sideml::provenance::PathSegment;
use crate::sideml::types::ChatRole;

/// One row's messages as normalisation's first stage reads them: one per stored message, tool blocks not yet
/// split, a streamed reading's blank text not yet dropped.
pub(super) struct RowMessages<'a> {
    pub(super) row: &'a MessageSpanRow,
    /// How many of the stored messages are the span's own; those after them are its log records'.
    pub(super) span_messages: usize,
    pub(super) messages: Vec<SideMLMessage>,
}

/// One stream's readings - the messages each stored message became - keyed by when it was written and where it sat.
type Readings = BTreeMap<(DateTime<Utc>, usize), Vec<usize>>;

/// The spans a view holds, by trace and span id: span ids are the client's, unique only within a trace. `None` for
/// a span whose rows disagree on where it sits in the tree or what it is, which no walk may read either way.
type SpanTree<'a> = HashMap<(&'a str, &'a str), Option<&'a MessageSpanRow>>;

/// Each row's streamed responses read whole - see the module documentation. `spans` is every row the view was
/// given, whatever became of its messages: the span tree an unknown terminal's evidence is found through. Rows
/// without a stream reading are left as they are.
pub(super) fn reassemble<'a>(spans: &'a [MessageSpanRow], rows: &mut [RowMessages<'a>]) {
    let streamed = |messages: &[SideMLMessage]| messages.iter().any(|m| m.stream.is_some());
    if !rows.iter().any(|row| streamed(&row.messages)) {
        return;
    }
    let mut tree: SpanTree<'a> = HashMap::new();
    for span in spans {
        tree.entry((span.trace_id.as_str(), span.span_id.as_str()))
            .and_modify(|known| {
                if known.is_some_and(|known| {
                    known.parent_span_id != span.parent_span_id
                        || known.observation_type != span.observation_type
                }) {
                    *known = None;
                }
            })
            .or_insert(Some(span));
    }
    let mut rows_of: HashMap<(&'a str, &'a str), Vec<usize>> = HashMap::new();
    for (index, read) in rows.iter().enumerate() {
        rows_of
            .entry((read.row.trace_id.as_str(), read.row.span_id.as_str()))
            .or_default()
            .push(index);
    }
    // The evidence an unknown terminal after chunks is resolved by: read once per generation, however many streams
    // it encloses, and before any row is rewritten.
    let mut evidence: HashMap<(&'a str, &'a str), Option<Vec<ContentBlock>>> = HashMap::new();
    let mut wrappers: Vec<Option<(&'a str, &'a str)>> = Vec::with_capacity(rows.len());
    for read in rows.iter() {
        let declares = |mark| read.messages.iter().any(|m| m.stream == Some(mark));
        let wrapper = (read
            .messages
            .iter()
            .any(|m| m.stream.is_some_and(StreamMark::is_chunk))
            && declares(StreamMark::Unknown))
        .then(|| generation_above(&tree, read.row))
        .flatten()
        .map(|wrapper| (read.row.trace_id.as_str(), wrapper));
        if let Some(key) = wrapper {
            evidence.entry(key).or_insert_with(|| {
                // A generation whose messages could not be read, or were suppressed, recorded nothing.
                rows_of
                    .get(&key)
                    .and_then(|copies| recorded_response(rows, copies))
            });
        }
        wrappers.push(wrapper);
    }
    for (read, wrapper) in rows.iter_mut().zip(wrappers) {
        if streamed(&read.messages) {
            let recorded = wrapper
                .and_then(|key| evidence.get(&key))
                .and_then(Option::as_deref);
            read.messages = reassemble_row(
                std::mem::take(&mut read.messages),
                read.span_messages,
                recorded,
            );
        }
    }
}

/// One row's messages with each segment of its streams replaced by the response it is.
fn reassemble_row(
    messages: Vec<SideMLMessage>,
    span_messages: usize,
    recorded: Option<&[ContentBlock]>,
) -> Vec<SideMLMessage> {
    // The span events' stream, then the log records'.
    let mut streams: [Readings; 2] = Default::default();
    for (index, message) in messages.iter().enumerate() {
        if message.stream.is_none() {
            continue;
        }
        let (signal, root) = signal(message, span_messages);
        streams[signal]
            .entry((message.timestamp, root))
            .or_default()
            .push(index);
    }

    let mut dropped = vec![false; messages.len()];
    let mut replaced: HashMap<usize, SideMLMessage> = HashMap::new();
    for readings in &streams {
        let mut chunks: Vec<usize> = Vec::new();
        for reading in readings.values() {
            let mark = messages[reading[0]]
                .stream
                .expect("only stream readings are keyed");
            if !mark.is_terminal() {
                chunks.extend(reading);
                continue;
            }
            let terminal = reading;
            let pieces = std::mem::take(&mut chunks);
            if pieces.is_empty() {
                continue;
            }
            // The join as shown - its calls with the ids their copies name - or as recorded, for comparing: which
            // of several alike calls a copy naming no id is cannot be told, so it is compared as naming none.
            let joined = |adopted: bool| {
                let calls = Calls::of(&messages, terminal, &pieces, false).with_ids(adopted);
                response_blocks(&messages, pieces.iter().chain(terminal), &calls)
            };
            // Compared as each would be shown: the terminal's blocks as they are, and the join as one response. An
            // aggregate restates its chunks, so a terminal that does not begin with them - the last piece, which
            // a cancelled call records - is none, whatever the generation recorded; and one that holds nothing
            // can only be the chunks' end.
            let resolved = match mark {
                StreamMark::Unknown => {
                    let shown: Vec<ContentBlock> = terminal
                        .iter()
                        .flat_map(|&index| messages[index].sideml.content.iter().cloned())
                        .collect();
                    let shown = whole(shown);
                    let chunked = whole(response_blocks(
                        &messages,
                        &pieces,
                        &Calls::of(&messages, terminal, &pieces, false),
                    ));
                    if shown.is_empty() && !chunked.is_empty() {
                        Some(StreamMark::Delta)
                    } else {
                        // The terminal begins with the chunks' text and reasoning, and holds every call they do -
                        // a call's place among alike ones is no evidence, since a copy naming no id may be any.
                        let restated = restates(&without_calls(&shown), &without_calls(&chunked))
                            && Calls::of(&messages, terminal, &pieces, false)
                                .terminal_holds_every_call();
                        recorded.and_then(|response| {
                            if states(response, &shown) && restated {
                                Some(StreamMark::Aggregate)
                            } else if states(response, &whole(joined(false))) {
                                Some(StreamMark::Delta)
                            } else {
                                None
                            }
                        })
                    }
                }
                declared => Some(declared),
            };
            match resolved {
                Some(StreamMark::Aggregate) => {
                    pieces.iter().for_each(|&index| dropped[index] = true);
                    replaced.extend(terminal_with_ids(&messages, terminal, &pieces));
                }
                // The join stands where the terminal stood, with its time, emission and finish reason.
                Some(StreamMark::Delta) => {
                    let mut response = messages[terminal[0]].clone();
                    response.sideml.content = joined(true);
                    response.sideml.finish_reason = terminal
                        .iter()
                        .find_map(|&index| messages[index].sideml.finish_reason);
                    replaced.insert(terminal[0], response);
                    for &index in pieces.iter().chain(&terminal[1..]) {
                        dropped[index] = true;
                    }
                }
                // Unresolved: the chunks are still pieces of one response, so they are shown joined, and the
                // terminal beside them as it was recorded.
                _ => {
                    join_chunks(&messages, &pieces, terminal, &mut replaced, &mut dropped);
                    replaced.extend(terminal_with_ids(&messages, terminal, &pieces));
                }
            }
        }
        // An interrupted stream: what arrived, joined.
        if !chunks.is_empty() {
            join_chunks(&messages, &chunks, &[], &mut replaced, &mut dropped);
        }
    }

    messages
        .into_iter()
        .enumerate()
        .filter_map(|(index, message)| match replaced.remove(&index) {
            Some(replacement) => Some(replacement),
            None => (!dropped[index]).then_some(message),
        })
        .collect()
}

/// A segment's chunks as one message, the response so far: where the last of them stood, with no finish reason.
fn join_chunks(
    messages: &[SideMLMessage],
    pieces: &[usize],
    beside: &[usize],
    replaced: &mut HashMap<usize, SideMLMessage>,
    dropped: &mut [bool],
) {
    let (&last, earlier) = pieces.split_last().expect("a segment has a chunk");
    let mut response = messages[last].clone();
    response.sideml.content =
        response_blocks(messages, pieces, &Calls::of(messages, beside, pieces, true));
    response.sideml.finish_reason = None;
    replaced.insert(last, response);
    for &index in earlier {
        dropped[index] = true;
    }
}

/// The blocks of `pieces` in order, as one response: adjacent text joined with nothing added, adjacent
/// reasoning joined until a piece carrying its signature ends it, every other block as it is. A chunk's call is a
/// call only where the producer marks it settled; each call `calls` knows is shown once, where it first
/// appears, with the id any copy of it names.
fn response_blocks<'m>(
    messages: &[SideMLMessage],
    pieces: impl IntoIterator<Item = &'m usize>,
    calls: &Calls<'_>,
) -> Vec<ContentBlock> {
    let mut out: Vec<ContentBlock> = Vec::new();
    for &index in pieces {
        let chunk = messages[index].stream == Some(StreamMark::Chunk);
        for block in &messages[index].sideml.content {
            if matches!(block, ContentBlock::ToolUse { .. }) {
                if chunk {
                    continue;
                }
                if let Some(shown) = calls.shown(block) {
                    if let Some(shown) = shown {
                        push_joined(&mut out, shown);
                    }
                    continue;
                }
            }
            push_joined(&mut out, block.clone());
        }
    }
    out
}

/// The terminal's messages whose calls take an id a settled chunk's copy names ([`Calls`]), as they are shown.
fn terminal_with_ids(
    messages: &[SideMLMessage],
    terminal: &[usize],
    pieces: &[usize],
) -> Vec<(usize, SideMLMessage)> {
    let calls = Calls::of(messages, terminal, pieces, true);
    terminal
        .iter()
        .filter_map(|&index| {
            let content: Vec<ContentBlock> = messages[index]
                .sideml
                .content
                .iter()
                .map(|block| calls.as_shown(block))
                .collect();
            (!same_response(&content, &messages[index].sideml.content)).then(|| {
                let mut message = messages[index].clone();
                message.sideml.content = content;
                (index, message)
            })
        })
        .collect()
}

/// The calls a terminal and its settled chunks hold, as one set of calls. Each reading's calls are distinct,
/// while alike calls of two readings are copies of one: a call naming an id is that id's, wherever it is, and
/// one naming none is any call of its name and arguments, taking the id a copy names. So there are as many calls
/// as there are ids, or as one reading holds, if more - and each is shown where it first appears.
struct Calls<'b> {
    calls: Vec<Call<'b>>,
    /// Every copy of a call the readings hold: the call it is, whether it is the call's first appearance, and
    /// whether the terminal holds it.
    copies: Vec<CallCopy<'b>>,
    /// Whether a call is shown with the id a copy of it names, or as it was recorded.
    adopted_ids: bool,
}

struct CallCopy<'b> {
    block: &'b ContentBlock,
    call: usize,
    first: bool,
    in_terminal: bool,
}

struct Call<'b> {
    name: &'b str,
    input: &'b JsonValue,
    id: Option<&'b str>,
}

impl<'b> Calls<'b> {
    /// The calls of the settled chunks among `pieces` and of the terminal, read in order - the terminal first
    /// where it is shown on its own, so its copy of a call is the one shown; last where it is joined after
    /// them.
    fn of<'m>(
        messages: &'b [SideMLMessage],
        terminal: &[usize],
        pieces: impl IntoIterator<Item = &'m usize>,
        terminal_first: bool,
    ) -> Self {
        let calls_in = |index: usize| -> Vec<&'b ContentBlock> {
            messages[index]
                .sideml
                .content
                .iter()
                .filter(|block| call_of(block).is_some())
                .collect()
        };
        let terminal_calls: Vec<&'b ContentBlock> =
            terminal.iter().flat_map(|&index| calls_in(index)).collect();
        let mut readings: Vec<Vec<&'b ContentBlock>> = pieces
            .into_iter()
            .filter(|&&index| messages[index].stream == Some(StreamMark::SettledChunk))
            .map(|&index| calls_in(index))
            .collect();
        if terminal_first {
            readings.insert(0, terminal_calls);
        } else {
            readings.push(terminal_calls);
        }
        let mut calls = Self {
            calls: Vec::new(),
            copies: Vec::new(),
            adopted_ids: true,
        };
        let terminal_at = if terminal_first {
            0
        } else {
            readings.len() - 1
        };
        for (at, reading) in readings.into_iter().enumerate() {
            calls.read(reading, at == terminal_at);
        }
        calls
    }

    /// Whether every call a chunk holds the terminal holds too.
    fn terminal_holds_every_call(&self) -> bool {
        self.copies
            .iter()
            .filter(|copy| !copy.in_terminal)
            .all(|copy| {
                self.copies
                    .iter()
                    .any(|other| other.in_terminal && other.call == copy.call)
            })
    }

    /// One reading's calls, each matched to a call no other copy in this reading is: those naming an id first,
    /// to that id's call or to an alike call naming none, which takes the id; then those naming none, to any
    /// alike call left. A call matched to none is a new one.
    fn read(&mut self, reading: Vec<&'b ContentBlock>, in_terminal: bool) {
        let mut taken: Vec<usize> = Vec::new();
        let (named, unnamed): (Vec<&'b ContentBlock>, Vec<&'b ContentBlock>) = reading
            .into_iter()
            .partition(|block| call_of(block).is_some_and(|(id, ..)| id.is_some()));
        for block in named.into_iter().chain(unnamed) {
            let Some((id, name, input)) = call_of(block) else {
                continue;
            };
            let free = |call: &Call<'b>, at: usize| {
                !taken.contains(&at) && call.name == name && call.input == input
            };
            let matched = match id {
                Some(id) => self
                    .calls
                    .iter()
                    .enumerate()
                    .position(|(at, call)| !taken.contains(&at) && call.id == Some(id))
                    .or_else(|| {
                        self.calls
                            .iter()
                            .enumerate()
                            .position(|(at, call)| call.id.is_none() && free(call, at))
                    }),
                None => self
                    .calls
                    .iter()
                    .enumerate()
                    .position(|(at, call)| free(call, at)),
            };
            let at = match matched {
                Some(at) => {
                    if self.calls[at].id.is_none() {
                        self.calls[at].id = id;
                    }
                    at
                }
                None => {
                    self.calls.push(Call { name, input, id });
                    self.calls.len() - 1
                }
            };
            let first = !self.copies.iter().any(|copy| copy.call == at);
            self.copies.push(CallCopy {
                block,
                call: at,
                first,
                in_terminal,
            });
            taken.push(at);
        }
    }

    /// For a copy these calls know: the call as shown, where this is its first appearance, or nothing, where it
    /// is a later copy. `None` for a block they do not know.
    fn shown(&self, block: &ContentBlock) -> Option<Option<ContentBlock>> {
        let copy = self
            .copies
            .iter()
            .find(|copy| std::ptr::eq(copy.block, block))?;
        Some(copy.first.then(|| self.with_id(block, copy.call)))
    }

    /// A block as shown: a call these calls know, with the id its call names.
    fn as_shown(&self, block: &ContentBlock) -> ContentBlock {
        match self
            .copies
            .iter()
            .find(|copy| std::ptr::eq(copy.block, block))
        {
            Some(copy) => self.with_id(block, copy.call),
            None => block.clone(),
        }
    }

    /// These calls, shown with the ids their copies name, or as recorded.
    fn with_ids(mut self, adopted: bool) -> Self {
        self.adopted_ids = adopted;
        self
    }

    fn with_id(&self, block: &ContentBlock, at: usize) -> ContentBlock {
        let mut shown = block.clone();
        if !self.adopted_ids {
            return shown;
        }
        if let (ContentBlock::ToolUse { id, .. }, Some(named)) = (&mut shown, self.calls[at].id) {
            *id = Some(named.to_string());
        }
        shown
    }
}

/// A call's id, name and arguments.
fn call_of(block: &ContentBlock) -> Option<(Option<&str>, &str, &JsonValue)> {
    match block {
        ContentBlock::ToolUse {
            id, name, input, ..
        } => Some((id.as_deref(), name.as_str(), input)),
        _ => None,
    }
}

/// Whether `whole` begins with `pieces`: every piece equal to the block it stands for, the last one's text or
/// reasoning allowed to stop part of the way through. A piece of reasoning may lack the signature its whole
/// carries, which a stream sends once the reasoning is complete.
fn restates(whole: &[ContentBlock], pieces: &[ContentBlock]) -> bool {
    pieces.len() <= whole.len()
        && pieces.iter().zip(whole).enumerate().all(|(at, pair)| {
            let begins = |piece: &str, text: &str| match at + 1 == pieces.len() {
                true => text.starts_with(piece),
                false => text == piece,
            };
            match pair {
                (
                    ContentBlock::Text {
                        text: piece,
                        citations: piece_citations,
                    },
                    ContentBlock::Text { text, citations },
                ) => begins(piece, text) && piece_citations == citations,
                (
                    ContentBlock::Thinking {
                        text: piece,
                        signature: piece_signature,
                    },
                    ContentBlock::Thinking { text, signature },
                ) => {
                    begins(piece, text)
                        && (piece_signature.is_none() || piece_signature == signature)
                }
                (piece, block) => {
                    same_response(std::slice::from_ref(piece), std::slice::from_ref(block))
                }
            }
        })
}

/// A response's text and reasoning, the calls between them left out.
fn without_calls(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    let mut out = Vec::new();
    for block in blocks
        .iter()
        .filter(|block| !matches!(block, ContentBlock::ToolUse { .. }))
    {
        push_joined(&mut out, block.clone());
    }
    out
}

fn push_joined(out: &mut Vec<ContentBlock>, block: ContentBlock) {
    match (out.last_mut(), block) {
        (
            Some(ContentBlock::Text {
                text: last,
                citations: last_citations,
            }),
            ContentBlock::Text { text, citations },
        ) if last_citations.is_empty() && citations.is_empty() => last.push_str(&text),
        (
            Some(ContentBlock::Thinking {
                text: last,
                signature: last_signature,
            }),
            ContentBlock::Thinking { text, signature },
            // The piece carrying the signature ends the reasoning it signs: what follows is reasoning of its own.
        ) if last_signature.is_none() => {
            last.push_str(&text);
            *last_signature = signature;
        }
        (_, block) => out.push(block),
    }
}

/// Which signal a message came on - 0 for the span's own events and attributes, 1 for its log records - and the
/// index of the stored message it was read from.
fn signal(message: &SideMLMessage, span_messages: usize) -> (usize, usize) {
    match message.position.segments().first() {
        Some(PathSegment::Index(root)) => (usize::from(*root >= span_messages), *root),
        _ => (0, usize::MAX),
    }
}

/// A response as a view shows it: without blank text beside visible content.
fn whole(mut blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    crate::sideml::remove_blank_text_beside_visible_content(&mut blocks);
    blocks
}

/// The response a generation recorded, as blocks: what each of its assistant output messages states, where every
/// one - on every carrier, signal and row - states the same. A span delivered twice, or an answer written on two
/// carriers, states it more than once; two messages are two statements, never one response joined from both.
/// `None` where it recorded none, or states two.
fn recorded_response(rows: &[RowMessages<'_>], copies: &[usize]) -> Option<Vec<ContentBlock>> {
    let mut stated: Option<Vec<ContentBlock>> = None;
    for &copy in copies {
        let RowMessages { row, messages, .. } = &rows[copy];
        for message in messages {
            if message.sideml.role != ChatRole::Assistant || message.stream.is_some() {
                continue;
            }
            let (event, attribute) = match &message.source {
                MessageSource::Event { name, .. } => (Some(name.as_str()), None),
                MessageSource::Attribute { key, .. } => (None, Some(key.as_str())),
            };
            let semantics =
                crate::sideml::carrier::semantics_for_context(&crate::rules::CarrierContext {
                    event,
                    attribute,
                    observation_type: row.observation_type.as_deref(),
                    span_name: row.span_name.as_deref(),
                    direction: message.direction,
                });
            if !semantics.carrier_holds_span_output {
                continue;
            }
            // As it is shown: a message holding "Par" and "is" as two blocks shows two, never "Paris".
            let statement = whole(message.sideml.content.clone());
            match &mut stated {
                Some(first) if !states(first, &statement) => return None,
                // A signature one statement dropped, another states: the response is the signed one.
                Some(first) => {
                    for pair in first.iter_mut().zip(statement) {
                        if let (
                            ContentBlock::Thinking {
                                signature: kept @ None,
                                ..
                            },
                            ContentBlock::Thinking {
                                signature: Some(signature),
                                ..
                            },
                        ) = pair
                        {
                            *kept = Some(signature);
                        }
                    }
                }
                None => stated = Some(statement),
            }
        }
    }
    stated
}

/// The nearest span above `row`'s, in its trace, that is a generation. `None` where the ancestry leaves the spans
/// the view holds, reaches a span its rows disagree on, or comes back to one it passed: a cycle, which only
/// malformed telemetry states.
fn generation_above<'a>(tree: &SpanTree<'a>, row: &'a MessageSpanRow) -> Option<&'a str> {
    let trace = row.trace_id.as_str();
    let origin = (*tree.get(&(trace, row.span_id.as_str()))?)?;
    let mut passed: HashSet<&str> = HashSet::from([origin.span_id.as_str()]);
    let mut parent = origin.parent_span_id.as_deref();
    while let Some(id) = parent {
        if !passed.insert(id) {
            return None;
        }
        let ancestor = (*tree.get(&(trace, id))?)?;
        if ancestor.observation_type.as_deref() == Some(obs_type::GENERATION) {
            return Some(ancestor.span_id.as_str());
        }
        parent = ancestor.parent_span_id.as_deref();
    }
    None
}

/// Whether two statements of a response agree: [`same_response`], except that reasoning one of them holds
/// unsigned agrees with the other's signed reasoning of the same text, and a call one names no id for agrees
/// with an alike call naming one. A carrier with no member for a signature drops it - ADK's record of a model
/// call does - and its copy of the reasoning is still a copy (`dedup::signature` reads it as one).
fn states(a: &[ContentBlock], b: &[ContentBlock]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (
                ContentBlock::Thinking {
                    text: a_text,
                    signature: a_signature,
                },
                ContentBlock::Thinking {
                    text: b_text,
                    signature: b_signature,
                },
            ) => {
                a_text == b_text
                    && (a_signature.is_none()
                        || b_signature.is_none()
                        || a_signature == b_signature)
            }
            // A call naming no id is any call of its name and arguments.
            (
                ContentBlock::ToolUse {
                    id: a_id,
                    name: a_name,
                    input: a_input,
                    provider_executed: a_provider,
                },
                ContentBlock::ToolUse {
                    id: b_id,
                    name: b_name,
                    input: b_input,
                    provider_executed: b_provider,
                },
            ) if a_id.is_none() || b_id.is_none() => {
                a_name == b_name && a_input == b_input && a_provider == b_provider
            }
            (a, b) => same_response(std::slice::from_ref(a), std::slice::from_ref(b)),
        })
}

/// Two responses are the same when they hold the same blocks in the same order, exactly: text and reasoning
/// byte for byte with its signature, a call by id, name and arguments, anything else by its whole form.
fn same_response(a: &[ContentBlock], b: &[ContentBlock]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (
                ContentBlock::Thinking {
                    text: a_text,
                    signature: a_signature,
                },
                ContentBlock::Thinking {
                    text: b_text,
                    signature: b_signature,
                },
            ) => a_text == b_text && a_signature == b_signature,
            (
                ContentBlock::ToolUse {
                    id: a_id,
                    name: a_name,
                    input: a_input,
                    provider_executed: a_provider,
                },
                ContentBlock::ToolUse {
                    id: b_id,
                    name: b_name,
                    input: b_input,
                    provider_executed: b_provider,
                },
            ) => a_id == b_id && a_name == b_name && a_input == b_input && a_provider == b_provider,
            (a, b) => serde_json::to_value(a).ok() == serde_json::to_value(b).ok(),
        })
}
