"""Image generation and critic evaluation sample.

Demonstrates:
- Image generation using OpenAI DALL-E
- Image critique and selection with multimodal tool results
"""

import base64
import tempfile
import uuid
from pathlib import Path

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.adk.tools import FunctionTool
from google.genai import types
from openai import OpenAI
from opentelemetry import trace

# Buffer for image Parts (avoids ADK session state serialization issues)
_pending_image_parts: list = []


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

    image_data = base64.b64decode(response.data[0].b64_json)

    output_dir = tempfile.mkdtemp(prefix="adk_images_")
    filename = f"generated_{uuid.uuid4().hex[:8]}.png"
    filepath = Path(output_dir) / filename

    with open(filepath, "wb") as f:
        f.write(image_data)

    return str(filepath)


def read_image(file_path: str) -> dict:
    """Read an image file for visual inspection.

    Args:
        file_path: Path to the image file

    Returns:
        Image metadata (actual image is injected via before_model_callback)
    """
    path = Path(file_path)
    if not path.exists():
        return {"status": "error", "message": f"Image not found: {file_path}"}

    data = path.read_bytes()
    _pending_image_parts.append(types.Part.from_bytes(data=data, mime_type="image/png"))
    return {"status": "loaded", "path": file_path, "size_bytes": len(data)}


def _inject_images(callback_context, llm_request):
    """Inject buffered image Parts into the LLM request.

    ADK doesn't support multimodal FunctionResponsePart in tool results,
    so we inject images via before_model_callback instead.
    """
    if _pending_image_parts and llm_request.contents:
        llm_request.contents[-1].parts.extend(_pending_image_parts)
        _pending_image_parts.clear()
    return None


async def run(model, trace_attrs: dict):
    """Run the image_gen sample."""
    tracer = trace.get_tracer(__name__)

    # Artist agent
    artist = LlmAgent(
        model=model,
        name="artist",
        instruction=(
            "You are an AI artist. When asked to generate images, use the generate_image tool "
            "with varied prompts for each image to create a diverse collection. "
            "Return a comma-separated list of the generated image paths."
        ),
        tools=[FunctionTool(func=generate_image)],
    )

    # Critic agent with image injection
    critic = LlmAgent(
        model=model,
        name="critic",
        instruction=(
            "You are an experienced art critic. Use the read_image tool to load and visually "
            "inspect each image. Evaluate based on creativity, composition, and appeal. "
            "Your final line must include: FINAL DECISION: [path to best image]"
        ),
        tools=[FunctionTool(func=read_image)],
        before_model_callback=_inject_images,
    )

    # Session service
    session_service = InMemorySessionService()

    with tracer.start_as_current_span(
        "adk.session",
        attributes=trace_attrs,
    ):
        # Single session for both agents
        session = await session_service.create_session(
            app_name="image_gen_sample",
            user_id="demo-user",
            session_id=trace_attrs["session.id"],
        )

        print("Artist generating images...")
        print("-" * 50)

        artist_runner = Runner(
            agent=artist,
            app_name="image_gen_sample",
            session_service=session_service,
        )

        artist_response = ""
        async for event in artist_runner.run_async(
            user_id="demo-user",
            session_id=session.id,
            new_message=types.Content(
                role="user",
                parts=[
                    types.Part(
                        text=(
                            "Generate 3 different images of a dog. Vary the style, setting, and mood "
                            "for each image. Use different prompts like: playful puppy in a park, "
                            "majestic dog portrait, dog playing in snow."
                        )
                    )
                ],
            ),
        ):
            if hasattr(event, "content") and event.content:
                for part in event.content.parts:
                    if hasattr(part, "text") and part.text:
                        artist_response += part.text

        print(f"Artist result:\n{artist_response}")

        print("\n" + "=" * 50)
        print("Critic evaluating...")
        print("-" * 50)

        critic_runner = Runner(
            agent=critic,
            app_name="image_gen_sample",
            session_service=session_service,
        )

        critic_response = ""
        async for event in critic_runner.run_async(
            user_id="demo-user",
            session_id=session.id,
            new_message=types.Content(
                role="user",
                parts=[
                    types.Part(
                        text=f"Evaluate these generated images and select the best one:\n{artist_response}"
                    )
                ],
            ),
        ):
            if hasattr(event, "content") and event.content:
                for part in event.content.parts:
                    if hasattr(part, "text") and part.text:
                        critic_response += part.text

        print(f"Critic result:\n{critic_response}")
