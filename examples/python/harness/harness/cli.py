"""``sample``: the command every Python suite runs.

::

    uv run --locked --directory examples/python/strands sample tool_use
    uv run --locked --directory examples/python/strands sample tool_use --sideseat --model haiku
    uv run --locked --directory examples/python/strands sample --list

The suite is the project in the current directory. Its ``pyproject.toml`` declares::

    [tool.sideseat-example]
    producer = "strands"            # fixture producer name
    integrations = ["strands"]      # what ``--sideseat`` passes to sideseat.init
    default-model = "sonnet"
    scenario-models = { server_tools = "fake-anthropic" }   # optional: a scenario only that model runs

and the directory holds ``native.py`` (``configure(native)``), ``models.py`` (``build(model)``), a
``logs.py`` where the suite declares the ``logs`` mode, and
``scenarios/<name>.py`` modules each defining ``run(run)``.
"""

from __future__ import annotations

import argparse
import asyncio
import importlib
import inspect
import sys
import time
import tomllib
import traceback
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

from harness import models
from harness.catalog import CATALOG
from harness.env import load_env
from harness.run import Run
from harness.telemetry import LogsTelemetry, NativeTelemetry, SdkTelemetry, Telemetry


@dataclass
class Suite:
    root: Path
    producer: str
    integrations: list[str]
    default_model: str
    #: Scenarios the suite's models cannot run, each with the model that can.
    scenario_models: dict[str, str]
    service_name: str
    #: The telemetry modes this suite has a program for. `native` and `sdk` are every suite's; a suite whose
    #: producer reports through its own log records declares `logs` and writes a `logs.py` beside `native.py`.
    modes: tuple[str, ...]

    @classmethod
    def load(cls, root: Path) -> Suite:
        config = tomllib.loads((root / "pyproject.toml").read_text())
        table = config.get("tool", {}).get("sideseat-example")
        if table is None:
            raise SystemExit(
                f"{root} has no [tool.sideseat-example] table; run `sample` from a suite"
            )
        return cls(
            root=root,
            producer=table["producer"],
            integrations=list(table.get("integrations", [table["producer"]])),
            default_model=table.get("default-model", models.DEFAULT),
            scenario_models=dict(table.get("scenario-models") or {}),
            service_name=table.get("service-name", table["producer"]),
            modes=tuple(table.get("modes", ("native", "sdk"))),
        )

    def scenarios(self) -> list[str]:
        found = sorted(
            p.stem
            for p in (self.root / "scenarios").glob("*.py")
            if p.stem != "__init__"
        )
        unknown = [name for name in found if name not in CATALOG]
        if unknown:
            raise SystemExit(f"scenarios outside the catalog in {self.root}: {unknown}")
        return [name for name in CATALOG if name in found]

    def module(self, name: str) -> ModuleType:
        if str(self.root) not in sys.path:
            sys.path.insert(0, str(self.root))
        return importlib.import_module(name)


def main(argv: list[str] | None = None) -> None:
    load_env()
    suite = Suite.load(Path.cwd())
    parser = argparse.ArgumentParser(
        prog="sample", description=f"Run {suite.producer} scenarios."
    )
    parser.add_argument("scenario", nargs="*", help="scenario names, or 'all'")
    parser.add_argument(
        "--sideseat", action="store_true", help="configure telemetry with SideSeat"
    )
    parser.add_argument(
        "--logs",
        action="store_true",
        help="configure the producer's logging channel alone (suites that declare the `logs` mode)",
    )
    parser.add_argument(
        "--model",
        help="model alias (see --list); a scenario the manifest pins in `scenario-models` keeps its own",
    )
    parser.add_argument("--list", action="store_true", help="list scenarios and models")
    args = parser.parse_args(argv)

    available = suite.scenarios()
    if args.list or not args.scenario:
        print("Scenarios:")
        for name in available:
            print(f"  {name:18} {CATALOG[name].summary}")
        print("\nModels:")
        for alias, model in models.MODELS.items():
            marker = " (default)" if alias == suite.default_model else ""
            print(f"  {alias:18} {model.surface}: {model.id}{marker}")
        return

    selected = available if args.scenario == ["all"] else args.scenario
    missing = [name for name in selected if name not in available]
    if missing:
        raise SystemExit(
            f"{suite.producer} has no scenario {missing}; available: {available}"
        )

    def model_of(scenario: str) -> models.Model:
        alias = models.scenario_model(suite.scenario_models, scenario, args.model)
        return models.resolve(alias or suite.default_model)

    for name in selected:
        model_of(name)  # an unknown alias stops the run before any telemetry starts
    telemetry: Telemetry
    if args.sideseat and args.logs:
        raise SystemExit("--sideseat and --logs are two modes; choose one")
    if args.sideseat:
        telemetry = SdkTelemetry(suite.integrations)
    elif args.logs:
        if "logs" not in suite.modes:
            raise SystemExit(
                f"{suite.producer} declares no `logs` mode; add it to `modes` in "
                "[tool.sideseat-example] and write a logs.py beside native.py"
            )
        logs = LogsTelemetry(suite.service_name)
        suite.module("logs").configure(logs)
        telemetry = logs
    else:
        native = NativeTelemetry(suite.service_name)
        suite.module("native").configure(native)
        telemetry = native
    build = suite.module("models").build

    failures: list[str] = []
    try:
        for name in selected:
            model = model_of(name)
            print(
                f"\n=== {suite.producer} / {name} ({telemetry.mode}, {model.alias}) ==="
            )
            started = time.monotonic()
            run = Run(suite.producer, name, model, telemetry, build)
            try:
                _invoke(suite.module(f"scenarios.{name}").run, run)
            except Exception as error:  # run every selected scenario, then fail
                traceback.print_exc()
                failures.append(f"{name}: {type(error).__name__}: {error}")
                continue
            print(f"--- {name} finished in {time.monotonic() - started:.1f}s")
    finally:
        telemetry.shutdown()
    if failures:
        raise SystemExit("\n".join(failures))


def _invoke(entry: Any, run: Run) -> None:
    if inspect.iscoroutinefunction(entry):
        asyncio.run(entry(run))
    else:
        entry(run)
