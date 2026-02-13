"""Common runner utilities shared across frameworks.

Provides base utilities for sample execution that can be extended
by framework-specific runners.
"""

import importlib
import traceback
import uuid
from typing import Any, Callable


def create_trace_attributes(framework: str, sample_name: str) -> dict:
    """Create standard trace attributes for a sample.

    Args:
        framework: Framework name (e.g., 'strands', 'autogen')
        sample_name: Sample name (e.g., 'tool_use')

    Returns:
        Dict with session.id and user.id attributes
    """
    return {
        "session.id": f"{framework}-{sample_name}-{uuid.uuid4().hex[:8]}",
        "user.id": "demo-user",
    }


def run_sample_module(
    module_path: str,
    model_or_client: Any,
    trace_attrs: dict,
    is_async: bool = False,
    extra_kwargs: dict | None = None,
):
    """Import and run a sample module.

    Args:
        module_path: Full module path (e.g., 'strands_sample.samples.tool_use')
        model_or_client: Model client or LLM instance to pass to sample
        trace_attrs: Trace attributes dict
        is_async: Whether the sample's run() function is async
        extra_kwargs: Additional kwargs to pass to run()
    """
    import asyncio

    module = importlib.import_module(module_path)
    kwargs = {"trace_attrs": trace_attrs, **(extra_kwargs or {})}

    if is_async:
        asyncio.run(module.run(model_or_client, **kwargs))
    else:
        module.run(model_or_client, **kwargs)


def run_all_samples_base(
    samples: dict[str, str],
    run_single: Callable[[str, Any], bool],
    args: Any,
) -> list[tuple[str, bool, str | None]]:
    """Run all samples and collect results.

    Args:
        samples: Dict of sample_name -> module_path
        run_single: Function that runs a single sample, returns success bool
        args: CLI args namespace to pass to run_single

    Returns:
        List of (name, success, error_message) tuples
    """
    results = []
    for name in samples:
        print(f"\n{'=' * 60}")
        print(f"Running: {name}")
        print(f"{'=' * 60}")
        try:
            success = run_single(name, args)
            results.append((name, success, None))
        except Exception as e:
            results.append((name, False, str(e)))
            print(f"FAILED: {e}")
            traceback.print_exc()

    # Summary
    print(f"\n{'=' * 60}")
    print("Summary")
    print(f"{'=' * 60}")
    passed = sum(1 for _, s, _ in results if s)
    for name, success, error in results:
        status = "OK" if success else f"FAILED: {error}"
        print(f"  {name}: {status}")
    print(f"\nPassed: {passed}/{len(results)}")

    return results
