"""
Strands E2E Test Suite for OpenTelemetry Integration

This module tests various Strands SDK features to validate OTel trace collection:
- Basic tools with decorators
- Tools with context injection
- Async tools
- Class-based tools
- Agents as Tools pattern
- Swarm Multi-Agent pattern
- Complex tool interactions
"""

import asyncio
import logging
import uuid
from typing import Any

import boto3
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from strands import Agent, tool, ToolContext
from strands.models import BedrockModel
from strands.multiagent import Swarm
from strands.telemetry import StrandsTelemetry
from strands.telemetry.config import get_otel_resource

# Configuration
MODEL_ID = "us.anthropic.claude-haiku-4-5-20251001-v1:0"
OTLP_ENDPOINT = "http://127.0.0.1:5001/otel/v1/traces"

# Enable debug logging for multiagent
logging.getLogger("strands.multiagent").setLevel(logging.DEBUG)
logging.basicConfig(
    format="%(levelname)s | %(name)s | %(message)s",
    handlers=[logging.StreamHandler()],
)


# =============================================================================
# Basic Tools
# =============================================================================


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
        "New York": "Partly cloudy with temperatures around 65°F",
        "London": "Rainy with temperatures around 55°F",
        "Tokyo": "Clear skies with temperatures around 70°F",
        "Paris": "Overcast with temperatures around 60°F",
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
    # Simulated search results
    return {
        "status": "success",
        "content": [
            {
                "json": {
                    "query": query,
                    "results": [
                        {
                            "title": f"Result {i+1} for '{query}'",
                            "url": f"https://example.com/{i}",
                        }
                        for i in range(min(max_results, 5))
                    ],
                }
            }
        ],
    }


# =============================================================================
# Tools with Context
# =============================================================================


@tool(context=True)
def get_agent_info(tool_context: ToolContext) -> str:
    """Get information about the current agent.

    Returns agent name and invocation state.
    """
    agent_name = tool_context.agent.name or "unnamed"
    custom_data = tool_context.invocation_state.get("custom_data", "none")
    return f"Agent: {agent_name}, Custom data: {custom_data}"


@tool(context=True)
def tool_chain_step(step_name: str, tool_context: ToolContext) -> dict:
    """Execute a step in a tool chain, tracking progress.

    Args:
        step_name: Name of the current step
    """
    # Get or initialize step counter
    steps = tool_context.invocation_state.get("steps", [])
    steps.append(step_name)

    return {
        "status": "success",
        "content": [
            {
                "json": {
                    "step": step_name,
                    "step_number": len(steps),
                    "previous_steps": steps[:-1],
                }
            }
        ],
    }


@tool(context=True)
def session_tracker(action: str, data: str, tool_context: ToolContext) -> str:
    """Track session data across tool calls.

    Args:
        action: Action to perform (store, retrieve, list)
        data: Data key or value depending on action
    """
    session = tool_context.invocation_state.get("session", {})

    if action == "store":
        key, value = data.split("=", 1) if "=" in data else (data, "")
        session[key] = value
        return f"Stored: {key}={value}"
    elif action == "retrieve":
        return f"Retrieved: {data}={session.get(data, 'not found')}"
    elif action == "list":
        return f"Session keys: {list(session.keys())}"
    return "Unknown action"


# =============================================================================
# Async Tools
# =============================================================================


@tool
async def async_api_call(endpoint: str, delay_ms: int = 100) -> dict:
    """Simulate an async API call with delay.

    Args:
        endpoint: API endpoint to call
        delay_ms: Simulated network delay in milliseconds
    """
    await asyncio.sleep(delay_ms / 1000)
    return {
        "status": "success",
        "content": [
            {
                "json": {
                    "endpoint": endpoint,
                    "response_time_ms": delay_ms,
                    "data": {"message": f"Response from {endpoint}"},
                }
            }
        ],
    }


@tool
async def parallel_fetch(urls: list[str]) -> dict:
    """Fetch multiple URLs in parallel.

    Args:
        urls: List of URLs to fetch
    """

    async def fetch_one(url: str) -> dict:
        await asyncio.sleep(0.05)  # Simulated fetch
        return {"url": url, "status": 200, "content": f"Content from {url}"}

    results = await asyncio.gather(*[fetch_one(url) for url in urls])
    return {
        "status": "success",
        "content": [{"json": {"fetched": len(results), "results": results}}],
    }


