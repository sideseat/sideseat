"""Multi-agent swarm orchestration sample."""

import logging

from strands import Agent, tool
from strands.multiagent import Swarm

# Enable debug logging for multiagent
logging.getLogger("strands.multiagent").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


@tool
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


@tool
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


@tool(name="search_web", description="Search the web for information")
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


def create_swarm_agents(model, trace_attrs: dict):
    """Create agents for swarm collaboration."""

    # Planner agent - entry point
    planner = Agent(
        name="planner",
        model=model,
        system_prompt="""You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Identify which specialist should handle each step
3. Hand off to the appropriate agent (researcher, coder, reviewer)
4. Coordinate the overall workflow""",
        tools=[calculator],
        trace_attributes=trace_attrs,
    )

    # Researcher agent
    researcher = Agent(
        name="researcher",
        model=model,
        system_prompt="""You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Hand off to planner when research is complete
4. Hand off to coder if implementation is needed""",
        tools=[web_search, weather_forecast],
        trace_attributes=trace_attrs,
    )

    # Coder agent
    coder = Agent(
        name="coder",
        model=model,
        system_prompt="""You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Hand off to reviewer for code review
4. Hand off to planner if clarification needed""",
        tools=[calculator],
        trace_attributes=trace_attrs,
    )

    # Reviewer agent
    reviewer = Agent(
        name="reviewer",
        model=model,
        system_prompt="""You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Hand off to coder if changes needed
4. Hand off to planner when review is complete""",
        tools=[calculator],
        trace_attributes=trace_attrs,
    )

    return [planner, researcher, coder, reviewer], planner


def run(model, trace_attrs: dict):
    """Run the swarm sample."""
    print("Creating swarm agents...")

    swarm_agents, entry_point = create_swarm_agents(model, trace_attrs)
    swarm = Swarm(
        swarm_agents,
        entry_point=entry_point,
        max_handoffs=10,
        max_iterations=15,
        execution_timeout=300.0,
        node_timeout=120.0,
    )

    print("Running swarm...")
    result = swarm(
        "Create a simple plan to build a weather app that shows forecasts for multiple cities"
    )

    print(f"\nStatus: {result.status}")
    print(f"Iterations: {result.execution_count}")

    if hasattr(result, "node_history") and result.node_history:
        print(f"Agent sequence: {' -> '.join(node.node_id for node in result.node_history)}")
