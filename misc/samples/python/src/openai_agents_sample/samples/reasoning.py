"""Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.

This sample shows how to:
1. Enable extended thinking (reasoning) for supported models
2. Use reasoning_effort to control thinking depth
3. Handle models that don't support extended thinking gracefully

Supported models:
- OpenAI: via reasoning_effort parameter

Note: Extended thinking requires specific model versions.
"""

from agents import Agent, ModelSettings, Runner
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


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the reasoning sample with extended thinking enabled."""
    tracer = trace.get_tracer(__name__)

    # Configure model settings for reasoning
    model_settings = None
    if enable_thinking:
        print("  Extended thinking: enabled (reasoning_effort=medium)")
        model_settings = ModelSettings(reasoning={"effort": "medium"})

    agent = Agent(
        name="ReasoningAssistant",
        model=model_id,
        instructions=SYSTEM_PROMPT,
        model_settings=model_settings,
    )

    print("Extended Thinking / Reasoning Sample")
    print("=" * 60)
    print()
    print("This sample demonstrates chain-of-thought reasoning.")
    print("For models that support it, you'll see the thinking process.")
    print()

    with tracer.start_as_current_span(
        "openai_agents.session",
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

            result = Runner.run_sync(agent, problem["prompt"])

            print("\n[Answer]")
            print(result.final_output)

    print(f"\n{'=' * 60}")
    print("Reasoning sample complete.")