# =============================================================================
# Class-Based Tools
# =============================================================================


class DatabaseTools:
    """Database operations with shared connection state."""

    def __init__(self, db_name: str = "test_db"):
        self.db_name = db_name
        self.tables: dict[str, list[dict]] = {
            "users": [
                {"id": 1, "name": "Alice", "role": "admin"},
                {"id": 2, "name": "Bob", "role": "user"},
            ],
            "products": [
                {"id": 1, "name": "Widget", "price": 9.99},
                {"id": 2, "name": "Gadget", "price": 19.99},
            ],
        }

    @tool
    def query_table(
        self, table: str, filter_key: str = "", filter_value: str = ""
    ) -> dict:
        """Query a database table with optional filtering.

        Args:
            table: Table name to query
            filter_key: Optional field to filter by
            filter_value: Value to filter for
        """
        if table not in self.tables:
            return {
                "status": "error",
                "content": [{"text": f"Table '{table}' not found"}],
            }

        results = self.tables[table]
        if filter_key and filter_value:
            results = [r for r in results if str(r.get(filter_key)) == filter_value]

        return {
            "status": "success",
            "content": [
                {
                    "json": {
                        "table": table,
                        "db": self.db_name,
                        "rows": results,
                        "count": len(results),
                    }
                }
            ],
        }

    @tool
    def insert_record(self, table: str, record: dict) -> str:
        """Insert a record into a table.

        Args:
            table: Table name
            record: Record data as dictionary
        """
        if table not in self.tables:
            self.tables[table] = []

        # Auto-increment ID
        max_id = max((r.get("id", 0) for r in self.tables[table]), default=0)
        record["id"] = max_id + 1
        self.tables[table].append(record)

        return f"Inserted record with id={record['id']} into {table}"

    @tool
    def get_schema(self) -> dict:
        """Get the database schema (list of tables and their columns)."""
        schema = {}
        for table, rows in self.tables.items():
            if rows:
                schema[table] = list(rows[0].keys())
            else:
                schema[table] = []

        return {
            "status": "success",
            "content": [{"json": {"db": self.db_name, "schema": schema}}],
        }


# =============================================================================
# Agents as Tools Pattern
# =============================================================================


def create_research_agent(model: Any) -> Agent:
    """Create a specialized research agent."""

    @tool
    def cite_source(topic: str, source_type: str = "web") -> str:
        """Cite a source for research.

        Args:
            topic: Topic to cite
            source_type: Type of source (web, book, journal)
        """
        return f"[{source_type.upper()}] Source for '{topic}': https://example.com/research/{topic.replace(' ', '-')}"

    return Agent(
        name="researcher",
        model=model,
        system_prompt="""You are a research specialist. When asked about any topic:
1. Provide factual information
2. Always cite your sources using the cite_source tool
3. Be concise but thorough""",
        tools=[cite_source, web_search],
    )


def create_analyst_agent(model: Any) -> Agent:
    """Create a specialized data analysis agent."""
    return Agent(
        name="analyst",
        model=model,
        system_prompt="""You are a data analyst. Your role is to:
1. Analyze data and provide insights
2. Use the calculator for any numerical analysis
3. Present findings in a structured format""",
        tools=[calculator],
    )


def create_agent_tools(model: Any) -> list:
    """Create agent-as-tool wrappers."""
    research_agent = create_research_agent(model)
    analyst_agent = create_analyst_agent(model)

    @tool
    def research_assistant(query: str) -> str:
        """Delegate research queries to the research specialist.

        Args:
            query: Research question requiring factual information
        """
        try:
            response = research_agent(query)
            return str(response)
        except Exception as e:
            return f"Research error: {e}"

    @tool
    def analysis_assistant(query: str) -> str:
        """Delegate analysis queries to the data analyst.

        Args:
            query: Data analysis question
        """
        try:
            response = analyst_agent(query)
            return str(response)
        except Exception as e:
            return f"Analysis error: {e}"

    return [research_assistant, analysis_assistant]


