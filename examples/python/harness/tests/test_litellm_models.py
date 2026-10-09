"""LiteLLM's model map is pinned: every catalogue model it can reach has an entry, and no run downloads one."""

from __future__ import annotations

import os
import sys
import types
from typing import Any

import pytest

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


def test_importing_the_harness_forbids_the_download() -> None:
    assert os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] == "True"


def test_pin_makes_the_pinned_entries_the_whole_description(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    folded: list[bool] = []
    fake: Any = types.ModuleType("litellm")
    pinned = litellm_models.entries()
    model_id = next(iter(pinned))
    # A map that said more about the model than the pinned entry does: a capability it must not keep.
    fake.model_cost = {model_id: {"bedrock_converse_supports_strict_tools": True}}

    def register_model(entries: dict[str, Any]) -> None:
        for key, value in entries.items():
            fake.model_cost.setdefault(key, {}).update(value)

    fake.register_model = register_model
    fake.add_known_models = lambda: folded.append(True)
    monkeypatch.setitem(sys.modules, "litellm", fake)
    monkeypatch.delenv("LITELLM_LOCAL_MODEL_COST_MAP", raising=False)
    litellm_models.pin()
    assert os.environ["LITELLM_LOCAL_MODEL_COST_MAP"] == "True"
    assert fake.model_cost[model_id] == pinned[model_id]
    assert folded, "the provider sets LiteLLM routes by are folded again"
