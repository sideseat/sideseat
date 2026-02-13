"""Image generation and critic evaluation sample.

Demonstrates:
- Multi-agent collaboration (artist + critic)
- Image generation using Amazon Titan Image Generator via Bedrock
- Multimodal critic evaluation with generated images
"""

import base64
import json
import os
import tempfile
import uuid
from pathlib import Path

import boto3
from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.messages import MultiModalMessage
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_core import Image
from opentelemetry import trace

AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))
IMAGE_MODEL = "amazon.titan-image-generator-v2:0"
IMAGE_SIZE = 512

_generated_paths: list[Path] = []


def generate_image(prompt: str) -> str:
    """Generate an image using Amazon Titan Image Generator.

    Args:
        prompt: Text description of the image to generate

    Returns:
        File path to the generated image, or error message
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

        output_dir = Path(tempfile.mkdtemp(prefix="autogen_images_"))
        filepath = output_dir / f"generated_{uuid.uuid4().hex[:8]}.png"
        filepath.write_bytes(image_bytes)
        _generated_paths.append(filepath)

        return str(filepath)

    except Exception as e:
        return f"Error generating image: {e}"


async def run(model_client, trace_attrs: dict):
    """Run the image_gen sample with artist and critic agents."""
    tracer = trace.get_tracer(__name__)

    # Artist agent that generates images via Bedrock Titan
    artist = AssistantAgent(
        name="artist",
        model_client=model_client,
        tools=[generate_image],
        reflect_on_tool_use=False,
        system_message=(
            "You are an AI artist. When asked to generate images, use the generate_image tool "
            "with varied prompts for each image to create a diverse collection. "
            "Your final output must contain ONLY a comma-separated list of the filesystem paths "
            "of generated images."
        ),
    )

    # Critic agent that evaluates images visually
    critic = AssistantAgent(
        name="critic",
        model_client=model_client,
        system_message=(
            "You are an art critic. Evaluate each image based on creativity, composition, "
            "and appeal. Choose the best one. "
            "Your final line must be: FINAL DECISION: [your choice]"
        ),
    )

    artist_termination = MaxMessageTermination(max_messages=10)
    artist_team = RoundRobinGroupChat([artist], termination_condition=artist_termination)

    critic_termination = MaxMessageTermination(max_messages=3)
    critic_team = RoundRobinGroupChat([critic], termination_condition=critic_termination)

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        # Phase 1: Artist generates images via Bedrock Titan
        print("Artist generating images...")
        print("-" * 50)

        _generated_paths.clear()
        artist_result = await artist_team.run(
            task="Generate 3 different creative images of a dog. "
            "Vary the style, setting, and mood for each."
        )

        paths = [p for p in _generated_paths if p.exists()]

        if not paths:
            print("[No images generated]")
            await model_client.close()
            return

        print(f"Generated {len(paths)} images")
        print("=" * 50)
        print("Critic evaluating...")
        print("-" * 50)

        # Phase 2: Critic evaluates images visually via MultiModalMessage
        images = [Image.from_file(p) for p in paths]
        content: list = ["Evaluate these generated images and select the best one:"]
        for i, (path, img) in enumerate(zip(paths, images), 1):
            content.append(f"\nImage {i} ({path.name}):")
            content.append(img)

        critic_result = await critic_team.run(
            task=MultiModalMessage(content=content, source="user"),
        )

        for message in critic_result.messages:
            if hasattr(message, "source") and message.source == critic.name:
                if hasattr(message, "content") and message.content:
                    print(f"Critic:\n{message.content}")

    await model_client.close()