# =============================================================================
# Swarm Multi-Agent Pattern
# =============================================================================


def create_swarm_agents(model: Any) -> tuple[list[Agent], Agent]:
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
    )

    return [planner, researcher, coder, reviewer], planner


# =============================================================================
# Test Functions
# =============================================================================


def test_basic_tools(agent: Agent) -> None:
    """Test basic tool functionality."""
    print("\n" + "=" * 60)
    print("TEST: Basic Tools")
    print("=" * 60)

    # Calculator test
    print("\n[Calculator Test]")
    response = agent("Calculate 25 multiplied by 4, then add 15 to the result")
    print(f"Response: {response}")

    # Weather test
    print("\n[Weather Test]")
    response = agent("What's the weather forecast for Tokyo for the next 5 days?")
    print(f"Response: {response}")

    # Web search test
    print("\n[Web Search Test]")
    response = agent("Search for information about AI agents and return 3 results")
    print(f"Response: {response}")


def test_context_tools(agent: Agent) -> None:
    """Test tools with context injection."""
    print("\n" + "=" * 60)
    print("TEST: Context Tools")
    print("=" * 60)

    # Agent info test
    print("\n[Agent Info Test]")
    response = agent(
        "Get information about yourself",
        invocation_state={"custom_data": "test-run-001"},
    )
    print(f"Response: {response}")

    # Session tracker test
    print("\n[Session Tracker Test]")
    response = agent(
        "Store 'user=Alice' in the session, then retrieve the 'user' value",
        invocation_state={"session": {}},
    )
    print(f"Response: {response}")


async def test_async_tools(agent: Agent) -> None:
    """Test async tool functionality."""
    print("\n" + "=" * 60)
    print("TEST: Async Tools")
    print("=" * 60)

    # Async API call test
    print("\n[Async API Test]")
    response = await agent.invoke_async(
        "Call the API endpoint '/users' with a 200ms delay"
    )
    print(f"Response: {response}")

    # Parallel fetch test
    print("\n[Parallel Fetch Test]")
    response = await agent.invoke_async(
        "Fetch these URLs in parallel: https://api.example.com/a, https://api.example.com/b, https://api.example.com/c"
    )
    print(f"Response: {response}")


def test_class_tools(agent: Agent) -> None:
    """Test class-based tools."""
    print("\n" + "=" * 60)
    print("TEST: Class-Based Tools")
    print("=" * 60)

    # Schema test
    print("\n[Schema Test]")
    response = agent("Get the database schema")
    print(f"Response: {response}")

    # Query test
    print("\n[Query Test]")
    response = agent("Query the users table and filter by role=admin")
    print(f"Response: {response}")

    # Insert test
    print("\n[Insert Test]")
    response = agent(
        "Insert a new user named 'Charlie' with role 'user' into the users table"
    )
    print(f"Response: {response}")


def test_agents_as_tools(orchestrator: Agent) -> None:
    """Test agents-as-tools pattern."""
    print("\n" + "=" * 60)
    print("TEST: Agents as Tools")
    print("=" * 60)

    # Research delegation
    print("\n[Research Delegation]")
    response = orchestrator("Research the latest developments in quantum computing")
    print(f"Response: {response}")

    # Analysis delegation
    print("\n[Analysis Delegation]")
    response = orchestrator(
        "Analyze the growth rate if something increases from 100 to 150 over 3 periods"
    )
    print(f"Response: {response}")

    # Combined query
    print("\n[Combined Query]")
    response = orchestrator(
        "First research AI frameworks, then analyze which one might be best for a small team based on complexity"
    )
    print(f"Response: {response}")


