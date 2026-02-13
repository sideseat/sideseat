"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis (jpg, png, etc.) via image_reader tool
- Document analysis (pdf) via document content
"""

from pathlib import Path

from strands import Agent
from strands_tools import image_reader


def run(model, trace_attrs: dict):
    """Run the files sample with image and PDF analysis."""
    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"

    agent = Agent(
        model=model,
        tools=[image_reader],
        system_prompt="You are a file analysis AI that can read images and documents.",
        trace_attributes=trace_attrs,
    )

    pdf_path = content_dir / "task.pdf"
    pdf_bytes = pdf_path.read_bytes()
    img_path = content_dir / "img.jpg"

    # Send PDF as document content block (Bedrock multimodal format)
    # Note: Bedrock requires alphanumeric names (no dots/special chars)
    result = agent(
        [
            {
                "text": (
                    f"Read the image '{img_path}'. "
                    "Describe its contents in detail using instructions from PDF."
                )
            },
            {
                "document": {
                    "format": "pdf",
                    "name": "task-document",
                    "source": {"bytes": pdf_bytes},
                }
            },
        ]
    )
    print(result)
