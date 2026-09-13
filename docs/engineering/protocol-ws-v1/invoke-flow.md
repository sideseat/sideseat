# AG-UI invoke flow — state machines

End-to-end view of one invocation, from frontend HTTP `POST` through
the server WS bridge to the SDK worker thread and back. Each diagram
maps states a single component traverses; transitions are labelled
with the event that fires them.

## 1. HTTP SSE handler (`server/src/api/routes/agui/mod.rs::run_agent`)

```mermaid
stateDiagram-v2
  [*] --> Validating
  Validating --> Failed_400: bad project_id / non-object body
  Validating --> Looking_up
  Looking_up --> Failed_404: registration missing
  Looking_up --> Failed_500: store error
  Looking_up --> Subscribing
  Subscribing --> Failed_500: subscribe error
  Subscribing --> Publishing_invoke
  Publishing_invoke --> Failed_500: publish error
  Publishing_invoke --> Streaming
  Streaming --> Streaming: InvokeReply::Event (non-terminal AG-UI)
  Streaming --> Streaming: TopicError::Lagged (warn + continue)
  Streaming --> Closed_terminal: AG-UI RUN_FINISHED / RUN_ERROR seen\n  → disarm cancel guard
  Streaming --> Closed_complete: InvokeReply::Complete\n  (synth RUN_FINISHED if no terminal)\n  → disarm cancel guard
  Streaming --> Closed_error: InvokeReply::Error\n  (synth RUN_ERROR if no terminal)\n  → disarm cancel guard
  Streaming --> Closed_timeout: invoke_timeout elapsed,\n  saw_any_event=false\n  → guard stays armed → cancel publishes
  Streaming --> Closed_shutdown: server shutdown_rx flips\n  → synth RUN_ERROR(internal)
  Streaming --> Closed_disconnect: client dropped TCP\n  → axum cancels future\n  → guard Drop publishes cancel
  Failed_400 --> [*]
  Failed_404 --> [*]
  Failed_500 --> [*]
  Closed_terminal --> [*]
  Closed_complete --> [*]
  Closed_error --> [*]
  Closed_timeout --> [*]
  Closed_shutdown --> [*]
  Closed_disconnect --> [*]
```

**Invariants enforced**
- `agent_request:{request_id}` topic subscription is established
  **before** `ConnectionControl::Invoke` is published, so an
  early-arrival `agent.event` from the SDK is never lost.
- The cancel guard is armed for the lifetime of the SSE stream and
  disarmed exactly on the four "stream produced or surfaced a
  terminal" paths. Any other dropping path (timeout, shutdown,
  client disconnect, panic) leaves the guard armed and its `Drop`
  publishes `ConnectionControl::Cancel` so the SDK never gets stuck
  in `agent_busy`.

## 2. SDK invocation worker (`sdk/python/.../client.py`)

```mermaid
stateDiagram-v2
  [*] --> Dispatch_check: agent.invoke received
  Dispatch_check --> Reject_agui_extra: ag_ui module missing
  Dispatch_check --> Reject_unregistered: no live registration
  Dispatch_check --> Reject_runtime: runtime.kind != inproc
  Dispatch_check --> Validate_input
  Validate_input --> Reject_bad_input: pydantic ValidationError
  Validate_input --> Reserve_slot
  Reserve_slot --> Reject_busy: agent_name in busy_agents
  Reserve_slot --> Spawn_worker: busy_agents.add + invocations[req_id] = state
  Spawn_worker --> Worker_running
  Worker_running --> Streaming_events: ag_ui_strands.run() begins
  Streaming_events --> Streaming_events: yield event\n  → send agent.event\n  → renderer.emit
  Streaming_events --> Cancelling: server agent.cancel sets inv.cancelled\n  + agent.cancel() invoked at handle time
  Cancelling --> Streaming_events: stub may yield once more
  Cancelling --> Cancelled_terminal: post-loop sees cancelled flag\n  → emit synth RUN_ERROR\n  → send agent.error{cancelled}
  Streaming_events --> Completed: stream exhausted, no cancel\n  → send agent.complete
  Streaming_events --> Failed: converter raised\n  → emit synth RUN_ERROR\n  → send agent.error{internal}
  Reject_agui_extra --> Cleanup
  Reject_unregistered --> Cleanup
  Reject_runtime --> Cleanup
  Reject_bad_input --> Cleanup
  Reject_busy --> Cleanup
  Cancelled_terminal --> Cleanup
  Completed --> Cleanup
  Failed --> Cleanup
  Cleanup --> [*]: invocations.pop, busy_agents.discard,\n  callback_handler restored,\n  renderer.finish
```

