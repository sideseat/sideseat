"""OpenAI Responses API calls — no trace grouping, no sessions.

Each API call produces its own independent trace.

Demonstrates:
- Responses API (sync)
- Responses API (streaming)
- Responses API with tool use (using previous_response_id)
"""

from sideseat import SideSeat


def run(openai_model, trace_attrs: dict, client: SideSeat):
    """Run independent Responses API calls."""

    # --- Responses (sync) ---
    print("--- Responses ---")
    response = openai_model.client.responses.create(
        model=openai_model.model_id,
        instructions="Answer in one sentence.",
        input="What is the speed of light?",
        max_output_tokens=1024,
    )
    print(f"Assistant: {response.output_text}")
    print()

    # --- Responses (streaming) ---
    print("--- Responses Stream ---")
    print("Assistant: ", end="")
    stream = openai_model.client.responses.create(
        model=openai_model.model_id,
        instructions="Answer in one sentence.",
        input="What is the boiling point of water?",
        max_output_tokens=1024,
        stream=True,
    )
    for event in stream:
        if event.type == "response.output_text.delta":
            print(event.delta, end="", flush=True)
    print()

    # --- Tool Use ---
    print()
    print("--- Responses with Tools ---")
    tools = [
        {
            "type": "function",
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
        }
    ]

    # Step 1: model requests tool use
    response = openai_model.client.responses.create(
        model=openai_model.model_id,
        instructions="Use tools when available.",
        input="What's the weather in Paris?",
        tools=tools,
        max_output_tokens=1024,
    )

    fn_call = None
    for item in response.output:
        if item.type == "function_call":
            fn_call = item
            print(f"Tool call: {fn_call.name}({fn_call.arguments})")

    # Step 2: provide result via previous_response_id
    if fn_call:
        response2 = openai_model.client.responses.create(
            model=openai_model.model_id,
            previous_response_id=response.id,
            input=[
                {
                    "type": "function_call_output",
                    "call_id": fn_call.call_id,
                    "output": "Sunny, 22C, light breeze",
                }
            ],
            tools=tools,
            max_output_tokens=1024,
        )
        print(f"Assistant: {response2.output_text}")
    print()
