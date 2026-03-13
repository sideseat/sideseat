"""Multi-agent swarm orchestration sample using sub-agents."""

import logging

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.genai import types

# Enable debug logging
logging.getLogger("google.adk").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)

APP_NAME = "swarm_app"


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


def create_swarm_agents(model):
    """Create agents for swarm collaboration using sub-agents."""

    # Reviewer agent (leaf agent)
    reviewer = LlmAgent(
        model=model,
        name="reviewer",
        instruction="""You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Provide final feedback on the proposed solution""",
        tools=[calculator],
    )

    # Coder agent (leaf agent)
    coder = LlmAgent(
        model=model,
        name="coder",
        instruction="""You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Provide code outlines and structure""",
        tools=[calculator],
    )

    # Researcher agent (leaf agent)
    researcher = LlmAgent(
        model=model,
        name="researcher",
        instruction="""You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Research best practices and recommendations""",
        tools=[web_search, weather_forecast],
    )

    # Planner agent - coordinator with sub-agents
    planner = LlmAgent(
        model=model,
        name="planner",
        instruction="""You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Delegate tasks to appropriate specialists (researcher, coder, reviewer)
3. Coordinate the overall workflow
4. Synthesize results from specialists into a cohesive response

You have access to specialist sub-agents:
- researcher: for gathering information and research
- coder: for writing code and implementation details
- reviewer: for reviewing and providing feedback""",
        tools=[calculator],
        sub_agents=[researcher, coder, reviewer],
    )

    return planner


async def run(model, trace_attrs: dict):
    """Run the swarm sample."""
    print("Creating swarm agents...")
    planner = create_swarm_agents(model)

    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name=APP_NAME,
        user_id="demo-user",
        session_id=trace_attrs["session.id"],
    )

    runner = Runner(
        agent=planner,
        app_name=APP_NAME,
        session_service=session_service,
    )

    prompt = "Create a simple plan to build a weather app that shows forecasts for multiple cities"

    print("Running swarm...")
    print(f"Prompt: {prompt}")
    print("-" * 50)

    user_message = types.Content(
        role="user",
        parts=[types.Part(text=prompt)],
    )

    async for event in runner.run_async(
        session_id=session.id,
        user_id="demo-user",
        new_message=user_message,
    ):
        if event.content and event.content.parts:
            for part in event.content.parts:
                if hasattr(part, "text") and part.text:
                    print("\nResponse:")
                    text = part.text
                    print(text[:500] if len(text) > 500 else text)
