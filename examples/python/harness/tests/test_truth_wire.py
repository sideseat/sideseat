"""The truth decoder reads every wire format the cassettes and fake models use, exactly."""

from __future__ import annotations

import base64
import json
import struct
import zlib
from pathlib import Path
from typing import Any

import pytest

from harness import capture, content
from harness.fakes import anthropic as fake_anthropic
from harness.fakes import google_genai as fake_gemini
from harness.fakes import openai as fake_openai
from harness.fakes import script
from harness.truth import wire
from harness.truth.framing import FramingError, eventstream_frames, server_sent_events
from harness.truth.wire import DecodeError, ModelCall, decode

# --- Encoders for the tests: what a provider would put on the wire ------------------------------


def frame(headers: dict[str, str], payload: bytes) -> bytes:
    encoded = b"".join(
        bytes([len(name)])
        + name.encode()
        + b"\x07"
        + struct.pack(">H", len(value))
        + value.encode()
        for name, value in headers.items()
    )
    total = 16 + len(encoded) + len(payload)
    prelude = struct.pack(">II", total, len(encoded))
    prelude += struct.pack(">I", zlib.crc32(prelude))
    message = prelude + encoded + payload
    return message + struct.pack(">I", zlib.crc32(message))


def converse_events(*events: tuple[str, dict[str, Any]]) -> bytes:
    return b"".join(
        frame(
            {
                ":event-type": name,
                ":content-type": "application/json",
                ":message-type": "event",
            },
            json.dumps(value).encode(),
        )
        for name, value in events
    )


def bedrock_chunks(events: list[dict[str, Any]]) -> bytes:
    """InvokeModelWithResponseStream: each Anthropic event base64-encoded in a ``chunk`` frame."""
    return b"".join(
        frame(
            {
                ":event-type": "chunk",
                ":content-type": "application/json",
                ":message-type": "event",
            },
            json.dumps(
                {"bytes": base64.b64encode(json.dumps(e).encode()).decode()}
            ).encode(),
        )
        for e in events
    )


def sse(events: list[Any], *, named: bool = False) -> bytes:
    lines = []
    for event in events:
        if named:
            name, value = event
            lines.append(f"event: {name}\ndata: {json.dumps(value)}\n\n")
        else:
            lines.append(
                f"data: {event if isinstance(event, str) else json.dumps(event)}\n\n"
            )
    return "".join(lines).encode()


def item(
    path: str, body: bytes, content_type: str, status: int = 200
) -> dict[str, Any]:
    return {
        "method": "POST",
        "path": path,
        "status": status,
        "headers": {"content-type": content_type},
        "body": base64.b64encode(body).decode(),
    }


CONVERSE = "/model/global.anthropic.claude-sonnet-5-5/converse"


# --- Framing ------------------------------------------------------------------------------------


def test_eventstream_frames_round_trip_headers_and_payload() -> None:
    body = frame(
        {":event-type": "messageStart", ":message-type": "event"},
        b'{"role":"assistant"}',
    )
    (decoded,) = list(eventstream_frames(body + b""))
    assert decoded.event_type == "messageStart"
    assert decoded.message_type == "event"
    assert decoded.payload == b'{"role":"assistant"}'


def test_eventstream_rejects_a_corrupted_or_truncated_message() -> None:
    body = bytearray(frame({":event-type": "x"}, b'{"a":1}'))
    body[-6] ^= 0x01
    with pytest.raises(FramingError, match="checksum"):
        list(eventstream_frames(bytes(body)))
    with pytest.raises(FramingError, match="ends inside"):
        list(eventstream_frames(frame({":event-type": "x"}, b"{}")[:-3]))


def test_server_sent_events_join_data_lines_and_skip_comments() -> None:
    body = b": keep-alive\r\nevent: delta\r\ndata: one\r\ndata: two\r\n\r\ndata: [DONE]\n\n"
    events = list(server_sent_events(body))
    assert [(e.event, e.data) for e in events] == [
        ("delta", "one\ntwo"),
        ("message", "[DONE]"),
    ]


# --- Bedrock Converse ---------------------------------------------------------------------------


