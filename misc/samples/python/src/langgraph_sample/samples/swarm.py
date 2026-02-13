"""Multi-agent swarm orchestration sample.

Demonstrates:
- LangGraph StateGraph for multi-agent workflows
- Agent handoff via tool calls
- Conditional routing between agents
- Planner/Researcher/Coder/Reviewer collaboration pattern
"""

from typing import Annotated, Literal

from langchain_core.messages import AIMessage, BaseMessage, HumanMessage
from langchain_core.tools import tool
from langgraph.graph import END, START, StateGraph
from langgraph.graph.message import add_messages
from langgraph.prebuilt import create_react_agent
from pydantic import BaseModel

# Constants
MAX_ITERATIONS = 15
MAX_HANDOFFS = 10


class AgentState(BaseModel):
    """State shared across all agents in the swarm."""

    messages: Annotated[list[BaseMessage], add_messages]
    next_agent: str = "planner"
    iteration_count: int = 0

    class Config:
        arbitrary_types_allowed = True


# --- Tools ---


@tool
def calculator(operation: str, a: float, b: float) -> str:
    """Perform basic arithmetic operations.

    Args:
        operation: The operation to perform (add, subtract, multiply, divide)
        a: First number
        b: Second number

    Returns:
        Result as string, or error message for invalid operations
    """
    operations = {
        "add": lambda x, y: x + y,
        "subtract": lambda x, y: x - y,
        "multiply": lambda x, y: x * y,
        "divide": lambda x, y: x / y if y != 0 else None,
    }

    if operation not in operations:
        return f"Error: Invalid operation '{operation}'. Use: add, subtract, multiply, divide"

    result = operations[operation](a, b)
    if result is None:
        return "Error: Division by zero"

    return str(result)


@tool
def weather_forecast(city: str, days: int = 3) -> str:
    """Get weather forecast for a city.

    Args:
        city: The name of the city
        days: Number of days for the forecast (1-7)

    Returns:
        Weather forecast description
    """
    days = max(1, min(days, 7))
    forecasts = {
        "New York": "Partly cloudy with temperatures around 65F",
        "London": "Rainy with temperatures around 55F",
        "Tokyo": "Clear skies with temperatures around 70F",
        "Paris": "Overcast with temperatures around 60F",
    }
    base = forecasts.get(city, f"Weather data unavailable for {city}")
    return f"{days}-day forecast for {city}: {base}"


@tool
def web_search(query: str, max_results: int = 5) -> dict:
    """Search the web and return results.

    Args:
        query: Search query string
        max_results: Maximum number of results to return (1-5)

    Returns:
        Dictionary with search status and results
    """
    max_results = max(1, min(max_results, 5))
    return {
        "status": "success",
        "query": query,
        "results": [
            {
                "title": f"Result {i + 1} for '{query}'",
                "url": f"https://example.com/{i}",
            }
            for i in range(max_results)
        ],
    }


# --- Handoff Tools ---


@tool
def handoff_to_planner() -> str:
    """Hand off the conversation to the planner agent for coordination."""
    return "Handing off to planner"


@tool
def handoff_to_researcher() -> str:
    """Hand off the conversation to the researcher agent for information gathering."""
    return "Handing off to researcher"


@tool
def handoff_to_coder() -> str:
    """Hand off the conversation to the coder agent for implementation."""
    return "Handing off to coder"


@tool
def handoff_to_reviewer() -> str:
    """Hand off the conversation to the reviewer agent for code review."""
    return "Handing off to reviewer"


@tool
def task_complete() -> str:
    """Mark the task as complete and end the workflow."""
    return "Task completed"


