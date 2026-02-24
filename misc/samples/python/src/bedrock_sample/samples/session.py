"""Multi-turn conversation with session and user tracking.

Demonstrates:
- session.id and user.id attributes on all spans
- Multi-turn conversation within a single session
- SideSeat UI groups all turns by session_id

The telemetry setup passes user_id and session_id to the SideSeat constructor,
so all spans created by the Bedrock instrumentation automatically include
these attributes. No manual attribute setting is needed.
"""


def run(bedrock, trace_attrs: dict):
    """Run a multi-turn session with user/session tracking."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]
    print(f"Session: {session_id}")
    print(f"User: {user_id}")
    print()

    system = [{"text": "You are a travel advisor. Help plan trips. Be concise (2-3 sentences)."}]
    config = {"maxTokens": 512}
    messages = []

    queries = [
        "I want to plan a 5-day trip to Japan. What cities should I visit?",
        "Tell me more about Kyoto. What are the must-see spots?",
        "What's the best time of year to visit?",
        "Recommend some local dishes I should try in each city.",
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

    print(f"Session complete: {len(queries)} turns, session_id={session_id}")