**Invariants enforced**
- The reservation/cleanup pair (`busy_agents.add` ⇒
  `busy_agents.discard`) sits inside `_invoke_worker_entry`'s outer
  `try/finally` so any exception raised before the converter starts
  still releases the slot.
- `_send_agui_event` is the only path that emits `agent.event`
  frames; renderer.emit happens synchronously alongside the wire
  send so the SDK terminal stays exactly in sync with the SSE
  pipe.
- Cancellation is two-phased: `_handle_cancel` calls
  `agent.cancel()` immediately so Strands wakes at its next
  checkpoint, and the loop's `_is_cancelled` re-check ensures the
  worker leaves cleanly even if `cancel()` is a no-op for that
  agent.

## 3. Cancellation propagation

```mermaid
sequenceDiagram
  autonumber
  participant Client as HTTP client
  participant SSE as SSE handler
  participant CtrlTopic as connection_control:{instance_id}
  participant WS as WS handler
  participant Worker as SDK worker

  Client->>SSE: POST /agents/{name}/runs
  SSE->>CtrlTopic: Invoke{req_id, target, run_input}
  CtrlTopic->>WS: deliver Invoke
  WS->>Worker: agent.invoke{req_id, run_input}
  Worker-->>WS: agent.event{...} (streaming)
  WS-->>SSE: InvokeReply::Event (per-request topic)
  SSE-->>Client: data: {agent.event JSON}\n\n

  Note over Client,SSE: Client disconnects (Ctrl-C / TCP close)
  SSE-->>SSE: stream future dropped
  SSE-->>SSE: InvokeCancelGuard::drop()\n  spawn publish_cancel
  SSE->>CtrlTopic: Cancel{req_id, target}
  CtrlTopic->>WS: deliver Cancel
  WS->>Worker: agent.cancel{req_id}
  Worker-->>Worker: inv.cancelled=true\n  + agent.cancel()
  Worker-->>WS: agent.error{cancelled}
  Worker-->>WS: cleanup → busy_agents.discard
```

**Race-resistance**
- The guard's `Drop` runs *synchronously* with the SSE future drop;
  the spawned `publish_cancel` task is independent of the SSE
  subscriber. Even if the topic broker takes time to deliver, the
  SDK eventually receives it and releases the slot.
- The worker's cleanup in `_invoke_worker_entry::finally` runs
  regardless of whether `agent.cancel` was delivered — if the cancel
  topic flaps, the worker still finishes when the converter does
  and frees `busy_agents`.

## 4. Error mapping (single source of truth)

| Phase | Failure | Wire frame | HTTP outcome |
|---|---|---|---|
| HTTP validation | bad project_id / non-object body | n/a | 400 `bad_request` |
| HTTP lookup | no registration matching `(project, agent, name)` | n/a | 404 `agent_not_registered` |
| HTTP topics | broker error | n/a | 500 `internal` |
| SDK dispatch | optional `ag_ui` not installed | `agent.error{agui_extra_missing}` | synth RUN_ERROR + close |
| SDK dispatch | live agent unregistered between find() and invoke | `agent.error{agent_not_registered}` | synth RUN_ERROR + close |
| SDK dispatch | non-inproc runtime kind | `agent.error{unsupported_runtime}` | synth RUN_ERROR + close |
| SDK dispatch | RunAgentInput invalid | `agent.error{bad_run_input}` | synth RUN_ERROR + close |
| SDK dispatch | concurrent invoke of same agent | `agent.error{agent_busy}` | synth RUN_ERROR + close |
| SDK runtime | converter raised | `agent.event{RUN_ERROR}` + `agent.error{internal}` | RUN_ERROR forwarded + close |
| SDK runtime | server cancel arrived | `agent.event{RUN_ERROR}` + `agent.error{cancelled}` | RUN_ERROR forwarded + close |
| Server timeout | first event > INVOKE_TIMEOUT_MS | n/a | synth RUN_ERROR{invoke_timeout} + close |
| Server shutdown | shutdown_rx flips | n/a | synth RUN_ERROR{internal} + close |
| Client disconnect | TCP / Ctrl-C | n/a (cancel emitted to SDK) | future dropped |
