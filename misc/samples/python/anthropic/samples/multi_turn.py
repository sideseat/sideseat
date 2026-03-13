"""Multi-turn conversation using the Anthropic Messages API.

Demonstrates:
- System prompt
- Multi-turn context accumulation
- Token usage tracking
"""

from sideseat import SideSeat


def run(model, trace_attrs: dict, client: SideSeat):
    """Run a multi-turn conversation with Messages API."""
    with client.trace("anthropic-chat"):
        messages = []

        queries = [
            "What is the capital of France?",
            "What about Germany?",
            "Which of the two cities has a larger population?",
        ]

        for i, query in enumerate(queries, 1):
            print(f"--- Turn {i} ---")
            print(f"User: {query}")

            messages.append({"role": "user", "content": query})

            response = model.client.messages.create(
                model=model.model_id,
                system="You are a helpful geography assistant. Answer in 1-2 sentences.",
                messages=messages,
                max_tokens=1024,
            )

            assistant_text = response.content[0].text
            messages.append({"role": "assistant", "content": assistant_text})

            usage = response.usage
            print(f"Assistant: {assistant_text}")
            print(f"  Tokens: in={usage.input_tokens} out={usage.output_tokens}")
            print()
