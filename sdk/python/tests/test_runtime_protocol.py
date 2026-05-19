"""Round-trip + schema validation for runtime protocol envelopes."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from sideseat.runtime.protocol import (
    PROTOCOL_VERSION,
    Envelope,
    ErrorCode,
    RegistrationManifest,
    make_envelope,
    parse_envelope,
)

SCHEMA_PATH = (
    Path(__file__).resolve().parent.parent / "src" / "sideseat" / "runtime" / "_schema.json"
)


def test_protocol_version_is_one() -> None:
    assert PROTOCOL_VERSION == 1


def test_make_envelope_round_trip() -> None:
    env = make_envelope("agent.register", {"name": "x"})
    assert env.v == 1
    assert env.type == "agent.register"
    assert isinstance(env.id, str)

    raw = env.to_json()
    parsed = parse_envelope(raw)
    assert parsed.type == "agent.register"
    assert parsed.payload == {"name": "x"}


def test_envelope_with_no_payload_serializes_empty_object() -> None:
    env = Envelope(type="pong", id="abc", payload=None)
    raw = env.to_json()
    assert json.loads(raw)["payload"] == {}


def test_error_codes_are_snake_case() -> None:
    expected = {
        "unsupported",
        "bad_payload",
        "too_large",
        "invalid_project_id",
        "hello_required",
        "replaced",
        "rate_limited",
        "internal",
        "agent_not_registered",
        "agent_busy",
        "invoke_timeout",
        "cancelled",
        "agui_extra_missing",
        "bad_run_input",
        "unsupported_runtime",
    }
    assert {c.value for c in ErrorCode} == expected


def test_registration_manifest_to_payload_drops_none_top_level() -> None:
    m = RegistrationManifest(name="x", framework=None, runtime={"kind": "inproc"})
    payload = m.to_payload()
    assert "framework" not in payload
    assert payload["runtime"] == {"kind": "inproc"}
    assert payload["tools"] == []


def test_schema_file_is_loadable() -> None:
    assert SCHEMA_PATH.exists(), f"schema not bundled at {SCHEMA_PATH}"
    schema = json.loads(SCHEMA_PATH.read_text())
    assert schema["$id"].startswith("https://sideseat.ai/protocol/ws-v1/")
    assert "frames" in schema["$defs"]


@pytest.mark.parametrize(
    "frame_type",
    [
        "hello",
        "agent.register",
        "agent.unregister",
        "mcp.register",
        "mcp.unregister",
        "pong",
        "welcome",
        "ack",
        "error",
        "ping",
        "replaced",
    ],
)
def test_envelope_carries_known_frame_types(frame_type: str) -> None:
    env = make_envelope(frame_type, {})
    assert env.type == frame_type
