"""LiteLLM's model map, pinned: the catalogue's models as LiteLLM describes them, never fetched at run time.

LiteLLM decides how it calls a model - which Bedrock API, which parameters it may send - from a model map it
downloads from GitHub when it is imported, and falls back to the copy its release bundles when it cannot.
A replay then depends on the network and on the day. A release whose bundled map does not know a catalogue
model calls Bedrock InvokeModel, where the cassette recorded Converse. So the harness forbids the download
(``ENV``, which every proxied run gets) and registers this module's entries (``pin``), which every suite
built on LiteLLM calls before it builds a model. Every release and every machine then reads the same
description.

``litellm_models.json`` holds, for each catalogue model reached through LiteLLM, its entry and its base
model's (the id without the cross-region prefix, which LiteLLM reads some capabilities from), copied from
LiteLLM's published map. A model added to the catalogue needs both
(``test_every_litellm_model_of_the_catalogue_is_pinned``).
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

PATH = Path(__file__).with_name("litellm_models.json")

#: Makes LiteLLM read the map its release bundles instead of downloading one.
ENV = {"LITELLM_LOCAL_MODEL_COST_MAP": "True"}


def entries() -> dict[str, dict[str, Any]]:
    """The pinned entries, by model id."""
    return json.loads(PATH.read_text())


def pin() -> None:
    """Makes the pinned entries the whole of what LiteLLM knows of these models.

    Registering merges an entry into what the map already said, so each entry is then set as it is pinned, and
    a capability another map stated cannot survive. Registering does not update the per-provider sets LiteLLM
    routes by - which models take Bedrock Converse - so the sets are folded again from the updated map. The
    harness package forbids the download when it is imported, before any suite imports LiteLLM.
    """
    os.environ.update(ENV)
    import litellm

    pinned = entries()
    litellm.register_model(pinned)
    for model_id, entry in pinned.items():
        # LiteLLM also reads a model under its routed names; a bundled entry there must not survive either.
        for name in (model_id, *aliases(model_id)):
            if name == model_id or name in litellm.model_cost:
                litellm.model_cost[name] = dict(entry)
    litellm.add_known_models()


def aliases(model_id: str) -> tuple[str, ...]:
    """The names LiteLLM may look a Bedrock model up under besides its id."""
    return (f"bedrock/{model_id}", f"bedrock/converse/{model_id}")
