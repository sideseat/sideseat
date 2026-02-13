"""Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.

This sample shows how to:
1. Enable extended thinking (reasoning) for supported models
2. Use budget_tokens to control thinking depth
3. Handle models that don't support extended thinking gracefully
4. Extract and display thinking content from responses

Supported models:
- Bedrock Claude (Sonnet 3.5+, Opus): via thinking parameter
- Anthropic Claude (Sonnet 3.5+, Opus): via thinking parameter

Note: Extended thinking requires specific model versions.
"""

from crewai import Agent, Crew, Process, Task
from opentelemetry import trace

# Challenging problems that benefit from step-by-step reasoning
REASONING_PROBLEMS = [
    {
        "name": "Logic Puzzle",
        "prompt": """Solve this logic puzzle step by step:

Three friends (Alice, Bob, Carol) each have a different pet (cat, dog, fish)
and live in different colored houses (red, blue, green).

Clues:
1. Alice doesn't live in the red house
2. The person with the cat lives in the blue house
3. Bob doesn't have a fish
4. Carol lives in the red house
5. The person in the green house has a dog

Who has which pet and lives in which house?""",
    },
    {
        "name": "Math Problem",
        "prompt": """A water tank has two pipes. Pipe A can fill the tank in 6 hours.
Pipe B can empty the tank in 8 hours. If both pipes are opened when the tank
is half full, how long will it take to fill the tank completely?

Show your reasoning step by step.""",
    },
    {
        "name": "Code Analysis",
        "prompt": """Analyze this Python function and explain what it computes:

```python
def mystery(n):
    if n <= 1:
        return n
    a, b = 0, 1
    for _ in range(2, n + 1):
        a, b = b, a + b
    return b
```

What mathematical sequence does this implement? Prove your answer by tracing
through for n=7.""",
    },
]

SYSTEM_PROMPT = """You are a precise analytical assistant that solves problems
using careful step-by-step reasoning. Always show your work and explain your
thought process clearly. When solving puzzles or problems:

1. First understand what is being asked
2. Identify the relevant information and constraints
3. Work through the problem systematically
4. Verify your answer against all given conditions
5. Present your final answer clearly"""


def run(llm, trace_attrs: dict):
    """Run the reasoning sample with extended thinking enabled."""
    tracer = trace.get_tracer(__name__)

    # The LLM passed in should already have thinking enabled via runner.
    # Additionally, enable CrewAI's native reasoning for planning and reflection.
    reasoning_agent = Agent(
        role="Analytical Problem Solver",
        goal="Solve complex problems using careful step-by-step reasoning",
        backstory=SYSTEM_PROMPT,
        llm=llm,
        verbose=False,
        reasoning=True,  # Enable CrewAI planning/reflection (complements model thinking)
        max_reasoning_attempts=3,  # Limit reasoning iterations
    )

    print("Extended Thinking / Reasoning Sample")
    print("=" * 60)
    print()
    print("This sample demonstrates chain-of-thought reasoning.")
    print("For models that support it, you'll see the thinking process.")
    print()

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        for i, problem in enumerate(REASONING_PROBLEMS, 1):
            print(f"\n{'=' * 60}")
            print(f"Problem {i}: {problem['name']}")
            print("-" * 60)
            print(
                problem["prompt"][:200] + "..."
                if len(problem["prompt"]) > 200
                else problem["prompt"]
            )
            print("-" * 60)

            task = Task(
                description=problem["prompt"],
                expected_output="A detailed solution with step-by-step reasoning and final answer",
                agent=reasoning_agent,
            )

            crew = Crew(
                agents=[reasoning_agent],
                tasks=[task],
                process=Process.sequential,
                verbose=False,
                share_crew=False,
            )

            result = crew.kickoff()

            print("\n[Answer]")
            print(result.raw)

    print(f"\n{'=' * 60}")
    print("Reasoning sample complete.")
