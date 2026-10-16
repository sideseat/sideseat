"""The capture's home and checkout path normalisation (``harness.paths``)."""

from __future__ import annotations

import base64
import json

import pytest
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)
from opentelemetry.proto.common.v1.common_pb2 import AnyValue, KeyValue

from harness.paths import (
    NotRoundTripping,
    normalise_body,
    normalise_otlp,
    normalise_text,
)


@pytest.mark.parametrize(
    ("raw", "normalised"),
    [
        (
            "/Users/sideseat/Desktop/dev/sideseat/examples/python/ag2/x.py",
            "/home/user/sideseat/examples/python/ag2/x.py",
        ),
        (
            "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a3/examples/x.py",
            "/home/user/sideseat/examples/x.py",
        ),
        ("/Users/sideseat/.local/share/uv/python", "/home/user/.local/share/uv/python"),
        ("/Users/test-user/dev/sideseat/misc/a", "/home/user/sideseat/misc/a"),
        ("/home/a.b/y and /Users/Jdoe1/z", "/home/user/y and /home/user/z"),
        (
            "pages/home/create and /home/user/sideseat/a",
            "pages/home/create and /home/user/sideseat/a",
        ),
    ],
)
def test_a_home_or_checkout_path_gets_its_placeholder_once(
    raw: str, normalised: str
) -> None:
    assert normalise_text(raw) == normalised
    assert normalise_text(normalised) == normalised


def test_a_transcript_body_is_normalised_inside_its_base64() -> None:
    inner = json.dumps({"text": "/Users/someone/x.py"}).encode()
    body = json.dumps({"bytes": base64.b64encode(inner).decode()}).encode()
    clean = normalise_body(body, "application/json")
    decoded = base64.b64decode(json.loads(clean)["bytes"])
    assert json.loads(decoded) == {"text": "/home/user/x.py"}
    assert normalise_body(clean, "application/json") == clean


def _export(text: str) -> bytes:
    message = ExportTraceServiceRequest()
    span = message.resource_spans.add().scope_spans.add().spans.add()
    span.name = "chat"
    span.attributes.append(KeyValue(key="trace", value=AnyValue(string_value=text)))
    return message.SerializeToString()


def test_a_protobuf_export_is_rewritten_string_by_string() -> None:
    raw = _export('File "/Users/someone/dev/x.py", line 3')
    clean = normalise_otlp(raw, "req", "pb")
    assert clean == _export('File "/home/user/dev/x.py", line 3')
    assert normalise_otlp(clean, "req", "pb") == clean


def _padded_length(raw: bytes) -> bytes:
    """The export with its first field's length written as a two-byte varint: valid protobuf a decoder reads, and
    re-encodes minimally - so not what it was given."""
    tag, length, payload = raw[:1], raw[1], raw[2:]
    assert length < 0x80 and len(payload) == length
    return tag + bytes([length | 0x80, 0x00]) + payload


def test_a_protobuf_export_that_would_re_encode_otherwise_is_refused() -> None:
    with pytest.raises(NotRoundTripping):
        normalise_otlp(_padded_length(_export("/Users/someone/x")), "req", "pb")
    # Without a path there is nothing to rewrite, and the bytes stay as they came.
    untouched = _padded_length(_export("no path here"))
    assert normalise_otlp(untouched, "req", "pb") == untouched


def test_a_placeholder_account_under_users_is_still_a_home() -> None:
    assert normalise_text("/Users/user/x") == "/home/user/x"


def test_a_worktree_knows_its_main_checkout(tmp_path) -> None:
    from harness.paths import _main_checkout

    (tmp_path / ".git").write_text("gitdir: /srv/main/.git/worktrees/frames\n")
    assert _main_checkout(tmp_path) == ["/srv/main"]
    (tmp_path / ".git").unlink()
    (tmp_path / ".git").mkdir()
    assert _main_checkout(tmp_path) == []


def test_a_protobuf_string_is_normalised_inside_its_base64() -> None:
    inner = base64.b64encode(b'{"path": "/Users/someone/x.png"}').decode()
    raw = _export(json.dumps({"bytes": inner}))
    clean = normalise_otlp(raw, "req", "pb")
    message = ExportTraceServiceRequest.FromString(clean)
    text = message.resource_spans[0].scope_spans[0].spans[0].attributes[0].value
    decoded = base64.b64decode(json.loads(text.string_value)["bytes"])
    assert json.loads(decoded) == {"path": "/home/user/x.png"}
    assert normalise_otlp(clean, "req", "pb") == clean


