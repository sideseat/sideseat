"""``python -m harness truth``: write ``server/tests/fixtures/truth/<producer>/<scenario>.json``.

Offline and deterministic: it reads committed cassettes, the fake models' script and the
conformance program, and never calls a model. ``--check`` writes nothing and fails when a committed
truth differs from what its sources now produce, or a derivable truth is missing.

A truth is one per producer and scenario: the native and SDK fixtures of a scenario replay one
cassette, so they describe one conversation. It lives beside the fixtures rather than beside the
cassette because the Rust goldens are its only reader, the fake-model and conformance scenarios have
no cassette to sit beside, and it is named by the fixture producer (``llama-index``), not the suite
directory (``llamaindex``).
"""

from __future__ import annotations

import argparse
import sys

from harness.truth import sources
from harness.truth.sources import Underivable

#: Fixtures no truth describes on purpose: hand-written shapes, and captures of APIs no catalog
#: scenario exercises.


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        prog="python -m harness truth", description=__doc__.split("\n\n")[0]
    )
    parser.add_argument(
        "target",
        nargs="*",
        help="<producer>/<scenario>, or <producer> for all its scenarios",
    )
    parser.add_argument(
        "--all", action="store_true", help="every producer and scenario"
    )
    parser.add_argument(
        "--check", action="store_true", help="fail if a committed truth is stale"
    )
    args = parser.parse_args(argv)

    known = sources.targets()
    if args.all:
        selected = known
    elif args.target:
        selected = []
        for name in args.target:
            producer, _, scenario = name.partition("/")
            matched = [
                t
                for t in known
                if t.producer == producer and scenario in ("", t.scenario)
            ]
            if not matched:
                raise SystemExit(
                    f"no scenario {name!r}; producers: {sorted({t.producer for t in known})}"
                )
            selected += matched
    else:
        parser.error("name a target or pass --all")

    written, stale, underivable = 0, [], []
    for target in selected:
        path = sources.path_of(target)
        try:
            rendered = sources.render(sources.build(target))
        except Underivable as reason:
            underivable.append((target, str(reason)))
            continue
        current = path.read_text() if path.exists() else None
        if current == rendered:
            continue
        if args.check:
            stale.append(f"{target.producer}/{target.scenario}")
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(rendered)
        written += 1

    covered = {(t.producer, t.scenario) for t in selected} - {
        (t.producer, t.scenario) for t, _ in underivable
    }
    print(f"[truth] {len(covered)} scenario(s) derived, {written} file(s) written")
    if underivable:
        print(f"[truth] {len(underivable)} scenario(s) without truth:")
        for target, reason in underivable:
            print(f"  {target.producer}/{target.scenario}: {reason}")
    if args.all:
        orphans = _fixtures_without_truth(covered)
        if orphans:
            print(f"[truth] {len(orphans)} fixture(s) no truth describes:")
            for label, reason in orphans:
                print(f"  {label}: {reason}")
    if stale:
        print("[truth] stale or missing: " + ", ".join(stale), file=sys.stderr)
        raise SystemExit(1)


def _fixtures_without_truth(covered: set[tuple[str, str]]) -> list[tuple[str, str]]:
    found = []
    for producer in sorted(p for p in sources.FIXTURES.iterdir() if p.is_dir()):
        if producer.name == "_synthetic":
            count = sum(1 for sample in producer.iterdir() if sample.is_dir())
            found.append(
                (f"_synthetic/* ({count})", "hand-written shapes, no producer")
            )
            continue
        for mode in sorted(m for m in producer.iterdir() if m.is_dir()):
            for scenario in sorted(s for s in mode.iterdir() if s.is_dir()):
                if (producer.name, scenario.name) in covered:
                    continue
                if (
                    declared := sources.uncovered(producer.name, mode.name)
                ) is not None:
                    reason = declared
                elif scenario.name == "canonical":
                    reason = "a pre-catalog capture whose emitting program is not in the repository"
                else:
                    reason = "no truth derived"
                found.append((f"{producer.name}/{mode.name}/{scenario.name}", reason))
    return found
