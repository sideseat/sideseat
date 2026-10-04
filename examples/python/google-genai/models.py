"""Maps a harness model alias to a Gemini Developer API client and model id."""

from dataclasses import dataclass

from google import genai
from google.genai import types

from harness import Model
from harness.clients import fake_url


@dataclass
class Gemini:
    client: genai.Client
    model: str


def build(model: Model) -> Gemini:
    if model.surface != "fake-gemini":
        raise SystemExit(
            f"the Google Gen AI suite runs the local Gemini endpoint; {model.alias} is {model.surface}"
        )
    # Against Google, `genai.Client()` reads GEMINI_API_KEY; the local endpoint accepts any key.
    client = genai.Client(
        api_key="fake", http_options=types.HttpOptions(base_url=fake_url(model.surface))
    )
    return Gemini(client=client, model=model.id)
