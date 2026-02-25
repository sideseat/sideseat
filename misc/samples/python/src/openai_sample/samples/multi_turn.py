"""Multi-turn conversation using the OpenAI Chat Completions API.

Demonstrates:
- System prompt
- Temperature and max_completion_tokens configuration
- Multi-turn context accumulation
- Token usage tracking
"""

from sideseat import SideSeat


def run(openai_model, trace_attrs: dict, client: SideSeat):
    """Run a multi-turn conversation with Chat Completions."""
    with client.trace("openai-chat"):
        messages = [
            {
                "role": "system",
                "content": "You are a helpful geography assistant. Answer in 1-2 sentences.",
            },
        ]

        queries = [
            "What is the capital of France?",
            "What about Germany?",
            "Which of the two cities has a larger population?",
        ]

        for i, query in enumerate(queries, 1):
            print(f"--- Turn {i} ---")
            print(f"User: {query}")

            messages.append({"role": "user", "content": query})

            response = openai_model.client.chat.completions.create(
                model=openai_model.model_id,
                messages=messages,
                max_completion_tokens=1024,
            )

            assistant_msg = response.choices[0].message
            messages.append({"role": "assistant", "content": assistant_msg.content})

            usage = response.usage
            print(f"Assistant: {assistant_msg.content}")
            print(f"  Tokens: in={usage.prompt_tokens} out={usage.completion_tokens}")
            print()