def test_converse_keeps_text_reasoning_and_tool_calls_in_order() -> None:
    body = {
        "output": {
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "reasoningContent": {
                            "reasoningText": {"text": "Think.", "signature": "s"}
                        }
                    },
                    {"reasoningContent": {"redactedContent": "b3BhcXVl"}},
                    {"text": "Checking.  \n"},
                    {
                        "toolUse": {
                            "toolUseId": "t1",
                            "name": "get_weather",
                            "input": {"city": "Rome"},
                        }
                    },
                ],
            }
        },
        "stopReason": "tool_use",
        "usage": {
            "inputTokens": 10,
            "outputTokens": 5,
            "cacheReadInputTokens": 3,
            "cacheWriteInputTokens": 2,
        },
    }
    call = decode(item(CONVERSE, json.dumps(body).encode(), "application/json"))
    assert call is not None
    assert call.model == "global.anthropic.claude-sonnet-5-5"
    assert call.parts == [
        wire.reasoning_part("Think.", signature="s"),
        wire.reasoning_part(None, redacted=True),
        wire.text_part("Checking.  \n"),
        wire.tool_call_part("t1", "get_weather", {"city": "Rome"}),
    ]
    assert call.finish == "tool_use"
    assert call.usage is not None
    assert (
        call.usage.input,
        call.usage.output,
        call.usage.cache_read,
        call.usage.cache_write,
    ) == (
        10,
        5,
        3,
        2,
    )


def test_converse_rejects_a_block_it_does_not_know() -> None:
    body = {
        "output": {"message": {"content": [{"image": {}}]}},
        "stopReason": "end_turn",
    }
    with pytest.raises(DecodeError, match="unknown Converse content block"):
        decode(item(CONVERSE, json.dumps(body).encode(), "application/json"))


def test_converse_stream_reassembles_text_reasoning_and_split_tool_arguments() -> None:
    body = converse_events(
        ("messageStart", {"role": "assistant"}),
        (
            "contentBlockDelta",
            {"contentBlockIndex": 0, "delta": {"reasoningContent": {"text": "Hm"}}},
        ),
        (
            "contentBlockDelta",
            {"contentBlockIndex": 0, "delta": {"reasoningContent": {"text": "m."}}},
        ),
        (
            "contentBlockDelta",
            {
                "contentBlockIndex": 0,
                "delta": {"reasoningContent": {"signature": "sig"}},
            },
        ),
        ("contentBlockStop", {"contentBlockIndex": 0}),
        ("contentBlockDelta", {"contentBlockIndex": 1, "delta": {"text": "Rome is "}}),
        ("contentBlockDelta", {"contentBlockIndex": 1, "delta": {"text": "sunny."}}),
        (
            "contentBlockStart",
            {
                "contentBlockIndex": 2,
                "start": {"toolUse": {"toolUseId": "t9", "name": "get_weather"}},
            },
        ),
        (
            "contentBlockDelta",
            {"contentBlockIndex": 2, "delta": {"toolUse": {"input": '{"ci'}}},
        ),
        (
            "contentBlockDelta",
            {"contentBlockIndex": 2, "delta": {"toolUse": {"input": 'ty": "Ro'}}},
        ),
        (
            "contentBlockDelta",
            {"contentBlockIndex": 2, "delta": {"toolUse": {"input": 'me"}'}}},
        ),
        ("messageStop", {"stopReason": "tool_use"}),
        ("metadata", {"usage": {"inputTokens": 7, "outputTokens": 3}, "metrics": {}}),
    )
    call = decode(
        item(CONVERSE + "-stream", body, "application/vnd.amazon.eventstream")
    )
    assert call is not None and call.streamed
    assert call.parts == [
        wire.reasoning_part("Hmm.", signature="sig"),
        wire.text_part("Rome is sunny."),
        wire.tool_call_part("t9", "get_weather", {"city": "Rome"}),
    ]
    assert call.finish == "tool_use"
    assert call.usage is not None and (call.usage.input, call.usage.output) == (7, 3)


def test_converse_stream_records_an_exception_frame() -> None:
    body = frame(
        {":exception-type": "throttlingException", ":message-type": "exception"},
        b'{"message":"slow down"}',
    )
    call = decode(
        item(CONVERSE + "-stream", body, "application/vnd.amazon.eventstream")
    )
    assert call is not None and not call.ok
    assert call.error is not None and "throttlingException" in call.error


# --- Anthropic Messages -------------------------------------------------------------------------

INVOKE = "/model/global.anthropic.claude-sonnet-5-5/invoke"