def test_swarm_pattern(swarm: Swarm) -> None:
    """Test swarm multi-agent pattern."""
    print("\n" + "=" * 60)
    print("TEST: Swarm Multi-Agent Pattern")
    print("=" * 60)

    print("\n[Swarm Execution]")
    result = swarm(
        "Create a simple plan to build a weather app that shows forecasts for multiple cities"
    )

    print(f"\nStatus: {result.status}")
    print(f"Iterations: {result.execution_count}")

    if hasattr(result, "node_history") and result.node_history:
        print(
            f"Agent sequence: {' -> '.join(node.node_id for node in result.node_history)}"
        )

    if hasattr(result, "accumulated_usage") and result.accumulated_usage:
        usage = result.accumulated_usage
        print(f"Total tokens: {usage.get('totalTokens', 'N/A')}")


async def test_swarm_streaming(swarm: Swarm) -> None:
    """Test swarm with streaming events."""
    print("\n" + "=" * 60)
    print("TEST: Swarm Streaming")
    print("=" * 60)

    print("\n[Streaming Events]")
    async for event in swarm.stream_async("Research what makes a good REST API design"):
        event_type = event.get("type", "")

        if event_type == "multiagent_node_start":
            print(f"  -> Agent '{event.get('node_id')}' taking control")

        elif event_type == "multiagent_handoff":
            from_nodes = ", ".join(event.get("from_node_ids", []))
            to_nodes = ", ".join(event.get("to_node_ids", []))
            print(f"  -> Handoff: {from_nodes} -> {to_nodes}")

        elif event_type == "multiagent_result":
            result = event.get("result")
            if result:
                print(f"  -> Swarm completed with status: {result.status}")


def main() -> None:
    """Run all E2E tests."""
    print("=" * 60)
    print("STRANDS COMPLEX E2E TEST SUITE")
    print("=" * 60)

    # Initialize model
    session = boto3.Session(region_name="us-east-1")
    model = BedrockModel(model_id=MODEL_ID, boto_session=session)

    telemetry = StrandsTelemetry()
    telemetry.setup_otlp_exporter(
        endpoint=OTLP_ENDPOINT,
        headers={"test-suite": "e2e", "version": "1.0"},
    )
    print(f"Telemetry configured for endpoint: {OTLP_ENDPOINT}")

    # Initialize database tools
    db_tools = DatabaseTools(db_name="e2e_test_db")

    # Create main agent with all basic tools
    basic_agent = Agent(
        name="e2e-test-agent",
        model=model,
        tools=[
            calculator,
            weather_forecast,
            web_search,
            get_agent_info,
            tool_chain_step,
            session_tracker,
            async_api_call,
            parallel_fetch,
            db_tools.query_table,
            db_tools.insert_record,
            db_tools.get_schema,
        ],
    )

    # Create orchestrator with agent tools
    agent_tools = create_agent_tools(model)
    orchestrator = Agent(
        name="orchestrator",
        model=model,
        system_prompt="""You are an orchestrator that delegates tasks to specialists:
- For research questions: use research_assistant
- For data analysis: use analysis_assistant
Always select the most appropriate specialist for each query.""",
        tools=agent_tools,
        trace_attributes={
            "session.id": f"strands-demo-{uuid.uuid4().hex[:16]}",
            "user.id": "demo-user",
        },
    )

    # Create swarm
    swarm_agents, entry_point = create_swarm_agents(model)
    swarm = Swarm(
        swarm_agents,
        entry_point=entry_point,
        max_handoffs=10,
        max_iterations=15,
        execution_timeout=300.0,
        node_timeout=120.0,
    )

    # Run tests
    try:
        # Basic tests
        test_basic_tools(basic_agent)
        test_context_tools(basic_agent)
        test_class_tools(basic_agent)

        # Async tests
        asyncio.run(test_async_tools(basic_agent))

        # Multi-agent tests
        test_agents_as_tools(orchestrator)
        test_swarm_pattern(swarm)
        asyncio.run(test_swarm_streaming(swarm))

        print("\n" + "=" * 60)
        print("ALL TESTS COMPLETED")
        print("=" * 60)

    except Exception as e:
        print(f"\nTest failed with error: {e}")
        raise
    finally:
        # Ensure telemetry is flushed before exit
        print("\nFlushing telemetry...")
        telemetry.tracer_provider.force_flush(timeout_millis=10000)
        telemetry.tracer_provider.shutdown()
        print("Telemetry flushed and shutdown complete.")


if __name__ == "__main__":
    main()