def test_a_base64_run_is_never_rewritten_inside() -> None:
    blob = "QUJD+/Users/someone/QUJDREVG"
    body = json.dumps({"bytes": blob}).encode()
    assert normalise_body(body, "application/json") == body


def test_a_short_or_escaped_base64_value_is_read() -> None:
    short = base64.b64encode(b"/home/ab").decode()
    assert len(short) < 16
    clean = normalise_body(json.dumps({"b": short}).encode(), "")
    assert base64.b64decode(json.loads(clean)["b"]) == b"/home/user"
    inner = base64.b64encode(b"x /Users/someone/y").decode()
    escaped = json.dumps({"doc": json.dumps({"b": inner})}).encode()
    clean = normalise_body(escaped, "")
    decoded = json.loads(json.loads(clean)["doc"])["b"]
    assert base64.b64decode(decoded) == b"x /home/user/y"


def test_no_local_root_can_grow_a_placeholder() -> None:
    from harness.paths import _guarded

    assert _guarded(["/", "/home", "/home/user", "/home/user/sideseat"]) == []
    assert _guarded(["/home/user/src/sideseat", "/srv/me/"]) == [
        "/home/user/src/sideseat",
        "/srv/me",
    ]


def test_a_relative_gitdir_is_resolved_against_the_worktree(tmp_path) -> None:
    from harness.paths import _main_checkout

    worktree = tmp_path / "frames"
    worktree.mkdir()
    (worktree / ".git").write_text("gitdir: ../main/.git/worktrees/frames\n")
    assert _main_checkout(worktree) == [str(tmp_path / "main")]


def test_a_path_behind_a_letter_length_prefix_is_found() -> None:
    text = "/Users/someone/" + "x" * 50
    assert len(text) == 0x41  # its length prefix is the byte `A`
    clean = normalise_otlp(_export(text), "req", "pb")
    assert clean == _export("/home/user/" + "x" * 50)


def test_a_protobuf_bytes_value_is_normalised() -> None:
    message = ExportTraceServiceRequest()
    span = message.resource_spans.add().scope_spans.add().spans.add()
    span.attributes.append(
        KeyValue(key="b", value=AnyValue(bytes_value=b"/Users/someone/x"))
    )
    clean = ExportTraceServiceRequest.FromString(
        normalise_otlp(message.SerializeToString(), "req", "pb")
    )
    value = clean.resource_spans[0].scope_spans[0].spans[0].attributes[0].value
    assert value.bytes_value == b"/home/user/x"


def _chunk(text: str) -> str:
    return "data: " + json.dumps(
        {"id": "c", "model": "m", "choices": [{"index": 0, "delta": {"content": text}}]}
    )


def test_a_path_split_across_stream_events_is_refused() -> None:
    from harness.paths import NotNormalisable, normalise_response

    def item(*texts: str) -> dict:
        body = "\n\n".join([*map(_chunk, texts), "data: [DONE]"]) + "\n\n"
        return {
            "method": "POST",
            "path": "/v1/chat/completions",
            "status": 200,
            "headers": {"content-type": "text/event-stream"},
            "body": base64.b64encode(body.encode()).decode(),
        }

    whole = normalise_response(item("see /Users/alice/x", " done"))
    assert b"/home/user/x" in base64.b64decode(whole["body"])
    with pytest.raises(NotNormalisable):
        normalise_response(item("see /Users/al", "ice/x"))
    # Split before the account, no event changes on its own; the reassembled text still names it.
    with pytest.raises(NotNormalisable):
        normalise_response(item("see /Us", "ers/alice/x"))


def test_a_path_of_base64_characters_is_a_path() -> None:
    body = json.dumps({"path": "/Users/someone/A"}).encode()
    assert json.loads(normalise_body(body, "")) == {"path": "/home/user/A"}


def test_an_identifier_spelling_a_path_is_left_alone() -> None:
    message = ExportTraceServiceRequest()
    span = message.resource_spans.add().scope_spans.add().spans.add()
    span.trace_id = b"/Users/alice/xyz"
    span.attributes.append(
        KeyValue(key="t", value=AnyValue(string_value="/Users/alice/x"))
    )
    clean = ExportTraceServiceRequest.FromString(
        normalise_otlp(message.SerializeToString(), "req", "pb")
    )
    span = clean.resource_spans[0].scope_spans[0].spans[0]
    assert span.trace_id == b"/Users/alice/xyz"
    assert span.attributes[0].value.string_value == "/home/user/x"


