"""Remove the capturing developer's account name from recorded bytes, including encoded model bodies.

A cassette stores each model response base64-encoded, and a streamed Bedrock response is a sequence of
AWS eventstream frames whose payloads can hold further base64 (``InvokeModelWithResponseStream`` wraps
each chunk as ``{"bytes": "..."}``). A plain search of the cassette file sees none of that, so a name the
model echoes - a tool named after a local path - would pass every text-level check. This module decodes
those layers, replaces the account name at equal length inside each, and re-encodes them, so a scrubbed
body still parses: frame lengths are unchanged and both CRC32 checksums are recomputed.
"""

from __future__ import annotations

import base64
import binascii
import getpass
import json
import re
from collections.abc import Callable, Iterator

from harness.truth.framing import FramingError, encode_frame, eventstream_frames

PLACEHOLDER_USER = b"sideseat"
EVENTSTREAM = "application/vnd.amazon.eventstream"

#: A JSON string value long enough to be worth trying as base64; shorter ones cannot hide a path.
_BASE64_STRING = re.compile(rb'"([A-Za-z0-9+/]{16,}={0,2})"')


def account_names() -> list[bytes]:
    """The forms of the current account name that can appear in captured bytes.

    Some producers lower-case what they derive from a path (CrewAI's MCP tool names), so the lower-case
    form is searched as well. Empty when the account already is the placeholder.
    """
    user = getpass.getuser().encode()
    if not user or user == PLACEHOLDER_USER:
        return []
    return list(dict.fromkeys([user, user.lower()]))


def scrub_account(raw: bytes) -> bytes:
    """Replace the account name with the placeholder, keeping every length unchanged."""
    for name in account_names():
        raw = raw.replace(name, PLACEHOLDER_USER[: len(name)].ljust(len(name), b"_"))
    return raw


def _within_base64(raw: bytes, transform: Callable[[bytes], bytes]) -> bytes:
    """Apply ``transform`` inside JSON string values that are base64, re-encoding the ones it changes."""

    def visit(match: re.Match[bytes]) -> bytes:
        try:
            inner = base64.b64decode(match[1], validate=True)
        except binascii.Error:
            return match[0]
        changed = _transform_layers(inner, transform)
        if changed == inner:
            return match[0]
        return b'"' + base64.b64encode(changed) + b'"'

    return _BASE64_STRING.sub(visit, raw)


def _transform_layers(raw: bytes, transform: Callable[[bytes], bytes]) -> bytes:
    return _within_base64(transform(raw), transform)


def transform_body(
    raw: bytes, content_type: str, transform: Callable[[bytes], bytes]
) -> bytes:
    """Apply ``transform`` to a model response body and to every encoded layer inside it.

    An eventstream body is rebuilt frame by frame; anything else - JSON, server-sent events, text - is
    transformed in place. A body labelled as an eventstream that does not parse is left unchanged by
    the frame pass but still transformed as bytes, since a scrub must not skip what it cannot decode.
    """
    if content_type.split(";")[0].strip().lower() == EVENTSTREAM:
        try:
            frames = list(eventstream_frames(raw))
        except FramingError:
            return _transform_layers(raw, transform)
        return b"".join(
            encode_frame(
                frame.encoded_headers, _transform_layers(frame.payload, transform)
            )
            for frame in frames
        )
    return _transform_layers(raw, transform)


def scrub_body(raw: bytes, content_type: str) -> bytes:
    """A model response body with the account name replaced in every layer."""
    return transform_body(raw, content_type, scrub_account)


def decoded_layers(raw: bytes, content_type: str) -> Iterator[bytes]:
    """Every decoded layer of a body: the body or each frame payload, and base64 found inside them."""
    pieces = [raw]
    if content_type.split(";")[0].strip().lower() == EVENTSTREAM:
        try:
            pieces = [frame.payload for frame in eventstream_frames(raw)]
        except FramingError:
            pass
    while pieces:
        piece = pieces.pop()
        yield piece
        for match in _BASE64_STRING.finditer(piece):
            try:
                pieces.append(base64.b64decode(match[1], validate=True))
            except binascii.Error:
                continue


def cassette_bodies(document: str) -> Iterator[tuple[int, bytes]]:
    """``(index, layer)`` for every decoded layer of every response body in a cassette document."""
    for index, item in enumerate(json.loads(document)["interactions"]):
        content_type = item.get("headers", {}).get("content-type", "")
        for layer in decoded_layers(base64.b64decode(item["body"]), content_type):
            yield index, layer
