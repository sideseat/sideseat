"""Multi-agent concurrent orchestration sample.

Demonstrates concurrent multi-agent collaboration using asyncio.gather():
- Researcher agent gathers information
- Technical agent evaluates feasibility
- Marketing agent creates messaging
- All agents run in parallel, results are aggregated
"""

import asyncio
import logging
from typing import Annotated

from agent_framework import ChatAgent, ai_function
from opentelemetry import trace
from pydantic import Field

logging.getLogger("agent_framework").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


@ai_function(approval_mode="never_require")
def calculator(
    operation: Annotated[
        str, Field(description="The operation to perform (add, subtract, multiply, divide)")
    ],
    a: Annotated[float, Field(description="First number")],
    b: Annotated[float, Field(description="Second number")],
) -> float:
    """Perform basic arithmetic operations."""
    operations = {
        "add": lambda x, y: x + y,
        "subtract": lambda x, y: x - y,
        "multiply": lambda x, y: x * y,
        "divide": lambda x, y: x / y if y != 0 else float("inf"),
    }
    if operation not in operations:
        return 0.0
    return operations[operation](a, b)


@ai_function(approval_mode="never_require")
def weather_forecast(
    city: Annotated[str, Field(description="The name of the city")],
    days: Annotated[int, Field(description="Number of days for the forecast")] = 3,
) -> str:
    """Get weather forecast for a city."""
    forecasts = {
        "New York": "Partly cloudy with temperatures around 65F",
        "London": "Rainy with temperatures around 55F",
        "Tokyo": "Clear skies with temperatures around 70F",
        "Paris": "Overcast with temperatures around 60F",
    }
    base = forecasts.get(city, "Weather data unavailable")
    return f"{days}-day forecast for {city}: {base}"


@ai_function(approval_mode="never_require")
def web_search(
    query: Annotated[str, Field(description="Search query string")],
    max_results: Annotated[int, Field(description="Maximum number of results")] = 5,
) -> dict:
    """Search the web and return results."""
    return {
        "query": query,
        "results": [
            {
                "title": f"Result {i + 1} for '{query}'",
                "url": f"https://example.com/{i}",
            }
            for i in range(min(max_results, 5))
        ],
    }


def create_agents(client) -> dict[str, ChatAgent]:
    """Create specialist agents for concurrent execution."""

    researcher = ChatAgent(
        chat_client=client,
        instructions=(
            "You are a market and product researcher. Given a prompt, provide concise, "
            "factual insights, opportunities, and risks. Be specific and data-driven."
        ),
        tools=[web_search, weather_forecast],
    )

    technical = ChatAgent(
        chat_client=client,
        instructions=(
            "You are a technical architect. Evaluate the technical feasibility, "
            "identify key implementation challenges, and propose a high-level architecture. "
            "Use the calculator tool for any numerical estimates."
        ),
        tools=[calculator],
    )

    marketing = ChatAgent(
        chat_client=client,
        instructions=(
            "You are a creative marketing strategist. Craft compelling value propositions, "
            "identify target audiences, and propose key marketing messages. "
            "Be creative and audience-focused."
        ),
    )

    reviewer = ChatAgent(
        chat_client=client,
        instructions=(
            "You are a senior project reviewer. Synthesize insights from research, "
            "technical, and marketing perspectives. Identify gaps and provide a final "
            "go/no-go recommendation with clear rationale."
        ),
        tools=[calculator],
    )

    return {
        "researcher": researcher,
        "technical": technical,
        "marketing": marketing,
        "reviewer": reviewer,
    }


async def run(client, trace_attrs: dict):
    """Run the swarm sample with concurrent multi-agent orchestration."""
    tracer = trace.get_tracer(__name__)

    print("Creating concurrent agent swarm...")

    agents = create_agents(client)

    prompt = "Create a simple plan to build a weather app that shows forecasts for multiple cities"

    print(f"Running concurrent swarm with prompt: {prompt}")
    print("-" * 50)

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        # Run researcher, technical, and marketing agents concurrently
        researcher_task = agents["researcher"].run(prompt)
        technical_task = agents["technical"].run(prompt)
        marketing_task = agents["marketing"].run(prompt)

        results = await asyncio.gather(
            researcher_task,
            technical_task,
            marketing_task,
            return_exceptions=True,
        )

        agent_names = ["researcher", "technical", "marketing"]
        outputs: list[tuple[str, str]] = []

        print("\n===== Concurrent Agent Responses =====")
        for name, result in zip(agent_names, results):
            if isinstance(result, Exception):
                print(f"\n[{name}]:\n[Error: {result}]")
            else:
                text = result.text or ""
                preview = text[:500] + ("..." if len(text) > 500 else "")
                print(f"\n[{name}]:\n{preview}")
                outputs.append((name, text))

        # Reviewer synthesizes all outputs sequentially
        if outputs:
            synthesis_prompt = (
                f"Original task: {prompt}\n\n"
                + "\n\n".join(f"[{name}]:\n{text}" for name, text in outputs)
                + "\n\nProvide a final go/no-go recommendation."
            )
            reviewer_result = await agents["reviewer"].run(synthesis_prompt)
            print(f"\n[reviewer]:\n{reviewer_result.text}")
