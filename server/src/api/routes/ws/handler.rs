//! WebSocket handler for `/api/v1/project/{project_id}/ws`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::extract::Path;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use futures::SinkExt;
use futures::stream::StreamExt;
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::extractors::is_valid_project_id;
use crate::core::constants::{
    WS_FRAME_RATE_LIMIT_COUNT, WS_FRAME_RATE_LIMIT_WINDOW_SECS, WS_HEARTBEAT_INTERVAL_SECS,
    WS_HELLO_TIMEOUT_SECS, WS_MAX_MESSAGE_BYTES, WS_PONG_GRACE_SECS,
};
use crate::data::registrations::{
    ConnectionControl, DisplacedOwner, PresenceEvent, RegistrationEntry, RegistrationKind,
    RegistrationManifest, UpsertOutcome,
};

use super::invoke::{InvokeReply, publish_invoke_reply};
use super::presence;
use super::protocol::{
    AckPayload, AgentCancelPayload, AgentInvokePayload, Envelope, ErrorCode, ErrorPayload,
    FrameParseError, InboundFrame, PingPayload, PongPayload, ReplacedPayload, WelcomePayload,
    frame_string, parse_inbound,
};
use super::rate_limit::RateLimiter;
use super::state::{ConnectionHandle, WsState};

/// Build a frame with a fresh UUID id.
fn fresh_frame<P: serde::Serialize>(r#type: &str, payload: &P) -> String {
    frame_string(r#type, &Uuid::new_v4().to_string(), payload)
}

/// Build a connection_control topic name. Single source of truth for the
/// `connection_control:{instance_id}` convention used in two places.
fn connection_control_topic(instance_id: &str) -> String {
    format!("connection_control:{}", instance_id)
}

#[derive(Debug, Deserialize)]
pub struct ProjectPath {
    pub project_id: String,
}

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Axum handler.
pub async fn ws_upgrade(
    State(state): State<WsState>,
    Path(ProjectPath { project_id }): Path<ProjectPath>,
    ws: WebSocketUpgrade,
) -> Result<Response, (StatusCode, String)> {
    if !is_valid_project_id(&project_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid_project_id".into()));
    }
    let configured = ws.max_message_size(WS_MAX_MESSAGE_BYTES);
    Ok(configured.on_upgrade(move |socket| run_connection(socket, state, project_id)))
}

async fn run_connection(socket: WebSocket, state: WsState, project_id: String) {
    let connection_id = state.make_connection_id();
    let (mut sender, mut receiver) = socket.split();

    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);

    let handle = Arc::new(ConnectionHandle {
        connection_id: connection_id.clone(),
        project_id: project_id.clone(),
        client_id: Mutex::new(None),
        outbound: out_tx.clone(),
    });
    state
        .connections
        .insert(connection_id.clone(), handle.clone());

    // Spawn writer task: drains the outbound channel into the socket.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
        let _ = sender.close().await;
    });

    // Send welcome.
    let welcome = fresh_frame(
        "welcome",
        &WelcomePayload {
            connection_id: connection_id.clone(),
            server_version: SERVER_VERSION.to_string(),
            max_message_bytes: WS_MAX_MESSAGE_BYTES,
            heartbeat_interval_secs: WS_HEARTBEAT_INTERVAL_SECS,
        },
    );
    if out_tx.send(welcome).await.is_err() {
        cleanup(&state, &handle).await;
        writer.abort();
        return;
    }

    // Subscribe to the per-instance control topic to receive `replaced`
    // notices for sockets we own.
    let control_topic = state
        .topics
        .broadcast_topic::<ConnectionControl>(&connection_control_topic(&state.instance_id));
    let mut control_sub = match control_topic.subscribe().await {
        Ok(sub) => Some(sub),
        Err(e) => {
            tracing::warn!(error = %e, "ws: failed to subscribe to control topic");
            None
        }
    };

    let mut rl = RateLimiter::new(WS_FRAME_RATE_LIMIT_COUNT, WS_FRAME_RATE_LIMIT_WINDOW_SECS);
    let mut shutdown_rx = state.shutdown_rx.clone();
    let mut hello_received = false;
    let hello_deadline = tokio::time::Instant::now() + Duration::from_secs(WS_HELLO_TIMEOUT_SECS);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(WS_HEARTBEAT_INTERVAL_SECS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = tokio::time::Instant::now();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = tokio::time::sleep_until(hello_deadline), if !hello_received => {
                send_error(
                    &out_tx,
                    None,
                    ErrorCode::HelloRequired,
                    "hello required within 5s",
                )
                .await;
                break;
            }
            _ = heartbeat.tick() => {
                // Reuse the same UUID as both envelope id and ping payload id
                // so the SDK can correlate pongs.
                let id = Uuid::new_v4().to_string();
                let frame = frame_string("ping", &id, &PingPayload { id: id.clone() });
                // try_send avoids piling pings onto a backed-up writer; if the
                // channel is full the writer is misbehaving and we'll catch it
                // via the pong-timeout path below anyway.
                if let Err(e) = out_tx.try_send(frame) {
                    tracing::debug!(error = %e, "ws: outbound full or closed; closing");
                    break;
                }
                if last_pong.elapsed()
                    > Duration::from_secs(WS_HEARTBEAT_INTERVAL_SECS + WS_PONG_GRACE_SECS)
                {
                    tracing::debug!(connection_id = %connection_id, "ws: pong timeout");
                    break;
                }
            }
            ctl = recv_control(&mut control_sub) => {
                if let Some(msg) = ctl {
                    handle_control(&state, &msg).await;
                }
            }
            inbound = receiver.next() => {
                let Some(message) = inbound else { break };
                let message = match message {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(error = %e, "ws: recv error");
                        break;
                    }
                };
                match message {
                    Message::Text(text) => {
                        let env: Envelope = match serde_json::from_str(&text) {
                            Ok(e) => e,
                            Err(e) => {
                                send_error(
                                    &out_tx,
                                    None,
                                    ErrorCode::BadPayload,
                                    format!("envelope parse: {e}"),
                                )
                                .await;
                                continue;
                            }
                        };
                        let parsed = match parse_inbound(&env) {
                            Ok(p) => p,
                            Err(FrameParseError::BadPayload(msg)) => {
                                send_error(
                                    &out_tx,
                                    Some(env.id.clone()),
                                    ErrorCode::BadPayload,
                                    msg,
                                )
                                .await;
                                continue;
                            }
                        };
                        // `agent.event` frames are exempt — they are
                        // SDK→server fan-out replies for in-flight invokes,
                        // not user-initiated traffic. All other frame types
                        // burn a token from the per-connection bucket.
                        if !matches!(parsed, InboundFrame::AgentEvent(_)) && !rl.allow() {
                            send_error(
                                &out_tx,
                                None,
                                ErrorCode::RateLimited,
                                "frame rate limit exceeded",
                            )
                            .await;
                            continue;
                        }
                        match parsed {
                            InboundFrame::Hello(payload) => {
                                if hello_received {
                                    send_error(
                                        &out_tx,
                                        Some(env.id.clone()),
                                        ErrorCode::BadPayload,
                                        "hello already received",
                                    )
                                    .await;
                                    continue;
                                }
                                hello_received = true;
                                *handle.client_id.lock() = Some(payload.client_id.clone());
                                send_ack(&out_tx, &env.id).await;
                            }
                            _ if !hello_received => {
                                send_error(
                                    &out_tx,
                                    Some(env.id.clone()),
                                    ErrorCode::HelloRequired,
                                    "hello required first",
                                )
                                .await;
                            }
                            InboundFrame::Register(kind, manifest) => {
                                handle_register(
                                    &state, &handle, &out_tx, &env.id, kind, manifest,
                                )
                                .await;
                            }
                            InboundFrame::Unregister(kind, payload) => {
                                handle_unregister(
                                    &state,
                                    &handle,
                                    &out_tx,
                                    &env.id,
                                    kind,
                                    &payload.name,
                                )
                                .await;
                            }
                            InboundFrame::Pong(PongPayload { .. }) => {
                                last_pong = tokio::time::Instant::now();
                                let cid = handle.client_id.lock().clone();
                                if let Some(cid) = cid
                                    && let Err(e) = state.registrations.touch(&cid).await
                                {
                                    tracing::debug!(error = %e, "ws: touch failed");
                                }
                            }
                            InboundFrame::AgentEvent(payload) => {
                                publish_invoke_reply(
                                    &state,
                                    &payload.request_id,
                                    InvokeReply::Event(payload.event),
                                )
                                .await;
                            }
                            InboundFrame::AgentEventChunk(payload) => {
                                let request_id = payload.request_id.clone();
                                match state.reassembler.feed(payload) {
                                    super::chunks::FeedOutcome::Pending => {}
                                    super::chunks::FeedOutcome::Complete(value) => {
                                        publish_invoke_reply(
                                            &state,
                                            &request_id,
                                            InvokeReply::Event(value),
                                        )
                                        .await;
                                    }
                                    super::chunks::FeedOutcome::Failed(err) => {
                                        tracing::warn!(
                                            error = %err,
                                            request_id = %request_id,
                                            "ws: chunk reassembly failed",
                                        );
                                        // 1. Surface the failure to the SSE
                                        //    consumer so it sees a terminal
                                        //    AG-UI RUN_ERROR.
                                        publish_invoke_reply(
                                            &state,
                                            &request_id,
                                            InvokeReply::Error {
                                                code: ErrorCode::TooLarge,
                                                message: err.to_string(),
                                            },
                                        )
                                        .await;
                                        // 2. Tell the SDK to abort this
                                        //    invoke so it stops streaming
                                        //    into the void and frees its
                                        //    `busy_agents` slot for the
                                        //    next request.
                                        let frame = fresh_frame(
                                            "agent.cancel",
                                            &AgentCancelPayload {
                                                request_id: request_id.clone(),
                                            },
                                        );
                                        if let Err(e) = out_tx.try_send(frame) {
                                            tracing::debug!(
                                                error = %e,
                                                "ws: cancel after reassembly fail dropped",
                                            );
                                        }
                                        // 3. Drop any further partials we
                                        //    might still be holding for
                                        //    this request.
                                        state.reassembler.drop_request(&request_id);
                                    }
                                }
                            }
                            InboundFrame::AgentComplete(payload) => {
                                state.reassembler.drop_request(&payload.request_id);
                                publish_invoke_reply(
                                    &state,
                                    &payload.request_id,
                                    InvokeReply::Complete,
                                )
                                .await;
                            }
                            InboundFrame::AgentError(payload) => {
                                state.reassembler.drop_request(&payload.request_id);
                                publish_invoke_reply(
                                    &state,
                                    &payload.request_id.clone(),
                                    InvokeReply::Error {
                                        code: payload.code,
                                        message: payload.message,
                                    },
                                )
                                .await;
                            }
                            InboundFrame::Unknown(t) => {
                                send_error(
                                    &out_tx,
                                    Some(env.id.clone()),
                                    ErrorCode::Unsupported,
                                    format!("unsupported frame type: {t}"),
                                )
                                .await;
                            }
                        }
                    }
                    Message::Binary(_) => {
                        send_error(&out_tx, None, ErrorCode::BadPayload, "binary frames unsupported").await;
                    }
                    Message::Ping(p) => {
                        // axum/tungstenite respond automatically; nothing to do.
                        let _ = p;
                    }
                    Message::Pong(_) => {
                        // protocol-level pong; ignore (we use app-level pongs).
                    }
                    Message::Close(_) => break,
                }
            }
        }
    }

    cleanup(&state, &handle).await;
    drop(out_tx);
    let _ = writer.await;
}

