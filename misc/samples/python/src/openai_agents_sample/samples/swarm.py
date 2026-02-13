"""Multi-agent swarm orchestration sample using handoffs."""

import logging

from agents import Agent, Runner, function_tool
from opentelemetry import trace

# Enable debug logging
logging.getLogger("agents").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


@function_tool
def calculator(operation: str, a: float, b: float) -> float:
    """Perform basic arithmetic operations.

    Args:
        operation: The operation to perform (add, subtract, multiply, divide)
        a: First number
        b: Second number
    """
    operations = {
        "add": lambda x, y: x + y,
        "subtract": lambda x, y: x - y,
        "multiply": lambda x, y: x * y,
        "divide": lambda x, y: x / y if y != 0 else float("inf"),
    }
    if operation not in operations:
        return 0.0
    return operations[operation](a, b)


@function_tool
def weather_forecast(city: str, days: int = 3) -> str:
    """Get weather forecast for a city.

    Args:
        city: The name of the city
        days: Number of days for the forecast
    """
    forecasts = {
        "New York": "Partly cloudy with temperatures around 65F",
        "London": "Rainy with temperatures around 55F",
        "Tokyo": "Clear skies with temperatures around 70F",
        "Paris": "Overcast with temperatures around 60F",
    }
    base = forecasts.get(city, "Weather data unavailable")
    return f"{days}-day forecast for {city}: {base}"


@function_tool
def web_search(query: str, max_results: int = 5) -> dict:
    """Search the web and return results.

    Args:
        query: Search query string
        max_results: Maximum number of results to return
    """
    return {
        "status": "success",
        "content": [
            {
                "json": {
                    "query": query,
                    "results": [
                        {
                            "title": f"Result {i + 1} for '{query}'",
                            "url": f"https://example.com/{i}",
                        }
                        for i in range(min(max_results, 5))
                    ],
                }
            }
        ],
    }


def create_swarm_agents(model_id: str):
    """Create agents for swarm collaboration with handoffs."""

    # Reviewer agent (no outgoing handoffs - terminal)
    reviewer = Agent(
        name="reviewer",
        model=model_id,
        instructions="""You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Provide final feedback on the proposed solution""",
        tools=[calculator],
    )

    # Coder agent - can handoff to reviewer
    coder = Agent(
        name="coder",
        model=model_id,
        instructions="""You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Hand off to reviewer when code is ready for review""",
        tools=[calculator],
        handoffs=[reviewer],
    )

    # Researcher agent - can handoff to coder
    researcher = Agent(
        name="researcher",
        model=model_id,
        instructions="""You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Hand off to coder if implementation is needed""",
        tools=[web_search, weather_forecast],
        handoffs=[coder],
    )

    # Planner agent - entry point, can handoff to any specialist
    planner = Agent(
        name="planner",
        model=model_id,
        instructions="""You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Identify which specialist should handle each step
3. Hand off to the appropriate agent (researcher, coder, reviewer)
4. Coordinate the overall workflow

Start by handing off to the researcher for initial research.""",
        tools=[calculator],
        handoffs=[researcher, coder, reviewer],
    )

    return planner


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the swarm sample."""
    tracer = trace.get_tracer(__name__)

    print("Creating swarm agents...")
    entry_agent = create_swarm_agents(model_id)

    prompt = "Create a simple plan to build a weather app that shows forecasts for multiple cities"

    print("Running swarm...")
    print(f"Prompt: {prompt}")
    print("-" * 50)

    with tracer.start_as_current_span(
        "openai_agents.session",
        attributes=trace_attrs,
    ):
        result = Runner.run_sync(entry_agent, prompt)

        print("\nFinal response:")
        print(result.final_output[:500] if len(result.final_output) > 500 else result.final_output)
