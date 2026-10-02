"""The shared example tools as Smolagents tools."""

from smolagents import tool

from harness import content

get_weather = tool(content.get_weather)
get_precipitation = tool(content.get_precipitation)
book_flight = tool(content.book_flight)