async fn recv_control(
    sub: &mut Option<crate::data::topics::BroadcastTopicSubscriber<ConnectionControl>>,
) -> Option<ConnectionControl> {
    if let Some(s) = sub.as_mut() {
        s.recv().await.ok()
    } else {
        std::future::pending().await
    }
}

async fn handle_control(state: &WsState, msg: &ConnectionControl) {
    match msg {
        ConnectionControl::Replaced {
            target_client_id,
            kind,
            name,
        } => {
            for h in find_local_connections_for_client(state, target_client_id) {
                let frame = fresh_frame(
                    "replaced",
                    &ReplacedPayload {
                        kind: *kind,
                        name: name.clone(),
                    },
                );
                let _ = h.outbound.send(frame).await;
                // The connection task closes its socket once it observes the
                // channel drop or recv-loop end.
            }
        }
        ConnectionControl::Invoke {
            target_client_id,
            request_id,
            agent_name,
            run_input,
        } => {
            for h in find_local_connections_for_client(state, target_client_id) {
                let frame = fresh_frame(
                    "agent.invoke",
                    &AgentInvokePayload {
                        request_id: request_id.clone(),
                        agent_name: agent_name.clone(),
                        run_input: run_input.clone(),
                    },
                );
                let _ = h.outbound.send(frame).await;
            }
        }
        ConnectionControl::Cancel {
            target_client_id,
            request_id,
        } => {
            for h in find_local_connections_for_client(state, target_client_id) {
                let frame = fresh_frame(
                    "agent.cancel",
                    &AgentCancelPayload {
                        request_id: request_id.clone(),
                    },
                );
                let _ = h.outbound.send(frame).await;
            }
        }
    }
}

