"""``python -m harness <command>``: harness commands that belong to no suite.

::

    uv run --locked --directory examples/python/harness python -m harness truth strands/tool_use
    uv run --locked --directory examples/python/harness python -m harness truth --all
    uv run --locked --directory examples/python/harness python -m harness truth --all --check
"""

from __future__ import annotations

import sys


def main(argv: list[str] | None = None) -> None:
    args = sys.argv[1:] if argv is None else argv
    if not args or args[0] != "truth":
        raise SystemExit(
            "usage: python -m harness truth [<producer>/<scenario> ...] [--all] [--check]"
        )
    from harness.truth.cli import main as truth

    truth(args[1:])


if __name__ == "__main__":
    main()
