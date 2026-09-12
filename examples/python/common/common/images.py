"""Bedrock image generation shared by the image_gen samples.

Previously each suite called OpenAI DALL-E directly, which needs OPENAI_API_KEY. These
samples exist to produce telemetry, not to demonstrate a particular image provider, so
they now go through Bedrock like the rest of the suites.

Bedrock's image models take incompatible request bodies - Stability uses a flat
prompt/mode shape, Amazon's Titan and Nova Canvas use taskType/textToImageParams - but
all of them return the PNG in `images[0]`, so only the request differs.
"""

from __future__ import annotations

import base64
import json
import os
import tempfile
import uuid
from pathlib import Path

import boto3

# amazon.titan-image-generator-v2:0 was retired and now returns ResourceNotFoundException
# in every region. Stability SD3.5 Large is the current on-demand model, and it is only
# offered in a subset of regions - set AWS_REGION to one of them (us-west-2 works).
DEFAULT_IMAGE_MODEL = "stability.sd3-5-large-v1:0"


def _request_body(model_id: str, prompt: str, *, width: int, height: int) -> str:
    if model_id.startswith("stability."):
        return json.dumps(
            {"prompt": prompt, "mode": "text-to-image", "output_format": "png"}
        )
    return json.dumps(
        {
            "taskType": "TEXT_IMAGE",
            "textToImageParams": {"text": prompt},
            "imageGenerationConfig": {
                "numberOfImages": 1,
                "width": width,
                "height": height,
                "cfgScale": 8.0,
            },
        }
    )


def generate_image_bedrock(
    prompt: str,
    *,
    width: int = 1024,
    height: int = 1024,
    prefix: str = "sample_images_",
) -> str:
    """Generate one image and return the path it was written to.

    The model comes from IMAGE_GEN_MODEL, falling back to `DEFAULT_IMAGE_MODEL`.
    """
    model_id = os.getenv("IMAGE_GEN_MODEL") or DEFAULT_IMAGE_MODEL
    # botocore reads AWS_DEFAULT_REGION and ignores AWS_REGION, so a sample configured
    # with only AWS_REGION would silently use the region from ~/.aws/config - where the
    # image model usually is not offered. Pass it through explicitly.
    region = os.getenv("AWS_DEFAULT_REGION") or os.getenv("AWS_REGION")
    client = boto3.client(
        "bedrock-runtime", **({"region_name": region} if region else {})
    )

    response = client.invoke_model(
        modelId=model_id,
        contentType="application/json",
        body=_request_body(model_id, prompt, width=width, height=height),
    )
    payload = json.loads(response["body"].read())

    images = payload.get("images") or payload.get("artifacts") or []
    if not images:
        raise RuntimeError(
            f"{model_id} returned no image. Response keys: {sorted(payload)}"
        )
    first = images[0]
    # Titan/Stability return a base64 string; older Stability shapes nest it under
    # `base64` inside each artifact.
    encoded = first if isinstance(first, str) else first.get("base64", "")

    output_dir = Path(tempfile.mkdtemp(prefix=prefix))
    filepath = output_dir / f"generated_{uuid.uuid4().hex[:8]}.png"
    filepath.write_bytes(base64.b64decode(encoded))
    return str(filepath)
