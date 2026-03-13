"""Image generation and critic evaluation sample.

Demonstrates multi-agent collaboration:
- Artist agent generates images using Amazon Titan Image Generator via Bedrock
- Critic agent evaluates images using multimodal analysis
"""

import base64
import json
import os
import tempfile
import uuid
from pathlib import Path
from typing import Annotated

import boto3
from agent_framework import Agent, Content, Message, tool
from opentelemetry import trace
from pydantic import Field

AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))
IMAGE_MODEL = "amazon.titan-image-generator-v2:0"
IMAGE_SIZE = 512

_generated_paths: list[Path] = []


@tool(approval_mode="never_require")
def generate_image(
    prompt: Annotated[
        str, Field(description="Text description of the image to generate")
    ],
) -> str:
    """Generate an image using Amazon Titan Image Generator via Bedrock.

    Returns the filesystem path to the generated image, or an error message.
    """
    if not prompt or not prompt.strip():
        return "Error: Empty prompt provided"

    try:
        bedrock = boto3.client("bedrock-runtime", region_name=AWS_REGION)

        body = {
            "taskType": "TEXT_IMAGE",
            "textToImageParams": {"text": prompt},
            "imageGenerationConfig": {
                "numberOfImages": 1,
                "height": IMAGE_SIZE,
                "width": IMAGE_SIZE,
                "cfgScale": 8.0,
            },
        }

        response = bedrock.invoke_model(
            modelId=IMAGE_MODEL,
            body=json.dumps(body),
            contentType="application/json",
            accept="application/json",
        )

        result = json.loads(response["body"].read())

        if "images" not in result or not result["images"]:
            return "Error: No images returned from model"

        image_bytes = base64.b64decode(result["images"][0])

        output_dir = Path(tempfile.mkdtemp(prefix="agent_framework_images_"))
        filepath = output_dir / f"generated_{uuid.uuid4().hex[:8]}.png"
        filepath.write_bytes(image_bytes)
        _generated_paths.append(filepath)

        return str(filepath)

    except Exception as e:
        return f"Error generating image: {e}"


async def run(client, trace_attrs: dict):
    """Run the image_gen sample with artist and critic agents."""
    tracer = trace.get_tracer(__name__)

    # Artist agent generates images via Bedrock Titan
    artist = Agent(
        client=client,
        instructions=(
            "You are an AI artist. When asked to generate images, use the generate_image tool "
            "with varied prompts for each image to create a diverse collection. "
            "Your final output must contain ONLY a comma-separated list of the filesystem paths "
            "of generated images."
        ),
        tools=[generate_image],
    )

    # Critic agent evaluates generated images visually
    critic = Agent(
        client=client,
        instructions=(
            "You are an art critic. Evaluate each image based on creativity, composition, "
            "and appeal. Choose the best one. "
            "Your final line must be: FINAL DECISION: [your choice]"
        ),
    )

    with tracer.start_as_current_span(
        "agent_framework.session", attributes=trace_attrs
    ):
        # Phase 1: Artist generates images via Bedrock Titan
        print("Artist generating images...")
        print("-" * 50)

        _generated_paths.clear()
        await artist.run(
            "Generate 3 different creative images of a dog. "
            "Vary the style, setting, and mood for each."
        )

        paths = [p for p in _generated_paths if p.exists()]

        if not paths:
            print("[No images generated]")
            return

        print(f"Generated {len(paths)} images")
        print("=" * 50)
        print("Critic evaluating...")
        print("-" * 50)

        # Phase 2: Critic evaluates images via multimodal message
        contents: list = [
            Content.from_text(
                "Evaluate these generated images and select the best one:"
            )
        ]
        for i, path in enumerate(paths, 1):
            image_bytes = path.read_bytes()
            contents.append(Content.from_text(f"\nImage {i} ({path.name}):"))
            contents.append(Content.from_data(data=image_bytes, media_type="image/png"))

        critic_message = Message(role="user", contents=contents)
        critic_result = await critic.run(critic_message)
        print(f"Critic:\n{critic_result.text}")
