"""CLI argument parsing for Pydantic AI samples."""

import argparse
import sys
from pathlib import Path

from dotenv import load_dotenv

# Load .env from examples/ (three levels up: <suite>/ -> python/ -> examples/).
_examples_dir = Path(__file__).parents[2]
load_dotenv(_examples_dir / ".env", override=True)

from runner import run_sample

from config import SAMPLES


def print_available_options() -> None:
    """Print available deterministic samples."""
    print("Available Samples:")
    print("-" * 50)
    for name in SAMPLES:
        print(f"  {name}")
    print()
    print("Model Aliases:")
    print("-" * 50)
    print("  test                 -> deterministic FunctionModel")


def create_parser() -> argparse.ArgumentParser:
    """Build the command-line parser used by the fixture harness."""
    parser = argparse.ArgumentParser(
        prog="telemetry-pydantic-ai",
        description="Run Pydantic AI samples with native or SideSeat telemetry",
    )
    parser.add_argument(
        "sample",
        nargs="?",
        choices=list(SAMPLES),
        help="Sample to run",
    )
    parser.add_argument(
        "--model",
        default="test",
        choices=["test"],
        help="Deterministic model alias (default: test)",
    )
    parser.add_argument(
        "--sideseat",
        action="store_true",
        help="Use SideSeat SDK instead of native Logfire/OpenTelemetry",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List available samples",
    )
    return parser


def main() -> None:
    """Run the selected sample."""
    args = create_parser().parse_args()
    if args.list:
        print_available_options()
        return
    if not args.sample:
        create_parser().print_help()
        sys.exit(1)
    run_sample(args.sample, args)