fn find_local_connections_for_client(
    state: &WsState,
    target_client_id: &str,
) -> Vec<Arc<ConnectionHandle>> {
    state
        .connections
        .iter()
        .filter_map(|entry| {
            let h = entry.value().clone();
            let same = h
                .client_id
                .lock()
                .as_deref()
                .map(|id| id == target_client_id)
                .unwrap_or(false);
            if same { Some(h) } else { None }
        })
        .collect()
}

async fn handle_register(
    state: &WsState,
    handle: &Arc<ConnectionHandle>,
    out_tx: &mpsc::Sender<String>,
    ref_id: &str,
    kind: RegistrationKind,
    manifest: RegistrationManifest,
) {
    let Some(client_id) = handle.client_id.lock().clone() else {
        send_error(
            out_tx,
            Some(ref_id.to_string()),
            ErrorCode::HelloRequired,
            "hello required first",
        )
        .await;
        return;
    };

    if manifest.name.trim().is_empty() {
        send_error(
            out_tx,
            Some(ref_id.to_string()),
            ErrorCode::BadPayload,
            "manifest.name is empty",
        )
        .await;
        return;
    }

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    let entry = RegistrationEntry {
        project_id: handle.project_id.clone(),
        kind,
        name: manifest.name.clone(),
        manifest,
        owner_client_id: client_id.clone(),
        owning_instance_id: state.instance_id.as_str().to_string(),
        last_heartbeat_secs: now_secs,
    };

    // Single clone into the store; subsequent references re-use `entry`.
    let outcome = state.registrations.upsert(entry.clone()).await;
    match outcome {
        Ok(UpsertOutcome::Inserted) | Ok(UpsertOutcome::UpdatedSameOwner) => {
            send_ack(out_tx, ref_id).await;
            presence::publish(state, &PresenceEvent::Registered(entry)).await;
        }
        Ok(UpsertOutcome::Replaced(prev)) => {
            send_ack(out_tx, ref_id).await;
            // Tell the previous owner's instance to displace its socket.
            let control_topic = state
                .topics
                .broadcast_topic::<ConnectionControl>(&connection_control_topic(&prev.instance_id));
            if let Err(e) = control_topic
                .publish(&ConnectionControl::Replaced {
                    target_client_id: prev.client_id.clone(),
                    kind,
                    name: entry.name.clone(),
                })
                .await
            {
                tracing::warn!(error = %e, "ws: connection_control publish failed");
            }
            let new_owner = DisplacedOwner {
                client_id,
                instance_id: state.instance_id.as_str().to_string(),
            };
            presence::publish(
                state,
                &PresenceEvent::Replaced {
                    project_id: entry.project_id.clone(),
                    kind: entry.kind,
                    name: entry.name.clone(),
                    prev_owner: prev,
                    new_owner,
                },
            )
            .await;
        }
        Err(e) => {
            tracing::error!(error = %e, "ws: upsert failed");
            send_error(
                out_tx,
                Some(ref_id.into()),
                ErrorCode::Internal,
                "store error",
            )
            .await;
        }
    }
}

