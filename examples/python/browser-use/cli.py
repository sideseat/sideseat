"""CLI for BrowserUse native and SideSeat telemetry captures."""

import argparse
import sys
from pathlib import Path

from dotenv import load_dotenv

load_dotenv(Path(__file__).parents[2] / ".env", override=True)

from runner import run_sample

from config import DEFAULT_MODEL, SAMPLES


def create_parser() -> argparse.ArgumentParser:
    """Build the sample command-line parser."""
    parser = argparse.ArgumentParser(
        prog="telemetry-browser-use",
        description="Run BrowserUse samples with native or SideSeat telemetry",
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
        help=f"OpenAI-compatible model ID (default: {DEFAULT_MODEL})",
    )
    parser.add_argument(
        "--sideseat",
        action="store_true",
        help="Use SideSeat SDK instead of native Laminar/OpenTelemetry",
    )
    parser.add_argument("--list", action="store_true", help="List available samples")
    return parser


def main() -> None:
    """Run the selected sample."""
    parser = create_parser()
    args = parser.parse_args()
    if args.list:
        print("Available Samples:")
        for name in SAMPLES:
            print(f"  {name}")
        return
    if not args.sample:
        parser.print_help()
        sys.exit(1)
    run_sample(args.sample, args)
