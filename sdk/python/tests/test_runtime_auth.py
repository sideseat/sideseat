"""The runtime channel's credential plumbing.

Separate from `test_runtime_client.py`, which skips entirely without the
`websockets` extra: these tests only construct the client and read the headers
it would send, and that plumbing is exactly what broke - so it must be covered
on a plain install too.
"""

from __future__ import annotations


def test_auth_headers_carry_the_configured_key() -> None:
    """The API key reaches the upgrade request, or an auth-enabled server 401s forever.

    The runtime channel registers and invokes agents, so the server requires a
    credential for it whenever auth is enabled. The key was configured on
    `SideSeat` and dropped when the runtime client was built, so
    `SideSeat(api_key=...).register(...).connect()` reconnect-looped against a
    server that was working correctly - the least debuggable kind of failure,
    since nothing was wrong at either end but the plumbing.
    """
    from sideseat.runtime.client import RuntimeClient

    with_key = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default", api_key="sk_test")
    assert with_key._auth_headers() == {"Authorization": "Bearer sk_test"}

    # No key configured is the `--no-auth` case: send nothing rather than an
    # empty credential.
    without_key = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default")
    assert without_key._auth_headers() == {}


def test_the_facade_passes_its_api_key_to_the_runtime_client() -> None:
    """`SideSeat(api_key=...)` must reach the runtime client.

    This is where the key was being lost.
    """
    import sideseat

    client = sideseat.SideSeat(
        api_key="sk_facade",
        endpoint="http://127.0.0.1:1",
        project_id="default",
        # Nothing is exported or instrumented here; only the plumbing is read.
        disabled=True,
        auto_instrument=False,
    )
    assert client.runtime._auth_headers() == {"Authorization": "Bearer sk_facade"}
