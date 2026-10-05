"""The shared example tools as Browser Use custom actions."""

from collections.abc import Callable
from typing import Any

from browser_use import Tools


def tools(*functions: Callable[..., Any]) -> Tools:
    """Browser Use's built-in browser actions plus ``functions`` as custom actions.

    The built-in web search is left out: it opens a public search engine, which the browser profile
    does not allow. Browser Use describes an action by a summary and the function's signature; the
    shared tools used here return text or raise, which is what an action may do.
    """
    registry: Tools = Tools(exclude_actions=["search"])
    for function in functions:
        summary = (function.__doc__ or function.__name__).split("\n\n")[0].strip()
        registry.action(summary)(function)
    return registry
