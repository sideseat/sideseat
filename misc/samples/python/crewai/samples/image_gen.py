"""Image generation and critic evaluation sample.

Demonstrates:
- Image generation using OpenAI DALL-E
- Image critique and selection
"""

import base64
import tempfile
import uuid
from pathlib import Path

from crewai import Agent, Crew, Process, Task
from crewai.tools import tool
from openai import OpenAI
from opentelemetry import trace


@tool
def generate_image(prompt: str) -> str:
    """Generate an image using OpenAI DALL-E.

    Args:
        prompt: The image description prompt

    Returns:
        Path to the generated image file
    """
    client = OpenAI()

    response = client.images.generate(
        model="dall-e-3",
        prompt=prompt,
        size="1024x1024",
        quality="standard",
        n=1,
        response_format="b64_json",
    )

    # Decode and save the image
    image_data = base64.b64decode(response.data[0].b64_json)

    output_dir = tempfile.mkdtemp(prefix="crewai_images_")
    filename = f"generated_{uuid.uuid4().hex[:8]}.png"
    filepath = Path(output_dir) / filename

    with open(filepath, "wb") as f:
        f.write(image_data)

    return str(filepath)


@tool
def get_image_info(image_path: str) -> str:
    """Get information about an image file.

    Args:
        image_path: Path to the image file

    Returns:
        Information about the image
    """
    path = Path(image_path)
    if not path.exists():
        return f"Image not found: {image_path}"

    size = path.stat().st_size
    return f"Image at {image_path}: {size} bytes, format: {path.suffix.upper()}"


def run(model, trace_attrs: dict):
    """Run the image_gen sample."""
    tracer = trace.get_tracer(__name__)

    # Artist agent that generates images
    artist = Agent(
        role="AI Artist",
        goal="Generate creative and varied images based on prompts",
        backstory=(
            "You are an AI artist. When asked to generate images, create varied prompts "
            "for each image to produce a diverse collection. Use the generate_image tool "
            "to create actual images."
        ),
        tools=[generate_image],
        llm=model,
        verbose=False,
    )

    # Critic agent that evaluates images
    critic = Agent(
        role="Art Critic",
        goal="Evaluate and select the best image from a collection",
        backstory=(
            "You are an experienced art critic. Evaluate images based on creativity, "
            "composition, and appeal. Your final output must include: "
            "FINAL DECISION: [path to best image]"
        ),
        tools=[get_image_info],
        llm=model,
        verbose=False,
    )

    # Artist task
    generate_task = Task(
        description=(
            "Generate 3 different images of a dog. Vary the style, setting, and mood "
            "for each image. Use different prompts like: playful puppy in a park, "
            "majestic dog portrait, dog playing in snow. "
            "Return a comma-separated list of the generated image paths."
        ),
        expected_output="A comma-separated list of image file paths",
        agent=artist,
    )

    # Critic task
    evaluate_task = Task(
        description=(
            "Evaluate the generated images and select the best one. "
            "Consider creativity, composition, and visual appeal. "
            "Your final line must be: FINAL DECISION: [path to best image]"
        ),
        expected_output="Evaluation of images with FINAL DECISION: [best image path]",
        agent=critic,
        context=[generate_task],
    )

    crew = Crew(
        agents=[artist, critic],
        tasks=[generate_task, evaluate_task],
        process=Process.sequential,
        verbose=False,
    )

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        print("Artist generating images...")
        print("-" * 50)

        result = crew.kickoff()

        print(f"\nFinal Result:\n{result.raw}")
