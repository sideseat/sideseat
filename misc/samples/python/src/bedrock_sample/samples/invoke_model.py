"""Direct Claude Messages API via invoke_model and invoke_model_with_response_stream.

Each API call produces its own independent trace.

Demonstrates:
- invoke_model (sync)
- invoke_model_with_response_stream (streaming)
- Extended thinking (sync + streaming)
- Tool use (single turn)
- Claude-specific request/response format (anthropic_version, messages array)

Note: invoke_model is Claude-specific. For multi-model support, use the Converse API.
"""

import json

from sideseat import SideSeat

_API_VERSION = "bedrock-2023-05-31"


def run(bedrock, trace_attrs: dict, client: SideSeat):
    """Run invoke_model samples."""

    # --- invoke_model (sync) ---
    print("--- invoke_model ---")
    body = json.dumps(
        {
            "anthropic_version": _API_VERSION,
            "max_tokens": 128,
            "system": "Answer in one sentence.",
            "messages": [{"role": "user", "content": "What is the speed of light?"}],
        }
    )
    response = bedrock.client.invoke_model(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )
    result = json.loads(response["body"].read())
    print(f"Assistant: {result['content'][0]['text']}")
    print()

    # --- invoke_model_with_response_stream ---
    print("--- invoke_model_with_response_stream ---")
    body = json.dumps(
        {
            "anthropic_version": _API_VERSION,
            "max_tokens": 128,
            "system": "Answer in one sentence.",
            "messages": [{"role": "user", "content": "What is the boiling point of water?"}],
        }
    )
    response = bedrock.client.invoke_model_with_response_stream(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )
    print("Assistant: ", end="")
    for event in response["body"]:
        chunk = json.loads(event["chunk"]["bytes"])
        if chunk["type"] == "content_block_delta":
            print(chunk["delta"].get("text", ""), end="", flush=True)
    print()

    # --- Extended Thinking (sync) ---
    print()
    print("--- invoke_model with Thinking ---")
    body = json.dumps(
        {
            "anthropic_version": _API_VERSION,
            "max_tokens": 8192,
            "system": "You are a math tutor. Show your work.",
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [{"role": "user", "content": "What is 27 * 453?"}],
        }
    )
    response = bedrock.client.invoke_model(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )
    result = json.loads(response["body"].read())
    for block in result["content"]:
        if block["type"] == "thinking":
            print(f"Thinking: {block['thinking'][:120]}...")
        elif block["type"] == "text":
            print(f"Assistant: {block['text']}")
    print()

    # --- Extended Thinking (streaming) ---
    print("--- invoke_model_with_response_stream with Thinking ---")
    body = json.dumps(
        {
            "anthropic_version": _API_VERSION,
            "max_tokens": 8192,
            "system": "You are a math tutor. Show your work.",
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [{"role": "user", "content": "What is 891 / 9?"}],
        }
    )
    response = bedrock.client.invoke_model_with_response_stream(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )
    thinking_text = ""
    print("Assistant: ", end="")
    for event in response["body"]:
        chunk = json.loads(event["chunk"]["bytes"])
        if chunk["type"] == "content_block_delta":
            if chunk["delta"]["type"] == "thinking_delta":
                thinking_text += chunk["delta"].get("thinking", "")
            elif chunk["delta"]["type"] == "text_delta":
                print(chunk["delta"].get("text", ""), end="", flush=True)
    if thinking_text:
        print()
        print(f"Thinking: {thinking_text[:120]}...")
    print()

    # --- Tool Use ---
    print("--- invoke_model with Tools ---")
    tools = [
        {
            "name": "get_weather",
            "description": "Get the current weather for a location.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City name, e.g. 'San Francisco'"}
                },
                "required": ["location"],
            },
        }
    ]
    messages: list[dict] = [{"role": "user", "content": "What's the weather in Paris?"}]

    # Step 1: model requests tool use
    body = json.dumps(
        {
            "anthropic_version": _API_VERSION,
            "max_tokens": 256,
            "system": "Use tools when available.",
            "tools": tools,
            "messages": messages,
        }
    )
    response = bedrock.client.invoke_model(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )
    result = json.loads(response["body"].read())
    messages.append({"role": "assistant", "content": result["content"]})

    tool_use = None
    for block in result["content"]:
        if block["type"] == "tool_use":
            tool_use = block
            print(f"Tool call: {tool_use['name']}({json.dumps(tool_use['input'])})")

    # Step 2: return tool result and get final answer
    if tool_use:
        messages.append(
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tool_use["id"],
                        "content": "Sunny, 22C, light breeze",
                    }
                ],
            }
        )
        body = json.dumps(
            {
                "anthropic_version": _API_VERSION,
                "max_tokens": 256,
                "system": "Use tools when available.",
                "tools": tools,
                "messages": messages,
            }
        )
        response = bedrock.client.invoke_model(
            modelId=bedrock.model_id,
            body=body,
            contentType="application/json",
        )
        result = json.loads(response["body"].read())
        for block in result["content"]:
            if block["type"] == "text":
                print(f"Assistant: {block['text']}")
    print()
