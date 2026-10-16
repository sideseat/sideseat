"""Home and checkout paths in recorded bytes, replaced by fixed placeholders.

A capture records whatever the producer saw: a traceback names the scenario file under the checkout, an
interpreter path names the developer's home. Committed, those name the account and the machine, and they differ
between two checkouts of one repository, so a fixture recorded from a worktree disagrees with a transcript
recorded from the main tree. Every recorded layer - OTLP payloads, transcript bodies, cassette bodies, base64
inside any of them, eventstream frames - therefore has its checkout root replaced by ``/home/user/sideseat`` and
any other home directory by ``/home/user``.

The replacement changes lengths. JSON and text take that as they are. A protobuf payload is rewritten
structurally, field by field, and only where it round-trips byte for byte first: every byte but the paths stays
what the producer sent, and a payload the decoder would re-encode differently is refused rather than re-encoded.
A streamed response is refused too when the replacement would change what its events reassemble to other than
by the replacement itself - a path split across two deltas. The same functions run on a fresh capture and on
committed bytes (``python -m harness normalise``), so both give one answer, and they are idempotent: the
placeholders are left alone, whatever the local layout.
"""

from __future__ import annotations

import base64
import binascii
import json
import os
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any

from harness.scrub import EVENTSTREAM, scrub_account
from harness.truth.framing import FramingError, encode_frame, eventstream_frames

PLACEHOLDER_HOME = "/home/user"
PLACEHOLDER_CHECKOUT = "/home/user/sideseat"

_REPO = Path(__file__).resolve().parents[4]


def _main_checkout(repo: Path) -> list[str]:
    """The main checkout of a linked worktree, read from its ``.git`` file (``gitdir: <main>/.git/worktrees/x``,
    the path absolute or relative to the worktree)."""
    try:
        line = (repo / ".git").read_text(encoding="utf-8").strip()
    except (IsADirectoryError, FileNotFoundError, NotADirectoryError):
        return []
    if not line.startswith("gitdir: "):
        return []
    gitdir = Path(os.path.normpath(repo / line.removeprefix("gitdir: ")))
    if gitdir.parent.name != "worktrees" or gitdir.parent.parent.name != ".git":
        return []
    return [str(gitdir.parent.parent.parent)]


def _guarded(roots: list[str]) -> list[str]:
    """The roots that cannot rewrite a placeholder: not ``/``, and neither a placeholder nor an ancestor of one,
    so a home of ``/home`` or a checkout at ``/home/user`` cannot grow the placeholder on every run."""
    kept = set()
    for root in roots:
        root = root.rstrip("/")
        if not root.startswith("/") or root in (PLACEHOLDER_HOME, PLACEHOLDER_CHECKOUT):
            continue
        if PLACEHOLDER_CHECKOUT.startswith(root + "/"):
            continue
        kept.add(root)
    return sorted(kept, key=len, reverse=True)


def _with_scrubbed(paths: list[str]) -> list[str]:
    # As the account scrub leaves them too, which runs on a payload first.
    return [
        form for path in paths for form in (path, scrub_account(path.encode()).decode())
    ]


#: The checkout this harness runs from, whatever it is called; when it is a linked worktree, the main checkout it
#: belongs to as well, since a worktree's run can still see paths of the main tree (a shared interpreter, a shared
#: cache); and the layouts committed captures were made in: the account name already scrubbed to its placeholder,
#: the replay archives' ``test-user``. The repository sweep proves no other layout is committed.
_CHECKOUTS = _guarded(
    _with_scrubbed([str(_REPO), *_main_checkout(_REPO)])
    + ["/Users/sideseat/Desktop/dev/sideseat", "/Users/test-user/dev/sideseat"]
)
#: The running account's home wherever it lives, inside or outside ``/Users`` and ``/home``.
_HOMES = _guarded(
    _with_scrubbed(
        [home for home in (os.environ.get("HOME", ""), str(Path.home())) if home]
    )
)

#: A literal character of a name: a word character, ``.``, ``-``, or any other non-ASCII character but space
#: and the ellipsis a preview truncates with - so a name of symbols is consumed whole. The alternatives are
#: disjoint (the second takes no word character), and names repeat possessively, so a long name cannot make the
#: pattern backtrack.
_NAME = r"(?:[\w.-]|(?=[^\x00-\x7f])[^\w\s\u2026])"
#: An absolute path starts where no name character, ``+`` or ``/`` precedes it: ``pages/home/x`` is a module path,
#: and a base64 run (whose alphabet holds ``+`` and ``/``) cannot hold a path anywhere but at its start.
_START = r"(?<![\w.+/-])"


def _literal(path: str) -> str:
    return "/".join(re.escape(part) for part in path.split("/"))


