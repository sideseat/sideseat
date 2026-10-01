"""A real one-step BrowserUse run covering model, agent, tool, and session spans."""

from typing import Any


async def run(
    model: Any,
    trace_attrs: dict[str, str],
    client: Any,
) -> None:
    """Run one deterministic headless browser step inside a named session."""
    from browser_use import Agent, BrowserProfile

    profile = BrowserProfile(
        headless=True,
        enable_default_extensions=False,
        highlight_elements=False,
        cross_origin_iframes=False,
        minimum_wait_page_load_time=0.0,
        wait_for_network_idle_page_load_time=0.0,
        wait_between_actions=0.0,
    )
    agent = Agent(
        task="Finish immediately with the text: deterministic local fixture",
        llm=model,
        browser_profile=profile,
        use_vision=False,
        use_judge=False,
        use_thinking=False,
        enable_planning=False,
        directly_open_url=False,
        enable_signal_handler=False,
        message_compaction=False,
        max_actions_per_step=1,
    )

    with client.trace(
        "browser-use-canonical",
        session_id=trace_attrs["session.id"],
        user_id=trace_attrs["user.id"],
    ):
        history = await agent.run(max_steps=1)

    result = history.final_result()
    if not history.is_done() or result != "deterministic local fixture":
        raise RuntimeError(
            f"BrowserUse did not complete the deterministic fixture: {result!r}"
        )
    print(f"Assistant: {result}")
