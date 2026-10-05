"""What a scenario receives: the model, the telemetry mode, and correlation identifiers."""

from __future__ import annotations

import os
from collections.abc import Callable
from contextlib import AbstractContextManager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from harness.catalog import USER_ID, session_id
from harness.models import Model
from harness.telemetry import Telemetry

EXAMPLES = Path(__file__).resolve().parents[3]
ASSETS = EXAMPLES / "assets"


@dataclass
class Run:
    producer: str
    scenario: str
    model: Model
    telemetry: Telemetry
    build_model: Callable[[Model], Any]
    _model_object: Any = field(default=None, init=False, repr=False)

    @property
    def mode(self) -> str:
        return self.telemetry.mode

    @property
    def session_id(self) -> str:
        return session_id(self.producer, self.scenario)

    @property
    def user_id(self) -> str:
        return USER_ID

    @property
    def llm(self) -> Any:
        """The framework's model object for ``--model``, built once by the suite's ``models.py``."""
        if self._model_object is None:
            self._model_object = self.build_model(self.model)
        return self._model_object

    def trace(
        self, name: str | None = None, *, session: str | None = None
    ) -> AbstractContextManager[Any]:
        """A root span for one conversation, attributed to this scenario's session and user."""
        return self.telemetry.trace(
            name or self.scenario.replace("_", "-"),
            session_id=session or self.session_id,
            user_id=self.user_id,
        )

    @staticmethod
    def asset(name: str) -> Path:
        """An input file from ``examples/assets``: ``img.jpg`` or ``task.pdf``."""
        return ASSETS / name


TOOLS = EXAMPLES.parent / "scripts" / "tools"


def mcp_calculator_command(*, relative: bool = False) -> list[str]:
    """The stdio command that starts the example MCP calculator server.

    ``relative=True`` names the server directory relative to the working directory, with forward
    slashes, for a framework that derives model-visible names from the command line: CrewAI names a
    stdio server's tools after ``command`` and ``args``, so an absolute path would put the capturing
    account's home directory into the conversation and make the recorded requests differ between
    machines. It is not the default because a framework may start the server in another directory
    (the Claude Agent SDK runs its CLI in a temporary workspace), where a relative path resolves
    elsewhere. Without a relative form (another Windows drive) the path stays absolute.
    """
    directory = TOOLS / "mcp-calculator"
    location = str(directory)
    if relative:
        try:
            location = Path(os.path.relpath(directory)).as_posix()
        except ValueError:
            location = directory.as_posix()
    return ["uv", "run", "--locked", "--directory", location, "mcp-calculator"]
