"""Streaming conversation using the Bedrock ConverseStream API.

Demonstrates:
- Streaming text output with real-time display
- Multi-turn streaming conversation
- Token usage from stream metadata
"""


def _collect_stream(stream) -> tuple[str, dict]:
    """Read a converse_stream response, printing chunks as they arrive.

    Returns (full_text, usage_dict).
    """
    text = ""
    usage = {}
    for event in stream:
        if "contentBlockDelta" in event:
            chunk = event["contentBlockDelta"]["delta"].get("text", "")
            print(chunk, end="", flush=True)
            text += chunk
        elif "metadata" in event:
            usage = event["metadata"].get("usage", {})
    print()
    return text, usage


def run(bedrock, trace_attrs: dict):
    """Run a multi-turn streaming conversation."""
    system = [{"text": "You are a creative writing assistant. Keep responses under 100 words."}]
    messages = []

    queries = [
        "Write a short poem about the ocean.",
        "Now write one about mountains, in the same style.",
    ]

    for i, query in enumerate(queries, 1):
        print(f"--- Turn {i} ---")
        print(f"User: {query}")
        print("Assistant: ", end="")

        messages.append({"role": "user", "content": [{"text": query}]})

        response = bedrock.client.converse_stream(
            modelId=bedrock.model_id,
            system=system,
            messages=messages,
            inferenceConfig={"maxTokens": 512},
        )

        text, usage = _collect_stream(response["stream"])
        messages.append({"role": "assistant", "content": [{"text": text}]})

        print(f"  Tokens: in={usage.get('inputTokens', 0)} out={usage.get('outputTokens', 0)}")
        print()
