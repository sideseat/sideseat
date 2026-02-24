"""Image analysis using the Bedrock Converse API with multimodal input.

Demonstrates:
- Sending an image (JPEG) as a binary content block
- Combining text and image in a single message
- Multi-turn follow-up questions about the image
"""

from pathlib import Path

# Content directory is at misc/content (5 levels up from this file)
CONTENT_DIR = Path(__file__).parents[5] / "content"


def run(bedrock, trace_attrs: dict):
    """Run image analysis with Bedrock Converse."""
    img_bytes = (CONTENT_DIR / "img.jpg").read_bytes()

    system = [{"text": "You are a visual analyst. Describe images accurately and concisely."}]
    config = {"maxTokens": 512}
    messages = []

    # Turn 1: Describe the image
    print("--- Turn 1: Describe image ---")
    messages.append(
        {
            "role": "user",
            "content": [
                {"text": "Describe this image in detail."},
                {
                    "image": {
                        "format": "jpeg",
                        "source": {"bytes": img_bytes},
                    }
                },
            ],
        }
    )

    response = bedrock.client.converse(
        modelId=bedrock.model_id,
        system=system,
        messages=messages,
        inferenceConfig=config,
    )

    assistant_msg = response["output"]["message"]
    messages.append(assistant_msg)
    print(f"Assistant: {assistant_msg['content'][0]['text']}")
    print()

    # Turn 2: Follow-up question about the image
    print("--- Turn 2: Follow-up ---")
    messages.append(
        {
            "role": "user",
            "content": [{"text": "What colors are most prominent in this image?"}],
        }
    )

    response = bedrock.client.converse(
        modelId=bedrock.model_id,
        system=system,
        messages=messages,
        inferenceConfig=config,
    )

    assistant_msg = response["output"]["message"]
    messages.append(assistant_msg)
    print(f"Assistant: {assistant_msg['content'][0]['text']}")
