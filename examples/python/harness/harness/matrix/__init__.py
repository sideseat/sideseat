"""The version matrix: a suite's scenarios replayed against pinned historical releases of its framework.

A suite's ``versions.toml`` names the releases (and the opt-in telemetry profiles) its framework has
emitted distinct telemetry under. Each variant gets its own uv environment, replays the suite's committed
cassettes offline, and is recorded under ``server/tests/fixtures/messages/<producer>/<mode>@<variant>/``,
so the golden runner checks every historical format next to the current one. ``--census`` runs the probe
scenario on *every* release of the support window and records which telemetry shape each one emits, so
a release with no variant of its own is still shown to emit a shape some committed fixture holds.
"""
