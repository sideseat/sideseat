# Security Policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Report it privately through
[GitHub's private vulnerability reporting](https://github.com/sideseat/sideseat/security/advisories/new).
That opens a draft advisory only you and the maintainers can see.

Please include:

- what an attacker can do, and what they need to start (network reach, a valid API key, a project id);
- the smallest reproduction you have — a request, a config, an OTLP payload;
- the version (`sideseat --version`) and the backends in use (DuckDB/SQLite or ClickHouse/PostgreSQL).

You will get an acknowledgement within **3 working days** and an assessment within **10**. If the report is
accepted we will agree a disclosure date with you, and credit you in the advisory unless you ask otherwise.

## Supported versions

| Version | Supported |
| --- | --- |
| 1.x (latest release) | Yes |
| Anything older | No — upgrade first |

The project is pre-1.0 in practice: fixes land on `main` and in the next release rather than being backported.

## What is in scope

SideSeat ingests telemetry from AI applications and serves it back, so the interesting boundaries are:

- **Ingestion** (`/otel/{project_id}/v1/*`, HTTP and gRPC) — anything that lets one project write into another,
  or that gets a request acknowledged without the data being stored.
- **The query API and the UI** — anything that returns one organisation's data to another, or bypasses
  `require_auth` / `verify_project_access`.
- **Authentication** — API-key verification, the JWT signing path, the secrets backends.
- **The SDK runtime channel** (`/ws`, AG-UI invoke) — registering or invoking an agent you do not own.
- **The MCP surface** — the same, reached through a different transport.
- **Deserialisation of untrusted input** — OTLP payloads, JSON attributes, file references.

## What is not a vulnerability

- **Running with `--no-auth`.** It disables authentication by design, for local development. It is not the
  default.
- **Findings that require the operator's own credentials or filesystem access.** A local operator already has
  the data.
- **A configuration the server refuses at startup.** Several unsafe combinations — a shared database beside
  per-instance file storage, `async_insert` without `wait_for_async_insert`, a non-durable Redis — are rejected
  rather than warned about. A report that one of those *is not* refused is very much in scope.
- **Denial of service by volume alone** against an instance you control.

## Hardening this repository applies to itself

- `gitleaks` runs on staged changes and on the whole tree (`make secret-scan`, `make secret-scan-tree`), and in
  the pre-commit and pre-push hooks.
- `cargo deny` checks advisories, licences and bans (`make harden-supply`).
- Dependencies are updated through Dependabot.
- No secret belongs in the repository. `examples/.env` is deliberately untracked, and the fixture-capture
  script discards any payload that looks like a credential.
