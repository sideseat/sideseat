use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use sideseat_ports::types::{
    ContentBodyObject, NormalizedSpan, ProjectId, SpanBodyAssociation, SpanBodyField,
    SpanBodySource,
};

use super::{BodyBytes, CollectedBodies, ContentBodyService};

pub(super) fn collect(spans: &[NormalizedSpan]) -> CollectedBodies {
    let mut objects = BTreeMap::new();
    let mut associations = Vec::new();
    let mut seen = HashSet::new();
    let mut content_digests = HashMap::new();

    for span in spans {
        let project = span
            .project_id
            .as_deref()
            .unwrap_or(sideseat_core::constants::DEFAULT_PROJECT_ID);
        let project_id = ProjectId::from(project);
        let identity = (
            project.to_string(),
            span.trace_id.clone(),
            span.span_id.clone(),
        );
        content_digests.insert(identity, span.content_digest.clone());
        for (field, body) in [
            (SpanBodyField::Messages, span.messages.as_deref()),
            (
                SpanBodyField::ToolDefinitions,
                span.tool_definitions.as_deref(),
            ),
            (SpanBodyField::ToolNames, span.tool_names.as_deref()),
            (SpanBodyField::RawSpan, span.raw_span.as_deref()),
        ] {
            let Some(body) = body else {
                continue;
            };
            let hash = ContentBodyService::hash(body.as_bytes());
            let key = (project.to_string(), hash.clone());
            objects.entry(key).or_insert_with(|| BodyBytes {
                object: ContentBodyObject {
                    project_id: project_id.clone(),
                    body_hash: hash.clone(),
                    logical_bytes: body.len() as u64,
                },
                bytes: Arc::from(body.as_bytes()),
            });
            let association_key = (
                project.to_string(),
                span.trace_id.clone(),
                span.span_id.clone(),
                field,
                hash.clone(),
            );
            if seen.insert(association_key) {
                associations.push(SpanBodyAssociation {
                    project_id: project_id.clone(),
                    trace_id: span.trace_id.clone(),
                    span_id: span.span_id.clone(),
                    field,
                    body_hash: hash,
                    logical_bytes: body.len() as u64,
                });
            }
        }
    }
    (objects, associations, content_digests)
}

pub(super) fn collect_sources(
    project_id: &ProjectId,
    sources: &[SpanBodySource],
) -> (
    BTreeMap<(String, String), BodyBytes>,
    Vec<SpanBodyAssociation>,
) {
    let mut objects = BTreeMap::new();
    let mut associations = Vec::new();
    let mut seen = HashSet::new();
    for source in sources {
        for (field, body) in [
            (SpanBodyField::Messages, source.messages.as_deref()),
            (
                SpanBodyField::ToolDefinitions,
                source.tool_definitions.as_deref(),
            ),
            (SpanBodyField::ToolNames, source.tool_names.as_deref()),
            (SpanBodyField::RawSpan, source.raw_span.as_deref()),
        ] {
            let Some(body) = body else {
                continue;
            };
            let hash = ContentBodyService::hash(body.as_bytes());
            objects
                .entry((project_id.to_string(), hash.clone()))
                .or_insert_with(|| BodyBytes {
                    object: ContentBodyObject {
                        project_id: project_id.clone(),
                        body_hash: hash.clone(),
                        logical_bytes: body.len() as u64,
                    },
                    bytes: Arc::from(body.as_bytes()),
                });
            if seen.insert((
                source.trace_id.clone(),
                source.span_id.clone(),
                field,
                hash.clone(),
            )) {
                associations.push(SpanBodyAssociation {
                    project_id: project_id.clone(),
                    trace_id: source.trace_id.clone(),
                    span_id: source.span_id.clone(),
                    field,
                    body_hash: hash,
                    logical_bytes: body.len() as u64,
                });
            }
        }
    }
    (objects, associations)
}
