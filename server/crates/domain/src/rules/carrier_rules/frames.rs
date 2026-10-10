//! Which requests a detached request frame frames, where the frame is recorded apart from them.
//!
//! A frame a producer records once - one system instruction, on the span that started a run - frames every
//! request sent with it, and only a key both state says which: the frame's record states it, and so does each
//! request span. The keys are read at ingest, because a read holds a span's messages and not its attributes,
//! and stored as derived columns: the request's on its span, the frame's on its record. Equal keys in one trace
//! join, and the span view of each joined request opens with the frame.

use std::collections::HashMap;

use crate::sideml::carrier::CarrierSemantics;

use super::super::schema::{FramesRequests, MatchSpec};
use super::CompileError;

/// The declared joins: the attribute every framed request is keyed by, and per frame carrier the attribute its
/// record's key is read from.
#[derive(Debug, Default)]
pub struct RequestFrames {
    request: Option<String>,
    frame_by_event: HashMap<String, String>,
}

impl RequestFrames {
    /// The span attribute every framed request is keyed by, where a carrier frames requests.
    ///
    /// Only its name: the key is the attribute's value as ingest renders it, which reads the telemetry's own
    /// value types - one renderer for the request's attribute and the frame's, so equal values are equal keys.
    pub fn request_attribute(&self) -> Option<&str> {
        self.request.as_deref()
    }

    /// The attribute of a record read by this event carrier that holds the frame's key, where the carrier frames
    /// requests.
    pub fn frame_attribute(&self, carrier: &str) -> Option<&str> {
        self.frame_by_event.get(carrier).map(String::as_str)
    }

    /// Whether any carrier frames requests, so a path with nothing to key can skip asking.
    pub fn is_empty(&self) -> bool {
        self.frame_by_event.is_empty()
    }

    /// The form a frame record's key is stored and read in: the project, the trace and the key, as one 128-bit
    /// digest.
    ///
    /// Scoped to the trace because a frame joins only the requests of its own trace, and because a key is not
    /// selective by itself: a producer keys by its system prompt, which every session sent with that prompt
    /// shares, so the records stating one key grow with the store's history, and a read keyed by it alone would
    /// cost what the history holds rather than what the trace does. Scoped to the project as well, because a
    /// client's trace ids are not globally unique: two projects reusing one trace id and one prompt must not
    /// share an index entry. Sixteen bytes, stored as an unsigned 128-bit integer, whatever the producer's key
    /// and the ids look like. The read still filters by project and trace, so only two keys of one trace colliding
    /// in 128 bits could join wrongly. Never zero, which is what a record that frames nothing holds where a
    /// backend has no cheap NULL. `None` for a record outside any trace or project, or one stating a blank key.
    pub fn stored_key(project_id: &str, trace_id: &str, key: &str) -> Option<u128> {
        if project_id.is_empty() || trace_id.is_empty() || key.trim().is_empty() {
            return None;
        }
        let mut hasher = blake3::Hasher::new();
        // Each part length-prefixed, so no two triples digest the same bytes whatever they contain.
        for part in [project_id, trace_id, key] {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
        let mut digest = [0u8; 16];
        digest.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Some(Self::key_from_digest(digest))
    }

    /// Sixteen digest bytes as a stored key. Zero - what a record that frames nothing holds where a backend has no
    /// cheap NULL - maps to one, so no digest can be read as the absence of a key.
    pub(crate) fn key_from_digest(digest: [u8; 16]) -> u128 {
        u128::from_be_bytes(digest).max(1)
    }
}

/// One clause's declaration, checked against what the clause says the carrier is.
pub(super) fn declare(
    frames: &mut RequestFrames,
    clause: &str,
    match_spec: &MatchSpec,
    semantics: &CarrierSemantics,
    declared: &FramesRequests,
) -> Result<(), CompileError> {
    let refuse = |detail: &'static str| CompileError::Frames {
        clause: clause.to_string(),
        detail,
    };
    if !semantics.carrier_is_detached_request_frame {
        return Err(refuse(
            "frames requests, but its facts do not make it a detached request frame - a carrier that is part \
             of its own span's request frames nothing apart from it",
        ));
    }
    // The frame's key is read where the frame is, at ingest, with no span context: a log record is read before
    // the span it names is known, so a clause qualified by the span could not be resolved there.
    let Some(event) = match_spec
        .event
        .as_deref()
        .filter(|_| match_spec.observation_type.is_none())
    else {
        return Err(refuse(
            "frames requests from a carrier that is not an event matched by name alone - the frame's key is read \
             from the record the frame was read from, where no span context exists to qualify it",
        ));
    };
    let attribute = |source: &super::super::schema::SourceName| {
        source
            .0
            .strip_prefix("attr:")
            .filter(|key| !key.is_empty())
            .map(str::to_string)
    };
    let (Some(frame), Some(request)) = (attribute(&declared.frame), attribute(&declared.request))
    else {
        return Err(refuse(
            "names a frame or request key that is not `attr:<key>` - a key is an attribute both records state",
        ));
    };
    match &frames.request {
        Some(existing) if *existing != request => {
            return Err(refuse(
                "keys its requests by an attribute another framing carrier does not - a span is keyed once, so \
                 every framing carrier names the same request key",
            ));
        }
        _ => frames.request = Some(request),
    }
    frames.frame_by_event.insert(event.to_string(), frame);
    Ok(())
}
