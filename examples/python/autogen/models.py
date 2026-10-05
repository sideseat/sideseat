"""Maps a harness model alias to AutoGen's OpenAI chat client on an OpenAI-compatible endpoint."""

from autogen_core.models import ModelFamily, ModelInfo
from autogen_ext.models.openai import OpenAIChatCompletionClient

from harness import Model
from harness.clients import fake_url


def build(model: Model) -> OpenAIChatCompletionClient:
    if model.surface != "fake-openai":
        raise SystemExit(
            f"the AutoGen suite runs the local OpenAI endpoint; {model.alias} is {model.surface}"
        )
    # The model is not one AutoGen knows by name, so its capabilities are stated.
    return OpenAIChatCompletionClient(
        model=model.id,
        base_url=fake_url(model.surface),
        api_key="fake",
        model_info=ModelInfo(
            vision=True,
            function_calling=True,
            json_output=True,
            structured_output=True,
            family=ModelFamily.UNKNOWN,
            multiple_system_messages=False,
        ),
    )
