"""Basic tool usage sample with weather forecast tool."""

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types

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

APP_NAME = "weather_app"


def temperature_forecast(city: str, days: int = 3) -> dict:
    """Get the temperature forecast for a given city and number of days.

    Args:
        city: The name of the city
        days: Number of days for the forecast
    """
    return {
        "status": "success",
        "content": [
            {
                "json": {
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
                },
            }
        ],
    }


def precipitation_forecast(city: str = "New York City", days: int = 3) -> str:
    """Get the precipitation forecast for a given city and number of days.

    Args:
        city: The name of the city
        days: Number of days for the forecast
    """
    return f"The precipitation forecast for {city} for the next {days} days is: Light rain expected on day 2."


async def run_query(runner, session_id: str, query: str, label: str):
    """Run a single query and print the result."""
    print(f"--- {label} ---")

    user_message = types.Content(
        role="user",
        parts=[types.Part(text=query)],
    )

    async for event in runner.run_async(
        session_id=session_id,
        user_id="demo-user",
        new_message=user_message,
    ):
        if event.content and event.content.parts:
            for part in event.content.parts:
                if hasattr(part, "text") and part.text:
                    print(f"Agent: {part.text}")


async def run(model, trace_attrs: dict):
    """Run the tool_use sample."""
    agent = LlmAgent(
        model=model,
        name="weather_assistant",
        instruction=SYSTEM_PROMPT,
        tools=[temperature_forecast, precipitation_forecast],
    )

    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name=APP_NAME,
        user_id="demo-user",
        session_id=trace_attrs["session.id"],
    )

    runner = Runner(
        agent=agent,
        app_name=APP_NAME,
        session_service=session_service,
    )

    queries = [
        ("First call (cache write)", "Provide a 3-day weather forecast for New York City."),
        ("Second call (cache read)", "Provide a 3-day weather forecast for New York City."),
        ("Third call (cache read)", "Provide a 7-day weather forecast for Los Angeles."),
        ("Fourth call (cache read)", "Provide a 14-day weather forecast for London."),
    ]

    for label, query in queries:
        await run_query(runner, session.id, query, label)
        print()
