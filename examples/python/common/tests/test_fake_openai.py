"""Regression tests for the deterministic OpenAI-compatible fixture server."""

import importlib.util
import json
from pathlib import Path
from types import ModuleType


def _fake_openai() -> ModuleType:
    path = (
        Path(__file__).resolve().parents[4]
        / "scripts"
        / "message-fixtures"
        / "fake-openai.py"
    )
    spec = importlib.util.spec_from_file_location("sideseat_fake_openai", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _tool(name: str, properties: dict) -> dict:
    return {
        "type": "function",
        "function": {
            "name": name,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": list(properties),
            },
        },
    }


def test_action_observation_protocol_finishes_with_final_answer() -> None:
    """A user-role observation is a tool result, not a request to repeat the tool."""
    fake = _fake_openai()
    body = {
        "model": "sideseat-local",
        "messages": [
            {"role": "system", "content": "Use tools, then call final_answer."},
            {"role": "user", "content": "What is the weather in Paris?"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call-weather",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": '{"location":"Paris"}',
                        },
                    }
                ],
            },
            {"role": "user", "content": "Observation:\nSunny, 22°C in Paris."},
        ],
        "tools": [
            _tool(
                "get_weather",
                {"location": {"type": "string", "description": "City"}},
            ),
            _tool(
                "final_answer",
                {"answer": {"description": "Final answer"}},
            ),
        ],
    }

    choice = fake.completion(body)["choices"][0]
    call = choice["message"]["tool_calls"][0]["function"]

    assert choice["finish_reason"] == "tool_calls"
    assert call["name"] == "final_answer"
    assert json.loads(call["arguments"]) == {"answer": "It is sunny and 22°C in Paris."}


def test_final_answer_uses_the_current_scientific_question() -> None:
    """A final-answer-only agent receives deterministic task-specific content."""
    fake = _fake_openai()
    body = {
        "model": "sideseat-local",
        "messages": [
            {"role": "system", "content": "Answer through final_answer."},
            {"role": "user", "content": "What is the speed of light?"},
        ],
        "tools": [
            _tool(
                "final_answer",
                {"answer": {"description": "Final answer"}},
            )
        ],
    }

    call = fake.completion(body)["choices"][0]["message"]["tool_calls"][0]["function"]
    assert json.loads(call["arguments"]) == {
        "answer": "The speed of light is 299,792,458 metres per second."
    }


def test_retained_observation_does_not_hide_a_new_task() -> None:
    """Smolagents history must answer the task after its retained observation."""
    fake = _fake_openai()
    body = {
        "model": "sideseat-local",
        "messages": [
            {"role": "system", "content": "Answer through final_answer."},
            {"role": "user", "content": "What is the speed of light?"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call-final-answer",
                        "type": "function",
                        "function": {
                            "name": "final_answer",
                            "arguments": json.dumps(
                                {
                                    "answer": (
                                        "The speed of light is 299,792,458 metres "
                                        "per second."
                                    )
                                }
                            ),
                        },
                    }
                ],
            },
            {
                "role": "user",
                "content": (
                    "Observation:\n"
                    "The speed of light is 299,792,458 metres per second.\n"
                    "New task:\n"
                    "What is the boiling point of water?"
                ),
            },
        ],
        "tools": [
            _tool(
                "final_answer",
                {"answer": {"description": "Final answer"}},
            )
        ],
    }

    call = fake.completion(body)["choices"][0]["message"]["tool_calls"][0]["function"]
    assert json.loads(call["arguments"]) == {
        "answer": "Water boils at 100°C at sea level."
    }
