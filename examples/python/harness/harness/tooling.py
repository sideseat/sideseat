"""The shared tools described for provider SDKs that take JSON-schema tool definitions.

Agent frameworks wrap :mod:`harness.content` functions with their own decorators. Provider client
libraries take a name, a description, and a JSON schema instead, and leave executing the call to the
application; :func:`spec` derives the definition from the function's signature and docstring, and
:func:`execute` runs a call the way an application would, turning an exception into an error result
the model can read.
"""

from __future__ import annotations

import inspect
import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

_JSON_TYPES = {str: "string", int: "integer", float: "number", bool: "boolean"}


@dataclass(frozen=True)
class ToolSpec:
    name: str
    description: str
    parameters: dict[str, Any]


def spec(function: Callable[..., Any]) -> ToolSpec:
    """The tool definition of a :mod:`harness.content` function."""
    doc = inspect.getdoc(function) or ""
    summary, _, rest = doc.partition("\n\n")
    described = _argument_descriptions(rest)
    properties: dict[str, Any] = {}
    required: list[str] = []
    hints = inspect.get_annotations(function, eval_str=True)
    for name, parameter in inspect.signature(function).parameters.items():
        schema: dict[str, Any] = {"type": _JSON_TYPES[hints[name]]}
        if name in described:
            schema["description"] = described[name]
        properties[name] = schema
        if parameter.default is inspect.Parameter.empty:
            required.append(name)
    return ToolSpec(
        name=function.__name__,
        description=summary.strip(),
        parameters={"type": "object", "properties": properties, "required": required},
    )


def _argument_descriptions(text: str) -> dict[str, str]:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "Args:":
        return {}
    result = {}
    for line in lines[1:]:
        name, separator, description = line.strip().partition(":")
        if separator:
            result[name.strip()] = description.strip()
    return result


@dataclass(frozen=True)
class Outcome:
    #: The result as the model reads it: JSON for structured results, the message for an error.
    text: str
    is_error: bool


def execute(
    tools: Mapping[str, Callable[..., Any]],
    name: str,
    arguments: str | Mapping[str, Any],
) -> Outcome:
    """Runs one tool call; an exception becomes an error outcome rather than ending the run."""
    parsed = json.loads(arguments) if isinstance(arguments, str) else dict(arguments)
    try:
        value = tools[name](**parsed)
    except Exception as error:
        return Outcome(f"{type(error).__name__}: {error}", is_error=True)
    return Outcome(
        value if isinstance(value, str) else json.dumps(value), is_error=False
    )
