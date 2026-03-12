"""Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.

This sample shows how to:
1. Enable extended thinking for supported models
2. Use OpenAI reasoning_effort or Anthropic thinking budget
3. Handle models that don't support extended thinking gracefully

Supported configurations:
- OpenAI (OpenAIResponsesClient): via reasoning_effort option
- Anthropic (AnthropicClient): via thinking option with budget_tokens
"""

from common.models import DEFAULT_THINKING_BUDGET

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


def _get_reasoning_options(client) -> dict:
    """Determine reasoning options based on the client type."""
    try:
        from agent_framework.openai import OpenAIResponsesClient

        if isinstance(client, OpenAIResponsesClient):
            return {"reasoning_effort": "medium"}
    except ImportError:
        pass

    try:
        from agent_framework.anthropic import AnthropicClient

        if isinstance(client, AnthropicClient):
            return {
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": DEFAULT_THINKING_BUDGET,
                }
            }
    except ImportError:
        pass

    return {}


async def run(client, trace_attrs: dict, provider: str = "openai"):
    """Run the reasoning sample with extended thinking enabled."""
    from agent_framework import Agent
    from opentelemetry import trace

    tracer = trace.get_tracer(__name__)

    reasoning_options = _get_reasoning_options(client)

    agent = Agent(
        client=client,
        instructions=SYSTEM_PROMPT,
    )

    print("Extended Thinking / Reasoning Sample")
    print("=" * 60)
    print()
    print("This sample demonstrates chain-of-thought reasoning.")
    if reasoning_options:
        print(f"Reasoning options: {reasoning_options}")
    else:
        print("Note: No extended thinking configured for this client type.")
    print()

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        for i, problem in enumerate(REASONING_PROBLEMS, 1):
            print(f"\n{'=' * 60}")
            print(f"Problem {i}: {problem['name']}")
            print("-" * 60)
            prompt_preview = problem["prompt"][:200]
            print(f"{prompt_preview}{'...' if len(problem['prompt']) > 200 else ''}")
            print("-" * 60)

            run_options = reasoning_options if reasoning_options else {}
            result = await agent.run(
                problem["prompt"], options=run_options if run_options else None
            )

            # Check for reasoning/thinking content in response
            if hasattr(result, "messages"):
                for msg in result.messages:
                    if hasattr(msg, "contents"):
                        for content in msg.contents:
                            if hasattr(content, "type") and content.type == "text_reasoning":
                                text = getattr(content, "text", "")
                                if text:
                                    print("\n[Thinking Process]")
                                    print("-" * 40)
                                    print(text[:1000] + "..." if len(text) > 1000 else text)
                                    print("-" * 40)

            print("\n[Answer]")
            print(result.text)

    print(f"\n{'=' * 60}")
    print("Reasoning sample complete.")
