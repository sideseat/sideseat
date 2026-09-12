"""Extended thinking using the Anthropic Messages API.

Demonstrates:
- Extended thinking with adaptive effort (sync)
- Extended thinking with streaming
- Parsing thinking and text content blocks
"""

from sideseat import SideSeat


def run(model, trace_attrs: dict, client: SideSeat):
    """Run extended thinking samples."""

    # --- Thinking (sync) ---
    print("--- Messages with Thinking ---")
    response = model.client.messages.create(
        model=model.model_id,
        system="You are a math tutor. Show your work.",
        messages=[{"role": "user", "content": "What is 27 * 453?"}],
        max_tokens=8192,
        # claude-sonnet-5 and newer reject thinking.type="enabled" with budget_tokens:
        # they take thinking.type="adaptive" plus output_config.effort instead.
        thinking={"type": "adaptive"},
        output_config={"effort": "medium"},
    )
    for block in response.content:
        if block.type == "thinking":
            print(f"Thinking: {block.thinking[:120]}...")
        elif block.type == "text":
            print(f"Assistant: {block.text}")
    print()

    # --- Thinking (streaming) ---
    print("--- Messages Stream with Thinking ---")
    thinking_text = ""
    print("Assistant: ", end="")
    with model.client.messages.stream(
        model=model.model_id,
        system="You are a math tutor. Show your work.",
        messages=[{"role": "user", "content": "What is 891 / 9?"}],
        max_tokens=8192,
        # claude-sonnet-5 and newer reject thinking.type="enabled" with budget_tokens:
        # they take thinking.type="adaptive" plus output_config.effort instead.
        thinking={"type": "adaptive"},
        output_config={"effort": "medium"},
    ) as stream:
        for event in stream:
            if event.type == "content_block_delta":
                if event.delta.type == "thinking_delta":
                    thinking_text += event.delta.thinking
                elif event.delta.type == "text_delta":
                    print(event.delta.text, end="", flush=True)
    if thinking_text:
        print()
        print(f"Thinking: {thinking_text[:120]}...")
    print()
