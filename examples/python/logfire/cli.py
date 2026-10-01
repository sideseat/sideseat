"""CLI for generic Logfire native and SideSeat telemetry captures."""

import argparse
import sys
from pathlib import Path

from dotenv import load_dotenv

load_dotenv(Path(__file__).parents[2] / ".env", override=True)

from runner import run_all_samples, run_sample

from config import DEFAULT_MODEL, SAMPLES


def create_parser() -> argparse.ArgumentParser:
    """Build the sample command-line parser."""
    parser = argparse.ArgumentParser(
        prog="telemetry-logfire",
        description="Run generic Logfire samples with native or SideSeat telemetry",
    )
    parser.add_argument(
        "sample",
        nargs="?",
        choices=[*SAMPLES, "all"],
        help="Sample to run",
    )
    parser.add_argument(
        "--model",
        default=DEFAULT_MODEL,
        help=f"Model ID recorded in deterministic spans (default: {DEFAULT_MODEL})",
    )
    parser.add_argument(
        "--sideseat",
        action="store_true",
        help="Use SideSeat SDK instead of native OpenTelemetry",
    )
    parser.add_argument("--list", action="store_true", help="List available samples")
    return parser


def main() -> None:
    """Run the selected sample."""
    args = create_parser().parse_args()
    if args.list:
        print("Available Samples:")
        for name in SAMPLES:
            print(f"  {name}")
        return
    if not args.sample:
        create_parser().print_help()
        sys.exit(1)
    if args.sample == "all":
        run_all_samples(args)
    else:
        run_sample(args.sample, args)
