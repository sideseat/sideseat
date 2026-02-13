"""Image generation and critic evaluation sample.

Demonstrates:
- Image generation using OpenAI DALL-E with image returned in tool result
- Image reading with visual content in tool result (like Strands image_reader)
- Image critique and selection via multimodal tool output
"""

import base64
import tempfile
import uuid
from pathlib import Path

from agents import Agent, Runner, ToolOutputImage, ToolOutputText, function_tool
from openai import OpenAI
from opentelemetry import trace


@function_tool
def generate_image(prompt: str) -> list:
    """Generate an image using OpenAI DALL-E.

    Args:
        prompt: The image description prompt

    Returns:
        Text with the file path and the generated image
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

    img_b64 = response.data[0].b64_json
    image_data = base64.b64decode(img_b64)

    output_dir = tempfile.mkdtemp(prefix="openai_agents_images_")
    filename = f"generated_{uuid.uuid4().hex[:8]}.png"
    filepath = Path(output_dir) / filename
    filepath.write_bytes(image_data)

    return [
        ToolOutputText(text=f"Generated image saved to {filepath}"),
        ToolOutputImage(image_url=f"data:image/png;base64,{img_b64}"),
    ]


@function_tool
def read_image(file_path: str) -> list:
    """Read an image file for visual inspection.

    Args:
        file_path: Path to the image file

    Returns:
        Text with metadata and the image content for visual analysis
    """
    path = Path(file_path)
    if not path.exists():
        return [ToolOutputText(text=f"Image not found: {file_path}")]

    data = path.read_bytes()
    img_b64 = base64.b64encode(data).decode()
    suffix = path.suffix.lower().lstrip(".")
    mime = f"image/{suffix}" if suffix in ("png", "jpeg", "gif", "webp") else "image/png"

    return [
        ToolOutputText(text=f"Image at {file_path}: {len(data)} bytes"),
        ToolOutputImage(image_url=f"data:{mime};base64,{img_b64}"),
    ]


def run(model: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the image_gen sample."""
    tracer = trace.get_tracer(__name__)

    # Artist agent that generates images
    artist = Agent(
        model=model,
        name="artist",
        instructions=(
            "You are an AI artist. When asked to generate images, use the generate_image tool "
            "with varied prompts for each image to create a diverse collection. "
            "Return a comma-separated list of the generated image paths."
        ),
        tools=[generate_image],
    )

    # Critic agent that evaluates images visually
    critic = Agent(
        model=model,
        name="critic",
        instructions=(
            "You are an experienced art critic. Use the read_image tool to load and visually "
            "inspect each image. Evaluate based on creativity, composition, and appeal. "
            "Your final line must include: FINAL DECISION: [path to best image]"
        ),
        tools=[read_image],
    )

    with tracer.start_as_current_span(
        "openai_agents.session",
        attributes=trace_attrs,
    ):
        print("Artist generating images...")
        print("-" * 50)

        # Generate images
        artist_result = Runner.run_sync(
            artist,
            "Generate 3 different images of a dog. Vary the style, setting, and mood "
            "for each image. Use different prompts like: playful puppy in a park, "
            "majestic dog portrait, dog playing in snow.",
        )

        print(f"Artist result:\n{artist_result.final_output}")

        print("\n" + "=" * 50)
        print("Critic evaluating...")
        print("-" * 50)

        # Have critic evaluate
        critic_result = Runner.run_sync(
            critic,
            f"Evaluate these generated images and select the best one:\n{artist_result.final_output}",
        )

        print(f"Critic result:\n{critic_result.final_output}")
