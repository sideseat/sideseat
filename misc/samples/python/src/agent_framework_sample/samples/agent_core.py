"""Memory and code execution sample demonstrating advanced agent capabilities.

Demonstrates:
1. Memory storage and retrieval using in-memory dictionary
2. Code execution using function tools
3. Combining memory-augmented reasoning with code verification
"""

from typing import Annotated

from agent_framework import ChatAgent, ai_function
from opentelemetry import trace
from pydantic import Field

SYSTEM_PROMPT = """You are an AI assistant that validates answers through code execution.
When asked about code, algorithms, or calculations, write Python code to verify your answers.
When asked about user preferences or personal information (like favorite numbers, names, etc.),
always check your memory first to retrieve any stored information."""

# Simulated memory storage
MEMORY_STORE: dict[str, str] = {
    "favorite_number": "7",
    "name": "Alice",
    "preferred_units": "metric",
    "programming_language": "Python",
}


@ai_function(approval_mode="never_require")
def execute_python_code(
    code: Annotated[str, Field(description="Python code to execute")],
) -> str:
    """Execute Python code and return the result."""
    import io
    import sys

    old_stdout = sys.stdout
    sys.stdout = io.StringIO()

    try:
        exec(code, {"__builtins__": __builtins__})  # noqa: S102
        output = sys.stdout.getvalue()
        return output if output else "Code executed successfully (no output)"
    except Exception as e:
        return f"Error executing code: {e}"
    finally:
        sys.stdout = old_stdout


@ai_function(approval_mode="never_require")
def retrieve_memory(
    key: Annotated[str, Field(description="The key to retrieve (e.g., 'favorite_number', 'name')")],
) -> str:
    """Retrieve a value from memory storage."""
    return MEMORY_STORE.get(key, f"No memory found for key: {key}")


@ai_function(approval_mode="never_require")
def store_memory(
    key: Annotated[str, Field(description="The key to store under")],
    value: Annotated[str, Field(description="The value to store")],
) -> str:
    """Store a value in memory storage."""
    MEMORY_STORE[key] = value
    return f"Stored in memory: {key} = {value}"


async def run(client, trace_attrs: dict):
    """Run the agent_core sample with memory and code execution."""
    tracer = trace.get_tracer(__name__)

    print("Agent Core Sample - Memory & Code Execution")
    print("=" * 50)

    print("\nStored memories:")
    for key, value in MEMORY_STORE.items():
        print(f"  {key}: {value}")
    print()

    agent = ChatAgent(
        chat_client=client,
        instructions=SYSTEM_PROMPT,
        tools=[execute_python_code, retrieve_memory, store_memory],
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        prompt = (
            "Calculate my favorite number squared. "
            "First retrieve my favorite number using the retrieve_memory tool "
            "with 'favorite_number' as the key. "
            "Then use the execute_python_code tool to compute and verify the square "
            "with code like: print(7 ** 2)"
        )

        print(f"Query: {prompt}")
        print("-" * 50)

        result = await agent.run(prompt)
        print(f"\nResponse: {result.text}")

    print("\n" + "=" * 50)
    print("Sample completed successfully!")
