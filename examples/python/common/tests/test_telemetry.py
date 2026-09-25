"""Regression tests for the shared sample telemetry setup."""

from typing import Any

import sideseat

from common.telemetry import setup_base_telemetry


def test_sideseat_mode_does_not_run_native_instrumentor(monkeypatch: Any) -> None:
    """SideSeat auto-instruments the framework, so the native callback must stay idle."""
    instrumentor_calls = 0

    class FakeTelemetry:
        def setup_console_exporter(self) -> "FakeTelemetry":
            return self

    class FakeSideSeat:
        def __init__(self, *, framework: str | list[str] | None) -> None:
            self.framework = framework
            self.telemetry = FakeTelemetry()

    def native_instrumentor(provider: Any = None) -> None:
        nonlocal instrumentor_calls
        instrumentor_calls += 1

    monkeypatch.setattr(sideseat, "SideSeat", FakeSideSeat)

    client = setup_base_telemetry(
        instrumentor=native_instrumentor,
        use_sideseat=True,
        framework="autogen",
    )

    assert isinstance(client, FakeSideSeat)
    assert client.framework == "autogen"
    assert instrumentor_calls == 0
