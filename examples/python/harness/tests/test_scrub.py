import base64
import json
import subprocess
from pathlib import Path

import pytest

from harness import proxy, scrub
from harness.capture import REPO
from harness.truth.framing import FramingError, encode_frame, eventstream_frames


@pytest.fixture
def account(monkeypatch: pytest.MonkeyPatch) -> bytes:
    monkeypatch.setattr(scrub.getpass, "getuser", lambda: "Jdoe1")
    return b"Jdoe1"


def headers(**values: str) -> bytes:
    return b"".join(
        bytes([len(name)])
        + name.encode()
        + b"\x07"
        + len(value).to_bytes(2, "big")
        + value.encode()
        for name, value in values.items()
    )


def stream(*payloads: bytes) -> bytes:
    return b"".join(
        encode_frame(
            headers(**{":event-type": "chunk", ":message-type": "event"}), payload
        )
        for payload in payloads
    )


def test_a_frame_round_trips_with_valid_checksums() -> None:
    encoded = headers(**{":event-type": "contentBlockDelta"})
    raw = encode_frame(encoded, b'{"delta": 1}')

    (frame,) = eventstream_frames(raw)

    assert frame.payload == b'{"delta": 1}'
    assert frame.encoded_headers == encoded
    assert frame.event_type == "contentBlockDelta"
    with pytest.raises(FramingError):
        list(eventstream_frames(raw[:-1] + bytes([raw[-1] ^ 1])))


def test_the_account_is_scrubbed_in_both_cases_at_its_length(account: bytes) -> None:
    raw = b"/Users/Jdoe1/x and opt_bin_uv_users_jdoe1_aa4c75f4"

    clean = scrub.scrub_account(raw)

    assert clean == b"/Users/sides/x and opt_bin_uv_users_sides_aa4c75f4"


def test_an_eventstream_body_is_scrubbed_inside_frames_and_nested_base64(
    account: bytes,
) -> None:
    inner = json.dumps({"name": "uv_users_jdoe1_tool"}).encode()
    raw = stream(
        b'{"toolUse": {"name": "uv_users_jdoe1_tool"}}',
        b'{"bytes": "' + base64.b64encode(inner) + b'"}',
        b'{"text": "nothing to see"}',
    )

    clean = scrub.scrub_body(raw, scrub.EVENTSTREAM)

    assert len(clean) == len(raw)
    frames = list(eventstream_frames(clean))  # raises on a stale prelude or message CRC
    assert [f.event_type for f in frames] == ["chunk"] * 3
    assert frames[0].payload == b'{"toolUse": {"name": "uv_users_sides_tool"}}'
    nested = base64.b64decode(json.loads(frames[1].payload)["bytes"])
    assert json.loads(nested) == {"name": "uv_users_sides_tool"}
    assert frames[2].payload == b'{"text": "nothing to see"}'
    assert not any(
        account.lower() in layer
        for layer in scrub.decoded_layers(clean, scrub.EVENTSTREAM)
    )


@pytest.mark.parametrize(
    "content_type",
    ["application/json", "text/event-stream; charset=utf-8"],
)
def test_json_and_sse_bodies_are_scrubbed_in_place(
    account: bytes, content_type: str
) -> None:
    raw = b'data: {"tool": "uv_users_Jdoe1_t"}\n\n'

    assert (
        scrub.scrub_body(raw, content_type) == b'data: {"tool": "uv_users_sides_t"}\n\n'
    )


def test_the_proxy_writes_a_scrubbed_cassette(account: bytes, tmp_path: Path) -> None:
    cassette = tmp_path / "cassette.json"
    body = stream(b'{"name": "users_jdoe1_x"}')
    with proxy.ModelProxy(cassette, record=True) as recording:
        recording._recorded.append(
            {
                "method": "POST",
                "path": "/model/m/converse-stream",
                "status": 200,
                "headers": {"content-type": scrub.EVENTSTREAM},
                "body": base64.b64encode(body).decode(),
                "request_sha256": "0",
            }
        )

    layers = [layer for _, layer in scrub.cassette_bodies(cassette.read_text())]
    assert b'{"name": "users_sides_x"}' in layers
    assert not any(b"jdoe1" in layer.lower() for layer in layers)


def committed_cassettes() -> list[Path]:
    listing = subprocess.run(
        ["git", "ls-files", "-z", "--", "examples/*/*/cassettes/*.json"],
        cwd=REPO,
        capture_output=True,
        check=True,
    )
    return [REPO / name.decode() for name in listing.stdout.split(b"\0") if name]


def test_no_committed_cassette_names_the_account_or_its_home() -> None:
    """Every decoded layer of every committed cassette body, which no text-level sweep can read."""
    cassettes = committed_cassettes()
    assert len(cassettes) > 100, f"only {len(cassettes)} cassettes found"
    home = str(Path.home()).encode()
    # A short account name would match inside unrelated words; the home path still covers it.
    names = [name for name in scrub.account_names() if len(name) >= 4]
    offenders = sorted(
        f"{path.relative_to(REPO)} interaction {index}"
        for path in cassettes
        for index, layer in scrub.cassette_bodies(path.read_text())
        if home in layer or any(name in layer for name in names)
    )
    assert not offenders, "cassettes name the capturing account:\n  " + "\n  ".join(
        offenders
    )
