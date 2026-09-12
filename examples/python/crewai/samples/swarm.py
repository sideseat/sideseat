"""Multi-agent swarm orchestration sample."""

import logging

from crewai import Agent, Crew, Process, Task
from crewai.tools import tool
from opentelemetry import trace

# Enable debug logging for crewai
logging.getLogger("crewai").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


@tool("calculator")
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


@tool("weather_forecast")
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


@tool("search_web")
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


def create_swarm_agents(llm):
    """Create agents for swarm collaboration."""

    # Planner agent - entry point
    planner = Agent(
        role="Project Planner",
        goal="Break down complex tasks into steps and coordinate the overall workflow",
        backstory="""You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Identify which specialist should handle each step
3. Hand off to the appropriate agent (researcher, coder, reviewer)
4. Coordinate the overall workflow""",
        llm=llm,
        tools=[calculator],
        verbose=False,
    )

    # Researcher agent
    researcher = Agent(
        role="Research Specialist",
        goal="Gather information on topics and provide factual answers",
        backstory="""You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Hand off to planner when research is complete
4. Hand off to coder if implementation is needed""",
        llm=llm,
        tools=[web_search, weather_forecast],
        verbose=False,
    )

    # Coder agent
    coder = Agent(
        role="Coding Specialist",
        goal="Write clean, efficient code and implement solutions",
        backstory="""You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Hand off to reviewer for code review
4. Hand off to planner if clarification needed""",
        llm=llm,
        tools=[calculator],
        verbose=False,
    )

    # Reviewer agent
    reviewer = Agent(
        role="Code Reviewer",
        goal="Review code for quality and correctness",
        backstory="""You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Hand off to coder if changes needed
4. Hand off to planner when review is complete""",
        llm=llm,
        tools=[calculator],
        verbose=False,
    )

    return [planner, researcher, coder, reviewer]


def run(llm, trace_attrs: dict):
    """Run the swarm sample."""
    tracer = trace.get_tracer(__name__)

    print("Creating swarm agents...")
    agents = create_swarm_agents(llm)
    planner, researcher, coder, reviewer = agents

    # Define tasks for hierarchical workflow
    planning_task = Task(
        description="Create a simple plan to build a weather app that shows forecasts for multiple cities",
        expected_output="A detailed plan with steps and assigned specialists",
        agent=planner,
    )

    research_task = Task(
        description="Research the requirements and best practices for building a weather app",
        expected_output="Research findings including API options and implementation best practices",
        agent=researcher,
    )

    coding_task = Task(
        description="Based on the research, outline the code structure for the weather app",
        expected_output="Code outline with main components and their responsibilities",
        agent=coder,
    )

    review_task = Task(
        description="Review the proposed code structure and provide feedback",
        expected_output="Review feedback with any suggested improvements",
        agent=reviewer,
    )

    # Create crew with hierarchical process for manager-style coordination
    crew = Crew(
        agents=agents,
        tasks=[planning_task, research_task, coding_task, review_task],
        process=Process.hierarchical,
        manager_llm=llm,
        verbose=True,
        share_crew=False,
    )

    print("Running swarm...")
    print("-" * 50)

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        result = crew.kickoff()

        print("\nStatus: Completed")
        print(f"Tasks: {len(crew.tasks)}")

        # Print task outputs
        if hasattr(result, "tasks_output"):
            print(
                f"Agent sequence: {' -> '.join(t.agent for t in result.tasks_output if hasattr(t, 'agent'))}"
            )

        print("\nFinal result:")
        print(result.raw[:500] if len(result.raw) > 500 else result.raw)
