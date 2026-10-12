//! A copy of a statement its producer scrubbed yields to the unscrubbed copy of that statement.
//!
//! A producer may deliver one span twice, once with its scrubber applied and once without (a retried export from
//! a process whose scrubbing changed). Both copies came from the producer, and raw keeps both, so preferring the
//! unscrubbed one shows nothing SideSeat was not sent - and it is what the wire said. This does not undo a
//! scrubber: a span delivered only scrubbed stays scrubbed.
//!
//! The marker is the producer's own declaration (`redaction_markers`), and a block is one only when its whole
//! content is the marker - never text that quotes it, never another producer's identical string. Partial scrubs
//! inside a longer text, and scrubbed fields outside message content, stay as stored.

use std::collections::HashMap;

use super::types::{BlockEntry, MarkerReport};
use crate::rules::redaction_markers::BlockScope;

/// Whether `block`'s whole content is a redaction marker its producer declares.
pub(in crate::sideml::feed) fn is_marker(block: &BlockEntry) -> bool {
    crate::rules::ruleset().redaction_markers.is_marker(
        &BlockScope {
            span_name: block.span_name.as_deref(),
            scope_name: block.scope_name.as_deref(),
            scope_version: block.scope_version.as_deref(),
        },
        &block.content,
    )
}

/// One statement read from one place: its trace and span, the carrier it was read from (its source, event and
/// attribute), its position in that carrier's payload, its role and kind, and the call and the tool or sender it
/// names. Two blocks agreeing on all of it are two deliveries of one statement; nothing wider is. A copy naming a
/// call or a tool the other does not name is not shown to be the same statement, so it does not yield - nor is
/// a message of a span's input the same statement as one at the same position of its output.
#[derive(PartialEq, Eq, Hash)]
struct Statement<'a> {
    trace: &'a str,
    span: &'a str,
    source: &'a str,
    event: Option<&'a str>,
    attribute: Option<&'a str>,
    position: &'a crate::sideml::provenance::PositionPath,
    role: &'a str,
    kind: &'a str,
    call: Option<&'a str>,
    said_by: Option<&'a str>,
}

/// The tool a call or result names: in its content, else at its message (`BlockEntry::name`, `tool_name`).
pub(in crate::sideml::feed) fn tool_named(block: &BlockEntry) -> Option<&str> {
    use crate::sideml::ContentBlock;
    let own = match &block.content {
        ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
        ContentBlock::ToolResult { name, .. } => name.as_deref(),
        _ => return None,
    };
    own.into_iter()
        .chain(block.tool_name.as_deref())
        .chain(block.name.as_deref())
        .find(|name| !name.is_empty())
}

/// The digest a report keeps of the tool a block names.
pub(in crate::sideml::feed) fn tool_digest(block: &BlockEntry) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    tool_named(block).map(|name| {
        let mut hasher = std::hash::DefaultHasher::new();
        name.hash(&mut hasher);
        hasher.finish()
    })
}

/// What a block reports for the markers it stands for: its own span failure and provider execution, and the tool
/// it names, when it is a marker, and whatever it took from markers in its place.
pub(in crate::sideml::feed) fn report_of(block: &BlockEntry) -> MarkerReport {
    use crate::sideml::ContentBlock;
    let mut report = block.marker_report;
    if is_marker(block) {
        report.span_failed |= block.is_error;
        report.by_provider |= matches!(
            block.content,
            ContentBlock::ToolResult {
                provider_executed: true,
                ..
            }
        );
        report.tool = report.tool.or(tool_digest(block));
    }
    report
}

/// Takes `heard` into `block`'s report where the tools they name agree - either naming none is agreement - so a
/// report never passes to a copy of another tool, however many copies it passed through on the way.
pub(in crate::sideml::feed) fn hear_report(block: &mut BlockEntry, heard: MarkerReport) {
    let own = block.marker_report.tool.or(tool_digest(block));
    if let (Some(own), Some(heard)) = (own, heard.tool)
        && own != heard
    {
        return;
    }
    let report = &mut block.marker_report;
    report.span_failed |= heard.span_failed;
    report.by_provider |= heard.by_provider;
    report.tool = report.tool.or(heard.tool);
}

/// Raises on a block that is not itself a marker what the markers it stands for reported.
pub(in crate::sideml::feed) fn raise_report(block: &mut BlockEntry) {
    use crate::sideml::ContentBlock;
    if is_marker(block) {
        return;
    }
    if block.marker_report.span_failed {
        block.is_error = true;
        block
            .status_code
            .get_or_insert_with(|| super::status::ERROR.to_string());
    }
    if block.marker_report.by_provider
        && let ContentBlock::ToolResult {
            provider_executed, ..
        } = &mut block.content
    {
        *provider_executed = true;
    }
}

