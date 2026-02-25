"""Session with multiple independent traces sharing a session ID.

Demonstrates:
- Multiple client.trace() calls with the same session_id and user_id
- Each trace is independent (own trace_id) but grouped by session in the UI
- Multi-turn conversation within each trace
- SideSeat sessions view groups all traces by session_id
"""

from sideseat import SideSeat


def _converse(bedrock, system, messages, query, config):
    """Send a query and return assistant text."""
    messages.append({"role": "user", "content": [{"text": query}]})
    response = bedrock.client.converse(
        modelId=bedrock.model_id,
        system=system,
        messages=messages,
        inferenceConfig=config,
    )
    assistant_msg = response["output"]["message"]
    messages.append(assistant_msg)
    return assistant_msg["content"][0]["text"]


def run(bedrock, trace_attrs: dict, client: SideSeat):
    """Run multiple traces sharing a session ID."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]
    config = {"maxTokens": 512}

    print(f"Session: {session_id}, User: {user_id}")
    print()

    # --- Trace 1: Trip planning ---
    with client.trace("trip-planning", session_id=session_id, user_id=user_id):
        print("=== Trace 1: Trip Planning ===")
        system = [{"text": "You are a travel advisor. Be concise (2-3 sentences)."}]
        messages = []

        text = _converse(
            bedrock,
            system,
            messages,
            "I want to visit Japan for 5 days. What cities should I see?",
            config,
        )
        print(f"  User: Plan a 5-day Japan trip")
        print(f"  Assistant: {text}")
        print()

        text = _converse(
            bedrock,
            system,
            messages,
            "Tell me more about Kyoto. What are the must-see spots?",
            config,
        )
        print(f"  User: More about Kyoto")
        print(f"  Assistant: {text}")
        print()

    # --- Trace 2: Food recommendations ---
    with client.trace("food-recommendations", session_id=session_id, user_id=user_id):
        print("=== Trace 2: Food Recommendations ===")
        system = [
            {
                "text": "You are a food expert specializing in Japanese cuisine. Be concise (2-3 sentences)."
            }
        ]
        messages = []

        text = _converse(
            bedrock, system, messages, "What are the must-try dishes in Tokyo?", config
        )
        print(f"  User: Must-try dishes in Tokyo")
        print(f"  Assistant: {text}")
        print()

        text = _converse(bedrock, system, messages, "What about street food in Osaka?", config)
        print(f"  User: Street food in Osaka")
        print(f"  Assistant: {text}")
        print()

    # --- Trace 3: Practical tips ---
    with client.trace("practical-tips", session_id=session_id, user_id=user_id):
        print("=== Trace 3: Practical Tips ===")
        system = [{"text": "You are a Japan travel logistics expert. Be concise (2-3 sentences)."}]
        messages = []

        text = _converse(
            bedrock,
            system,
            messages,
            "What's the best way to get around between cities in Japan?",
            config,
        )
        print(f"  User: Getting around Japan")
        print(f"  Assistant: {text}")
        print()

        text = _converse(
            bedrock, system, messages, "Should I get a Japan Rail Pass for 5 days?", config
        )
        print(f"  User: Japan Rail Pass?")
        print(f"  Assistant: {text}")
        print()

    print(f"Session complete: 3 traces, 6 calls, session_id={session_id}")
