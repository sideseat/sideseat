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