async fn handle_unregister(
    state: &WsState,
    handle: &Arc<ConnectionHandle>,
    out_tx: &mpsc::Sender<String>,
    ref_id: &str,
    kind: RegistrationKind,
    name: &str,
) {
    let Some(client_id) = handle.client_id.lock().clone() else {
        send_error(
            out_tx,
            Some(ref_id.to_string()),
            ErrorCode::HelloRequired,
            "hello required first",
        )
        .await;
        return;
    };

    match state
        .registrations
        .remove(&handle.project_id, kind, name, &client_id)
        .await
    {
        Ok(Some(entry)) => {
            send_ack(out_tx, ref_id).await;
            presence::publish(state, &unregistered_event(entry)).await;
        }
        Ok(None) => {
            // Idempotent: unknown name is fine.
            tracing::debug!(name = %name, "ws: unregister of unknown entry (or wrong owner)");
            send_ack(out_tx, ref_id).await;
        }
        Err(e) => {
            tracing::error!(error = %e, "ws: remove failed");
            send_error(
                out_tx,
                Some(ref_id.into()),
                ErrorCode::Internal,
                "store error",
            )
            .await;
        }
    }
}

async fn cleanup(state: &WsState, handle: &Arc<ConnectionHandle>) {
    state.connections.remove(&handle.connection_id);

    let client_id = match handle.client_id.lock().clone() {
        Some(id) => id,
        None => return,
    };

    let removed = match state.registrations.remove_all_for_client(&client_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "ws: cleanup remove_all failed");
            return;
        }
    };
    for entry in removed {
        presence::publish(state, &unregistered_event(entry)).await;
    }
}

fn unregistered_event(entry: RegistrationEntry) -> PresenceEvent {
    PresenceEvent::Unregistered {
        project_id: entry.project_id,
        kind: entry.kind,
        name: entry.name,
        owner: DisplacedOwner {
            client_id: entry.owner_client_id,
            instance_id: entry.owning_instance_id,
        },
    }
}

async fn send_ack(out: &mpsc::Sender<String>, ref_id: &str) {
    let frame = fresh_frame(
        "ack",
        &AckPayload {
            ref_id: ref_id.to_string(),
        },
    );
    let _ = out.send(frame).await;
}

async fn send_error(
    out: &mpsc::Sender<String>,
    ref_id: Option<String>,
    code: ErrorCode,
    message: impl Into<String>,
) {
    let frame = fresh_frame(
        "error",
        &ErrorPayload {
            code,
            message: message.into(),
            ref_id,
        },
    );
    let _ = out.send(frame).await;
}
