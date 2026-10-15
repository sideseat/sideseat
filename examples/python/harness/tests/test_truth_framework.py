"""What a suite declares about its framework's own effects on a conversation: turns it states and never sends,
and tool results it hands the model as its own rendering."""

from __future__ import annotations

from typing import Any

import pytest

from harness.truth.framework import Framework, rendered_results, unsent_gaps


def _request(*messages: tuple[str, list[str]]) -> dict[str, Any]:
    return {
        "messages": [
            {"role": role, "parts": [{"part": {"type": kind}} for kind in kinds]}
            for role, kinds in messages
        ]
    }


def test_an_unsent_turn_is_declared_for_requests_ending_in_tool_results_only() -> None:
    framework = Framework.of(
        {
            "states_unsent": [
                {
                    "text": "Handle it.",
                    "reason": "appended, then dropped by the client",
                    "source": "client.py:1",
                    "scenarios": ["tool_use"],
                    "modes": {"tool_use": ["native@1.0"]},
                }
            ]
        }
    )
    requests = {
        "p/native@1.0/tool_use": {
            "calls": {
                "call-001": _request(("user", ["text"])),
                "call-002": _request(
                    ("user", ["text"]), ("user", ["tool_result", "tool_result"])
                ),
                "call-003": _request(("user", ["tool_result", "text"])),
            }
        },
        "p/native@2.0/tool_use": {
            "calls": {"call-002": _request(("user", ["tool_result"]))}
        },
    }
    gaps = unsent_gaps(framework, "tool_use", requests)
    assert [(g["subject"], g["modes"], g["text"]) for g in gaps] == [
        ("call-002", ["native@1.0"], "Handle it.")
    ]
    assert unsent_gaps(framework, "chat", requests) == []


def test_an_unsent_turn_without_its_modes_is_refused() -> None:
    with pytest.raises(ValueError):
        Framework.of(
            {
                "states_unsent": [
                    {
                        "text": "Handle it.",
                        "reason": "dropped",
                        "source": "client.py:1",
                        "scenarios": ["tool_use"],
                    }
                ]
            }
        )


_RENDERED = (
    "meta={'fastmcp': {'wrap_result': True}} content=[TextContent(type='text', text='395.0', "
    "annotations=None, meta=None)] structured_content={'result': 395.0} is_error=False "
    "result_type='complete'"
)


def _rendering_document(text: str) -> tuple[dict[str, Any], dict[str, Any]]:
    document = {
        "facts": [
            {
                "id": "fact-003",
                "kind": "tool_result",
                "value": {
                    "call_id": "c1",
                    "name": "calculate",
                    "value": 395,
                    "is_error": False,
                },
            }
        ]
    }
    part = {
        "part": {"type": "tool_result", "content": [{"type": "text", "text": text}]},
        "new_fact": "fact-003",
    }
    requests = {
        "p/native/mcp_tools": {
            "calls": {"call-002": {"messages": [{"role": "user", "parts": [part]}]}}
        }
    }
    return document, requests


_RENDERS = Framework.of(
    {
        "result_renderings": [
            {
                "format": "mcp_call_tool_result_str",
                "source": "f.py:1",
                "scenarios": ["mcp_tools"],
            }
        ]
    }
)


def test_a_rendered_result_is_the_text_the_model_was_sent() -> None:
    document, requests = _rendering_document(_RENDERED)
    rendered_results(_RENDERS, "mcp_tools", document, requests)
    assert document["facts"][0]["value"]["value"] == _RENDERED


@pytest.mark.parametrize(
    "text",
    [
        '{"result": 395.0}',
        _RENDERED.replace("395.0", "396.0"),
        _RENDERED.replace("is_error=False", "is_error=True"),
        _RENDERED.replace(
            "structured_content={'result': 395.0}",
            "structured_content={'value': 395.0}",
        ),
        "395.0",
        _RENDERED.replace(
            "structured_content={'result': 395.0}",
            "structured_content={'result': '395.0'}",
        ),
        _RENDERED.replace(
            "structured_content={'result': 395.0}",
            "structured_content={'result': True}",
        ),
        _RENDERED.replace("text='395.0'", "text='three hundred and ninety-five'"),
    ],
)
def test_a_different_rendering_is_refused(text: str) -> None:
    document, requests = _rendering_document(text)
    with pytest.raises(ValueError):
        rendered_results(_RENDERS, "mcp_tools", document, requests)


def test_a_result_sent_another_way_by_another_fixture_is_refused() -> None:
    document, requests = _rendering_document(_RENDERED)
    requests["p/sdk/mcp_tools"] = {
        "calls": {
            "call-002": {
                "messages": [
                    {
                        "role": "user",
                        "parts": [
                            {
                                "part": {
                                    "type": "tool_result",
                                    "content": [
                                        {"type": "text", "text": '{"result": 395.0}'}
                                    ],
                                },
                                "new_fact": "fact-003",
                            }
                        ],
                    }
                ]
            }
        }
    }
    with pytest.raises(ValueError):
        rendered_results(_RENDERS, "mcp_tools", document, requests)
    document, requests = _rendering_document(_RENDERED)
    part = requests["p/native/mcp_tools"]["calls"]["call-002"]["messages"][0]["parts"][
        0
    ]
    part["part"]["content"].append({"type": "text", "text": "more"})
    with pytest.raises(ValueError):
        rendered_results(_RENDERS, "mcp_tools", document, requests)
