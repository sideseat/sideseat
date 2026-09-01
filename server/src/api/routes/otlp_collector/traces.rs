//! Traces export endpoint

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value;

use super::encoding::{OtlpContentType, decode_request, success_response};
use super::{OtlpState, inject_project_id_traces};
use crate::api::extractors::is_valid_project_id;
use crate::core::constants::BACKPRESSURE_RETRY_AFTER_SECS;
use crate::domain::traces::{DropReason, IngestOutcome, strip_unstorable_spans};
use crate::utils::debug::write_debug;
use crate::utils::otlp::PROJECT_ID_ATTR;

/// Maximum retry attempts for trace publish
const PUBLISH_MAX_ATTEMPTS: u32 = 3;

/// Base delay in milliseconds for exponential backoff
const PUBLISH_BASE_DELAY_MS: u64 = 50;

pub async fn export(
    State(state): State<OtlpState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Validate project_id
    if !is_valid_project_id(&project_id) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain")],
            "Invalid project_id",
        )
            .into_response();
    }

    // Say so now, rather than accepting the request and dropping its spans at the write. The write path
    // is where the fence is authoritative; this is where the exporter can still be told.
    if !super::project_accepts_writes(&state, &project_id).await {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "Unknown project, or it is being deleted",
        )
            .into_response();
    }

    let content_type = OtlpContentType::from_headers(&headers);

    // Parse request (protobuf or JSON based on content type)
    let mut request: ExportTraceServiceRequest = match decode_request(&body, content_type) {
        Ok(req) => req,
        Err(e) => return e.into_response(content_type),
    };

    // Check for existing project_id in request and log if mismatched
    check_project_id_mismatch(&request, &project_id);

    // Inject project_id into resource attributes (path takes precedence)
    inject_project_id_traces(&mut request, &project_id);

    // Write to debug file if debug mode is enabled
    if let Some(ref debug_path) = state.debug_path {
        write_debug(debug_path, "traces.jsonl", &project_id, &request).await;
    }

    // Acknowledge only what is durable.
    //
    // The queue below is at-least-once *when the topic backend is a Redis stream*: a message is held
    // until acknowledged and an unacknowledged one is reclaimed, so answering before the write is a
    // promise the deployment can keep. The default backend is in memory, where it is not: a crash between
    // the answer and the batch write loses traces the exporter has counted as delivered. So with such a
    // backend the request writes first and answers second - measured at about 2.3 milliseconds warm,
    // which is not a cost worth a lost trace.
    if !state.durable_queue
        && let Some(pipeline) = state.trace_pipeline.as_ref()
    {
        match pipeline.ingest_now(&request).await {
            IngestOutcome::Stored => {}
            // The project stopped accepting writes between the check above and the write. Reported, not
            // swallowed: a success for records that were discarded is the failure mode this whole path is
            // about.
            // Nothing was stored, and *why* decides the answer.
            //
            // Every drop used to be a 404 saying "unknown project", which is a lie for a live project whose
            // span simply cannot be stored - and an expensive one: 404 tells the exporter to retry elsewhere,
            // so it retried the same doomed payload indefinitely while its operator hunted a project that was
            // in fact healthy.
            IngestOutcome::Dropped { spans, reason } => match reason {
                DropReason::Gone => {
                    return (
                        StatusCode::NOT_FOUND,
                        [(header::CONTENT_TYPE, "text/plain")],
                        "Unknown project, trace or session, or it is being deleted",
                    )
                        .into_response();
                }
                // The project is fine and a retry cannot help, so this is a success that reports the
                // rejection through the field OTLP provides for it.
                DropReason::Unstorable => {
                    let response = ExportTraceServiceResponse {
                        partial_success: Some(ExportTracePartialSuccess {
                            rejected_spans: spans as i64,
                            error_message:
                                "spans were rejected as unstorable; check their timestamps are \
                                            within 1900-2299"
                                    .to_string(),
                        }),
                    };
                    return success_response(&response, content_type);
                }
            },
            // Something *was* stored, so this is a success that reports what it rejected - the field OTLP
            // provides for exactly this. A batch naming a live project and a dying one must not lose the
            // live one's spans to a refusal, nor have the dying one's counted as delivered.
            IngestOutcome::PartlyDropped { spans } => {
                let response = ExportTraceServiceResponse {
                    partial_success: Some(ExportTracePartialSuccess {
                        rejected_spans: spans as i64,
                        error_message: "some spans' project is unknown or is being deleted"
                            .to_string(),
                    }),
                };
                return success_response(&response, content_type);
            }
            IngestOutcome::Failed => {
                tracing::error!(%project_id, "Failed to store traces");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(
                        HeaderName::from_static("retry-after"),
                        BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
                    )],
                )
                    .into_response();
            }
        }
        let response = ExportTraceServiceResponse {
            partial_success: None,
        };
        return success_response(&response, content_type);
    }

    // Unstorable spans are rejected *here*, before the queue answers 200 for them.
    //
    // A durable queue's 200 means "safely published", which is the right promise - but the consumer later
    // discards a span whose timestamp no backend can store, and nothing downstream can report back to a
    // request that has already returned. So a live project exporting a year-2300 timestamp got an unqualified
    // success and then found nothing stored. Storability depends only on the payload, so it is settled and
    // reported now; only what can be stored is queued.
    let unstorable = strip_unstorable_spans(&mut request);
    let remaining_spans: usize = request
        .resource_spans
        .iter()
        .flat_map(|r| r.scope_spans.iter())
        .map(|s| s.spans.len())
        .sum();
    if unstorable > 0 && remaining_spans == 0 {
        // Nothing left to queue. A retry cannot help, and the project is fine, so this is a success that
        // reports the rejection rather than a 404 blaming the project.
        let response = ExportTraceServiceResponse {
            partial_success: Some(ExportTracePartialSuccess {
                rejected_spans: unstorable as i64,
                error_message:
                    "spans were rejected as unstorable; check their timestamps are within \
                                1900-2299"
                        .to_string(),
            }),
        };
        return success_response(&response, content_type);
    }

    // Publish to stream topic with retry (at-least-once delivery)
    let mut last_error = None;
    for attempt in 1..=PUBLISH_MAX_ATTEMPTS {
        match state.trace_topic.publish(&request).await {
            Ok(_) => {
                if attempt > 1 {
                    tracing::debug!(attempt, "Trace publish succeeded after retry");
                }
                last_error = None;
                break;
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < PUBLISH_MAX_ATTEMPTS {
                    let delay =
                        Duration::from_millis(PUBLISH_BASE_DELAY_MS * 2_u64.pow(attempt - 1));
                    tracing::warn!(
                        error = %last_error.as_ref().unwrap(),
                        attempt,
                        delay_ms = delay.as_millis(),
                        "Retrying trace publish after transient error"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    if let Some(e) = last_error {
        tracing::warn!(error = %e, attempts = PUBLISH_MAX_ATTEMPTS, "Failed to publish traces after retries");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                HeaderName::from_static("retry-after"),
                BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
            )],
        )
            .into_response();
    }

    // Return OTLP-compliant response (matching request content type).
    //
    // Reporting the spans stripped above, if any: the rest were queued, so this is a success that says what
    // it would not take rather than an unqualified one that loses them silently.
    let response = ExportTraceServiceResponse {
        partial_success: (unstorable > 0).then(|| ExportTracePartialSuccess {
            rejected_spans: unstorable as i64,
            error_message:
                "some spans were rejected as unstorable; check their timestamps are within 1900-2299"
                    .to_string(),
        }),
    };
    success_response(&response, content_type)
}

/// Check if request contains a project_id that mismatches the path project_id
fn check_project_id_mismatch(request: &ExportTraceServiceRequest, path_project_id: &str) {
    for resource_spans in &request.resource_spans {
        if let Some(ref resource) = resource_spans.resource {
            for attr in &resource.attributes {
                if attr.key == PROJECT_ID_ATTR
                    && let Some(ref value) = attr.value
                    && let Some(any_value::Value::StringValue(existing_id)) = &value.value
                    && existing_id != path_project_id
                {
                    tracing::warn!(
                        path_project_id,
                        request_project_id = %existing_id,
                        "Project ID mismatch: request contains different project_id than URL path"
                    );
                }
            }
        }
    }
}
