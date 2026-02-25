"""CLI argument parsing for OpenAI samples."""

import argparse
import sys
from pathlib import Path

from dotenv import load_dotenv

# Load .env from misc/ directory (4 levels up from this file)
_misc_dir = Path(__file__).parents[4]
load_dotenv(_misc_dir / ".env", override=True)

from openai_sample.config import DEFAULT_MODEL, MODEL_ALIASES, SAMPLES
from openai_sample.runner import run_all_samples, run_sample


def print_available_options():
    """Print available samples and model aliases."""
    print("Available Samples:")
    print("-" * 50)
    for name in SAMPLES:
        print(f"  {name}")
    print()

    print("Model Aliases:")
    print("-" * 50)
    for alias, (provider, model_id) in MODEL_ALIASES.items():
        print(f"  {alias:20} -> {provider}/{model_id}")
    print()
    print(f"Default: {DEFAULT_MODEL}")


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="openai-provider",
        description="Run OpenAI API samples with SideSeat instrumentation",
    )

    parser.add_argument(
        "sample",
        nargs="?",
        choices=list(SAMPLES.keys()) + ["all"],
        help="Sample to run (or 'all' to run all samples)",
    )

    parser.add_argument(
        "--model",
        default=DEFAULT_MODEL,
        help=f"Model alias or full model ID (default: {DEFAULT_MODEL})",
    )

    parser.add_argument(
        "--sideseat",
        action="store_true",
        help="Use SideSeat SDK with OTLP export and file exporter",
    )

    parser.add_argument(
        "--list",
        action="store_true",
        help="List available samples and model aliases",
    )

    return parser


def main():
    parser = create_parser()
    args = parser.parse_args()

    if args.list:
        print_available_options()
        return

    if not args.sample:
        parser.print_help()
        sys.exit(1)

    if args.sample == "all":
        run_all_samples(args)
    else:
        run_sample(args.sample, args)
