"""PDF document analysis using the Bedrock Converse API.

Demonstrates:
- Sending a PDF as a document content block
- Combining PDF and image in one request for cross-reference analysis
- Multi-turn Q&A about document content
"""

from pathlib import Path

# Content directory is at misc/content (5 levels up from this file)
CONTENT_DIR = Path(__file__).parents[5] / "content"


def run(bedrock, trace_attrs: dict):
    """Run PDF and multimodal document analysis."""
    pdf_bytes = (CONTENT_DIR / "task.pdf").read_bytes()
    img_bytes = (CONTENT_DIR / "img.jpg").read_bytes()

    system = [{"text": "You are a document analyst. Summarize content accurately and concisely."}]
    config = {"maxTokens": 512}
    messages = []

    # Turn 1: Analyze the PDF
    print("--- Turn 1: PDF analysis ---")
    messages.append(
        {
            "role": "user",
            "content": [
                {"text": "Summarize the key points of this document."},
                {
                    "document": {
                        "format": "pdf",
                        "name": "task-document",
                        "source": {"bytes": pdf_bytes},
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

    # Turn 2: Cross-reference PDF and image
    print("--- Turn 2: PDF + image cross-reference ---")
    messages.append(
        {
            "role": "user",
            "content": [
                {
                    "text": (
                        "Now look at this image. Using the instructions from the PDF document, "
                        "describe what you see in the image."
                    ),
                },
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

    # Turn 3: Follow-up without re-sending the files
    print("--- Turn 3: Follow-up ---")
    messages.append(
        {
            "role": "user",
            "content": [
                {"text": "Based on both the document and image, what is the main takeaway?"}
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
