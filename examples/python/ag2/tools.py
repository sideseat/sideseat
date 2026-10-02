"""The shared example tools as AG2 tools."""

from ag2 import tool

from harness import content

get_weather = tool(content.get_weather)
get_precipitation = tool(content.get_precipitation)
book_flight = tool(content.book_flight)