#: The paths of plain text: what JSON spells with escapes is matched once decoded (``_text``), so no pattern here
#: knows an escape. A Windows profile keeps its drive and separators and gets the placeholder account; its name
#: may hold single spaces where the path goes on past it (``Alice Smith\x``), and a name that ends the path ends
#: at its first space, so the prose after a path is never taken for its account. The placeholder account is
#: matched too (and kept), so the profile's tail is never read as a Unix path.
_PATHS = re.compile(
    _START
    + "(?:"
    + "|".join(
        [
            rf"(?P<checkout>(?:{'|'.join(map(_literal, _CHECKOUTS))})"
            rf"(?:/\.claude/worktrees/{_NAME}++)?)(?!{_NAME})"
        ]
        + (
            [rf"(?P<home>{'|'.join(map(_literal, _HOMES))})(?!{_NAME})"]
            if _HOMES
            else []
        )
        + [
            rf"(?P<other>/Users/{_NAME}++|/home/(?!user(?!{_NAME})){_NAME}++)",
            rf"(?P<windows>[A-Za-z]:(?P<sep>[\\/])Users(?P=sep))"
            rf"(?:{_NAME}++(?: {_NAME}++)+(?=(?P=sep))|{_NAME}++)",
        ]
    )
    + ")"
)


def _replacement(found: re.Match[str]) -> str:
    if found.group("windows") is not None:
        return found["windows"] + "user"
    if found.group("checkout") is not None:
        return PLACEHOLDER_CHECKOUT
    return PLACEHOLDER_HOME


class NotNormalisable(ValueError):
    """A recording whose paths cannot be replaced without changing anything else: a protobuf payload that would
    not re-encode to the bytes it was decoded from, or a stream whose path is split across events."""


#: Kept as a name for what a protobuf refusal is.
NotRoundTripping = NotNormalisable

#: A JSON string token - in a document, or in text that quotes one. Not across a line: prose quotes need not
#: pair, and a JSON string holds no raw line break.
_STRING = re.compile(r'"(?:[^"\\\n\r]|\\.)*"')
#: A string that is all base64: eight characters already encode a short home path.
_BASE64_VALUE = re.compile(r"[A-Za-z0-9+/]{8,}={0,2}")


def _plain(text: str) -> str:
    return _PATHS.sub(_replacement, text)


def _value(value: str) -> str:
    """One decoded JSON string: base64 decoded and normalised inside, else text at any depth of nesting.

    A value whose decoded bytes hold a path is re-encoded around them. Otherwise it is text, and the path start
    (``_START``) is what keeps an encoded payload intact: inside a base64 run every character is preceded by
    one of its alphabet, so only a value's first characters could be read as a path, and only by spelling one -
    a home prefix and a name. A value that decodes and also spells a path from its start is read as the path:
    a directory path of base64 characters only (``/Users/<name>/Desktop``: one in four such paths has a valid
    base64 length) is common, and a payload that starts so by chance is about one in 64^7.
    """
    if _BASE64_VALUE.fullmatch(value):
        try:
            inner = base64.b64decode(value, validate=True)
        except binascii.Error:
            inner = None
        if inner is not None and (changed := _layers(inner)) != inner:
            return base64.b64encode(changed).decode("ascii")
    return _text(value)


def _encode_like(value: str, token: str) -> str:
    """``value`` as a JSON string written the way ``token`` was: escaped to ASCII if it escaped non-ASCII, with
    ``/`` escaped if it escaped ``/``."""
    encoded = json.dumps(value, ensure_ascii="\\u" in token)
    return encoded.replace("/", "\\/") if "\\/" in token else encoded


def _token(token: str) -> str:
    try:
        value = json.loads(token)
    except ValueError:
        # Not JSON (a quoted Windows path in prose holds `\U`): its text as it stands.
        return _plain(token)
    changed = _value(value)
    return token if changed == value else _encode_like(changed, token)


def _text(text: str) -> str:
    """Text with every JSON string in it read for what it means - its escapes decoded, the documents nested in it
    read the same way - and plain text between them, all normalised."""
    out: list[str] = []
    last = 0
    for found in _STRING.finditer(text):
        out.append(_plain(text[last : found.start()]))
        out.append(_token(found[0]))
        last = found.end()
    out.append(_plain(text[last:]))
    return "".join(out)


def normalise_text(text: str) -> str:
    """The text with its checkout root and home directories replaced by the placeholders: in plain text, inside
    every JSON string it holds whatever the string's escapes, and inside base64 values; each pass replaces once,
    so no replacement is itself rewritten."""
    return _text(text)


def normalise_bytes(raw: bytes) -> bytes:
    """``normalise_text`` over bytes, undecodable bytes passed through as they are."""
    text = raw.decode("utf-8", errors="surrogateescape")
    changed = _text(text)
    return raw if changed == text else changed.encode("utf-8", errors="surrogateescape")


#: Every layer is reached through ``_text``: JSON strings, the documents inside them and base64 values.
_layers = normalise_bytes


