"""The shared tools as Anthropic tool definitions, and the application side of the tool loop."""

from typing import Any

from models import MAX_TOKENS, Claude

from harness import content
from harness.tooling import execute, spec

FUNCTIONS = {
    function.__name__: function
    for function in (
        content.get_weather,
        content.get_precipitation,
        content.book_flight,
    )
}


def definitions(*names: str) -> list[dict[str, Any]]:
    return [
        {
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.parameters,
        }
        for tool in (spec(FUNCTIONS[name]) for name in names)
    ]


def results(blocks: list[Any]) -> list[dict[str, Any]]:
    """The user turn answering every ``tool_use`` block of an assistant response."""
    answers = []
    for block in blocks:
        if block.type != "tool_use":
            continue
        outcome = execute(FUNCTIONS, block.name, block.input)
        answers.append(
            {
                "type": "tool_result",
                "tool_use_id": block.id,
                "content": outcome.text,
                "is_error": outcome.is_error,
            }
        )
    return answers


def assistant_turn(response: Any) -> dict[str, Any]:
    # Plain dictionaries, as an application stores history: the response's block objects would be
    # re-sent correctly, but instrumentation reading the request records them only as strings.
    return {
        "role": "assistant",
        "content": [
            block.model_dump(mode="json", exclude_none=True)
            for block in response.content
        ],
    }


def converse(claude: Claude, messages: list[dict[str, Any]], **request: Any) -> str:
    """Calls the model until it stops asking for tools; returns the final text."""
    while True:
        response = claude.client.messages.create(
            model=claude.model,
            system=content.SYSTEM,
            max_tokens=MAX_TOKENS,
            messages=messages,
            **request,
        )
        messages.append(assistant_turn(response))
        if response.stop_reason != "tool_use":
            return "".join(
                block.text for block in response.content if block.type == "text"
            )
        messages.append({"role": "user", "content": results(response.content)})
