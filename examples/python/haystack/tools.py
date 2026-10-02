"""The shared example tools as Haystack tools."""

from haystack.tools import create_tool_from_function

from harness import content

get_weather = create_tool_from_function(content.get_weather)
get_precipitation = create_tool_from_function(content.get_precipitation)
book_flight = create_tool_from_function(content.book_flight)
