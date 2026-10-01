"""Generic Logfire history, streaming, tools, traces, and sessions."""

from typing import Any

import logfire

SYSTEM_PROMPT = "Answer scientific questions in one sentence."
FIRST_QUESTION = "What is the speed of light?"
FIRST_ANSWER = "Light travels at 299,792,458 metres per second in vacuum."
SECOND_QUESTION = "What is the boiling point of water?"
SECOND_ANSWER = "Water boils at 100°C at standard atmospheric pressure."
THIRD_QUESTION = "Which planet is known as the Red Planet?"
THIRD_ANSWER = "Mars is known as the Red Planet."
TOOL_SYSTEM_PROMPT = "Use tools when they are available."
TOOL_QUESTION = "What is the weather in Paris?"
TOOL_RESULT = "Sunny, 22°C, light breeze in Paris."
TOOL_ANSWER = "The weather in Paris is sunny at 22°C with a light breeze."
TOOL_CALL_ID = "call-weather-1"


def _chat_attributes(model_id: str) -> dict[str, str]:
    """Return stable GenAI attributes shared by each Logfire span."""
    return {
        "gen_ai.operation.name": "chat",
        "gen_ai.request.model": model_id,
        "gen_ai.response.model": model_id,
    }


def _history(model_id: str, client: Any, session_id: str, user_id: str) -> None:
    """Emit event and prompt carriers whose repeated history must collapse."""
    first_turn = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": FIRST_QUESTION},
    ]
    with client.trace(
        "logfire-history",
        session_id=session_id,
        user_id=user_id,
    ):
        with logfire.span(
            "logfire events request",
            events=[
                {
                    "event.name": "gen_ai.system.message",
                    "content": SYSTEM_PROMPT,
                },
                {
                    "event.name": "gen_ai.user.message",
                    "content": FIRST_QUESTION,
                },
            ],
            request_data={
                "model": model_id,
                "messages": first_turn,
            },
            response_data={
                "message": {
                    "role": "assistant",
                    "content": FIRST_ANSWER,
                }
            },
            **_chat_attributes(model_id),
        ):
            pass

        with logfire.span(
            "logfire prompt streaming response",
            prompt=[
                *first_turn,
                {"role": "assistant", "content": FIRST_ANSWER},
                {"role": "user", "content": SECOND_QUESTION},
            ],
            response_data={
                "combined_chunk_content": SECOND_ANSWER,
                "chunk_count": 3,
            },
            **_chat_attributes(model_id),
        ):
            pass

        with logfire.span(
            "logfire request data fallback",
            request_data={
                "model": model_id,
                "messages": [
                    {"role": "user", "content": THIRD_QUESTION},
                ],
            },
            response_data={
                "message": {
                    "role": "assistant",
                    "content": THIRD_ANSWER,
                }
            },
            **_chat_attributes(model_id),
        ):
            pass


def _tool_roundtrip(
    model_id: str,
    client: Any,
    session_id: str,
    user_id: str,
) -> None:
    """Emit a complete tool conversation plus its request-side definition."""
    conversation = [
        {"role": "system", "content": TOOL_SYSTEM_PROMPT},
        {"role": "user", "content": TOOL_QUESTION},
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {
                    "id": TOOL_CALL_ID,
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": '{"location":"Paris"}',
                    },
                }
            ],
        },
        {
            "role": "tool",
            "tool_call_id": TOOL_CALL_ID,
            "name": "get_weather",
            "content": TOOL_RESULT,
        },
        {"role": "assistant", "content": TOOL_ANSWER},
    ]
    with (
        client.trace(
            "logfire-tool-roundtrip",
            session_id=session_id,
            user_id=user_id,
        ),
        logfire.span(
            "logfire all messages tool roundtrip",
            all_messages_events=conversation,
            request_data={
                "model": model_id,
                "messages": conversation[:2],
                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "description": "Get deterministic weather for a city.",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "location": {"type": "string"},
                                },
                                "required": ["location"],
                            },
                        },
                    }
                ],
            },
            response_data={
                "message": {
                    "role": "assistant",
                    "content": TOOL_ANSWER,
                }
            },
            **_chat_attributes(model_id),
        ),
    ):
        pass


def run(model_id: str, trace_attrs: dict[str, str], client: Any) -> None:
    """Emit two traces sharing one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]
    _history(model_id, client, session_id, user_id)
    _tool_roundtrip(model_id, client, session_id, user_id)
