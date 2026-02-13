"""Memory and code execution sample demonstrating advanced agent capabilities.

Demonstrates:
1. Memory storage and retrieval using SQLite session
2. Code execution using function tools
3. Combining memory-augmented reasoning with code verification
"""

import tempfile

from agents import Agent, Runner, SQLiteSession, function_tool
from opentelemetry import trace

SYSTEM_PROMPT = """You are an AI assistant that validates answers through code execution.
When asked about code, algorithms, or calculations, write Python code to verify your answers.
When asked about user preferences or personal information (like favorite numbers, names, etc.),
always check your memory first to retrieve any stored information."""

# Simulated memory storage
USER_PREFERENCES = {
    "favorite_number": "7",
    "name": "Alice",
    "preferred_units": "metric",
    "programming_language": "Python",
}


@function_tool
def execute_python_code(code: str) -> str:
    """Execute Python code and return the result.

    Args:
        code: Python code to execute
    """
    import io
    import sys

    old_stdout = sys.stdout
    sys.stdout = io.StringIO()

    try:
        exec(code, {"__builtins__": __builtins__})
        output = sys.stdout.getvalue()
        return output if output else "Code executed successfully (no output)"
    except Exception as e:
        return f"Error executing code: {str(e)}"
    finally:
        sys.stdout = old_stdout


@function_tool
def retrieve_user_preference(preference_type: str) -> str:
    """Retrieve a user preference from memory.

    Args:
        preference_type: Type of preference to retrieve (e.g., 'favorite_number', 'name')
    """
    return USER_PREFERENCES.get(preference_type, f"No preference found for: {preference_type}")


@function_tool
def store_user_preference(preference_type: str, value: str) -> str:
    """Store a user preference in memory.

    Args:
        preference_type: Type of preference to store
        value: The preference value to store
    """
    USER_PREFERENCES[preference_type] = value
    return f"Stored preference: {preference_type} = {value}"


def run(model: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the agent_core sample with memory and code execution."""
    tracer = trace.get_tracer(__name__)

    print("Agent Core Sample - Memory & Code Execution")
    print("=" * 50)

    # Create SQLite session for conversation persistence
    db_path = tempfile.mktemp(suffix=".db", prefix="openai_agents_memory_")
    session = SQLiteSession(session_id="demo-session", db_path=db_path)

    print(f"Session database: {db_path}")
    print("\nStored preferences:")
    for key, value in USER_PREFERENCES.items():
        print(f"  {key}: {value}")
    print()

    # Create agent with memory and code tools
    agent = Agent(
        model=model,
        name="memory_code_assistant",
        instructions=SYSTEM_PROMPT,
        tools=[execute_python_code, retrieve_user_preference, store_user_preference],
    )

    with tracer.start_as_current_span(
        "openai_agents.session",
        attributes=trace_attrs,
    ):
        prompt = (
            "Calculate my favorite number squared. "
            "First retrieve my favorite number using the retrieve_user_preference tool "
            "with 'favorite_number' as the argument. "
            "Then use the execute_python_code tool to compute and verify the square."
        )

        print(f"Query: {prompt}")
        print("-" * 50)

        result = Runner.run_sync(agent, prompt, session=session)

        print(f"\nResponse:\n{result.final_output}")

    print("\n" + "=" * 50)
    print("Sample completed successfully!")
