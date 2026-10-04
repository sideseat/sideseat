"""The shared example tools with Responses API function definitions."""

from __future__ import annotations

import re
from collections.abc import Callable
from typing import Any

from conversation import Tool
from pydantic import TypeAdapter

from harness import content


def spec(function: Callable[..., Any]) -> dict[str, Any]:
    """A function tool definition from a function's signature and Google-style docstring."""
    doc = function.__doc__ or ""
    schema = TypeAdapter(function).json_schema()
    for name, description in re.findall(r"^\s+(\w+): (.+)$", doc, re.MULTILINE):
        if name in schema["properties"]:
            schema["properties"][name]["description"] = description
    return {
        "type": "function",
        "name": function.__name__,
        "description": doc.strip().splitlines()[0],
        "parameters": schema,
    }


get_weather = Tool(spec(content.get_weather), content.get_weather)
get_precipitation = Tool(spec(content.get_precipitation), content.get_precipitation)
book_flight = Tool(spec(content.book_flight), content.book_flight)
