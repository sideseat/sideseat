"""Direct Claude Messages API via invoke_model and invoke_model_with_response_stream.

Demonstrates:
- invoke_model (synchronous, full response)
- invoke_model_with_response_stream (streaming chunks)
- Claude-specific request/response format (anthropic_version, messages array)

Note: Full message event capture requires a Claude model. Non-Claude models
get basic span attributes (model, tokens) without detailed message events.
"""

import json


def run(bedrock, trace_attrs: dict):
    """Run invoke_model samples (sync and streaming)."""
    # --- Synchronous invoke_model ---
    print("--- invoke_model (sync) ---")
    body = json.dumps(
        {
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": 256,
            "system": "You are a science educator. Explain concepts simply in 2-3 sentences.",
            "messages": [
                {"role": "user", "content": "What is quantum entanglement?"},
            ],
        }
    )

    response = bedrock.client.invoke_model(
        modelId=bedrock.model_id,
        body=body,
        contentType="application/json",
    )

    result = json.loads(response["body"].read())
    text = result["content"][0]["text"]
    usage = result.get("usage", {})
    print(f"Assistant: {text}")
    print(f"  Tokens: in={usage.get('input_tokens', 0)} out={usage.get('output_tokens', 0)}")
    print()

    # --- Streaming invoke_model ---
    print("--- invoke_model_with_response_stream ---")
    body = json.dumps(
        {
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": 256,
            "system": "You are a science educator. Explain concepts simply in 2-3 sentences.",
            "messages": [
                {"role": "user", "content": "What is dark matter?"},
            ],
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
            delta_text = chunk["delta"].get("text", "")
            print(delta_text, end="", flush=True)
    print()
