//! AG-UI run-agent HTTP endpoint.
//!
//! `POST /api/v1/project/{project_id}/agents/{name}/runs`
//! Body: AG-UI `RunAgentInput` JSON (verbatim pass-through; SDK validates).
//! Response: `text/event-stream` of AG-UI events as `data: <json>\n\n`
//! frames (no `event:` line — matches the AG-UI encoder).
//!
//! Routes the call to whichever SDK owns `(project, kind=Agent, name)` via
//! the per-instance `connection_control` topic, fans replies back through a
//! per-request `agent_request:{request_id}` broadcast topic, synthesises a
//! terminal `RunErrorEvent` if the SDK fails before any AG-UI event flowed,
//! cancels the SDK on HTTP-client disconnect.
//!
//! See plan: /Users/spugachev/.claude/plans/sdk-python-polymorphic-biscuit.md

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::extractors::is_valid_project_id;
use crate::core::TopicError;
use crate::core::constants::{INVOKE_TIMEOUT_MS, WS_MAX_MESSAGE_BYTES};
use crate::data::registrations::ConnectionControl;

/// HTTP body limit for `/agents/{name}/runs`. Sized so that whatever
/// passes here also fits in the WS frame that carries
/// `ConnectionControl::Invoke{run_input}` to the SDK. We reserve 8 KiB
/// for envelope overhead (frame headers, request_id, agent_name, JSON
/// punctuation around the run_input value).
const AGUI_HTTP_BODY_LIMIT: usize = WS_MAX_MESSAGE_BYTES - 8 * 1024;

use super::ws::WsState;
use super::ws::invoke::{InvokeReply, invoke_topic_name};
use super::ws::protocol::ErrorCode;

#[derive(Debug, Deserialize)]
pub struct RunPath {
    pub project_id: String,
    pub name: String,
}

/// Build the AG-UI router. Uses the same `WsState` as the WS handler so
/// HTTP and WS share the registrations store + topics service.
pub fn routes(state: WsState) -> Router<()> {
    Router::new()
        .route(
            "/project/{project_id}/agents/{name}/runs",
            post(run_agent).layer(DefaultBodyLimit::max(AGUI_HTTP_BODY_LIMIT)),
        )
        .with_state(state)
}

/// RAII guard that publishes an `agent.cancel` to the SDK if the SSE
/// stream is dropped without a clean terminal event — typically when the
/// HTTP client disconnects (Ctrl-C on curl). Without this, the SDK keeps
/// the run alive until it ends naturally and the agent stays `busy`,
/// blocking subsequent invokes.
struct InvokeCancelGuard {
    state: WsState,
    owning_instance: String,
    target_client_id: String,
    request_id: String,
    armed: bool,
}

impl InvokeCancelGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InvokeCancelGuard {
    fn drop(&mut self) {
        // Always free reassembler partials for this request so memory isn't
        // pinned across the SSE close, regardless of whether the guard was
        // disarmed.
        self.state.reassembler.drop_request(&self.request_id);

        if !self.armed {
            return;
        }
        // We're in a sync `Drop`. Spawn the cancel publish only if a
        // tokio runtime is still around — otherwise (server shutdown
        // already tore it down) skip silently. `tokio::spawn` would panic.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(
                request_id = %self.request_id,
                "agui: skipping cancel publish; no tokio runtime in Drop"
            );
            return;
        };
        let state = self.state.clone();
        let owning = std::mem::take(&mut self.owning_instance);
        let target = std::mem::take(&mut self.target_client_id);
        let req = self.request_id.clone();
        handle.spawn(async move {
            publish_cancel(&state, &owning, &target, &req).await;
        });
    }
}

