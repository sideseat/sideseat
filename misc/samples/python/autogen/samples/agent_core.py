"""Memory and code execution sample demonstrating advanced agent capabilities.

Demonstrates:
1. Memory storage and retrieval using in-memory dictionary (simulating vector memory)
2. Code execution using function tools
3. Combining memory-augmented reasoning with code verification
"""

from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.teams import RoundRobinGroupChat
from opentelemetry import trace

SYSTEM_PROMPT = """You are an AI assistant that validates answers through code execution.
When asked about code, algorithms, or calculations, write Python code to verify your answers.
When asked about user preferences or personal information (like favorite numbers, names, etc.),
always check your memory first to retrieve any stored information."""

# Simulated memory storage
MEMORY_STORE = {
    "favorite_number": "7",
    "name": "Alice",
    "preferred_units": "metric",
    "programming_language": "Python",
}


async def execute_python_code(code: str) -> str:
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


async def retrieve_memory(key: str) -> str:
    """Retrieve a value from memory storage.

    Args:
        key: The key to retrieve (e.g., 'favorite_number', 'name')
    """
    return MEMORY_STORE.get(key, f"No memory found for key: {key}")


async def store_memory(key: str, value: str) -> str:
    """Store a value in memory storage.

    Args:
        key: The key to store under
        value: The value to store
    """
    MEMORY_STORE[key] = value
    return f"Stored in memory: {key} = {value}"


async def run(model_client, trace_attrs: dict):
    """Run the agent_core sample with memory and code execution."""
    tracer = trace.get_tracer(__name__)

    print("Agent Core Sample - Memory & Code Execution")
    print("=" * 50)

    print("\nStored memories:")
    for key, value in MEMORY_STORE.items():
        print(f"  {key}: {value}")
    print()

    # Create agent with memory and code tools
    agent = AssistantAgent(
        name="memory_code_assistant",
        model_client=model_client,
        tools=[execute_python_code, retrieve_memory, store_memory],
        system_message=SYSTEM_PROMPT,
    )

    termination = MaxMessageTermination(max_messages=7)
    team = RoundRobinGroupChat([agent], termination_condition=termination)

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        prompt = (
            "Calculate my favorite number squared. "
            "First retrieve my favorite number using the retrieve_memory tool "
            "with 'favorite_number' as the key. "
            "Then use the execute_python_code tool to compute and verify the square "
            "with code like: print(7 ** 2)"
        )

        print(f"Query: {prompt}")
        print("-" * 50)

        result = await team.run(task=prompt)

        # Print the conversation
        for message in result.messages:
            if hasattr(message, "content") and message.content:
                source = getattr(message, "source", "unknown")
                content = message.content
                if isinstance(content, list):
                    content = str(content)[:500]
                print(f"\n[{source}]: {content}")

    await model_client.close()

    print("\n" + "=" * 50)
    print("Sample completed successfully!")
