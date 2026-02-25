"""Session with multiple independent traces sharing a session ID.

Demonstrates:
- Multiple client.trace() calls with the same session_id and user_id
- Each trace is independent (own trace_id) but grouped by session in the UI
- Multi-turn conversation within each trace
- SideSeat sessions view groups all traces by session_id
"""

from sideseat import SideSeat


def _chat(model, messages, query, system):
    """Send a query and return assistant text."""
    messages.append({"role": "user", "content": query})
    response = model.client.messages.create(
        model=model.model_id,
        system=system,
        messages=messages,
        max_tokens=2048,
    )
    assistant_text = response.content[0].text
    messages.append({"role": "assistant", "content": assistant_text})
    return assistant_text


def run(model, trace_attrs: dict, client: SideSeat):
    """Run multiple traces sharing a session ID."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    print(f"Session: {session_id}, User: {user_id}")
    print()

    # --- Trace 1: Trip planning ---
    with client.trace("trip-planning", session_id=session_id, user_id=user_id):
        print("=== Trace 1: Trip Planning ===")
        system = "You are a travel advisor. Be concise (2-3 sentences)."
        messages = []

        text = _chat(
            model,
            messages,
            "I want to visit Japan for 5 days. What cities should I see?",
            system,
        )
        print("  User: Plan a 5-day Japan trip")
        print(f"  Assistant: {text}")
        print()

        text = _chat(
            model,
            messages,
            "Tell me more about Kyoto. What are the must-see spots?",
            system,
        )
        print("  User: More about Kyoto")
        print(f"  Assistant: {text}")
        print()

    # --- Trace 2: Food recommendations ---
    with client.trace("food-recommendations", session_id=session_id, user_id=user_id):
        print("=== Trace 2: Food Recommendations ===")
        system = (
            "You are a food expert specializing in Japanese cuisine. Be concise (2-3 sentences)."
        )
        messages = []

        text = _chat(model, messages, "What are the must-try dishes in Tokyo?", system)
        print("  User: Must-try dishes in Tokyo")
        print(f"  Assistant: {text}")
        print()

        text = _chat(model, messages, "What about street food in Osaka?", system)
        print("  User: Street food in Osaka")
        print(f"  Assistant: {text}")
        print()

    # --- Trace 3: Practical tips ---
    with client.trace("practical-tips", session_id=session_id, user_id=user_id):
        print("=== Trace 3: Practical Tips ===")
        system = "You are a Japan travel logistics expert. Be concise (2-3 sentences)."
        messages = []

        text = _chat(
            model,
            messages,
            "What's the best way to get around between cities in Japan?",
            system,
        )
        print("  User: Getting around Japan")
        print(f"  Assistant: {text}")
        print()

        text = _chat(model, messages, "Should I get a Japan Rail Pass for 5 days?", system)
        print("  User: Japan Rail Pass?")
        print(f"  Assistant: {text}")
        print()

    print(f"Session complete: 3 traces, 6 calls, session_id={session_id}")