def test_invoke_model_decodes_an_anthropic_message() -> None:
    body = {
        "type": "message",
        "id": "msg_1",
        "model": "claude-sonnet-5-5",
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "Plan.", "signature": "sig"},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "Done."},
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "calculate",
                "input": {"expression": "1+1"},
            },
        ],
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 4,
            "output_tokens": 2,
            "cache_read_input_tokens": 1,
            "cache_creation_input_tokens": 0,
            "output_tokens_details": {"thinking_tokens": 1},
        },
    }
    call = decode(item(INVOKE, json.dumps(body).encode(), "application/json"))
    assert call is not None
    # Bedrock names the inference profile in the path and the base model in the body.
    assert (call.model, call.response_model) == (
        "global.anthropic.claude-sonnet-5-5",
        "claude-sonnet-5-5",
    )
    assert call.response_id == "msg_1"
    assert [p["type"] for p in call.parts] == [
        "reasoning",
        "reasoning",
        "text",
        "tool_call",
    ]
    assert call.parts[1] == wire.reasoning_part(None, redacted=True)
    assert call.usage is not None and call.usage.reasoning == 1


def anthropic_events() -> list[dict[str, Any]]:
    return [
        {
            "type": "message_start",
            "message": {
                "id": "msg_2",
                "model": "claude-sonnet-5-5",
                "content": [],
                "usage": {
                    "input_tokens": 50,
                    "output_tokens": 1,
                    "cache_read_input_tokens": 8,
                },
            },
        },
        {
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "thinking", "thinking": ""},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Wei"},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "gh."},
        },
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "signature_delta", "signature": "s"},
        },
        {"type": "content_block_stop", "index": 0},
        {
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "text", "text": ""},
        },
        {
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "text_delta", "text": "Paris "},
        },
        {
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "text_delta", "text": "is dry."},
        },
        {"type": "content_block_stop", "index": 1},
        {
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_2",
                "name": "get_weather",
                "input": {},
            },
        },
        {
            "type": "content_block_delta",
            "index": 2,
            "delta": {"type": "input_json_delta", "partial_json": ""},
        },
        {
            "type": "content_block_delta",
            "index": 2,
            "delta": {"type": "input_json_delta", "partial_json": '{"city":'},
        },
        {
            "type": "content_block_delta",
            "index": 2,
            "delta": {
                "type": "input_json_delta",
                "partial_json": ' "Paris", "days": 2}',
            },
        },
        {"type": "content_block_stop", "index": 2},
        {
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"input_tokens": 50, "output_tokens": 30},
        },
        {"type": "message_stop"},
    ]


def expected_anthropic_parts() -> list[dict[str, Any]]:
    return [
        wire.reasoning_part("Weigh.", signature="s"),
        wire.text_part("Paris is dry."),
        wire.tool_call_part("toolu_2", "get_weather", {"city": "Paris", "days": 2}),
    ]


def test_invoke_with_response_stream_reassembles_anthropic_deltas() -> None:
    body = bedrock_chunks(anthropic_events())
    call = decode(
        item(
            INVOKE + "-with-response-stream", body, "application/vnd.amazon.eventstream"
        )
    )
    assert call is not None and call.streamed
    assert call.parts == expected_anthropic_parts()
    assert call.finish == "tool_use"
    # message_delta's usage overrides message_start's; what it leaves out is kept.
    assert call.usage is not None
    assert (call.usage.input, call.usage.output, call.usage.cache_read) == (50, 30, 8)


def test_messages_api_stream_decodes_named_server_sent_events() -> None:
    events = [(event["type"], event) for event in anthropic_events()]
    call = decode(item("/v1/messages", sse(events, named=True), "text/event-stream"))
    assert call is not None and call.parts == expected_anthropic_parts()


def test_fake_anthropic_server_output_decodes_to_its_script_reply() -> None:
    body = {
        "model": "claude-sonnet-5-5",
        "messages": [{"role": "user", "content": content.TOOL_USE}],
        "tools": [{"name": "get_weather", "input_schema": {"type": "object"}}],
    }
    message = fake_anthropic.message(body)
    streamed = sse(fake_anthropic.events_of(message), named=True)
    whole = decode(
        item("/v1/messages", json.dumps(message).encode(), "application/json")
    )
    parts = decode(item("/v1/messages", streamed, "text/event-stream"))
    assert whole is not None and parts is not None
    assert whole.parts == parts.parts
    assert [p["arguments"]["city"] for p in whole.tool_calls] == ["Paris", "Tokyo"]


