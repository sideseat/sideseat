"""Tool use with the Bedrock Converse API.

Demonstrates:
- Tool definition via toolConfig
- Handling tool_use stop reason in a loop
- Sending tool results back to the model
- Multi-tool calls in a single response
"""

import json

TOOL_CONFIG = {
    "tools": [
        {
            "toolSpec": {
                "name": "get_weather",
                "description": "Get the current weather for a city.",
                "inputSchema": {
                    "json": {
                        "type": "object",
                        "properties": {
                            "city": {
                                "type": "string",
                                "description": "City name",
                            },
                        },
                        "required": ["city"],
                    }
                },
            }
        },
        {
            "toolSpec": {
                "name": "get_time",
                "description": "Get the current local time for a city.",
                "inputSchema": {
                    "json": {
                        "type": "object",
                        "properties": {
                            "city": {
                                "type": "string",
                                "description": "City name",
                            },
                        },
                        "required": ["city"],
                    }
                },
            }
        },
    ]
}

MOCK_WEATHER = {
    "Paris": {"temperature_c": 18, "condition": "Partly cloudy", "humidity": 65},
    "Tokyo": {"temperature_c": 24, "condition": "Sunny", "humidity": 50},
    "New York": {"temperature_c": 15, "condition": "Overcast", "humidity": 72},
}

MOCK_TIME = {
    "Paris": "14:30 CET",
    "Tokyo": "22:30 JST",
    "New York": "08:30 EST",
}


def _execute_tool(name: str, input_data: dict) -> dict:
    """Execute a mock tool and return the result."""
    city = input_data.get("city", "Unknown")
    if name == "get_weather":
        return MOCK_WEATHER.get(city, {"temperature_c": 20, "condition": "Unknown", "humidity": 50})
    if name == "get_time":
        return {"city": city, "local_time": MOCK_TIME.get(city, "12:00 UTC")}
    return {"error": f"Unknown tool: {name}"}


def run(bedrock, trace_attrs: dict):
    """Run a tool use conversation with Bedrock Converse."""
    messages = [
        {
            "role": "user",
            "content": [{"text": "What's the weather and time in Paris and Tokyo?"}],
        }
    ]

    print("User: What's the weather and time in Paris and Tokyo?")
    print()

    iteration = 0
    while True:
        iteration += 1
        response = bedrock.client.converse(
            modelId=bedrock.model_id,
            messages=messages,
            toolConfig=TOOL_CONFIG,
            inferenceConfig={"maxTokens": 1024},
        )

        stop_reason = response["stopReason"]
        assistant_msg = response["output"]["message"]
        messages.append(assistant_msg)

        if stop_reason != "tool_use":
            # Final text response
            text = assistant_msg["content"][0]["text"]
            print(f"Assistant: {text}")
            break

        # Execute requested tools
        tool_results = []
        for block in assistant_msg["content"]:
            if "toolUse" not in block:
                continue
            tool_use = block["toolUse"]
            name = tool_use["name"]
            tool_input = tool_use["input"]
            result = _execute_tool(name, tool_input)
            print(
                f"  [{iteration}] Tool call: {name}({json.dumps(tool_input)}) -> {json.dumps(result)}"
            )
            tool_results.append(
                {
                    "toolResult": {
                        "toolUseId": tool_use["toolUseId"],
                        "content": [{"json": result}],
                    }
                }
            )

        messages.append({"role": "user", "content": tool_results})
