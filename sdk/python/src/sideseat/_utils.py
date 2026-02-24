"""Shared internal utilities."""

import importlib.util
import sys


def _module_available(name: str) -> bool:
    """Check if a module is importable without importing it."""
    if name in sys.modules:
        return sys.modules[name] is not None
    return importlib.util.find_spec(name) is not None
