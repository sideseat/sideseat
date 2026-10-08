//! Edge admission: a budget on the OTLP bytes in flight, taken before a body is read.
//!
//! Every export the server is reading, decoding or persisting holds its body's bytes of one process-wide budget,
//! shared by HTTP and gRPC, until it is answered. An export the budget cannot hold is answered at once - HTTP
//! `503 Service Unavailable` with `Retry-After`, gRPC `UNAVAILABLE` - before any of its body is read, and OTLP
//! exporters retry both. Without it the bytes held were bounded only by how many connections clients opened:
//! each could carry a body up to the 64 MB limit, read whole before the pipeline's own budgets applied.
//!
//! An export larger than the whole budget is admitted only when nothing else is in flight, so the largest body
//! the routes accept is never refused for good, and the bytes held never exceed the budget or that one body.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderName, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use sideseat_core::constants::{BACKPRESSURE_RETRY_AFTER_SECS, OTLP_BODY_LIMIT};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The process's budget of OTLP body bytes in flight.
#[derive(Debug)]
pub struct IngestAdmission {
    permits: Arc<Semaphore>,
    budget: usize,
}

/// The bytes one admitted export holds, released when it is answered.
#[derive(Debug)]
pub struct Admitted {
    _held: Option<OwnedSemaphorePermit>,
}

impl IngestAdmission {
    /// A budget of `budget_bytes`, at least one byte.
    pub fn new(budget_bytes: u64) -> Self {
        let budget = usize::try_from(budget_bytes)
            .unwrap_or(usize::MAX)
            .clamp(1, Semaphore::MAX_PERMITS);
        Self {
            permits: Arc::new(Semaphore::new(budget)),
            budget,
        }
    }

    /// The budget, in bytes.
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Hold `bytes` more of the budget for an export that already holds `held`, or nothing if the budget cannot.
    ///
    /// What an export holds is capped at the whole budget, so a body larger than it waits for nothing else to be
    /// in flight rather than for a budget that can never be enough.
    fn try_hold(&self, held: usize, bytes: usize) -> Option<Option<OwnedSemaphorePermit>> {
        let wanted = held
            .saturating_add(bytes)
            .min(self.budget)
            .saturating_sub(held);
        if wanted == 0 {
            return Some(None);
        }
        let wanted = u32::try_from(wanted).ok()?;
        Arc::clone(&self.permits)
            .try_acquire_many_owned(wanted)
            .ok()
            .map(Some)
    }

    /// Admit an export of `bytes`, or refuse it.
    pub fn try_admit(&self, bytes: usize) -> Option<Admitted> {
        self.try_hold(0, bytes).map(|held| Admitted { _held: held })
    }

    /// Bytes of the budget held now.
    pub fn in_flight(&self) -> usize {
        self.budget - self.permits.available_permits()
    }
}

/// The HTTP answer to an export the budget cannot hold.
fn busy() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (
                HeaderName::from_static("retry-after"),
                BACKPRESSURE_RETRY_AFTER_SECS.to_string(),
            ),
            (header::CONTENT_TYPE, "text/plain".to_string()),
        ],
        "OTLP ingest is holding as many bytes as it may; retry",
    )
        .into_response()
}

fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        [(header::CONTENT_TYPE, "text/plain")],
        "OTLP export larger than the body limit",
    )
        .into_response()
}

/// Admit an OTLP HTTP export before its body is read, and hold its bytes until it is answered.
///
/// A body with a declared length is admitted by it, unread. One without is read here, its bytes held as they
/// arrive, and refused when the budget runs out or the body passes the route's limit.
pub async fn admit_http(
    State(admission): State<Arc<IngestAdmission>>,
    request: Request,
    next: Next,
) -> Response {
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if let Some(length) = declared {
        if length > OTLP_BODY_LIMIT {
            return too_large();
        }
        let Some(admitted) = admission.try_admit(length) else {
            return busy();
        };
        let response = next.run(request).await;
        drop(admitted);
        return response;
    }
    let (parts, body) = request.into_parts();
    let mut stream = body.into_data_stream();
    let mut chunks: Vec<Bytes> = Vec::new();
    let mut held: Option<OwnedSemaphorePermit> = None;
    let mut read = 0usize;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return (
                StatusCode::BAD_REQUEST,
                "could not read the OTLP export body",
            )
                .into_response();
        };
        if read.saturating_add(chunk.len()) > OTLP_BODY_LIMIT {
            return too_large();
        }
        let Some(more) = admission.try_hold(read, chunk.len()) else {
            return busy();
        };
        read += chunk.len();
        if let Some(more) = more {
            match held.as_mut() {
                Some(permit) => permit.merge(more),
                None => held = Some(more),
            }
        }
        chunks.push(chunk);
    }
    let body = match chunks.len() {
        0 => Body::empty(),
        1 => Body::from(chunks.pop().expect("one chunk")),
        _ => Body::from(chunks.concat()),
    };
    let response = next.run(Request::from_parts(parts, body)).await;
    drop(held);
    response
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
