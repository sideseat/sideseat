"""The shared example tools as functions the Google Gen AI client can call itself."""

import types
import typing
from collections.abc import Callable
from typing import Any

from harness import content


def _callable(function: Callable[..., Any]) -> Callable[..., Any]:
    # The client builds each declaration from the function's annotations and rejects the strings
    # that postponed evaluation leaves there, so the copy carries the evaluated types.
    copy = types.FunctionType(
        function.__code__,
        function.__globals__,
        function.__name__,
        function.__defaults__,
    )
    copy.__doc__ = function.__doc__
    copy.__annotations__ = typing.get_type_hints(function)
    return copy


get_weather = _callable(content.get_weather)
get_precipitation = _callable(content.get_precipitation)
book_flight = _callable(content.book_flight)
