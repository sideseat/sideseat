"""Simple Bedrock API calls — no trace grouping, no sessions.

Each API call produces its own independent trace.

Demonstrates:
- Converse (sync)
- Converse (streaming)
- Converse with extended thinking (sync + streaming, Claude only)
- Converse with tool use (single turn)
- Works with any Bedrock model (Claude, Nova, etc.)
"""

import json

from sideseat import SideSeat


def _is_claude(model_id: str) -> bool:
    return "anthropic" in model_id.lower() or "claude" in model_id.lower()


def run(bedrock, trace_attrs: dict, client: SideSeat):
    """Run independent Bedrock API calls."""

    # --- Converse ---
    print("--- Converse ---")
    response = bedrock.client.converse(
        modelId=bedrock.model_id,
        system=[{"text": "Answer in one sentence."}],
        messages=[{"role": "user", "content": [{"text": "What is the speed of light?"}]}],
        inferenceConfig={"maxTokens": 128},
    )
    print(f"Assistant: {response['output']['message']['content'][0]['text']}")
    print()

    # --- Converse Stream ---
    print("--- Converse Stream ---")
    response = bedrock.client.converse_stream(
        modelId=bedrock.model_id,
        system=[{"text": "Answer in one sentence."}],
        messages=[{"role": "user", "content": [{"text": "What is the boiling point of water?"}]}],
        inferenceConfig={"maxTokens": 128},
    )
    print("Assistant: ", end="")
    for event in response["stream"]:
        if "contentBlockDelta" in event:
            print(event["contentBlockDelta"]["delta"].get("text", ""), end="", flush=True)
    print()

    # --- Extended Thinking (Claude only) ---
    if _is_claude(bedrock.model_id):
        print()
        print("--- Converse with Thinking ---")
        response = bedrock.client.converse(
            modelId=bedrock.model_id,
            system=[{"text": "You are a math tutor. Show your work."}],
            messages=[{"role": "user", "content": [{"text": "What is 27 * 453?"}]}],
            inferenceConfig={"maxTokens": 8192},
            additionalModelRequestFields={"thinking": {"type": "enabled", "budget_tokens": 1024}},
        )
        for block in response["output"]["message"]["content"]:
            if "reasoningContent" in block:
                text = block["reasoningContent"].get("reasoningText", {}).get("text", "")
                print(f"Thinking: {text[:120]}...")
            elif "text" in block:
                print(f"Assistant: {block['text']}")
        print()

        print("--- Converse Stream with Thinking ---")
        response = bedrock.client.converse_stream(
            modelId=bedrock.model_id,
            system=[{"text": "You are a math tutor. Show your work."}],
            messages=[{"role": "user", "content": [{"text": "What is 891 / 9?"}]}],
            inferenceConfig={"maxTokens": 8192},
            additionalModelRequestFields={"thinking": {"type": "enabled", "budget_tokens": 1024}},
        )
        thinking_text = ""
        for event in response["stream"]:
            if "contentBlockDelta" in event:
                delta = event["contentBlockDelta"]["delta"]
                if "reasoningContent" in delta:
                    thinking_text += delta["reasoningContent"].get("text", "")
                elif "text" in delta:
                    print(delta["text"], end="", flush=True)
        if thinking_text:
            print()
            print(f"Thinking: {thinking_text[:120]}...")
        print()

    # --- Tool Use ---
    print("--- Converse with Tools ---")
    tool_config = {
        "tools": [
            {
                "toolSpec": {
                    "name": "get_weather",
                    "description": "Get the current weather for a location.",
                    "inputSchema": {
                        "json": {
                            "type": "object",
                            "properties": {
                                "location": {
                                    "type": "string",
                                    "description": "City name, e.g. 'San Francisco'",
                                }
                            },
                            "required": ["location"],
                        }
                    },
                }
            }
        ]
    }
    messages = [{"role": "user", "content": [{"text": "What's the weather in Paris?"}]}]

    # Step 1: model requests tool use
    response = bedrock.client.converse(
        modelId=bedrock.model_id,
        system=[{"text": "Use tools when available."}],
        messages=messages,
        toolConfig=tool_config,
        inferenceConfig={"maxTokens": 256},
    )
    assistant_msg = response["output"]["message"]
    messages.append(assistant_msg)

    tool_use = None
    for block in assistant_msg["content"]:
        if "toolUse" in block:
            tool_use = block["toolUse"]
            print(f"Tool call: {tool_use['name']}({json.dumps(tool_use['input'])})")

    # Step 2: return tool result and get final answer
    if tool_use:
        messages.append(
            {
                "role": "user",
                "content": [
                    {
                        "toolResult": {
                            "toolUseId": tool_use["toolUseId"],
                            "content": [{"text": "Sunny, 22°C, light breeze"}],
                        }
                    }
                ],
            }
        )
        response = bedrock.client.converse(
            modelId=bedrock.model_id,
            system=[{"text": "Use tools when available."}],
            messages=messages,
            toolConfig=tool_config,
            inferenceConfig={"maxTokens": 256},
        )
        for block in response["output"]["message"]["content"]:
            if "text" in block:
                print(f"Assistant: {block['text']}")
    print()
