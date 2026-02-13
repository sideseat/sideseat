"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via multimodal messages
- PDF document analysis via page rendering (pymupdf)
"""

from io import BytesIO
from pathlib import Path

import pymupdf
from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.messages import MultiModalMessage
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_core import Image
from opentelemetry import trace
from PIL import Image as PILImage

SYSTEM_PROMPT = "You are a file analysis AI that can read images and documents."


def pdf_to_images(pdf_path: Path, max_pages: int = 3) -> list[Image]:
    """Render PDF pages as AutoGen Image objects via pymupdf."""
    images: list[Image] = []
    doc = pymupdf.open(pdf_path)
    for page_num in range(min(len(doc), max_pages)):
        page = doc[page_num]
        pix = page.get_pixmap(dpi=150)
        pil_img = PILImage.open(BytesIO(pix.tobytes("png")))
        images.append(Image(pil_img))
    doc.close()
    return images


async def run(model_client, trace_attrs: dict):
    """Run the files sample with image and PDF analysis."""
    tracer = trace.get_tracer(__name__)

    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    # Load image using AutoGen's Image class for multimodal message
    image = Image.from_file(img_path)

    # Render PDF pages as images (AutoGen has no native PDF support)
    pdf_pages = pdf_to_images(pdf_path)

    agent = AssistantAgent(
        name="file_analyzer",
        model_client=model_client,
        system_message=SYSTEM_PROMPT,
    )

    termination = MaxMessageTermination(max_messages=3)
    team = RoundRobinGroupChat([agent], termination_condition=termination)

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        multimodal_message = MultiModalMessage(
            content=[
                "Describe the image contents in detail.",
                image,
                f"Now read the attached PDF document ({pdf_path.name}, {len(pdf_pages)} page(s))"
                " and follow its instructions.",
                *pdf_pages,
            ],
            source="user",
        )

        print(f"Analyzing image: {img_path}")
        print(f"Analyzing PDF: {pdf_path} ({len(pdf_pages)} page(s) rendered)")
        print("-" * 50)

        result = await team.run(task=multimodal_message)

        for message in result.messages:
            if hasattr(message, "content") and message.content:
                if hasattr(message, "source") and message.source == agent.name:
                    print(f"Analysis:\n{message.content}")

    await model_client.close()
