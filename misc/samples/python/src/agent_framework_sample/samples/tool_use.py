"""Basic tool usage sample with weather forecast tools and prompt caching."""

from typing import Annotated

from agent_framework import ChatAgent, ai_function
from opentelemetry import trace
from pydantic import Field

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
40. Sign off with a friendly closing."""


@ai_function(approval_mode="never_require")
def temperature_forecast(
    city: Annotated[str, Field(description="The name of the city")],
    days: Annotated[int, Field(description="Number of days for the forecast")] = 3,
) -> dict:
    """Get the temperature forecast for a given city and number of days."""
    return {
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


@ai_function(approval_mode="never_require")
def precipitation_forecast(
    city: Annotated[str, Field(description="The name of the city")] = "New York City",
    days: Annotated[int, Field(description="Number of days for the forecast")] = 3,
) -> str:
    """Get the precipitation forecast for a given city and number of days."""
    return (
        f"The precipitation forecast for {city} for the next {days} days is: "
        "Light rain expected on day 2."
    )


async def run(client, trace_attrs: dict):
    """Run the tool_use sample with prompt caching."""
    tracer = trace.get_tracer(__name__)

    agent = ChatAgent(
        chat_client=client,
        instructions=SYSTEM_PROMPT,
        tools=[temperature_forecast, precipitation_forecast],
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        # First call
        print("--- First call ---")
        result = await agent.run("Provide a 3-day weather forecast for New York City.")
        print(f"Agent: {result.text}\n")

        # Second call
        print("--- Second call ---")
        result = await agent.run("Provide a 3-day weather forecast for New York City.")
        print(f"Agent: {result.text}\n")

        # Third call
        print("--- Third call ---")
        result = await agent.run("Provide a 7-day weather forecast for Los Angeles.")
        print(f"Agent: {result.text}\n")

        # Fourth call
        print("--- Fourth call ---")
        result = await agent.run("Provide a 14-day weather forecast for London.")
        print(f"Agent: {result.text}\n")
