# SideSeat WS Protocol v1 — error codes

Single source of truth. Mirrored as Rust and Python enums, both
serialised as snake_case strings on the wire.

| Code                  | Meaning                                                                                  | Connection survives? |
|-----------------------|------------------------------------------------------------------------------------------|----------------------|
| `unsupported`         | Frame `type` is unknown (or reserved-but-not-implemented in v1, e.g. `agent.invoke`).    | Yes                  |
| `bad_payload`         | JSON parse error or payload fails schema validation.                                     | Yes                  |
| `too_large`           | Frame exceeds the server-advertised `max_message_bytes`.                                 | Yes                  |
| `invalid_project_id`  | Path `project_id` failed `is_valid_project_id`. Returned as HTTP 400 before WS upgrade.  | n/a (no upgrade)     |
| `hello_required`      | First post-`welcome` SDK frame was not `hello`, or no `hello` arrived within 5 s.        | No (close 4000)      |
| `replaced`            | Sent to a socket whose `(kind, name)` registration was claimed by a different client_id. | No (close 4000)      |
| `rate_limited`        | Client exceeded the 100-frames / 10 s rolling token bucket.                              | Yes                  |
| `internal`            | Server-side bug.                                                                         | Yes                  |
| `agent_not_registered`| HTTP `POST /agents/{name}/runs` had no live registration matching `(project, agent, name)`. | n/a (404 before SDK reach) |
| `agent_busy`          | A second invoke arrived while the agent was running. Strands enforces serial execution.  | Yes                  |
| `invoke_timeout`      | Server didn't see any `agent.event` within `INVOKE_TIMEOUT_MS` after sending invoke.     | n/a (504 SSE close)  |
| `cancelled`           | SDK aborted the run via `Strands.Agent.cancel()` after `agent.cancel` from server.       | Yes                  |
| `agui_extra_missing`  | SDK received `agent.invoke` but the `[agui]` extra is not installed.                     | Yes                  |
| `bad_run_input`       | `RunAgentInput` payload failed pydantic validation in the SDK.                           | Yes                  |
| `unsupported_runtime` | The registered agent has a non-`inproc` runtime kind (v2 supports only inproc).          | Yes                  |

## Reserved frame types (v1: `error.unsupported` only)

These are placeholders for the v2 invocation extension; documented so
forward-evolution stays additive:

- `agent.invoke`
- `agent.delta`
- `agent.complete`
- `agent.cancel`
- `mcp.invoke`

## Close codes

- `1000` — normal closure (e.g. SDK `disconnect()`).
- `4000` — application-level close (`replaced`, `hello_required`).