def normalise_body(raw: bytes, content_type: str) -> bytes:
    """A recorded HTTP body with every layer normalised: eventstream frames rebuilt around their payloads, with
    their lengths and checksums, and base64 inside JSON at any depth."""
    if content_type.split(";")[0].strip().lower() == EVENTSTREAM:
        try:
            frames = list(eventstream_frames(raw))
        except FramingError:
            return _layers(raw)
        return b"".join(
            encode_frame(frame.encoded_headers, _layers(frame.payload))
            for frame in frames
        )
    return _layers(raw)


def normalise_response(item: dict[str, Any]) -> dict[str, Any]:
    """A cassette interaction with its response body normalised, refused when the body's decoded model call
    changes other than by the replacement of its own text - a path the provider streamed across two deltas, which
    no per-event replacement can see whole."""
    from harness.truth.wire import DecodeError, decode

    body = base64.b64decode(item["body"])
    clean = normalise_body(body, (item.get("headers") or {}).get("content-type", ""))
    normalised = (
        item
        if clean == body
        else {**item, "body": base64.b64encode(clean).decode("ascii")}
    )
    where = f"{item.get('method')} {item.get('path')}"
    try:
        before = decode(item)
    except (DecodeError, ValueError, KeyError) as error:
        # A body no decoder reads is normalised byte by byte only, which is safe where it changed nothing.
        if clean != body:
            raise NotNormalisable(
                f"{where}: a changed body no decoder can verify ({error})"
            ) from error
        return item
    if before is None:
        # No model call to compare: a changed body nothing can verify is refused, as one no decoder reads is.
        if clean != body:
            raise NotNormalisable(f"{where}: a changed body no decoder can verify")
        return item
    try:
        after = decode(normalised)
    except (DecodeError, ValueError, KeyError) as error:
        raise NotNormalisable(
            f"{where}: the normalised body no longer decodes ({error})"
        ) from error
    # The decoded call, normalised as the body was - base64 inside a value included.
    expected = json.loads(
        _layers(
            json.dumps(before.to_json(), sort_keys=True, ensure_ascii=False).encode(
                "utf-8"
            )
        ).decode("utf-8")
    )
    # Compared as values: the replacement may spell a string's escapes differently, and only its meaning counts.
    if after is None or after.to_json() != expected:
        raise NotNormalisable(
            f"{where}: a home path is split across the response's events"
        )
    return normalised


def _message_type(signal: str) -> type:
    from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
        ExportLogsServiceRequest,
    )
    from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
        ExportMetricsServiceRequest,
    )
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
    )

    return {
        "req": ExportTraceServiceRequest,
        "logs": ExportLogsServiceRequest,
        "metrics": ExportMetricsServiceRequest,
    }[signal]


def _fields(message: Any) -> bool:
    """Normalise every string and bytes field of a protobuf message, at any depth; whether any changed."""
    changed = False
    for field, value in message.ListFields():
        repeated = field.is_repeated
        # Of the bytes fields only a value's, which holds content: the others are identifiers (a trace id, a
        # span id), whose bytes are opaque even where they happen to spell a path.
        if field.type == field.TYPE_STRING or (
            field.type == field.TYPE_BYTES and field.name == "bytes_value"
        ):
            change: Callable[[Any], Any] = (
                _normalise_string if field.type == field.TYPE_STRING else _layers
            )
            items = list(value) if repeated else [value]
            clean = [change(item) for item in items]
            if clean == items:
                continue
            changed = True
            if repeated:
                value[:] = clean
            else:
                setattr(message, field.name, clean[0])
        elif field.type == field.TYPE_MESSAGE:
            if field.message_type.GetOptions().map_entry:
                raise NotNormalisable(
                    f"a map field ({field.name}) has no stable order to re-encode in"
                )
            for item in value if repeated else [value]:
                changed = _fields(item) or changed
    return changed


def _normalise_string(value: str) -> str:
    """One protobuf string field, base64 JSON values inside it included (a message body an attribute holds)."""
    return _layers(value.encode("utf-8")).decode("utf-8")


def normalise_otlp(raw: bytes, signal: str, suffix: str) -> bytes:
    """One recorded OTLP export with its paths normalised; ``signal`` is the file prefix (``req``, ``logs``,
    ``metrics``) and ``suffix`` the encoding (``json``, ``pb``).

    A protobuf payload is decoded whole rather than searched: a string's length prefix is a byte like any
    other, and one that reads as a letter would hide the path behind it from a text search."""
    if suffix != "pb":
        return _layers(raw)
    message = _message_type(signal)()
    message.ParseFromString(raw)
    if not _fields(message):
        return raw
    original = _message_type(signal)()
    original.ParseFromString(raw)
    if original.SerializeToString() != raw:
        raise NotNormalisable(
            f"a {signal} export holding a home path does not re-encode to its own bytes"
        )
    return message.SerializeToString()
