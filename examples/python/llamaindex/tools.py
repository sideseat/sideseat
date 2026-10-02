"""The shared example tools as LlamaIndex function tools."""

from llama_index.core.tools import FunctionTool

from harness import content

get_weather = FunctionTool.from_defaults(content.get_weather)
get_precipitation = FunctionTool.from_defaults(content.get_precipitation)
book_flight = FunctionTool.from_defaults(content.book_flight)
