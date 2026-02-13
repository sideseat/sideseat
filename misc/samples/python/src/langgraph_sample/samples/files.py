"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via base64 encoding
- Document analysis (PDF) via base64 encoding
- Multimodal message construction for LangChain
- Using ChatBedrockConverse with images and documents
"""

import base64
from pathlib import Path

from langchain_core.messages import HumanMessage, SystemMessage


def run(model, trace_attrs: dict):
    """Run the files sample with image and PDF analysis.

    This sample demonstrates multimodal capabilities:
    - Reading and encoding image files (JPEG/PNG)
    - Reading and encoding PDF documents
    - Constructing multimodal LangChain messages
    - Analyzing visual and document content together

    Args:
        model: LangChain chat model instance (must support multimodal)
        trace_attrs: Dictionary with session.id and user.id for tracing

    Raises:
        FileNotFoundError: If required content files are missing
    """
    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"

    # Validate content directory exists
    if not content_dir.exists():
        raise FileNotFoundError(
            f"Content directory not found: {content_dir}\n"
            "Ensure misc/content/ directory exists with img.jpg and task.pdf"
        )

    # Define file paths
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    # Validate files exist
    for path, desc in [(img_path, "Image file"), (pdf_path, "PDF file")]:
        if not path.exists():
            raise FileNotFoundError(f"{desc} not found: {path}")

    # Read and encode files
    try:
        img_bytes = img_path.read_bytes()
        pdf_bytes = pdf_path.read_bytes()
    except IOError as e:
        raise IOError(f"Failed to read content files: {e}") from e

    img_base64 = base64.standard_b64encode(img_bytes).decode("utf-8")
    pdf_base64 = base64.standard_b64encode(pdf_bytes).decode("utf-8")

    print(f"Loaded image: {img_path.name} ({len(img_bytes):,} bytes)")
    print(f"Loaded PDF: {pdf_path.name} ({len(pdf_bytes):,} bytes)")

    # Construct multimodal message using LangChain format
    messages = [
        SystemMessage(content="You are a file analysis AI that can read images and documents."),
        HumanMessage(
            content=[
                {
                    "type": "text",
                    "text": "Describe the contents of this image in detail using instructions from the PDF document.",
                },
                {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/jpeg;base64,{img_base64}"},
                },
                {
                    "type": "file",
                    "mime_type": "application/pdf",
                    "base64": pdf_base64,
                    "name": "task-document",
                },
            ]
        ),
    ]

    try:
        result = model.invoke(messages)
        print("\nAnalysis Result:")
        print("-" * 40)
        print(result.content)
    except Exception as e:
        print(f"[Error during analysis: {e}]")
