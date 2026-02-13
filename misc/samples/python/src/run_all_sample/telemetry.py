"""Run all telemetry samples in sequence.

This module executes the default sample (tool_use) from each framework
to verify telemetry integration is working correctly.
"""

import sys
import traceback

from adk_sample.cli import main as adk_main
from autogen_sample.cli import main as autogen_main
from crewai_sample.cli import main as crewai_main
from langgraph_sample.cli import main as langgraph_main
from openai_agents_sample.cli import main as openai_agents_main

# All frameworks use cli.main as their entry point
from strands_sample.cli import main as strands_main

FRAMEWORKS = [
    ("Strands", strands_main),
    ("Google ADK", adk_main),
    ("LangGraph", langgraph_main),
    ("CrewAI", crewai_main),
    ("AutoGen", autogen_main),
    ("OpenAI Agents", openai_agents_main),
]


def main():
    """Run all framework samples and report results."""
    print("=" * 60)
    print("Running all telemetry samples")
    print("=" * 60)

    results = []

    # Save original argv and replace with minimal args to run tool_use sample
    original_argv = sys.argv
    sys.argv = ["telemetry-all", "tool_use"]

    try:
        for name, framework_main in FRAMEWORKS:
            print()
            print("-" * 60)
            print(f"Running: {name}")
            print("-" * 60)
            try:
                framework_main()
                results.append((name, True, None))
                print(f"[OK] {name} completed successfully")
            except SystemExit:
                # CLI may call sys.exit(0) on success
                results.append((name, True, None))
                print(f"[OK] {name} completed successfully")
            except Exception as e:
                results.append((name, False, str(e)))
                print(f"[FAILED] {name}: {e}")
                traceback.print_exc()
    finally:
        sys.argv = original_argv

    print()
    print("=" * 60)
    print("Summary")
    print("=" * 60)

    passed = sum(1 for _, success, _ in results if success)
    failed = len(results) - passed

    for name, success, error in results:
        status = "OK" if success else f"FAILED: {error}"
        print(f"  {name}: {status}")

    print()
    print(f"Passed: {passed}/{len(results)}, Failed: {failed}/{len(results)}")

    if failed > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
