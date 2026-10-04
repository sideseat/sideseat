"""Maps a harness model alias to a Google Gen AI client in Vertex AI (Enterprise) mode."""

from dataclasses import dataclass

from google import genai
from google.genai import types
from google.oauth2.credentials import Credentials

from harness import Model
from harness.clients import fake_url


@dataclass
class Gemini:
    client: genai.Client
    model: str


def build(model: Model) -> Gemini:
    if model.surface != "fake-gemini":
        raise SystemExit(
            f"the Vertex AI suite runs the local Gemini endpoint; {model.alias} is {model.surface}"
        )
    # Against Google Cloud, `genai.Client(enterprise=True, project=..., location=...)` uses
    # Application Default Credentials; the local endpoint accepts any token.
    client = genai.Client(
        enterprise=True,
        project="sideseat-example",
        location="us-central1",
        credentials=Credentials(token="fake"),
        http_options=types.HttpOptions(base_url=fake_url(model.surface)),
    )
    return Gemini(client=client, model=model.id)
