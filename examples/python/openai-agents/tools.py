"""The shared example tools as Agents SDK function tools."""

from typing import Any

from agents import RunContextWrapper, function_tool

from harness import content


def _report_error(context: RunContextWrapper[Any], error: Exception) -> str:
    # The SDK's default tells the model only that the tool failed, so it retries blindly; the error itself
    # is what lets it answer.
    return f"{type(error).__name__}: {error}"


get_weather = function_tool(content.get_weather)
get_precipitation = function_tool(content.get_precipitation)
book_flight = function_tool(content.book_flight, failure_error_function=_report_error)
