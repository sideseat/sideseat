"""The shared example tools as AutoGen function tools."""

import inspect
from collections.abc import Callable
from typing import Any

from autogen_core.tools import FunctionTool

from harness import content


def _tool(function: Callable[..., Any]) -> FunctionTool:
    return FunctionTool(
        function, description=inspect.getdoc(function) or function.__name__
    )


get_weather = _tool(content.get_weather)
get_precipitation = _tool(content.get_precipitation)
book_flight = _tool(content.book_flight)
