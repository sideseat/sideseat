"""The shared example tools as LangChain tools."""

from langchain_core.tools import tool

from harness import content

get_weather = tool(content.get_weather, parse_docstring=True)
get_precipitation = tool(content.get_precipitation, parse_docstring=True)
book_flight = tool(content.book_flight, parse_docstring=True)
