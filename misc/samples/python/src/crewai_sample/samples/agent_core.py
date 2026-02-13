"""Memory and code execution sample demonstrating advanced agent capabilities.

Demonstrates:
1. Memory storage and retrieval using CrewAI's built-in memory
2. Code execution using CodeInterpreterTool
3. Combining memory-augmented reasoning with code verification
"""

import tempfile

from crewai import Agent, Crew, Process, Task
from crewai.tools import tool
from opentelemetry import trace

SYSTEM_PROMPT = """You are an AI assistant that validates answers through code execution.
When asked about code, algorithms, or calculations, write Python code to verify your answers.
When asked about user preferences or personal information (like favorite numbers, names, etc.),
always check your memory first to retrieve any stored information."""


@tool
def execute_python_code(code: str) -> str:
    """Execute Python code and return the result.

    Args:
        code: Python code to execute

    Returns:
        Output from code execution
    """
    import io
    import sys

    # Capture stdout
    old_stdout = sys.stdout
    sys.stdout = io.StringIO()

    try:
        # Execute the code
        exec(code, {"__builtins__": __builtins__})
        output = sys.stdout.getvalue()
        return output if output else "Code executed successfully (no output)"
    except Exception as e:
        return f"Error executing code: {str(e)}"
    finally:
        sys.stdout = old_stdout


@tool
def retrieve_user_preference(preference_type: str) -> str:
    """Retrieve a user preference from memory.

    Args:
        preference_type: Type of preference to retrieve (e.g., 'favorite_number', 'name')

    Returns:
        The stored preference value
    """
    # Simulated memory storage (in real use, this would query actual memory)
    preferences = {
        "favorite_number": "7",
        "name": "Alice",
        "preferred_units": "metric",
        "programming_language": "Python",
    }
    return preferences.get(preference_type, f"No preference found for: {preference_type}")


def run(model, trace_attrs: dict):
    """Run the agent_core sample with memory and code execution."""
    tracer = trace.get_tracer(__name__)

    print("Agent Core Sample - Memory & Code Execution")
    print("=" * 50)

    # Initialize short-term memory
    memory_dir = tempfile.mkdtemp(prefix="crewai_memory_")
    print(f"Memory storage: {memory_dir}")

    # Store some memories using the memory tool simulation
    print("\nStoring memories...")
    memories = [
        "User's favorite number is 7",
        "User prefers metric units for measurements",
        "User's name is Alice",
        "User likes Python programming language",
    ]
    for mem in memories:
        print(f"  Stored: {mem}")

    print()

    # Create agent with code execution capability
    memory_code_agent = Agent(
        role="Memory Code Assistant",
        goal="Retrieve user preferences and validate calculations with code",
        backstory=SYSTEM_PROMPT,
        tools=[execute_python_code, retrieve_user_preference],
        llm=model,
        verbose=False,
        allow_code_execution=True,
        # NOTE: Using "unsafe" for demo simplicity. In production, use "safe" (Docker-based)
        # for sandboxed execution. See: https://docs.crewai.com/concepts/agents#code-execution
        code_execution_mode="unsafe",
    )

    # Task that requires both memory and code execution
    calculation_task = Task(
        description=(
            "Calculate the user's favorite number squared. "
            "First retrieve the user's favorite number using the retrieve_user_preference tool "
            "with 'favorite_number' as the argument. "
            "Then use the execute_python_code tool to compute and verify the square. "
            "Show both the retrieved value and the calculation result."
        ),
        expected_output="The user's favorite number and its square, verified by code execution",
        agent=memory_code_agent,
    )

    crew = Crew(
        agents=[memory_code_agent],
        tasks=[calculation_task],
        process=Process.sequential,
        verbose=False,
        memory=True,  # Enable crew memory
    )

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        prompt = "Calculate my favorite number squared."
        print(f"Query: {prompt}")
        print("-" * 50)

        result = crew.kickoff()

        print(f"\nResponse:\n{result.raw}")

    print("\n" + "=" * 50)
    print("Sample completed successfully!")
