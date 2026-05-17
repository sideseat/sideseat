"""Wire-level frames for the SideSeat SDK WebSocket channel (v1).

Mirror of `server/protocol/ws-v1/schema.json` and `codes.md`. The schema is
bundled as `_schema.json` next to this module for round-trip tests.
"""

from __future__ import annotations

import enum
import json
import uuid
from dataclasses import asdict, dataclass, field
from typing import Any

PROTOCOL_VERSION = 1


class ErrorCode(str, enum.Enum):
    UNSUPPORTED = "unsupported"
    BAD_PAYLOAD = "bad_payload"
    TOO_LARGE = "too_large"
    INVALID_PROJECT_ID = "invalid_project_id"
    HELLO_REQUIRED = "hello_required"
    REPLACED = "replaced"
    RATE_LIMITED = "rate_limited"
    INTERNAL = "internal"


@dataclass
class Envelope:
    type: str
    id: str
    payload: Any = field(default=None)
    v: int = PROTOCOL_VERSION

    def to_json(self) -> str:
        # Use a stable shape: keep `payload` even if None, since unknown
        # optional fields are tolerated by both ends.
        return json.dumps(
            {
                "v": self.v,
                "type": self.type,
                "id": self.id,
                "payload": self.payload if self.payload is not None else {},
            },
            separators=(",", ":"),
        )


def make_envelope(type_: str, payload: Any) -> Envelope:
    return Envelope(type=type_, id=str(uuid.uuid4()), payload=payload)


def parse_envelope(raw: str) -> Envelope:
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError("frame is not an object")
    return Envelope(
        v=int(data.get("v", 1)),
        type=str(data["type"]),
        id=str(data["id"]),
        payload=data.get("payload", {}),
    )


# ---------------------------------------------------------------------------
# Manifest types (kept simple to stay schema-compatible)
# ---------------------------------------------------------------------------


@dataclass
class RegistrationManifest:
    name: str
    framework: str | None = None
    runtime: dict[str, Any] | None = None
    model: str | None = None
    system_prompt: str | None = None
    tools: list[Any] = field(default_factory=list)
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_payload(self) -> dict[str, Any]:
        out = asdict(self)
        # Drop None top-level fields to keep frames small; server tolerates
        # missing optional fields.
        return {k: v for k, v in out.items() if v is not None}