def test_an_anthropic_web_search_is_one_provider_run_with_its_sources() -> None:
    # The call and its result are two blocks on the wire, and one provider-run call in the truth,
    # whether the answer arrives whole or streamed.
    body = {
        "model": "claude-sonnet-5-5",
        "messages": [{"role": "user", "content": content.SERVER_TOOLS}],
        "tools": [{"type": "web_search_20250305", "name": "web_search"}],
    }
    message = fake_anthropic.message(body)
    whole = decode(
        item("/v1/messages", json.dumps(message).encode(), "application/json")
    )
    streamed = decode(
        item(
            "/v1/messages",
            sse(fake_anthropic.events_of(message), named=True),
            "text/event-stream",
        )
    )
    assert whole is not None and streamed is not None
    query, sources = script.SEARCHES[content.SERVER_TOOLS]
    (searched,) = message["content"][:1]
    assert whole.parts == [
        wire.server_call_part(
            searched["id"], "web_search", {"query": query}, {"sources": sources}
        ),
        wire.text_part(script.ANSWERS[content.SERVER_TOOLS]),
    ]
    assert streamed.parts == whole.parts
    assert whole.tool_calls == [] and whole.finish == "stop"


# --- OpenAI -------------------------------------------------------------------------------------


def test_chat_completion_stream_reassembles_split_tool_arguments() -> None:
    chunks = [
        {
            "id": "c1",
            "model": "gpt",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [{"index": 0, "delta": {"content": "On "}}],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [{"index": 0, "delta": {"content": "it."}}],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_a",
                                "function": {
                                    "name": "get_weather",
                                    "arguments": '{"ci',
                                },
                            }
                        ]
                    },
                }
            ],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {"index": 0, "function": {"arguments": 'ty":"Oslo"}'}}
                        ]
                    },
                }
            ],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
        {
            "id": "c1",
            "model": "gpt",
            "choices": [],
            "usage": {"prompt_tokens": 9, "completion_tokens": 4},
        },
        "[DONE]",
    ]
    call = decode(item("/openai/v1/chat/completions", sse(chunks), "text/event-stream"))
    assert call is not None and call.streamed
    assert call.parts == [
        wire.text_part("On it."),
        wire.tool_call_part("call_a", "get_weather", {"city": "Oslo"}),
    ]
    assert call.finish == "tool_use"
    assert call.usage is not None and call.usage.input_includes_cache


def test_fake_chat_completion_whole_and_streamed_decode_alike() -> None:
    body = {
        "model": "gpt-6.1-sol",
        "messages": [{"role": "user", "content": content.CHAT}],
    }
    whole = decode(
        item(
            "/v1/chat/completions",
            json.dumps(fake_openai.completion(body)).encode(),
            "application/json",
        )
    )
    streamed = decode(
        item(
            "/v1/chat/completions",
            sse(
                fake_openai.completion_chunks(
                    {**body, "stream_options": {"include_usage": True}}
                )
            ),
            "text/event-stream",
        )
    )
    assert whole is not None and streamed is not None
    assert (
        whole.parts == streamed.parts == [wire.text_part(script.ANSWERS[content.CHAT])]
    )
    assert whole.usage == streamed.usage


def responses_body() -> dict[str, Any]:
    return {
        "id": "resp_1",
        "model": "global.openai.gpt-6.1-sol",
        "status": "completed",
        "instructions": content.SYSTEM,
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [],
                "content": [],
                "encrypted_content": "x",
            },
            {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "Checking Rome.", "annotations": []}
                ],
            },
            {
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "get_weather",
                "arguments": '{"city":"Rome","days":1}',
            },
        ],
        "usage": {
            "input_tokens": 20,
            "input_tokens_details": {"cached_tokens": 4},
            "output_tokens": 6,
            "output_tokens_details": {"reasoning_tokens": 2},
        },
    }


def test_responses_echoes_the_system_prompt_and_reports_encrypted_reasoning() -> None:
    call = decode(
        item(
            "/openai/v1/responses",
            json.dumps(responses_body()).encode(),
            "application/json",
        )
    )
    assert call is not None
    assert call.system_echo == content.SYSTEM
    assert call.parts[0] == wire.reasoning_part(None, signature="x")
    assert call.parts[2] == wire.tool_call_part(
        "call_1", "get_weather", {"city": "Rome", "days": 1}
    )
    assert call.finish == "tool_use"
    assert call.usage is not None and (call.usage.cache_read, call.usage.reasoning) == (
        4,
        2,
    )


