"""File analysis sample demonstrating multimodal capabilities.

Demonstrates:
- Image analysis via multimodal agent
- PDF/document analysis
"""

from pathlib import Path

from crewai import Agent, Crew, Process, Task
from crewai.tools import tool
from opentelemetry import trace

SYSTEM_PROMPT = "You are a file analysis AI that can read images and documents."


@tool
def read_file_contents(file_path: str) -> str:
    """Read and return the contents of a text file or describe a binary file.

    Args:
        file_path: Path to the file to read
    """
    path = Path(file_path)
    if not path.exists():
        return f"File not found: {file_path}"

    # For text files, read contents
    if path.suffix in [".txt", ".md", ".json", ".py", ".csv"]:
        return path.read_text()

    # For binary files, return metadata
    size = path.stat().st_size
    return f"Binary file: {path.name}, size: {size} bytes, type: {path.suffix}"


@tool
def analyze_image_metadata(image_path: str) -> str:
    """Get metadata about an image file.

    Args:
        image_path: Path to the image file
    """
    path = Path(image_path)
    if not path.exists():
        return f"Image not found: {image_path}"

    size = path.stat().st_size
    return f"Image: {path.name}, size: {size} bytes, format: {path.suffix.upper()}"


def run(model, trace_attrs: dict):
    """Run the files sample with image and PDF analysis."""
    tracer = trace.get_tracer(__name__)

    # Content directory is at misc/content (5 levels up from this file)
    content_dir = Path(__file__).parents[5] / "content"
    img_path = content_dir / "img.jpg"
    pdf_path = content_dir / "task.pdf"

    # Create multimodal-capable agent
    file_analyzer = Agent(
        role="File Analyzer",
        goal="Analyze files including images and documents",
        backstory=SYSTEM_PROMPT,
        tools=[read_file_contents, analyze_image_metadata],
        llm=model,
        verbose=False,
    )

    # Task to analyze the image following PDF instructions
    analysis_task = Task(
        description=f"""Analyze the image at {img_path}.

The task document (PDF at {pdf_path}) contains the following instructions:
"Describe the image contents in detail. Focus on:
1. Main subjects and objects
2. Colors and composition
3. Any text or symbols visible
4. Overall mood or atmosphere"

First get metadata about the image, then provide a detailed analysis following these instructions.
Base your analysis on what you can infer from the image metadata and common image analysis principles.""",
        expected_output="A detailed analysis of the image following the task instructions",
        agent=file_analyzer,
    )

    crew = Crew(
        agents=[file_analyzer],
        tasks=[analysis_task],
        process=Process.sequential,
        verbose=False,
    )

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        print(f"Analyzing image: {img_path}")
        print(f"Task document: {pdf_path}")
        print("-" * 50)

        result = crew.kickoff()

        print(f"Analysis:\n{result.raw}")
