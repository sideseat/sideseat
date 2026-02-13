"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via vision model (base64 image input)
- PDF document analysis via page rendering (pymupdf)
"""

import base64
from pathlib import Path

import pymupdf
from agents import Agent, Runner, function_tool
from opentelemetry import trace

SYSTEM_PROMPT = "You are a file analysis AI that can read images and documents."


def pdf_to_images(pdf_path: Path, max_pages: int = 3) -> list[dict]:
    """Render PDF pages as base64 input_image items via pymupdf."""
    images: list[dict] = []
    doc = pymupdf.open(pdf_path)
    for page_num in range(min(len(doc), max_pages)):
        page = doc[page_num]
        pix = page.get_pixmap(dpi=150)
        img_b64 = base64.b64encode(pix.tobytes("png")).decode()
        images.append(
            {
                "type": "input_image",
                "image_url": f"data:image/png;base64,{img_b64}",
            }
        )
    doc.close()
    return images


@function_tool
def read_file_metadata(file_path: str) -> str:
    """Get metadata about a file.

    Args:
        file_path: Path to the file
    """
    path = Path(file_path)
    if not path.exists():
        return f"File not found: {file_path}"

    size = path.stat().st_size
    return f"File: {path.name}, size: {size} bytes, type: {path.suffix}"


def run(model: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the files sample with image and PDF analysis."""
    tracer = trace.get_tracer(__name__)

    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    # Read image as base64
    img_b64 = base64.b64encode(img_path.read_bytes()).decode()

    # Render PDF pages as images (input_file is unreliable across GPT models)
    pdf_pages = pdf_to_images(pdf_path)

    agent = Agent(
        model=model,
        name="file_analyzer",
        instructions=SYSTEM_PROMPT,
        tools=[read_file_metadata],
    )

    with tracer.start_as_current_span(
        "openai_agents.session",
        attributes=trace_attrs,
    ):
        print(f"Analyzing image: {img_path}")
        print(f"Analyzing PDF: {pdf_path} ({len(pdf_pages)} page(s) rendered)")
        print("-" * 50)

        # Send image and PDF pages as multimodal content (Responses API format)
        result = Runner.run_sync(
            agent,
            [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": (
                                "Read the attached image. "
                                "Describe its contents in detail "
                                "using instructions from the attached PDF pages "
                                "(rendered as images)."
                            ),
                        },
                        {
                            "type": "input_image",
                            "image_url": f"data:image/jpeg;base64,{img_b64}",
                        },
                        *pdf_pages,
                    ],
                }
            ],
        )

        print(f"Analysis:\n{result.final_output}")
