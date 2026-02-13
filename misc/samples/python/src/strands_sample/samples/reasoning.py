"""Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.

This sample shows how to:
1. Enable extended thinking (reasoning) for supported models
2. Use budget_tokens to control thinking depth
3. Handle models that don't support extended thinking gracefully
4. Extract and display thinking content from responses

Supported models:
- Bedrock Claude (Sonnet 3.5+, Opus): via additional_request_fields
- Anthropic Claude (Sonnet 3.5+, Opus): via thinking parameter

Note: Extended thinking requires specific model versions. Older models
will work normally but without visible reasoning steps.
"""

from strands import Agent

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


def extract_thinking(response) -> str | None:
    """Extract thinking/reasoning content from response if available."""
    if not hasattr(response, "message") or not response.message:
        return None

    content = response.message.get("content", [])
    if not isinstance(content, list):
        return None

    for block in content:
        if isinstance(block, dict):
            # Check for thinking block (Anthropic format)
            if block.get("type") == "thinking":
                return block.get("thinking", block.get("text"))
            # Check for reasoning block (alternative format)
            if block.get("type") == "reasoning":
                return block.get("reasoning", block.get("text"))

    return None


def extract_text(response) -> str:
    """Extract main text content from response."""
    if not hasattr(response, "message") or not response.message:
        return str(response)

    content = response.message.get("content", [])
    if isinstance(content, str):
        return content

    if isinstance(content, list):
        texts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                texts.append(block.get("text", ""))
            elif isinstance(block, str):
                texts.append(block)
        return "\n".join(texts)

    return str(response)


def run(model, trace_attrs: dict):
    """Run the reasoning sample with extended thinking enabled."""
    # Enable extended thinking for the model if supported
    # This is configured at the model level, not agent level
    # The model passed in should already have thinking enabled via runner

    agent = Agent(
        model=model,
        system_prompt=SYSTEM_PROMPT,
        trace_attributes=trace_attrs,
    )

    print("Extended Thinking / Reasoning Sample")
    print("=" * 60)
    print()
    print("This sample demonstrates chain-of-thought reasoning.")
    print("For models that support it, you'll see the thinking process.")
    print()

    for i, problem in enumerate(REASONING_PROBLEMS, 1):
        print(f"\n{'=' * 60}")
        print(f"Problem {i}: {problem['name']}")
        print("-" * 60)
        print(
            problem["prompt"][:200] + "..." if len(problem["prompt"]) > 200 else problem["prompt"]
        )
        print("-" * 60)

        response = agent(problem["prompt"])

        # Try to extract thinking content
        thinking = extract_thinking(response)
        if thinking:
            print("\n[Thinking Process]")
            print("-" * 40)
            # Truncate very long thinking for display
            if len(thinking) > 1000:
                print(thinking[:1000] + "\n... (truncated)")
            else:
                print(thinking)
            print("-" * 40)

        # Extract and display main response
        answer = extract_text(response)
        print("\n[Answer]")
        print(answer)

        # Show token usage if available
        if hasattr(response, "metrics") and response.metrics:
            try:
                summary = response.metrics.get_summary()
                if "thinking" in str(summary).lower() or "reasoning" in str(summary).lower():
                    print(f"\n[Metrics] {summary}")
            except Exception:
                pass

    print(f"\n{'=' * 60}")
    print("Reasoning sample complete.")