def responses_stream_events(final_text: str = "Checking Rome.") -> list[Any]:
    final = responses_body()
    final["output"][1]["content"][0]["text"] = final_text
    reasoning, message, function = final["output"]
    return [
        {
            "type": "response.created",
            "response": {**final, "status": "in_progress", "output": []},
        },
        {"type": "response.output_item.added", "output_index": 0, "item": reasoning},
        {"type": "response.output_item.done", "output_index": 0, "item": reasoning},
        {
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {**message, "content": []},
        },
        {
            "type": "response.content_part.added",
            "output_index": 1,
            "content_index": 0,
            "part": {"type": "output_text", "text": ""},
        },
        {
            "type": "response.output_text.delta",
            "output_index": 1,
            "content_index": 0,
            "delta": "Checking ",
        },
        {
            "type": "response.output_text.delta",
            "output_index": 1,
            "content_index": 0,
            "delta": "Rome.",
        },
        {
            "type": "response.output_text.done",
            "output_index": 1,
            "content_index": 0,
            "text": "Checking Rome.",
        },
        {
            "type": "response.output_item.added",
            "output_index": 2,
            "item": {**function, "arguments": ""},
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 2,
            "delta": '{"city":"Ro',
        },
        {
            "type": "response.function_call_arguments.delta",
            "output_index": 2,
            "delta": 'me","days":1}',
        },
        {"type": "response.output_item.done", "output_index": 2, "item": function},
        {"type": "response.completed", "response": final},
        "[DONE]",
    ]


def test_responses_stream_reassembles_deltas_and_matches_the_completed_response() -> (
    None
):
    call = decode(
        item(
            "/openai/v1/responses", sse(responses_stream_events()), "text/event-stream"
        )
    )
    whole = decode(
        item(
            "/openai/v1/responses",
            json.dumps(responses_body()).encode(),
            "application/json",
        )
    )
    assert call is not None and whole is not None
    assert call.streamed and call.parts == whole.parts
    assert call.finish == "tool_use"


def _search_item(**action: Any) -> dict[str, Any]:
    return {
        "id": "ws_1",
        "type": "web_search_call",
        "status": "completed",
        "action": {"type": "search", **action},
    }


def test_a_responses_web_search_owes_every_query_it_ran_and_every_source() -> None:
    searched = _search_item(
        query="louvre hours",
        queries=["louvre hours", "louvre tuesday"],
        sources=[
            {"type": "url", "url": "https://a"},
            {"type": "url", "url": "https://b"},
        ],
    )
    body = {**responses_body(), "output": [searched]}
    call = decode(
        item("/openai/v1/responses", json.dumps(body).encode(), "application/json")
    )
    assert call is not None
    assert call.parts == [
        wire.server_call_part(
            "ws_1",
            "web_search",
            {"query": "louvre hours", "queries": ["louvre hours", "louvre tuesday"]},
            {"sources": ["https://a", "https://b"]},
        )
    ]
    # Opening a page is a call too: what it opened is what it asked for.
    opened = {**searched, "action": {"type": "open_page", "url": "https://a"}}
    body = {**responses_body(), "output": [opened]}
    call = decode(
        item("/openai/v1/responses", json.dumps(body).encode(), "application/json")
    )
    assert call is not None and call.parts[0]["arguments"] == {"url": "https://a"}


def test_a_streamed_web_search_is_what_its_done_item_says() -> None:
    # The provider adds the item before it has searched; no delta fills it in, the done item does.
    started = {"id": "ws_1", "type": "web_search_call", "status": "in_progress"}
    finished = _search_item(
        query="louvre hours", sources=[{"type": "url", "url": "https://a"}]
    )
    final = {**responses_body(), "output": [finished]}
    events = [
        {"type": "response.created", "response": {**final, "output": []}},
        {"type": "response.output_item.added", "output_index": 0, "item": started},
        {"type": "response.web_search_call.in_progress", "output_index": 0},
        {"type": "response.output_item.done", "output_index": 0, "item": finished},
        {"type": "response.completed", "response": final},
    ]
    call = decode(item("/openai/v1/responses", sse(events), "text/event-stream"))
    assert call is not None
    assert call.parts == [
        wire.server_call_part(
            "ws_1", "web_search", {"query": "louvre hours"}, {"sources": ["https://a"]}
        )
    ]


def test_responses_stream_whose_deltas_disagree_with_the_snapshot_is_an_error() -> None:
    with pytest.raises(DecodeError, match="disagree"):
        decode(
            item(
                "/openai/v1/responses",
                sse(responses_stream_events("Other.")),
                "text/event-stream",
            )
        )


