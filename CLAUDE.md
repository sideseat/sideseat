@AGENTS.md

## Hard requirement: the fastest possible developer loop

The developer loop is a product requirement, not a convenience. Every change must keep it short.

- `make quick` is the inner loop. It must finish in under 60 seconds on a warm cache. It runs formatting,
  lint, and unit tests for the areas changed relative to `main`, and nothing else.
- While iterating, run only the narrowest command for the code you touched: one crate
  (`cargo nextest run -p <crate>`), one package (`uv run --locked pytest <path>`, `npm test -- <file>`),
  one golden (`cargo nextest run -p sideseat-server message_goldens -E 'test(<suite>)'`). Never start a
  workspace-wide build or test run to check a local change.
- Tests are deterministic and offline by default: replay captured OTLP fixtures and use the fake model
  servers. Live model calls happen only in `make capture`, never in `make quick`, `make test`, or CI.
- A new test, gate, hook, or dependency that makes `make quick` slower than the budget belongs in
  `make test` or an opt-in target instead. Measure it before adding it.
- Git hooks stay cheap: pre-commit runs formatting and the secret scan only; heavier checks belong to
  pre-push and CI.
- If the loop has become slow, fixing that takes priority over the feature you are working on.