def create_swarm_graph(model):
    """Create the multi-agent swarm graph.

    Args:
        model: LangChain chat model instance

    Returns:
        Compiled LangGraph StateGraph
    """
    # Create specialized agents
    planner = create_react_agent(
        model=model,
        tools=[calculator, handoff_to_researcher, handoff_to_coder, task_complete],
        prompt=(
            "You are a project planner. Your role is to:\n"
            "1. Break down complex tasks into steps\n"
            "2. Identify which specialist should handle each step\n"
            "3. Hand off to the appropriate agent (researcher, coder, reviewer)\n"
            "4. Coordinate the overall workflow"
        ),
    )

    researcher = create_react_agent(
        model=model,
        tools=[web_search, weather_forecast, handoff_to_planner, handoff_to_coder],
        prompt=(
            "You are a research specialist. Your role is to:\n"
            "1. Gather information on topics\n"
            "2. Provide factual, well-sourced answers\n"
            "3. Hand off to planner when research is complete\n"
            "4. Hand off to coder if implementation is needed"
        ),
    )

    coder = create_react_agent(
        model=model,
        tools=[calculator, handoff_to_reviewer, handoff_to_planner],
        prompt=(
            "You are a coding specialist. Your role is to:\n"
            "1. Write clean, efficient code\n"
            "2. Implement solutions based on requirements\n"
            "3. Hand off to reviewer for code review\n"
            "4. Hand off to planner if clarification needed"
        ),
    )

    reviewer = create_react_agent(
        model=model,
        tools=[calculator, handoff_to_coder, handoff_to_planner],
        prompt=(
            "You are a code reviewer. Your role is to:\n"
            "1. Review code for quality and correctness\n"
            "2. Suggest improvements\n"
            "3. Hand off to coder if changes needed\n"
            "4. Hand off to planner when review is complete"
        ),
    )

    def create_agent_node(agent, agent_name: str):
        """Create a node that runs an agent and determines next step."""

        def node(state: AgentState) -> AgentState:
            try:
                result = agent.invoke({"messages": state.messages})
                new_messages = result.get("messages", [])
            except Exception as e:
                # On error, return to planner with error message
                error_msg = AIMessage(content=f"Agent error: {e}")
                return AgentState(
                    messages=[error_msg],
                    next_agent="planner",
                    iteration_count=state.iteration_count + 1,
                )

            # Check for handoff or completion in tool calls
            next_agent = agent_name
            for msg in reversed(new_messages):
                if isinstance(msg, AIMessage) and msg.tool_calls:
                    for tc in msg.tool_calls:
                        name = (
                            tc.get("name", "") if isinstance(tc, dict) else getattr(tc, "name", "")
                        )
                        if "handoff_to_planner" in name:
                            next_agent = "planner"
                        elif "handoff_to_researcher" in name:
                            next_agent = "researcher"
                        elif "handoff_to_coder" in name:
                            next_agent = "coder"
                        elif "handoff_to_reviewer" in name:
                            next_agent = "reviewer"
                        elif "task_complete" in name:
                            next_agent = "end"

            return AgentState(
                messages=new_messages,
                next_agent=next_agent,
                iteration_count=state.iteration_count + 1,
            )

        return node

    def router(
        state: AgentState,
    ) -> Literal["planner", "researcher", "coder", "reviewer", "__end__"]:
        """Route to the next agent based on state."""
        if state.next_agent == "end" or state.iteration_count >= MAX_ITERATIONS:
            return END
        return state.next_agent

    # Build the graph
    graph = StateGraph(AgentState)

    # Add agent nodes
    graph.add_node("planner", create_agent_node(planner, "planner"))
    graph.add_node("researcher", create_agent_node(researcher, "researcher"))
    graph.add_node("coder", create_agent_node(coder, "coder"))
    graph.add_node("reviewer", create_agent_node(reviewer, "reviewer"))

    # Add edges
    graph.add_edge(START, "planner")
    graph.add_conditional_edges("planner", router)
    graph.add_conditional_edges("researcher", router)
    graph.add_conditional_edges("coder", router)
    graph.add_conditional_edges("reviewer", router)

    return graph.compile()


def run(model, trace_attrs: dict):
    """Run the swarm sample demonstrating multi-agent orchestration.

    This sample shows:
    - StateGraph for multi-agent workflow management
    - Handoff tools for agent-to-agent communication
    - Conditional routing based on tool calls
    - Iteration limits for safety

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    print("Creating swarm agents...")
    swarm = create_swarm_graph(model)

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    print("Running swarm...")

    initial_state = AgentState(
        messages=[
            HumanMessage(
                content="Create a simple plan to build a weather app that shows forecasts for multiple cities"
            )
        ],
        next_agent="planner",
        iteration_count=0,
    )

    try:
        result = swarm.invoke(initial_state, config=config)

        print(f"\nIterations: {result.iteration_count}")

        # Print final response
        print("\nFinal response:")
        for msg in reversed(result.messages):
            if isinstance(msg, AIMessage) and msg.content:
                print(msg.content)
                break
    except Exception as e:
        print(f"[Swarm Error: {e}]")