def test_base64_escaped_at_any_depth_is_read() -> None:
    inner = base64.b64encode(b"x /Users/someone/y").decode()
    doubly = json.dumps(json.dumps(json.dumps({"b": inner}))).encode()
    clean = normalise_body(doubly, "")
    decoded = json.loads(json.loads(json.loads(clean)))["b"]
    assert base64.b64decode(decoded) == b"x /home/user/y"


def test_a_changed_body_no_decoder_reads_is_refused() -> None:
    from harness.paths import NotNormalisable, normalise_response

    item = {
        "method": "POST",
        "path": "/not/a/model",
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body": base64.b64encode(b'{"p": "/Users/someone/x"}').decode(),
    }
    with pytest.raises(NotNormalisable):
        normalise_response(item)
    item["body"] = base64.b64encode(b'{"p": "no path"}').decode()
    assert normalise_response(item) is item


def test_an_archive_is_checked_member_by_member(tmp_path) -> None:
    import gzip
    import io

    from harness.normalise import _archive

    def member(text: bytes, name: str = "") -> bytes:
        buffer = io.BytesIO()
        with gzip.GzipFile(filename=name, mode="wb", fileobj=buffer, mtime=0) as out:
            out.write(text)
        return buffer.getvalue()

    clean = tmp_path / "clean.jsonl.gz"
    clean.write_bytes(member(b'{"a": 1}\n') + member(b'{"b": 2}\n'))
    assert _archive(clean) is None
    named = tmp_path / "named.jsonl.gz"
    # A later member's header names a file: rewritten, the content unchanged.
    named.write_bytes(member(b'{"a": 1}\n') + member(b'{"b": 2}\n', name="x.jsonl"))
    rewritten = _archive(named)
    assert rewritten is not None
    assert gzip.decompress(rewritten.read_bytes()) == b'{"a": 1}\n{"b": 2}\n'
    damaged = tmp_path / "damaged.jsonl.gz"
    data = bytearray(member(b'{"p": "/Users/someone/x"}\n'))
    data[-8] ^= 0xFF
    damaged.write_bytes(bytes(data))
    with pytest.raises(ValueError):
        _archive(damaged)
    assert sorted(p.name for p in tmp_path.iterdir() if p.suffix == ".tmp") == [
        rewritten.name
    ]


def test_a_damaged_archive_header_is_refused(tmp_path) -> None:
    import gzip

    from harness.normalise import _archive

    data = bytearray(gzip.compress(b'{"a": 1}\n', mtime=0))
    data[2] = 7  # not deflate
    damaged = tmp_path / "damaged.jsonl.gz"
    damaged.write_bytes(bytes(data))
    with pytest.raises(ValueError):
        _archive(damaged)


def test_a_response_is_verified_through_its_base64_too() -> None:
    from harness.paths import normalise_response

    encoded = base64.b64encode(b"see /Users/someone/x.png").decode()
    answer = {
        "id": "c",
        "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": encoded}}],
    }
    item = {
        "method": "POST",
        "path": "/v1/chat/completions",
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body": base64.b64encode(json.dumps(answer).encode()).decode(),
    }
    clean = normalise_response(item)
    text = json.loads(base64.b64decode(clean["body"]))["choices"][0]["message"][
        "content"
    ]
    assert base64.b64decode(text) == b"see /home/user/x.png"


