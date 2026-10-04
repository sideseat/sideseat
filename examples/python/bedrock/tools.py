"""The shared example tools as Converse tool specifications, and the loop that runs them.

A provider client has no agent loop of its own: the application sends the tool results back until the
model answers.
"""

import inspect
from collections.abc import Callable
from typing import Any

from models import Bedrock

from harness import content

_TYPES = {str: "string", int: "integer"}


def spec(function: Callable[..., Any]) -> dict[str, Any]:
    """A toolSpec from the function's signature and the descriptions in its docstring."""
    doc = inspect.getdoc(function) or ""
    summary, _, args = doc.partition("Args:")
    described = dict(
        line.strip().split(": ", 1) for line in args.splitlines() if ": " in line
    )
    signature = inspect.signature(function, eval_str=True)
    properties = {
        name: {"type": _TYPES[parameter.annotation], "description": described[name]}
        for name, parameter in signature.parameters.items()
    }
    required = [
        name
        for name, parameter in signature.parameters.items()
        if parameter.default is inspect.Parameter.empty
    ]
    return {
        "toolSpec": {
            "name": function.__name__,
            "description": summary.strip(),
            "inputSchema": {
                "json": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }
            },
        }
    }


def call(tools: dict[str, Callable[..., Any]], use: dict[str, Any]) -> dict[str, Any]:
    """Runs one toolUse block and returns its toolResult block, reporting a failure as an error."""
    try:
        result = tools[use["name"]](**use["input"])
    except (
        Exception
    ) as error:  # the model is told about the failure, as an agent loop would
        return {
            "toolResult": {
                "toolUseId": use["toolUseId"],
                "content": [{"text": str(error)}],
                "status": "error",
            }
        }
    payload = {"json": result} if isinstance(result, dict) else {"text": str(result)}
    return {"toolResult": {"toolUseId": use["toolUseId"], "content": [payload]}}


def converse(
    llm: Bedrock,
    question: str,
    functions: list[Callable[..., Any]],
) -> str:
    """Converse until the model stops asking for tools, and return its final text."""
    tools = {function.__name__: function for function in functions}
    messages: list[dict[str, Any]] = [{"role": "user", "content": [{"text": question}]}]
    while True:
        response = llm.client.converse(
            modelId=llm.model_id,
            system=[{"text": content.SYSTEM}],
            messages=messages,
            toolConfig={"tools": [spec(function) for function in functions]},
        )
        message = response["output"]["message"]
        messages.append(message)
        uses = [block["toolUse"] for block in message["content"] if "toolUse" in block]
        if not uses:
            return answer(message)
        messages.append({"role": "user", "content": [call(tools, use) for use in uses]})


def answer(message: dict[str, Any]) -> str:
    """The text of an assistant message. Current Claude models may lead with a reasoning block."""
    return "".join(block.get("text", "") for block in message["content"])
