"""Simple Anthropic Messages API calls — no trace grouping, no sessions.

Each API call produces its own independent trace.

Demonstrates:
- Messages (sync)
- Messages (streaming)
- Messages with tool use (single turn)
"""

import json

from sideseat import SideSeat


def run(model, trace_attrs: dict, client: SideSeat):
    """Run independent Anthropic Messages API calls."""

    # --- Messages (sync) ---
    print("--- Messages ---")
    response = model.client.messages.create(
        model=model.model_id,
        system="Answer in one sentence.",
        messages=[{"role": "user", "content": "What is the speed of light?"}],
        max_tokens=1024,
    )
    print(f"Assistant: {response.content[0].text}")
    print()

    # --- Messages (streaming) ---
    print("--- Messages Stream ---")
    print("Assistant: ", end="")
    with model.client.messages.stream(
        model=model.model_id,
        system="Answer in one sentence.",
        messages=[{"role": "user", "content": "What is the boiling point of water?"}],
        max_tokens=1024,
    ) as stream:
        for text in stream.text_stream:
            print(text, end="", flush=True)
    print()

    # --- Tool Use ---
    print()
    print("--- Messages with Tools ---")
    tools = [
        {
            "name": "get_weather",
            "description": "Get the current weather for a location.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "City name, e.g. 'San Francisco'",
                    }
                },
                "required": ["location"],
            },
        }
    ]
    messages: list[dict] = [{"role": "user", "content": "What's the weather in Paris?"}]

    with client.trace(
        "messages-tool-use",
        session_id=trace_attrs.get("session.id"),
        user_id=trace_attrs.get("user.id"),
    ):
        # Step 1: model requests tool use
        response = model.client.messages.create(
            model=model.model_id,
            system="Use tools when available.",
            messages=messages,
            tools=tools,
            max_tokens=1024,
        )
        messages.append({"role": "assistant", "content": response.content})

        tool_use = None
        for block in response.content:
            if block.type == "tool_use":
                tool_use = block
                print(f"Tool call: {tool_use.name}({json.dumps(tool_use.input)})")

        # Step 2: return tool result and get final answer
        if tool_use:
            messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": tool_use.id,
                            "content": "Sunny, 22C, light breeze",
                        }
                    ],
                }
            )
            response = model.client.messages.create(
                model=model.model_id,
                system="Use tools when available.",
                messages=messages,
                tools=tools,
                max_tokens=1024,
            )
            print(f"Assistant: {response.content[0].text}")
    print()
