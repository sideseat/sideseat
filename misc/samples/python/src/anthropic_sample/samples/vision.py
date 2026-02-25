"""Vision analysis using the Anthropic Messages API.

Demonstrates:
- Sending an image as base64
- Multi-turn Q&A about image content
- Trace grouping with client.trace()
"""

import base64
from pathlib import Path

from sideseat import SideSeat

# Content directory is at misc/content (5 levels up from this file)
CONTENT_DIR = Path(__file__).parents[5] / "content"


def run(model, trace_attrs: dict, client: SideSeat):
    """Run image analysis with Messages API."""
    with client.trace("anthropic-vision"):
        img_bytes = (CONTENT_DIR / "img.jpg").read_bytes()
        img_b64 = base64.b64encode(img_bytes).decode()

        # Turn 1: Describe the image
        print("--- Turn 1: Image analysis ---")
        messages = [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe what you see in this image."},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": img_b64,
                        },
                    },
                ],
            }
        ]

        response = model.client.messages.create(
            model=model.model_id,
            system="You are an image analyst. Describe content accurately and concisely.",
            messages=messages,
            max_tokens=2048,
        )

        assistant_text = response.content[0].text
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
        print()

        # Turn 2: Follow-up question about the image
        print("--- Turn 2: Follow-up ---")
        messages.append(
            {
                "role": "user",
                "content": "Based on what you see, what is the main subject or theme of this image?",
            }
        )

        response = model.client.messages.create(
            model=model.model_id,
            system="You are an image analyst. Describe content accurately and concisely.",
            messages=messages,
            max_tokens=2048,
        )

        assistant_text = response.content[0].text
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
        print()
