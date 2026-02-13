"""Memory and code execution sample demonstrating advanced agent capabilities.

Demonstrates:
1. Memory storage and retrieval using in-memory service
2. Code execution using function tools
3. Combining memory-augmented reasoning with code verification
"""

from google.adk.agents import LlmAgent
from google.adk.memory import InMemoryMemoryService
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.adk.tools import FunctionTool
from google.genai import types
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


def retrieve_user_preference(preference_type: str) -> str:
    """Retrieve a user preference from memory.

    Args:
        preference_type: Type of preference to retrieve (e.g., 'favorite_number', 'name')
    """
    return USER_PREFERENCES.get(preference_type, f"No preference found for: {preference_type}")


def store_user_preference(preference_type: str, value: str) -> str:
    """Store a user preference in memory.

    Args:
        preference_type: Type of preference to store
        value: The preference value to store
    """
    USER_PREFERENCES[preference_type] = value
    return f"Stored preference: {preference_type} = {value}"


async def run(model, trace_attrs: dict):
    """Run the agent_core sample with memory and code execution."""
    tracer = trace.get_tracer(__name__)

    print("Agent Core Sample - Memory & Code Execution")
    print("=" * 50)

    print("\nStored preferences:")
    for key, value in USER_PREFERENCES.items():
        print(f"  {key}: {value}")
    print()

    # Create tools
    code_tool = FunctionTool(func=execute_python_code)
    get_pref_tool = FunctionTool(func=retrieve_user_preference)
    set_pref_tool = FunctionTool(func=store_user_preference)

    # Create agent with memory and code tools
    agent = LlmAgent(
        model=model,
        name="memory_code_assistant",
        instruction=SYSTEM_PROMPT,
        tools=[code_tool, get_pref_tool, set_pref_tool],
    )

    # Session and memory services
    session_service = InMemorySessionService()
    memory_service = InMemoryMemoryService()

    session = await session_service.create_session(
        app_name="agent_core_sample",
        user_id="demo-user",
    )

    runner = Runner(
        agent=agent,
        app_name="agent_core_sample",
        session_service=session_service,
        memory_service=memory_service,
    )

    with tracer.start_as_current_span(
        "adk.session",
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

        response_text = ""
        async for event in runner.run_async(
            user_id="demo-user",
            session_id=session.id,
            new_message=types.Content(
                role="user",
                parts=[types.Part(text=prompt)],
            ),
        ):
            if hasattr(event, "content") and event.content:
                for part in event.content.parts:
                    if hasattr(part, "text") and part.text:
                        response_text += part.text

        print(f"\nResponse:\n{response_text}")

    print("\n" + "=" * 50)
    print("Sample completed successfully!")
