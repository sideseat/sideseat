"""Simple OpenAI Chat Completions API calls — no trace grouping, no sessions.

Each API call produces its own independent trace.

Demonstrates:
- Chat Completions (sync)
- Chat Completions (streaming)
- Chat Completions with tool use (single turn)
"""

from sideseat import SideSeat


def run(openai_model, trace_attrs: dict, client: SideSeat):
    """Run independent Chat Completions API calls."""

    # --- Chat Completions (sync) ---
    print("--- Chat Completions ---")
    response = openai_model.client.chat.completions.create(
        model=openai_model.model_id,
        messages=[
            {"role": "system", "content": "Answer in one sentence."},
            {"role": "user", "content": "What is the speed of light?"},
        ],
        max_completion_tokens=1024,
    )
    print(f"Assistant: {response.choices[0].message.content}")
    print()

    # --- Chat Completions (streaming) ---
    print("--- Chat Completions Stream ---")
    stream = openai_model.client.chat.completions.create(
        model=openai_model.model_id,
        messages=[
            {"role": "system", "content": "Answer in one sentence."},
            {"role": "user", "content": "What is the boiling point of water?"},
        ],
        max_completion_tokens=1024,
        stream=True,
    )
    print("Assistant: ", end="")
    for chunk in stream:
        delta = chunk.choices[0].delta
        if delta.content:
            print(delta.content, end="", flush=True)
    print()

    # --- Tool Use ---
    print()
    print("--- Chat Completions with Tools ---")
    tools = [
        {
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a location.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "City name, e.g. 'San Francisco'",
                        }
                    },
                    "required": ["location"],
                },
            },
        }
    ]
    messages = [
        {"role": "system", "content": "Use tools when available."},
        {"role": "user", "content": "What's the weather in Paris?"},
    ]

    with client.trace(
        "completions-tool-use",
        session_id=trace_attrs.get("session.id"),
        user_id=trace_attrs.get("user.id"),
    ):
        # Step 1: model requests tool use
        response = openai_model.client.chat.completions.create(
            model=openai_model.model_id,
            messages=messages,
            tools=tools,
            max_completion_tokens=1024,
        )
        assistant_msg = response.choices[0].message
        messages.append(assistant_msg)

        tool_call = None
        if assistant_msg.tool_calls:
            tool_call = assistant_msg.tool_calls[0]
            print(f"Tool call: {tool_call.function.name}({tool_call.function.arguments})")

        # Step 2: return tool result and get final answer
        if tool_call:
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": tool_call.id,
                    "content": "Sunny, 22C, light breeze",
                }
            )
            response = openai_model.client.chat.completions.create(
                model=openai_model.model_id,
                messages=messages,
                tools=tools,
                max_completion_tokens=1024,
            )
            print(f"Assistant: {response.choices[0].message.content}")
    print()
