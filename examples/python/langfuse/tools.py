"""The shared tools as OpenAI function definitions, and the application side of each API's loop.

The loop and each tool run inside Langfuse ``@observe`` observations, which is how a Langfuse user
instruments their own code around the wrapped OpenAI client.

Conversations without tools use Chat Completions. Tools go through the Responses API: current GPT
models always reason, and Bedrock serves function tools to a reasoning model only there. Both loops
are stateless and re-send the whole conversation on each request.
"""

from collections.abc import Sequence
from typing import Any

from langfuse import observe
from models import OpenAIModel

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
    """Responses API function tools."""
    return [
        {
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        }
        for tool in (spec(FUNCTIONS[name]) for name in names)
    ]


def system() -> dict[str, str]:
    return {"role": "system", "content": content.SYSTEM}


def chat(model: OpenAIModel, messages: list[dict[str, Any]], **request: Any) -> str:
    """One Chat Completions request; the answer joins ``messages``."""
    response = model.client.chat.completions.create(
        model=model.id, messages=messages, **request
    )
    answer = response.choices[0].message.content or ""
    messages.append({"role": "assistant", "content": answer})
    return answer


@observe(name="agent", as_type="agent")
def respond(
    model: OpenAIModel,
    items: list[dict[str, Any]],
    *,
    tools: Sequence[dict[str, Any]] = (),
    stream: bool = False,
    **request: Any,
) -> str:
    """Calls the Responses API until the model stops calling tools; returns the final text."""
    while True:
        response = _create(model, items, tools, stream, request)
        output = [item.model_dump(exclude_none=True) for item in response.output]
        items.extend(output)
        calls = [item for item in output if item["type"] == "function_call"]
        if not calls:
            return str(response.output_text)
        for call in calls:
            items.append(
                {
                    "type": "function_call_output",
                    "call_id": call["call_id"],
                    "output": _run_tool(call["name"], call.get("arguments") or "{}"),
                }
            )


@observe(as_type="tool", capture_input=True, capture_output=True)
def _run_tool(name: str, arguments: str) -> str:
    return execute(FUNCTIONS, name, arguments).text


def _create(
    model: OpenAIModel,
    items: list[dict[str, Any]],
    tools: Sequence[dict[str, Any]],
    stream: bool,
    request: dict[str, Any],
) -> Any:
    fields: dict[str, Any] = {
        "model": model.id,
        "instructions": content.SYSTEM,
        "input": items,
        **request,
    }
    if tools:
        fields["tools"] = list(tools)
    if not stream:
        return model.client.responses.create(**fields)
    completed = None
    for event in model.client.responses.create(**fields, stream=True):
        if event.type == "response.output_text.delta":
            print(event.delta, end="", flush=True)
        elif event.type == "response.completed":
            completed = event.response
    print()
    if completed is None:
        raise RuntimeError("the response stream ended without completing")
    return completed
