#!/usr/bin/env python3
"""Credential-free import and telemetry smoke test for one Python sample suite."""

from __future__ import annotations

import argparse
import importlib
import inspect
import os
import sys
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("project")
    parser.add_argument("mode", choices=("native", "sdk"))
    return parser.parse_args()


def shutdown_owner(value: Any) -> None:
    owner = value[0] if isinstance(value, tuple) else value
    if owner is None:
        return
    shutdown = getattr(owner, "shutdown", None)
    if shutdown is not None:
        shutdown()


def main() -> None:
    args = parse_args()
    project = Path(args.project).resolve()
    os.chdir(project)
    sys.path.insert(0, str(project))

    config = importlib.import_module("config")
    samples = config.SAMPLES
    for module_name in samples.values():
        importlib.import_module(module_name)

    cli = importlib.import_module("cli")
    parser = cli.create_parser()
    assert parser.format_help().strip()

    telemetry_setup = importlib.import_module("telemetry_setup")
    setup = telemetry_setup.setup_telemetry
    parameters = inspect.signature(setup).parameters
    kwargs: dict[str, Any] = {}
    if "use_sideseat" in parameters:
        kwargs["use_sideseat"] = args.mode == "sdk"

    result = setup(**kwargs)
    owner = result[0] if isinstance(result, tuple) else result
    if args.mode == "sdk":
        assert owner is not None, "SDK mode did not return a telemetry owner"
    elif owner is None:
        # Claude Agent SDK's native branch is configured entirely through the
        # subprocess environment returned as the second tuple item.
        assert isinstance(result, tuple) and isinstance(result[1], dict)
        assert result[1].get("OTEL_TRACES_EXPORTER") == "otlp"

    shutdown_owner(result)
    print(f"{project.name}: {args.mode}: {len(samples)} samples")


if __name__ == "__main__":
    main()
