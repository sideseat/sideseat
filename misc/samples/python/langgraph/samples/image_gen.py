"""Image generation and critic evaluation sample.

Demonstrates:
- Multi-agent collaboration (Artist + Critic pattern)
- Image generation using Amazon Titan Image Generator
- Tool definition for image operations
- Sequential agent handoff
"""

import base64
import json
import os
import tempfile
import uuid
from pathlib import Path

import boto3
from langchain_core.messages import AIMessage, SystemMessage
from langchain_core.tools import tool
from langgraph.prebuilt import create_react_agent

# Constants
AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))
IMAGE_MODEL = "amazon.titan-image-generator-v2:0"
IMAGE_SIZE = 512


def get_bedrock_client():
    """Get Bedrock runtime client (lazy initialization)."""
    return boto3.client("bedrock-runtime", region_name=AWS_REGION)


def get_image_output_dir() -> Path:
    """Get or create temporary directory for generated images."""
    output_dir = Path(tempfile.gettempdir()) / "langgraph_images"
    output_dir.mkdir(exist_ok=True)
    return output_dir


@tool
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
        bedrock = get_bedrock_client()

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

        # Validate response
        if "images" not in result or not result["images"]:
            return "Error: No images returned from model"

        image_base64 = result["images"][0]
        image_bytes = base64.b64decode(image_base64)

        # Save to temp file
        output_dir = get_image_output_dir()
        file_path = output_dir / f"generated_{uuid.uuid4().hex[:8]}.png"
        file_path.write_bytes(image_bytes)

        return str(file_path)

    except Exception as e:
        return f"Error generating image: {e}"


@tool
def read_image(file_path: str) -> list:
    """Read an image file and return it for visual analysis.

    Args:
        file_path: Path to the image file to analyze

    Returns:
        Content blocks with the image data for the model to analyze
    """
    try:
        path = Path(file_path)
        if not path.exists():
            return f"Error: File not found: {file_path}"

        image_bytes = path.read_bytes()
        image_b64 = base64.b64encode(image_bytes).decode()
        size_kb = len(image_bytes) / 1024
        suffix = path.suffix.lower().lstrip(".")
        media_type = f"image/{suffix}" if suffix else "image/png"

        return [
            {"type": "text", "text": f"{path.name} ({size_kb:.1f} KB)"},
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": image_b64,
                },
            },
        ]

    except Exception as e:
        return f"Error reading image: {e}"


def extract_response(result: dict) -> str:
    """Extract the final text response from agent result."""
    messages = result.get("messages", [])
    for msg in reversed(messages):
        if isinstance(msg, AIMessage) and msg.content:
            if isinstance(msg.content, str):
                return msg.content
            if isinstance(msg.content, list):
                for block in msg.content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        return block.get("text", "")
    return "[No response generated]"


def run(model, trace_attrs: dict):
    """Run the image_gen sample demonstrating multi-agent collaboration.

    This sample shows:
    - Artist agent: Generates multiple image variations
    - Critic agent: Evaluates and selects the best image
    - Sequential handoff between agents
    - Tool-based image generation and reading

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    # Artist agent generates images based on prompts
    artist = create_react_agent(
        model=model,
        tools=[generate_image],
        prompt=SystemMessage(
            content=(
                "You will be instructed to generate a number of images of a given subject. "
                "Vary the prompt for each generated image to create a variety of options. "
                "Your final output must contain ONLY a comma-separated list of the filesystem paths of generated images."
            )
        ),
    )

    # Critic agent evaluates and selects the best image
    critic = create_react_agent(
        model=model,
        tools=[read_image],
        prompt=SystemMessage(
            content=(
                "You will be provided with a list of filesystem paths, each containing an image. "
                "Describe each image, and then choose which one is best. "
                "Your final line of output must be as follows: "
                "FINAL DECISION: <path to final decision image>"
            )
        ),
    )

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    # Phase 1: Artist generates images
    print("Artist generating images...")
    try:
        artist_result = artist.invoke(
            {"messages": [("user", "Generate 3 images of a dog")]},
            config=config,
        )
        artist_response = extract_response(artist_result)
        print(f"Artist result: {artist_response}")
    except Exception as e:
        print(f"[Artist Error: {e}]")
        return

    # Validate artist produced paths
    if not artist_response or "Error" in artist_response:
        print("[Artist failed to generate images]")
        return

    # Phase 2: Critic evaluates images
    print("\nCritic evaluating images...")
    try:
        critic_result = critic.invoke(
            {"messages": [("user", artist_response)]},
            config=config,
        )
        critic_response = extract_response(critic_result)
        print(f"Critic result: {critic_response}")
    except Exception as e:
        print(f"[Critic Error: {e}]")
