"""Maps a harness model alias to an Agents SDK model on the Responses API."""

from agents import OpenAIResponsesModel

from harness import Model
from harness.clients import openai_client


def build(model: Model) -> OpenAIResponsesModel:
    # Current GPT models always reason, and Bedrock's OpenAI-compatible endpoint serves function tools to
    # a reasoning model only on the Responses API, never on Chat Completions.
    if model.surface not in {"bedrock-openai", "fake-openai"}:
        raise SystemExit(
            f"the OpenAI Agents suite runs the Responses API; {model.alias} is {model.surface}"
        )
    # Reasoning can take minutes; a shorter read timeout retries a request the model already answered,
    # and the retry records a second, different answer.
    client = openai_client(model, asynchronous=True).with_options(timeout=600)
    return OpenAIResponsesModel(model=model.id, openai_client=client)
