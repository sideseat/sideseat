"""Regression tests for the deterministic Gemini-compatible fixture server."""

import importlib.util
from pathlib import Path
from types import ModuleType


def _fake_google_genai() -> ModuleType:
    path = (
        Path(__file__).resolve().parents[4]
        / "scripts"
        / "message-fixtures"
        / "fake-google-genai.py"
    )
    spec = importlib.util.spec_from_file_location("sideseat_fake_google_genai", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_developer_and_vertex_model_routes_are_supported() -> None:
    """The real clients use different prefixes around the same Gemini operations."""
    fake = _fake_google_genai()

    assert fake.is_supported_model_path(
        "/v1beta/models/gemini-2.5-flash:generateContent"
    )
    assert fake.is_supported_model_path(
        "/v1beta1/projects/test/locations/us-central1/"
        "publishers/google/models/gemini-2.5-flash:streamGenerateContent"
    )
    assert not fake.is_supported_model_path(
        "/v1beta1/projects/test/locations/us-central1/endpoints/custom:predict"
    )