def test_a_highly_compressed_archive_inflates_in_bounded_chunks(tmp_path) -> None:
    import gzip

    from harness.normalise import _CHUNK, _members

    lines = b'{"a": 1}\n' * (3 * _CHUNK // 9 + 7)
    archive = tmp_path / "big.jsonl.gz"
    archive.write_bytes(gzip.compress(lines, mtime=0))
    chunks = [chunk for _, chunk in _members(archive)]
    assert max(map(len, chunks)) <= _CHUNK
    assert b"".join(chunks) == lines


def test_an_empty_member_still_shows_its_header(tmp_path) -> None:
    import gzip
    import io

    from harness.normalise import _archive

    buffer = io.BytesIO()
    with gzip.GzipFile(filename="named.jsonl", mode="wb", fileobj=buffer, mtime=0):
        pass
    archive = tmp_path / "a.jsonl.gz"
    archive.write_bytes(gzip.compress(b'{"a": 1}\n', mtime=0) + buffer.getvalue())
    rewritten = _archive(archive)
    assert rewritten is not None
    assert gzip.decompress(rewritten.read_bytes()) == b'{"a": 1}\n'
    assert normalise_text(r"/Users/alice\u0085failed") == r"/home/user\u0085failed"


def test_a_long_windows_account_is_matched_in_linear_time() -> None:
    import time

    started = time.monotonic()
    assert normalise_text("C:\\Users\\" + "é" * 4000 + "\\x") == "C:\\Users\\user\\x"
    assert time.monotonic() - started < 1


def _json(text: str, **kwargs) -> str:
    return json.dumps({"p": text}, **kwargs)


def test_a_json_string_is_normalised_for_what_it_means() -> None:
    cases = {
        # Each separator escaped or not on its own, and an escaped letter part of the account.
        r'{"p":"/Users\/alice/x"}': {"p": "/home/user/x"},
        r'{"p":"/home/user\u006eame/x"}': {"p": "/home/user/x"},
        r'{"p":"/Users/Jos\u00e9/x"}': {"p": "/home/user/x"},
        r'{"p":"/Users/\ud83d\ude00/x"}': {"p": "/home/user/x"},
        # An escaped separator ends the account; escaped whitespace and the preview ellipsis end it too.
        r'{"p":"/Users/alice\u002fx"}': {"p": "/home/user/x"},
        r'{"p":"/Users/alice\u00a0failed"}': {"p": "/home/user\u00a0failed"},
        r'{"p":"/home/us\u2026"}': {"p": "/home/user\u2026"},
        # An escaped letter joins a word, so a module path stays one; an escaped newline ends the text before.
        r'{"p":"page\u0073/home/create"}': {"p": "pages/home/create"},
        r'{"p":"\n/Users/alice/x"}': {"p": "\n/home/user/x"},
    }
    for document, meaning in cases.items():
        assert json.loads(normalise_text(document)) == meaning, document


def test_an_escaped_document_keeps_its_escaping() -> None:
    assert normalise_text(r'"\/Users\/someone\/x"') == r'"\/home\/user\/x"'
    assert normalise_text(r'"\/home\/user\/x"') == r'"\/home\/user\/x"'
    assert normalise_text(_json("/Users/Jos\u00e9/x", ensure_ascii=True)) == _json(
        "/home/user/x"
    )
    # A document nested in a string, nested in another.
    nested = json.dumps(json.dumps(json.dumps({"p": "/Users/bob/x"})))
    assert json.loads(json.loads(json.loads(normalise_text(nested)))) == {
        "p": "/home/user/x"
    }


def test_windows_profiles_in_every_spelling() -> None:
    assert normalise_text(r"C:\Users\alice\x") == r"C:\Users\user\x"
    assert normalise_text(r"C:\Users\Alice Smith\x") == r"C:\Users\user\x"
    assert normalise_text(r"see C:\Users\bob is home") == r"see C:\Users\user is home"
    # In plain text a backslash is only a separator: `\u1234` is a directory.
    assert normalise_text(r"C:\Users\alice\u1234\x") == r"C:\Users\user\u1234\x"
    assert normalise_text("D:/Users/bob/x and C:/Users/user/y") == (
        "D:/Users/user/x and C:/Users/user/y"
    )
    assert normalise_text("/usr/bin:/Users/bob/y") == "/usr/bin:/home/user/y"
    for meaning in ["C:\\Users\\\u674e\\x", "C:\\Users\\alice\\x", "C:/Users/alice/x"]:
        document = json.dumps({"p": meaning}, ensure_ascii=True)
        assert json.loads(normalise_text(document))["p"] in (
            "C:\\Users\\user\\x",
            "C:/Users/user/x",
        )


def test_an_accented_or_symbol_account_is_replaced_whole() -> None:
    assert normalise_text("/Users/José/x") == "/home/user/x"
    assert normalise_text("/Users/alice😀/x") == "/home/user/x"
    assert normalise_text("/Users/😀/x") == "/home/user/x"
    assert normalise_text("/home/us…") == "/home/user…"


def test_a_response_is_verified_by_meaning_not_spelling() -> None:
    from harness.paths import normalise_response

    text = "see /Users/alice/x; BS/".replace("BS", chr(92))
    answer = {
        "id": "c",
        "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text}}],
    }
    item = {
        "method": "POST",
        "path": "/v1/chat/completions",
        "status": 200,
        "headers": {"content-type": "application/json"},
        "body": base64.b64encode(json.dumps(answer).encode()).decode(),
    }
    clean = json.loads(base64.b64decode(normalise_response(item)["body"]))
    assert clean["choices"][0]["message"]["content"] == text.replace(
        "/Users/alice", "/home/user"
    )
