"""Loads ``examples/.env`` for credentials and endpoints. The process environment wins."""

from __future__ import annotations

from pathlib import Path

from dotenv import load_dotenv

ENV_FILE = Path(__file__).resolve().parents[3] / ".env"


def load_env() -> None:
    # override=False: the capture tool and CI set endpoints in the environment, and a developer's
    # .env must not redirect a capture to their running server.
    load_dotenv(ENV_FILE, override=False)
