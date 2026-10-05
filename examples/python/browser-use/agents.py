"""One Browser Use agent setup for every scenario: a headless local browser that stays on the local site.

The SDK run of a scenario replays the native run's model answers, and both must record the same
conversation, so what varies between two runs and would reach a request is switched off:
screenshots (``use_vision``), and the judge and planning calls, which add model requests the
conversation does not need. Nothing may reach the public internet.
"""

import os
import sys
from pathlib import Path
from typing import Any

# Browser Use reports product telemetry, syncs to its cloud, and checks PyPI for a newer release unless
# told not to; none of that belongs in a capture.
for _key in (
    "ANONYMIZED_TELEMETRY",
    "BROWSER_USE_CLOUD_SYNC",
    "BROWSER_USE_VERSION_CHECK",
):
    os.environ.setdefault(_key, "false")

from browser_use import Agent, BrowserProfile, Tools  # noqa: E402

from harness import Run, content
from tools import tools as browser_tools
from travel_site import URL


#: Where ``uvx browser-use install`` (Playwright's ``install chromium``) puts its headless shell.
_PLAYWRIGHT_BROWSERS = {
    "darwin": ("~/Library/Caches/ms-playwright", "chrome-headless-shell"),
    "linux": ("~/.cache/ms-playwright", "chrome-headless-shell"),
    "win32": (
        os.path.join(os.getenv("LOCALAPPDATA") or "~", "ms-playwright"),
        "chrome-headless-shell.exe",
    ),
}


def browser_executable() -> str:
    """Playwright's Chromium headless shell, newest first.

    Browser Use 0.13 looks for Playwright's builds under their old names, misses the current ones, and
    launches the machine's own Chrome instead - with that profile's policies and startup pages, which
    differ from machine to machine. The headless shell is what Playwright itself runs headless.
    """
    if configured := os.getenv("BROWSER_USE_EXECUTABLE_PATH"):
        return configured
    root, executable = _PLAYWRIGHT_BROWSERS.get(
        sys.platform, _PLAYWRIGHT_BROWSERS["linux"]
    )
    base = Path(os.getenv("PLAYWRIGHT_BROWSERS_PATH") or root).expanduser()
    found = sorted(
        base.glob(f"chromium_headless_shell-*/chrome-headless-shell-*/{executable}"),
        key=lambda path: int(path.parts[-3].rpartition("-")[2] or 0),
        reverse=True,
    )
    if not found:
        raise SystemExit(
            f"no Playwright Chromium under {base}; run `uvx browser-use install`"
            " or set BROWSER_USE_EXECUTABLE_PATH"
        )
    return str(found[0])


def profile() -> BrowserProfile:
    return BrowserProfile(
        headless=True,
        executable_path=browser_executable(),
        user_data_dir=None,
        # Otherwise the viewport is the machine's screen, which the page state the model reads reflects.
        viewport={"width": 1280, "height": 800},
        window_size={"width": 1280, "height": 800},
        enable_default_extensions=False,
        allowed_domains=[URL],
        highlight_elements=False,
        captcha_solver=False,
    )


def agent(run: Run, task: str, *, tools: Tools | None = None, **options: Any) -> Agent:
    return Agent(
        task=task,
        llm=run.llm,
        tools=tools if tools is not None else browser_tools(),
        browser_profile=profile(),
        extend_system_message=content.SYSTEM,
        use_vision=False,
        use_judge=False,
        enable_planning=False,
        enable_signal_handler=False,
        **options,
    )


def answer(history: Any) -> str:
    """The agent's final answer, or an error naming why it has none."""
    result = history.final_result()
    if not history.is_done() or not result:
        raise RuntimeError(
            f"Browser Use finished without an answer: {history.errors()}"
        )
    return str(result)