fn statement(block: &BlockEntry) -> Statement<'_> {
    // Who said it: the tool a call or result names, or the sender a message names (`BlockEntry::name`).
    let tool = tool_named(block).or_else(|| block.name.as_deref().filter(|name| !name.is_empty()));
    Statement {
        trace: block.trace_id.as_str(),
        span: block.span_id.as_str(),
        source: block.source_type.as_str(),
        event: block.event_name.as_deref(),
        attribute: block.source_attribute.as_deref(),
        position: &block.position,
        role: block.role.as_str(),
        kind: block.entry_type.as_str(),
        call: block.tool_use_id.as_deref().filter(|id| !id.is_empty()),
        said_by: tool,
    }
}

/// One thing several copies state: nothing yet, one value they all agree on, or values that disagree - which
/// pass nothing on.
#[derive(Clone, Default, PartialEq)]
enum Agreed<T> {
    #[default]
    Unsaid,
    One(T),
    Disagreed,
}

impl<T: PartialEq> Agreed<T> {
    fn add(&mut self, value: T) {
        *self = match std::mem::take(self) {
            Agreed::Unsaid => Agreed::One(value),
            Agreed::One(current) if current == value => Agreed::One(current),
            _ => Agreed::Disagreed,
        };
    }
}

/// Drops each marker block whose statement another, unscrubbed block also states. A statement every copy of
/// which is a marker, or none of which is, keeps every copy, exactly as before.
///
/// What only the marker copies say besides their content passes first to every unscrubbed copy of the statement,
/// decided over all of them at once so no arrival order speaks: a failure, span failure or provider execution any
/// of them reports, and a finish or status every copy stating one agrees on, where the copy states none. Each unscrubbed copy is the statement, so none is chosen over another.
/// Its hash is left as it was read: it is the observation's identity, the one a later trace's replay of it
/// matches, as deduplication's own adoption leaves it.
pub(in crate::sideml::feed) fn yield_marker_copies(blocks: &mut Vec<BlockEntry>) {
    use crate::sideml::ContentBlock;
    let markers: Vec<bool> = blocks.iter().map(is_marker).collect();
    if !markers.contains(&true) {
        return;
    }
    // A composed block has no position, so nothing is the same statement as it.
    let placed = |index: usize| !blocks[index].position.is_empty();
    let mut copies: HashMap<Statement<'_>, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (index, marker) in markers.iter().enumerate() {
        if *marker && placed(index) {
            copies
                .entry(statement(&blocks[index]))
                .or_default()
                .0
                .push(index);
        }
    }
    for (index, marker) in markers.iter().enumerate() {
        if !*marker
            && placed(index)
            && let Some((_, unscrubbed)) = copies.get_mut(&statement(&blocks[index]))
        {
            unscrubbed.push(index);
        }
    }
    let settled: Vec<(Vec<usize>, Vec<usize>)> = copies
        .into_values()
        .filter(|(_, unscrubbed)| !unscrubbed.is_empty())
        .collect();
    let mut yields = vec![false; blocks.len()];
    for (scrubbed, unscrubbed) in settled {
        // Flags a copy raises only when true: any marker raising one is the statement raising it.
        let failed = scrubbed.iter().any(|&m| {
            matches!(
                blocks[m].content,
                ContentBlock::ToolResult { is_error: true, .. }
            )
        });
        // Every marker of one statement names its one tool, the statement's.
        let mut report = MarkerReport::default();
        for &marker in &scrubbed {
            let heard = report_of(&blocks[marker]);
            report.span_failed |= heard.span_failed;
            report.by_provider |= heard.by_provider;
            report.tool = report.tool.or(heard.tool);
        }
        // Values a copy may state differently: what passes is what every copy stating one - unscrubbed copies
        // too - agrees on, so an unscrubbed copy without one never takes a value another unscrubbed copy contradicts.
        let mut finish = Agreed::Unsaid;
        let mut status = Agreed::Unsaid;
        for &copy in scrubbed.iter().chain(&unscrubbed) {
            if let Some(stated) = blocks[copy].finish_reason {
                finish.add(stated);
            }
            if let Some(stated) = blocks[copy].status_code.as_deref() {
                status.add(stated);
            }
        }
        let finish = match finish {
            Agreed::One(stated) => Some(stated),
            _ => None,
        };
        let status = match status {
            Agreed::One(stated) => Some(stated.to_string()),
            _ => None,
        };
        for &marker in &scrubbed {
            yields[marker] = true;
        }
        for &twin in &unscrubbed {
            let survivor = &mut blocks[twin];
            if let ContentBlock::ToolResult { is_error, .. } = &mut survivor.content {
                *is_error |= failed;
            }
            if survivor.finish_reason.is_none() {
                survivor.finish_reason = finish;
            }
            if survivor.status_code.is_none() {
                survivor.status_code.clone_from(&status);
            }
            hear_report(survivor, report);
            raise_report(survivor);
        }
    }
    let mut yields = yields.into_iter();
    blocks.retain(|_| !yields.next().unwrap_or(false));
}
