"""The truth decoder reads what a provider adds to an answer: tools it ran itself, and citations."""

from __future__ import annotations

import pytest

from harness.truth import wire
from harness.truth.wire import DecodeError


def test_a_cited_converse_answer_is_its_text() -> None:
    # With citations enabled, the answer arrives split around the passages it cites.
    cited = {
        "citationsContent": {
            "content": [{"text": "It opens at 9 am"}],
            "citations": [
                {
                    "title": "guide",
                    "sourceContent": [{"text": "The Louvre is open from 9 am."}],
                    "location": {
                        "documentChar": {"documentIndex": 0, "start": 0, "end": 29}
                    },
                }
            ],
        }
    }
    body = {
        "output": {
            "message": {
                "role": "assistant",
                "content": [{"text": "According to the guide, "}, cited, {"text": "."}],
            }
        },
        "stopReason": "end_turn",
    }
    call = wire.converse(body)
    assert call.parts == [
        wire.text_part("According to the guide, "),
        wire.text_part("It opens at 9 am"),
        wire.text_part("."),
    ]
    with pytest.raises(DecodeError, match="cited content"):
        wire.converse(
            {
                "output": {
                    "message": {"content": [{"citationsContent": {"content": [{}]}}]}
                }
            }
        )


def test_an_anthropic_search_error_is_a_failed_provider_run() -> None:
    message = {
        "type": "message",
        "id": "msg_1",
        "model": "claude-sonnet-5-5",
        "stop_reason": "end_turn",
        "content": [
            {
                "type": "server_tool_use",
                "id": "srvtoolu_1",
                "name": "web_search",
                "input": {"query": "q"},
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_1",
                "content": {
                    "type": "web_search_tool_result_error",
                    "error_code": "max_uses_exceeded",
                },
            },
        ],
    }
    call = wire.anthropic_message(message)
    assert call.parts == [
        wire.server_call_part(
            "srvtoolu_1",
            "web_search",
            {"query": "q"},
            {"error_code": "max_uses_exceeded"},
            is_error=True,
        )
    ]


def test_an_anthropic_search_result_for_no_call_is_refused() -> None:
    orphan = {
        "type": "web_search_tool_result",
        "tool_use_id": "srvtoolu_9",
        "content": [],
    }
    with pytest.raises(DecodeError, match="no call"):
        wire.anthropic_message(
            {"type": "message", "id": "msg_1", "content": [orphan], "stop_reason": None}
        )
