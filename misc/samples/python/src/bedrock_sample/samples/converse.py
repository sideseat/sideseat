"""Multi-turn conversation using the Bedrock Converse API.

Demonstrates:
- System prompt
- Inference config (temperature, maxTokens)
- Multi-turn context accumulation
- Token usage and cost tracking
"""


def run(bedrock, trace_attrs: dict):
    """Run a multi-turn conversation with Bedrock Converse."""
    system = [{"text": "You are a helpful geography assistant. Answer in 1-2 sentences."}]
    config = {"temperature": 0.7, "maxTokens": 256}
    messages = []

    queries = [
        "What is the capital of France?",
        "What about Germany?",
        "Which of the two cities has a larger population?",
    ]

    for i, query in enumerate(queries, 1):
        print(f"--- Turn {i} ---")
        print(f"User: {query}")

        messages.append({"role": "user", "content": [{"text": query}]})

        response = bedrock.client.converse(
            modelId=bedrock.model_id,
            system=system,
            messages=messages,
            inferenceConfig=config,
        )

        assistant_msg = response["output"]["message"]
        messages.append(assistant_msg)

        text = assistant_msg["content"][0]["text"]
        usage = response.get("usage", {})
        print(f"Assistant: {text}")
        print(f"  Tokens: in={usage.get('inputTokens', 0)} out={usage.get('outputTokens', 0)}")
        print()
