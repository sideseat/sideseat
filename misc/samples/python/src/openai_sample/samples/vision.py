"""Vision analysis using the OpenAI Chat Completions API.

Demonstrates:
- Sending an image as a base64 data URL
- Multi-turn Q&A about image content
- Trace grouping with client.trace()
"""

import base64
from pathlib import Path

from sideseat import SideSeat

# Content directory is at misc/content (5 levels up from this file)
CONTENT_DIR = Path(__file__).parents[5] / "content"


def run(openai_model, trace_attrs: dict, client: SideSeat):
    """Run image analysis with Chat Completions."""
    with client.trace("openai-vision"):
        img_bytes = (CONTENT_DIR / "img.jpg").read_bytes()
        img_b64 = base64.b64encode(img_bytes).decode()

        messages = [
            {
                "role": "system",
                "content": "You are an image analyst. Describe content accurately and concisely.",
            },
        ]

        # Turn 1: Describe the image
        print("--- Turn 1: Image analysis ---")
        messages.append(
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe what you see in this image."},
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/jpeg;base64,{img_b64}"},
                    },
                ],
            }
        )

        response = openai_model.client.chat.completions.create(
            model=openai_model.model_id,
            messages=messages,
            max_completion_tokens=2048,
        )

        assistant_text = response.choices[0].message.content
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

        response = openai_model.client.chat.completions.create(
            model=openai_model.model_id,
            messages=messages,
            max_completion_tokens=2048,
        )

        assistant_text = response.choices[0].message.content
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
        print()
