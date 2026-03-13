"""Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.

Demonstrates:
- Extended thinking (reasoning) for Claude models
- Budget tokens configuration for thinking depth
- Extracting and displaying thinking content
- Solving complex problems requiring step-by-step reasoning

Supported models:
- Bedrock Claude (Sonnet, Haiku 4.5+): via additional_model_request_fields
- Anthropic Claude (Sonnet, Haiku 4.5+): via thinking parameter

Note: Extended thinking requires specific model versions. Older models
will work normally but without visible reasoning steps.
"""

from typing import Optional

from langchain_core.messages import AIMessage, SystemMessage

# Constants
THINKING_TRUNCATE_LENGTH = 1000

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


def extract_thinking(response: AIMessage) -> Optional[str]:
    """Extract thinking/reasoning content from response if available.

    Args:
        response: AIMessage from model invocation

    Returns:
        Thinking content string, or None if not present
    """
    if not response.content:
        return None

    # Handle content as list of blocks
    if isinstance(response.content, list):
        for block in response.content:
            if isinstance(block, dict):
                block_type = block.get("type", "")
                if block_type == "thinking":
                    return block.get("thinking") or block.get("text")
                if block_type == "reasoning":
                    return block.get("reasoning") or block.get("text")

    # Check additional_kwargs for thinking
    if hasattr(response, "additional_kwargs") and response.additional_kwargs:
        thinking = response.additional_kwargs.get("thinking")
        if thinking:
            if isinstance(thinking, list):
                for block in thinking:
                    if isinstance(block, dict) and block.get("type") == "thinking":
                        return block.get("thinking")
            elif isinstance(thinking, str):
                return thinking

    return None


def extract_text(response: AIMessage) -> str:
    """Extract main text content from response.

    Args:
        response: AIMessage from model invocation

    Returns:
        Text content string
    """
    if not response.content:
        return "[No content]"

    if isinstance(response.content, str):
        return response.content

    if isinstance(response.content, list):
        texts = []
        for block in response.content:
            if isinstance(block, dict) and block.get("type") == "text":
                text = block.get("text", "")
                if text:
                    texts.append(text)
            elif isinstance(block, str):
                texts.append(block)
        if texts:
            return "\n".join(texts)

    return str(response.content)


def run(model, trace_attrs: dict):
    """Run the reasoning sample with extended thinking enabled.

    This sample demonstrates:
    - Extended thinking/chain-of-thought reasoning
    - Solving logic puzzles, math problems, and code analysis
    - Extracting and displaying thinking content when available
    - Graceful handling of models without thinking support

    The model passed in should already have thinking enabled via runner
    if the model supports it.

    Args:
        model: LangChain chat model instance (optionally with thinking enabled)
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    print("Extended Thinking / Reasoning Sample")
    print("=" * 60)
    print()
    print("This sample demonstrates chain-of-thought reasoning.")
    print("For models that support it, you'll see the thinking process.")
    print()

    messages = [SystemMessage(content=SYSTEM_PROMPT)]

    for i, problem in enumerate(REASONING_PROBLEMS, 1):
        print(f"\n{'=' * 60}")
        print(f"Problem {i}: {problem['name']}")
        print("-" * 60)

        # Show truncated problem for display
        prompt = problem["prompt"]
        display_prompt = prompt[:200] + "..." if len(prompt) > 200 else prompt
        print(display_prompt)
        print("-" * 60)

        try:
            result = model.invoke(messages + [("user", prompt)])

            # Try to extract thinking content
            thinking = extract_thinking(result)
            if thinking:
                print("\n[Thinking Process]")
                print("-" * 40)
                # Truncate very long thinking for display
                if len(thinking) > THINKING_TRUNCATE_LENGTH:
                    print(thinking[:THINKING_TRUNCATE_LENGTH] + "\n... (truncated)")
                else:
                    print(thinking)
                print("-" * 40)

            # Extract and display main response
            answer = extract_text(result)
            print("\n[Answer]")
            print(answer)

            # Show token usage if available
            if hasattr(result, "usage_metadata") and result.usage_metadata:
                usage = result.usage_metadata
                input_tokens = usage.get("input_tokens", "N/A")
                output_tokens = usage.get("output_tokens", "N/A")
                print(f"\n[Usage] Input: {input_tokens}, Output: {output_tokens}")

        except Exception as e:
            print(f"[Error: {e}]")

    print(f"\n{'=' * 60}")
    print("Reasoning sample complete.")