def test_fake_responses_reasoning_summary_decodes_as_visible_reasoning() -> None:
    body = {
        "model": "gpt-6.1-sol",
        "input": [{"type": "message", "role": "user", "content": content.REASONING}],
        "reasoning": {"summary": "detailed"},
    }
    call = decode(
        item(
            "/v1/responses",
            json.dumps(fake_openai.response(body)).encode(),
            "application/json",
        )
    )
    assert call is not None
    assert call.parts == [
        wire.reasoning_part(script.THOUGHTS[content.REASONING]),
        wire.text_part(script.ANSWERS[content.REASONING]),
    ]


def test_a_failed_call_is_kept_with_its_error_and_a_non_model_request_is_none() -> None:
    error = json.dumps(
        {"error": {"message": "server had an error", "type": "server_error"}}
    )
    call = decode(
        item("/openai/v1/responses", error.encode(), "application/json", status=500)
    )
    assert call is not None and not call.ok and call.error == "server had an error"
    profiles = item(
        "/inference-profiles?type=SYSTEM_DEFINED",
        b"<UnknownOperationException/>",
        "",
        404,
    )
    assert decode(profiles) is None


# --- Gemini -------------------------------------------------------------------------------------


def test_gemini_stream_joins_split_text_and_keeps_thoughts_apart() -> None:
    body = {
        "contents": [{"role": "user", "parts": [{"text": content.REASONING}]}],
        "generationConfig": {"thinkingConfig": {"includeThoughts": True}},
    }
    parts = fake_gemini.parts_of(script.reply(fake_gemini.request_of(body)))
    chunks = fake_gemini.stream_of(parts, "r1")
    assert len(chunks) == 4  # the thought and the answer, each split in two
    path = f"/v1beta/models/{fake_gemini.MODEL_VERSION}:streamGenerateContent"
    call = decode(item(path, sse(chunks), "text/event-stream"))
    assert call is not None
    assert call.parts == [
        wire.reasoning_part(script.THOUGHTS[content.REASONING]),
        wire.text_part(script.ANSWERS[content.REASONING]),
    ]
    assert (call.response_model, call.finish) == (fake_gemini.MODEL_VERSION, "stop")


def test_gemini_function_calls_finish_as_tool_use() -> None:
    body = {
        "contents": [{"role": "user", "parts": [{"text": content.STREAMING}]}],
        "tools": [
            {
                "functionDeclarations": [
                    {"name": "get_weather", "parametersJsonSchema": {}}
                ]
            }
        ],
    }
    parts = fake_gemini.parts_of(script.reply(fake_gemini.request_of(body)))
    response = fake_gemini.chunk(parts, final=True, response_id="r2")
    path = f"/v1beta/models/{fake_gemini.MODEL_VERSION}:generateContent"
    call = decode(item(path, json.dumps(response).encode(), "application/json"))
    assert call is not None and call.finish == "tool_use"
    assert call.tool_calls[0]["arguments"] == {"city": "Rome", "days": 1}


# --- Every committed cassette -------------------------------------------------------------------


def cassettes() -> list[Path]:
    return sorted(
        path
        for root in (capture.PYTHON_SUITES, capture.JAVASCRIPT_EXAMPLES)
        for path in root.glob("*/cassettes/*.json")
    )


def test_every_committed_cassette_decodes_completely() -> None:
    decoded = 0
    for path in cassettes():
        calls, _ = wire.decode_cassette(path)
        assert calls, f"{path} records no model call"
        for call in calls:
            assert call.ok or call.status >= 300, f"{path}: {call.error}"
            if call.ok:
                assert call.parts, f"{path}: a successful call with no output"
                assert call.finish in ("stop", "tool_use", "max_tokens"), (
                    f"{path}: {call.finish}"
                )
                decoded += 1
    assert decoded > 400


def test_signed_reasoning_is_sealed_by_its_signature_digest_never_by_the_signature() -> (
    None
):
    import hashlib

    part = wire.reasoning_part("", signature="abc")
    assert part["signed"] is True
    assert part["seal"] == hashlib.sha256(b"abc").hexdigest()
    assert "abc" not in json.dumps({k: v for k, v in part.items() if k != "seal"})
    assert wire.reasoning_part("", signature="abd")["seal"] != part["seal"]
    assert "seal" not in wire.reasoning_part("Visible.", signed=True)
