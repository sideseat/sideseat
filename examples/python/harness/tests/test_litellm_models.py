"""LiteLLM's model map is pinned: every catalogue model it can reach has an entry, and no run downloads one."""

from __future__ import annotations

from harness import litellm_models
from harness.models import MODELS
from harness.proxy import client_environment


def test_every_litellm_model_of_the_catalogue_is_pinned() -> None:
    pinned = litellm_models.entries()
    reached = {
        model.id for model in MODELS.values() if model.litellm_id.startswith("bedrock/")
    }
    missing = sorted(reached - set(pinned))
    assert not missing, (
        f"pin these catalogue models in {litellm_models.PATH.name}: {missing}"
    )
    for model_id in reached:
        entry = pinned[model_id]
        assert entry.get("litellm_provider") and entry.get("mode") == "chat", model_id
    # The base model too, where LiteLLM knows one: it reads capabilities - strict tools, say - from it, and the
    # bundled map a release falls back to may not know it, which changes the request it sends.
    bases = {
        model_id.split(".", 1)[1]
        for model_id in reached
        if model_id.split(".", 1)[0] in {"global", "us", "eu", "apac"}
        and model_id.split(".", 1)[1].startswith("anthropic.")
    }
    assert not sorted(bases - set(pinned)), sorted(bases - set(pinned))


def test_a_proxied_run_never_downloads_litellm_s_model_map() -> None:
    environment = client_environment("http://127.0.0.1:1")
    assert environment["LITELLM_LOCAL_MODEL_COST_MAP"] == "True"
