"""The shared example tools as OpenInference tools with Converse tool specs."""

from __future__ import annotations

import re
from collections.abc import Callable
from typing import Any

from converse import Tool
from pydantic import TypeAdapter
from tracing import tracer

from harness import content


def spec(function: Callable[..., Any]) -> dict[str, Any]:
    """A Converse ``toolSpec`` from a function's signature and Google-style docstring."""
    doc = function.__doc__ or ""
    schema = TypeAdapter(function).json_schema()
    for name, description in re.findall(r"^\s+(\w+): (.+)$", doc, re.MULTILINE):
        if name in schema["properties"]:
            schema["properties"][name]["description"] = description
    return {
        "name": function.__name__,
        "description": doc.strip().splitlines()[0],
        "inputSchema": {"json": schema},
    }


def _wrap(function: Callable[..., Any]) -> Tool:
    tool_spec = spec(function)
    traced = tracer.tool(
        name=tool_spec["name"],
        description=tool_spec["description"],
        parameters=tool_spec["inputSchema"]["json"],
    )(function)
    return Tool(tool_spec, traced)


get_weather = _wrap(content.get_weather)
get_precipitation = _wrap(content.get_precipitation)
book_flight = _wrap(content.book_flight)