#[tracing::instrument(skip_all, fields(project_id, agent = %name, request_id))]
async fn run_agent(
    State(state): State<WsState>,
    Path(RunPath { project_id, name }): Path<RunPath>,
    Json(run_input): Json<serde_json::Value>,
) -> Result<impl IntoResponse, ErrorResponse> {
    tracing::Span::current().record("project_id", tracing::field::display(&project_id));
    if !is_valid_project_id(&project_id) {
        return Err(ErrorResponse::bad_request("invalid_project_id"));
    }
    if !run_input.is_object() {
        return Err(ErrorResponse::bad_request(
            "run_input must be a JSON object (RunAgentInput)",
        ));
    }

    // 1. Find the live registration. Resolves any invokable kind
    //    (agent → graph → swarm) by name; mcp is skipped server-side.
    let entry = state
        .registrations
        .find_by_name(&project_id, &name)
        .await
        .map_err(|e| ErrorResponse::internal(e.to_string()))?
        .ok_or_else(|| ErrorResponse::not_found("registration_not_found"))?;

    let request_id = Uuid::new_v4().to_string();
    tracing::Span::current().record("request_id", tracing::field::display(&request_id));
    let owning_instance = entry.owning_instance_id.clone();
    let target_client = entry.owner_client_id.clone();

    // 2. Subscribe BEFORE publishing the invoke control message so we
    //    don't race the SDK's first reply.
    let topic = state
        .topics
        .broadcast_topic::<InvokeReply>(&invoke_topic_name(&request_id));
    let mut sub = topic
        .subscribe()
        .await
        .map_err(|e| ErrorResponse::internal(format!("subscribe failed: {e}")))?;

    // 3. Publish the invoke onto the owning instance's control topic.
    let control = state
        .topics
        .broadcast_topic::<ConnectionControl>(&format!("connection_control:{}", owning_instance));
    if let Err(e) = control
        .publish(&ConnectionControl::Invoke {
            target_client_id: target_client.clone(),
            request_id: request_id.clone(),
            agent_name: name.clone(),
            run_input,
        })
        .await
    {
        return Err(ErrorResponse::internal(format!(
            "control publish failed: {e}"
        )));
    }

    // 4. Arm a cancel guard that fires `agent.cancel` to the SDK if the
    //    SSE stream is dropped without a clean terminal event. We disarm
    //    it inside the stream on every clean termination path so we
    //    don't double-cancel a finished run.
    let mut cancel_guard = InvokeCancelGuard {
        state: state.clone(),
        owning_instance: owning_instance.clone(),
        target_client_id: target_client.clone(),
        request_id: request_id.clone(),
        armed: true,
    };

    let invoke_timeout = invoke_timeout(&state);
    let mut shutdown_rx = state.shutdown_rx.clone();

    let stream = async_stream::stream! {
        let mut saw_terminal_event = false;
        let mut saw_any_event = false;
        let first_event_deadline = tokio::time::Instant::now() + invoke_timeout;

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        yield Ok::<Event, Infallible>(synth_run_error(
                            ErrorCode::Internal,
                            "server shutting down",
                        ));
                        break;
                    }
                }
                _ = tokio::time::sleep_until(first_event_deadline), if !saw_any_event => {
                    yield Ok::<Event, Infallible>(synth_run_error(
                        ErrorCode::InvokeTimeout,
                        "no SDK reply within timeout",
                    ));
                    // Leave the cancel guard armed: the SDK never replied,
                    // so it might still send the invoke results into the void;
                    // a cancel on its way nudges it to abort and free the
                    // `busy_agents` slot.
                    break;
                }
                result = sub.recv() => {
                    match result {
                        Ok(InvokeReply::Event(value)) => {
                            saw_any_event = true;
                            if is_terminal_agui_event(&value) {
                                saw_terminal_event = true;
                            }
                            match serde_json::to_string(&value) {
                                Ok(data) => yield Ok::<Event, Infallible>(Event::default().data(data)),
                                Err(e) => {
                                    tracing::warn!(error = %e, "agui: serialise event failed");
                                }
                            }
                            if saw_terminal_event {
                                cancel_guard.disarm();
                                break;
                            }
                        }
                        Ok(InvokeReply::Complete) => {
                            // Defensive: if the SDK omitted a terminal AG-UI
                            // event, synth one so AG-UI clients always see
                            // a proper RUN_FINISHED / RUN_ERROR.
                            if !saw_terminal_event {
                                yield Ok::<Event, Infallible>(synth_run_finished());
                            }
                            cancel_guard.disarm();
                            break;
                        }
                        Ok(InvokeReply::Error { code, message }) => {
                            if !saw_terminal_event {
                                yield Ok::<Event, Infallible>(synth_run_error(code, &message));
                            }
                            cancel_guard.disarm();
                            break;
                        }
                        Err(TopicError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "agui: subscriber lagged");
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        // Guard's Drop runs as the stream future is dropped (clean exit
        // OR client disconnect); see `InvokeCancelGuard::drop`.
        drop(cancel_guard);
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    // Disable buffering on common reverse proxies (nginx).
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    // We deliberately do NOT use Sse::keep_alive — its synthetic comments
    // confuse strict AG-UI clients that only expect `data:` frames.
    Ok((headers, Sse::new(stream)))
}

async fn publish_cancel(
    state: &WsState,
    owning_instance: &str,
    target_client_id: &str,
    request_id: &str,
) {
    let control = state
        .topics
        .broadcast_topic::<ConnectionControl>(&format!("connection_control:{}", owning_instance));
    if let Err(e) = control
        .publish(&ConnectionControl::Cancel {
            target_client_id: target_client_id.to_string(),
            request_id: request_id.to_string(),
        })
        .await
    {
        tracing::warn!(error = %e, "agui: cancel publish failed");
    }
}

/// Best-effort detection of an AG-UI terminal event by inspecting the
/// `type` field. AG-UI uses camelCase aliases.
fn is_terminal_agui_event(value: &serde_json::Value) -> bool {
    matches!(
        value.get("type").and_then(|v| v.as_str()),
        Some("RUN_FINISHED") | Some("RUN_ERROR")
    )
}

fn synth_run_error(code: ErrorCode, message: &str) -> Event {
    let payload = serde_json::json!({
        "type": "RUN_ERROR",
        "message": message,
        "code": serde_json::to_string(&code).unwrap_or_default().trim_matches('"').to_string(),
    });
    Event::default().data(payload.to_string())
}

fn synth_run_finished() -> Event {
    let payload = serde_json::json!({
        "type": "RUN_FINISHED",
    });
    Event::default().data(payload.to_string())
}

// ============================================================================
// Error response shape
// ============================================================================

pub struct ErrorResponse {
    status: StatusCode,
    code: String,
    message: String,
}

impl ErrorResponse {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request".into(),
            message: message.into(),
        }
    }
    fn not_found(code: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: code.into(),
            message: "agent not registered for this project".into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal".into(),
            message: message.into(),
        }
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "code": self.code,
            "message": self.message,
        });
        (self.status, Json(body)).into_response()
    }
}

/// Resolve the per-invoke timeout, honouring a test override on `WsState`.
/// Production keeps `invoke_timeout_override` as `None` and the const wins.
fn invoke_timeout(state: &WsState) -> Duration {
    state
        .invoke_timeout_override
        .unwrap_or_else(|| Duration::from_millis(INVOKE_TIMEOUT_MS))
}
