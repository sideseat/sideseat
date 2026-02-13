"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via multimodal content (inline bytes)
- PDF/document analysis via multimodal content (inline bytes)
"""

from pathlib import Path

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.adk.tools import FunctionTool
from google.genai import types
from opentelemetry import trace


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


def read_text_file(file_path: str) -> str:
    """Read contents of a text file.

    Args:
        file_path: Path to the text file
    """
    path = Path(file_path)
    if not path.exists():
        return f"File not found: {file_path}"

    if path.suffix in [".txt", ".md", ".json", ".py", ".csv"]:
        return path.read_text()[:2000]

    return f"Cannot read binary file: {path.name}"


async def run(model, trace_attrs: dict):
    """Run the files sample with image and PDF analysis."""
    tracer = trace.get_tracer(__name__)

    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    # Read file bytes for multimodal content
    img_bytes = img_path.read_bytes()
    pdf_bytes = pdf_path.read_bytes()

    # Create tools
    metadata_tool = FunctionTool(func=read_file_metadata)
    text_tool = FunctionTool(func=read_text_file)

    # Create file analyzer agent
    agent = LlmAgent(
        model=model,
        name="file_analyzer",
        instruction="You are a file analysis AI that can read images and documents.",
        tools=[metadata_tool, text_tool],
    )

    # Create session service and runner
    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name="files_sample",
        user_id="demo-user",
    )

    runner = Runner(
        agent=agent,
        app_name="files_sample",
        session_service=session_service,
    )

    with tracer.start_as_current_span(
        "adk.session",
        attributes=trace_attrs,
    ):
        print(f"Analyzing image: {img_path}")
        print(f"Task document: {pdf_path}")
        print("-" * 50)

        # Send image and PDF as inline multimodal content
        # (matching how Strands/Vercel send files directly to the model)
        response_text = ""
        async for event in runner.run_async(
            user_id="demo-user",
            session_id=session.id,
            new_message=types.Content(
                role="user",
                parts=[
                    types.Part(
                        text=(
                            "Describe the contents of this image in detail "
                            "using instructions from the PDF document."
                        )
                    ),
                    types.Part.from_bytes(
                        data=img_bytes,
                        mime_type="image/jpeg",
                    ),
                    types.Part.from_bytes(
                        data=pdf_bytes,
                        mime_type="application/pdf",
                    ),
                ],
            ),
        ):
            if hasattr(event, "content") and event.content:
                for part in event.content.parts:
                    if hasattr(part, "text") and part.text:
                        response_text += part.text

        print(f"Analysis:\n{response_text}")
