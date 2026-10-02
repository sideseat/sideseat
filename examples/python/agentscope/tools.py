"""The shared example tools as AgentScope tools."""

from agentscope.tool import FunctionTool

from harness import content

get_weather = FunctionTool(content.get_weather)
get_precipitation = FunctionTool(content.get_precipitation)
book_flight = FunctionTool(content.book_flight)
