"""The two streaming transports the recorded model responses use: AWS eventstream and SSE.

Bedrock's streaming operations frame each event as an ``application/vnd.amazon.eventstream``
message; OpenAI, Anthropic and Gemini stream ``text/event-stream``. Both are decoded strictly: a
frame whose checksum does not match, or a stream that ends inside a frame, is an error rather than a
shorter answer, because a truncated decode would make the oracle agree with a truncated parser.
"""

from __future__ import annotations

import struct
import zlib
from collections.abc import Iterator
from dataclasses import dataclass


class FramingError(ValueError):
    pass


@dataclass(frozen=True)
class Frame:
    headers: dict[str, object]
    payload: bytes

    @property
    def message_type(self) -> str:
        return str(self.headers.get(":message-type", "event"))

    @property
    def event_type(self) -> str:
        return str(
            self.headers.get(":event-type") or self.headers.get(":exception-type") or ""
        )


# Header value types of the eventstream encoding: fixed widths, or a 2-byte length prefix.
_FIXED = {2: 1, 3: 2, 4: 4, 5: 8, 8: 8, 9: 16}


def eventstream_frames(body: bytes) -> Iterator[Frame]:
    """Every message of an AWS eventstream body, its prelude and message CRCs verified."""
    offset = 0
    while offset < len(body):
        if len(body) - offset < 16:
            raise FramingError(f"eventstream ends inside a prelude at byte {offset}")
        total, header_length, prelude_crc = struct.unpack_from(">III", body, offset)
        if zlib.crc32(body[offset : offset + 8]) != prelude_crc:
            raise FramingError(
                f"eventstream prelude checksum mismatch at byte {offset}"
            )
        end = offset + total
        if end > len(body):
            raise FramingError(f"eventstream ends inside a message at byte {offset}")
        (message_crc,) = struct.unpack_from(">I", body, end - 4)
        if zlib.crc32(body[offset : end - 4]) != message_crc:
            raise FramingError(
                f"eventstream message checksum mismatch at byte {offset}"
            )
        headers_start = offset + 12
        headers = _headers(body[headers_start : headers_start + header_length])
        payload = body[headers_start + header_length : end - 4]
        yield Frame(headers, payload)
        offset = end


def _headers(raw: bytes) -> dict[str, object]:
    headers: dict[str, object] = {}
    index = 0
    while index < len(raw):
        name_length = raw[index]
        name = raw[index + 1 : index + 1 + name_length].decode()
        index += 1 + name_length
        kind = raw[index]
        index += 1
        value: object
        if kind in (0, 1):
            value = kind == 0
        elif kind in _FIXED:
            width = _FIXED[kind]
            chunk = raw[index : index + width]
            value = chunk if kind == 9 else int.from_bytes(chunk, "big", signed=True)
            index += width
        elif kind in (6, 7):
            (length,) = struct.unpack_from(">H", raw, index)
            chunk = raw[index + 2 : index + 2 + length]
            value = chunk.decode() if kind == 7 else chunk
            index += 2 + length
        else:
            raise FramingError(f"unknown eventstream header type {kind} for {name!r}")
        headers[name] = value
    return headers


@dataclass(frozen=True)
class ServerSentEvent:
    event: str
    data: str


def server_sent_events(body: bytes) -> Iterator[ServerSentEvent]:
    """The events of a ``text/event-stream`` body; ``data:`` lines of one event are joined."""
    text = body.decode("utf-8")
    event, data = "", []
    for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        if not line:
            if data:
                yield ServerSentEvent(event or "message", "\n".join(data))
            event, data = "", []
            continue
        if line.startswith(":"):
            continue
        field, _, value = line.partition(":")
        value = value.removeprefix(" ")
        if field == "event":
            event = value
        elif field == "data":
            data.append(value)
    if data:
        yield ServerSentEvent(event or "message", "\n".join(data))
