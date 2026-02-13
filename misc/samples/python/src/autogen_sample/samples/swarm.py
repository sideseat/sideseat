"""Multi-agent swarm orchestration sample."""

import logging

from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import HandoffTermination, MaxMessageTermination
from autogen_agentchat.messages import HandoffMessage
from autogen_agentchat.teams import Swarm
from opentelemetry import trace

# Enable debug logging for autogen
logging.getLogger("autogen_agentchat").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


async def calculator(operation: str, a: float, b: float) -> float:
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


async def weather_forecast(city: str, days: int = 3) -> str:
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


async def web_search(query: str, max_results: int = 5) -> dict:
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


def create_swarm_agents(model_client):
    """Create agents for swarm collaboration."""

    # Planner agent - entry point
    planner = AssistantAgent(
        name="planner",
        model_client=model_client,
        handoffs=["researcher", "coder", "reviewer"],
        system_message="""You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Identify which specialist should handle each step
3. Hand off to the appropriate agent (researcher, coder, reviewer)
4. Coordinate the overall workflow

When you need research, use 'handoff_to_researcher'.
When you need code, use 'handoff_to_coder'.
When you need review, use 'handoff_to_reviewer'.
When the task is complete, provide a final summary.""",
        tools=[calculator],
    )

    # Researcher agent
    researcher = AssistantAgent(
        name="researcher",
        model_client=model_client,
        handoffs=["planner", "coder"],
        system_message="""You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Hand off to planner when research is complete
4. Hand off to coder if implementation is needed

Use 'handoff_to_planner' when done researching.
Use 'handoff_to_coder' if code needs to be written.""",
        tools=[web_search, weather_forecast],
    )

    # Coder agent
    coder = AssistantAgent(
        name="coder",
        model_client=model_client,
        handoffs=["planner", "reviewer"],
        system_message="""You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Hand off to reviewer for code review
4. Hand off to planner if clarification needed

Use 'handoff_to_reviewer' when code is ready for review.
Use 'handoff_to_planner' if you need clarification.""",
        tools=[calculator],
    )

    # Reviewer agent
    reviewer = AssistantAgent(
        name="reviewer",
        model_client=model_client,
        handoffs=["planner", "coder"],
        system_message="""You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Hand off to coder if changes needed
4. Hand off to planner when review is complete

Use 'handoff_to_coder' if changes are needed.
Use 'handoff_to_planner' when review is complete.""",
        tools=[calculator],
    )

    return [planner, researcher, coder, reviewer], planner


async def run(model_client, trace_attrs: dict):
    """Run the swarm sample."""
    tracer = trace.get_tracer(__name__)

    print("Creating swarm agents...")
    agents, entry_point = create_swarm_agents(model_client)

    # Create termination conditions
    max_messages = MaxMessageTermination(max_messages=15)
    handoff_termination = HandoffTermination(target="user")
    termination = max_messages | handoff_termination

    # Create swarm team
    swarm = Swarm(
        participants=agents,
        termination_condition=termination,
    )

    prompt = "Create a simple plan to build a weather app that shows forecasts for multiple cities"

    print("Running swarm...")
    print(f"Prompt: {prompt}")
    print("-" * 50)

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        result = await swarm.run(task=prompt)

        # Print agent sequence
        agent_sequence = []
        for message in result.messages:
            if hasattr(message, "source"):
                if message.source not in agent_sequence or agent_sequence[-1] != message.source:
                    agent_sequence.append(message.source)

        print(f"\nAgent sequence: {' -> '.join(agent_sequence)}")
        print(f"Total messages: {len(result.messages)}")

        # Print final response
        for message in reversed(result.messages):
            if (
                hasattr(message, "content")
                and message.content
                and not isinstance(message, HandoffMessage)
            ):
                print(f"\nFinal response from {message.source}:")
                print(message.content[:500] if len(message.content) > 500 else message.content)
                break

    await model_client.close()
