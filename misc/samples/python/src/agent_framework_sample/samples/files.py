"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via Content.from_data()
- PDF document analysis via Content.from_data() with application/pdf
"""

from pathlib import Path

from agent_framework import ChatAgent, ChatMessage, DataContent, TextContent
from opentelemetry import trace


async def run(client, trace_attrs: dict):
    """Run the files sample with image and PDF analysis."""
    tracer = trace.get_tracer(__name__)

    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    image_bytes = img_path.read_bytes()
    pdf_bytes = pdf_path.read_bytes()

    agent = ChatAgent(
        chat_client=client,
        instructions="You are a file analysis AI that can read images and documents.",
    )

    message = ChatMessage(
        role="user",
        contents=[
            TextContent(
                text=f"Read the image '{img_path.name}'. "
                "Describe its contents in detail using instructions from the PDF."
            ),
            DataContent(data=image_bytes, media_type="image/jpeg"),
            DataContent(
                data=pdf_bytes,
                media_type="application/pdf",
                additional_properties={"filename": "task.pdf"},
            ),
        ],
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        print(f"Analyzing image: {img_path.name}")
        print(f"Analyzing PDF: {pdf_path.name}")
        print("-" * 50)

        result = await agent.run(message)
        print(result.text)
