"""CLI argument parsing for Google GenAI samples."""

import argparse
import sys
from pathlib import Path

from dotenv import load_dotenv

_examples_dir = Path(__file__).parents[2]
load_dotenv(_examples_dir / ".env", override=True)

from runner import run_sample

from config import DEFAULT_MODEL, SAMPLES


def print_available_options() -> None:
    """Print available samples and the deterministic capture model."""
    print("Available Samples:")
    print("-" * 50)
    for name in SAMPLES:
        print(f"  {name}")
    print()
    print("Model Aliases:")
    print("-" * 50)
    print(f"  {DEFAULT_MODEL}")


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="google-genai-provider",
        description="Run Google GenAI samples with native or SideSeat telemetry",
    )
    parser.add_argument(
        "sample",
        nargs="?",
        choices=list(SAMPLES),
        help="Sample to run",
    )
    parser.add_argument(
        "--model",
        default=DEFAULT_MODEL,
        help=f"Model ID (default: {DEFAULT_MODEL})",
    )
    parser.add_argument(
        "--sideseat",
        action="store_true",
        help="Use SideSeat SDK instead of native Logfire/OpenTelemetry",
    )
    parser.add_argument("--list", action="store_true", help="List available samples")
    return parser


def main() -> None:
    parser = create_parser()
    args = parser.parse_args()
    if args.list:
        print_available_options()
        return
    if not args.sample:
        parser.print_help()
        sys.exit(1)
    run_sample(args.sample, args)
