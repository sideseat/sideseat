"""Basic tool usage sample with weather forecast tool and prompt caching.

Demonstrates:
- Custom tool definition using @tool decorator
- ReAct agent with multiple tools
- System prompt configuration
- Multiple sequential agent invocations (prompt caching behavior)
"""

from langchain_core.messages import AIMessage, SystemMessage
from langchain_core.tools import tool
from langgraph.prebuilt import create_react_agent


@tool
def temperature_forecast(city: str, days: int = 3) -> dict:
    """Get the temperature forecast for a given city and number of days.

    Args:
        city: The name of the city
        days: Number of days for the forecast (1-7)

    Returns:
        dict with status, city, days, and forecast list
    """
    # Clamp days to valid range
    days = max(1, min(days, 7))

    return {
        "status": "success",
        "city": city,
        "days": days,
        "forecast": [
            {
                "day": i + 1,
                "condition": "Sunny",
                "high": 25 + i,
                "low": 15 + i,
            }
            for i in range(days)
        ],
    }


@tool
def precipitation_forecast(city: str = "New York City", days: int = 3) -> str:
    """Get the precipitation forecast for a given city and number of days.

    Args:
        city: The name of the city
        days: Number of days for the forecast (1-7)

    Returns:
        String description of precipitation forecast
    """
    days = max(1, min(days, 7))
    return f"The precipitation forecast for {city} for the next {days} days is: Light rain expected on day 2."


# System prompt with guidelines repeated 5x for prompt caching demo (matches Strands exactly)
SYSTEM_PROMPT = """You're a helpful weather assistant. Use the weather_forecast tool to get weather data.

Guidelines (Important!!!):
1. Always use the weather_forecast tool for weather information.
2. Keep responses concise and friendly.
3. Default to New York City if no city specified.
4. Default to 3 days if no duration specified.
5. Maximum forecast is 7 days.
6. Greet the user warmly.
7. Thank the user at the end.
8. If multiple cities requested, handle each separately.
9. For extreme weather, include safety tips.
10. Only provide forecasts, not historical data.
11. Be transparent about tool limitations.
12. Encourage checking forecasts regularly.
13. Maintain user privacy.
14. Prioritize user satisfaction.
15. Stay on topic - weather only.
16. Verify tool output before responding.
17. Accommodate format preferences when possible.
18. Create positive user experiences.
19. If location unsupported, inform politely.
20. Sign off with a friendly closing.
21. Always use the weather_forecast tool for weather information.
22. Keep responses concise and friendly.
23. Default to New York City if no city specified.
24. Default to 3 days if no duration specified.
25. Maximum forecast is 7 days.
26. Greet the user warmly.
27. Thank the user at the end.
28. If multiple cities requested, handle each separately.
29. For extreme weather, include safety tips.
30. Only provide forecasts, not historical data.
31. Be transparent about tool limitations.
32. Encourage checking forecasts regularly.
33. Maintain user privacy.
34. Prioritize user satisfaction.
35. Stay on topic - weather only.
36. Verify tool output before responding.
37. Accommodate format preferences when possible.
38. Create positive user experiences.
39. If location unsupported, inform politely.
40. Sign off with a friendly closing.
41. Always use the weather_forecast tool for weather information.
42. Keep responses concise and friendly.
43. Default to New York City if no city specified.
44. Default to 3 days if no duration specified.
45. Maximum forecast is 7 days.
46. Greet the user warmly.
47. Thank the user at the end.
48. If multiple cities requested, handle each separately.
49. For extreme weather, include safety tips.
50. Only provide forecasts, not historical data.
51. Be transparent about tool limitations.
52. Encourage checking forecasts regularly.
53. Maintain user privacy.
54. Prioritize user satisfaction.
55. Stay on topic - weather only.
56. Verify tool output before responding.
57. Accommodate format preferences when possible.
58. Create positive user experiences.
59. If location unsupported, inform politely.
60. Sign off with a friendly closing.
61. Always use the weather_forecast tool for weather information.
62. Keep responses concise and friendly.
63. Default to New York City if no city specified.
64. Default to 3 days if no duration specified.
65. Maximum forecast is 7 days.
66. Greet the user warmly.
67. Thank the user at the end.
68. If multiple cities requested, handle each separately.
69. For extreme weather, include safety tips.
70. Only provide forecasts, not historical data.
71. Be transparent about tool limitations.
72. Encourage checking forecasts regularly.
73. Maintain user privacy.
74. Prioritize user satisfaction.
75. Stay on topic - weather only.
76. Verify tool output before responding.
77. Accommodate format preferences when possible.
78. Create positive user experiences.
79. If location unsupported, inform politely.
80. Sign off with a friendly closing.
81. Always use the weather_forecast tool for weather information.
82. Keep responses concise and friendly.
83. Default to New York City if no city specified.
84. Default to 3 days if no duration specified.
85. Maximum forecast is 7 days.
86. Greet the user warmly.
87. Thank the user at the end.
88. If multiple cities requested, handle each separately.
89. For extreme weather, include safety tips.
90. Only provide forecasts, not historical data.
91. Be transparent about tool limitations.
92. Encourage checking forecasts regularly.
93. Maintain user privacy.
94. Prioritize user satisfaction.
95. Stay on topic - weather only.
96. Verify tool output before responding.
97. Accommodate format preferences when possible.
98. Create positive user experiences.
99. If location unsupported, inform politely.
100. Sign off with a friendly closing."""


def extract_response(result: dict) -> str:
    """Extract the final text response from agent result.

    Args:
        result: The agent invoke result dictionary

    Returns:
        The extracted text response or error message
    """
    messages = result.get("messages", [])
    for msg in reversed(messages):
        if isinstance(msg, AIMessage) and msg.content:
            if isinstance(msg.content, str):
                return msg.content
            # Handle content blocks (list of dicts)
            if isinstance(msg.content, list):
                for block in msg.content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        return block.get("text", "")
    return "[No response generated]"


def run(model, trace_attrs: dict):
    """Run the tool_use sample demonstrating weather forecast tools.

    This sample shows:
    - Tool definition with @tool decorator
    - ReAct agent creation with create_react_agent
    - Multiple sequential calls to demonstrate prompt caching behavior
    - Different query variations (cities, forecast lengths)

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    # Create ReAct agent with tools
    agent = create_react_agent(
        model=model,
        tools=[temperature_forecast, precipitation_forecast],
        prompt=SystemMessage(content=SYSTEM_PROMPT),
    )

    # Config with session/user metadata for tracing
    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    def invoke_agent(prompt: str) -> str:
        """Invoke the agent and return the final response."""
        try:
            result = agent.invoke({"messages": [("user", prompt)]}, config=config)
            return extract_response(result)
        except Exception as e:
            return f"[Error: {e}]"

    # Sequential calls demonstrating prompt caching
    queries = [
        (
            "First call (cache write)",
            "Provide a 3-day weather forecast for New York City.",
        ),
        (
            "Second call (cache read)",
            "Provide a 3-day weather forecast for New York City.",
        ),
        (
            "Third call (cache read)",
            "Provide a 7-day weather forecast for Los Angeles.",
        ),
        ("Fourth call (cache read)", "Provide a 14-day weather forecast for London."),
    ]

    for label, query in queries:
        print(f"--- {label} ---")
        result = invoke_agent(query)
        print(result)
        print()
