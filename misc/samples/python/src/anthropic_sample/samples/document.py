"""PDF document analysis using the Anthropic Messages API.

Demonstrates:
- Sending a PDF as a base64 document content block
- Combining PDF and image in one request for cross-reference analysis
- Multi-turn Q&A about document content
"""

import base64
from pathlib import Path

from sideseat import SideSeat

# Content directory is at misc/content (5 levels up from this file)
CONTENT_DIR = Path(__file__).parents[5] / "content"


def run(model, trace_attrs: dict, client: SideSeat):
    """Run PDF and multimodal document analysis."""
    with client.trace("anthropic-document"):
        pdf_bytes = (CONTENT_DIR / "task.pdf").read_bytes()
        img_bytes = (CONTENT_DIR / "img.jpg").read_bytes()
        pdf_b64 = base64.b64encode(pdf_bytes).decode()
        img_b64 = base64.b64encode(img_bytes).decode()

        system = "You are a document analyst. Summarize content accurately and concisely."
        messages = []

        # Turn 1: Analyze the PDF
        print("--- Turn 1: PDF analysis ---")
        messages.append(
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Summarize the key points of this document."},
                    {
                        "type": "document",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": pdf_b64,
                        },
                    },
                ],
            }
        )

        response = model.client.messages.create(
            model=model.model_id,
            system=system,
            messages=messages,
            max_tokens=512,
        )

        assistant_text = response.content[0].text
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
        print()

        # Turn 2: Cross-reference PDF and image
        print("--- Turn 2: PDF + image cross-reference ---")
        messages.append(
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": (
                            "Now look at this image. Using the instructions from the PDF document, "
                            "describe what you see in the image."
                        ),
                    },
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
        )

        response = model.client.messages.create(
            model=model.model_id,
            system=system,
            messages=messages,
            max_tokens=512,
        )

        assistant_text = response.content[0].text
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
        print()

        # Turn 3: Follow-up without re-sending the files
        print("--- Turn 3: Follow-up ---")
        messages.append(
            {
                "role": "user",
                "content": "Based on both the document and image, what is the main takeaway?",
            }
        )

        response = model.client.messages.create(
            model=model.model_id,
            system=system,
            messages=messages,
            max_tokens=512,
        )

        assistant_text = response.content[0].text
        messages.append({"role": "assistant", "content": assistant_text})
        print(f"Assistant: {assistant_text}")
