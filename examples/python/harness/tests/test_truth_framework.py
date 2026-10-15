"""What a suite declares about its framework's own effects on a conversation: turns it states and never sends."""

from __future__ import annotations

from typing import Any

import pytest

from harness.truth.framework import Framework, unsent_gaps


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
