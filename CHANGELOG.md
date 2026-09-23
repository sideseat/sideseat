# Changelog

**Release notes live on [GitHub Releases](https://github.com/sideseat/sideseat/releases), which is the
authoritative record.**

This file is a pointer rather than a copy, deliberately. `make publish-release` creates each release with
`gh release create --generate-notes`, so the notes are derived from the commits in the tag. A hand-maintained
duplicate here would be a second source of truth that drifts from the first, and the drift would be invisible
until someone trusted the wrong one.

## Versioning

The version lives in `server/Cargo.toml` and is synchronised across the server and the CLI by `make bump`
(`TYPE=patch|minor|major`). The SDKs are versioned independently, because they release on their own cadence.

Tags are `vMAJOR.MINOR.PATCH`. Until 1.0 is declared stable, treat a minor bump as potentially breaking: the
OTel GenAI semantic conventions this project reads are themselves in development, so the shape of what is
stored can change with them.

## What a release contains

- Prebuilt CLI binaries for five platforms, published to npm as `sideseat`.
- The Python package `sideseat` on PyPI, and `@sideseat/sdk` on npm.
- A Homebrew formula, generated from `packaging/homebrew/sideseat.rb.tmpl`.

## Finding what changed in a specific area

The commit history is the detailed record, and commit messages in this repository state the defect, the
measurement that established it, and the mutation that verified the fix. `git log --oneline -- server/crates/domain`
is usually more informative than a summarised changelog entry would be.
