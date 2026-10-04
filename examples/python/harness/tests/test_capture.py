from harness.capture import anonymise


def test_the_cli_attachment_directory_is_pinned_at_its_length() -> None:
    raw = (
        b"[Image: source: /private/tmp/claude-504/-private-var-T-suite/"
        b"a7ad154d-40ab-4873-8951-05668b1b8aa8/images/1.jpg]"
    )

    pinned = anonymise(raw)

    assert len(pinned) == len(raw)
    assert b"/00000000-0000-0000-0000-000000000000/images/1.jpg" in pinned
    assert anonymise(pinned) == pinned


def test_other_uuids_are_left_alone() -> None:
    raw = b'{"session.id": "a7ad154d-40ab-4873-8951-05668b1b8aa8"}'

    assert anonymise(raw) == raw


def test_a_subagent_keeps_one_pinned_id_across_payloads() -> None:
    agents: dict[bytes, bytes] = {}
    first = anonymise(b"agentId: a2e3a1f7805cb8cf3 (use SendMessage)", agents)
    second = anonymise(b'a message from \\"a2e3a1f7805cb8cf3\\"', agents)

    assert first == b"agentId: a0000000000000001 (use SendMessage)"
    assert b"a0000000000000001" in second


def test_a_subagent_duration_is_zeroed_at_its_length() -> None:
    assert (
        anonymise(b"tool_uses: 2\\nduration_ms: 73</usage>")
        == b"tool_uses: 2\\nduration_ms: 00</usage>"
    )
