"""Tests for AWS Bedrock InvokeModel instrumentation and resilience."""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import MagicMock, patch

import pytest
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import StatusCode

from sideseat.integrations._bedrock.agent_runtime import patch_bedrock_agent_client
from sideseat.integrations._bedrock.runtime import patch_bedrock_client


@pytest.fixture
def tracer_setup() -> tuple[TracerProvider, InMemorySpanExporter]:
    """Create a TracerProvider with in-memory exporter for assertions."""
    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    return provider, exporter


def _make_stream_chunks(
    text: str = "Hello!",
    stop_reason: str = "end_turn",
    input_tokens: int = 10,
    output_tokens: int = 5,
) -> list[dict[str, Any]]:
    """Build a typical Converse stream chunk sequence."""
    return [
        {"contentBlockStart": {"contentBlockIndex": 0, "start": {}}},
        {
            "contentBlockDelta": {
                "contentBlockIndex": 0,
                "delta": {"text": text},
            }
        },
        {"contentBlockStop": {"contentBlockIndex": 0}},
        {"messageStop": {"stopReason": stop_reason}},
        {
            "metadata": {
                "usage": {
                    "inputTokens": input_tokens,
                    "outputTokens": output_tokens,
                }
            }
        },
    ]


# ---------------------------------------------------------------------------
# InvokeModel
# ---------------------------------------------------------------------------


def _mock_botocore_modules() -> dict[str, Any]:
    """Set up mock botocore modules for sys.modules patching."""
    mock_response = MagicMock()
    mock_response.StreamingBody = MagicMock()
    return {
        "botocore": MagicMock(),
        "botocore.response": mock_response,
    }


class TestInvokeModel:
    def test_claude_invoke_model(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """InvokeModel with Claude extracts messages."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response_body = json.dumps(
            {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hi there!"}],
                "model": "claude-3-5-sonnet-20241022",
                "usage": {"input_tokens": 8, "output_tokens": 4},
                "stop_reason": "end_turn",
            }
        ).encode()

        body_mock = MagicMock()
        body_mock.read.return_value = response_body
        client.invoke_model = MagicMock(return_value={"body": body_mock})

        with patch.dict("sys.modules", _mock_botocore_modules()):
            patch_bedrock_client(client, provider)
            client.invoke_model(
                modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
                body=json.dumps(
                    {
                        "messages": [{"role": "user", "content": "Hello"}],
                        "max_tokens": 100,
                    }
                ),
            )

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        assert attrs["gen_ai.response.model"] == "claude-3-5-sonnet-20241022"
        assert attrs["gen_ai.usage.input_tokens"] == 8
        assert attrs["gen_ai.usage.output_tokens"] == 4

        events = spans[0].events
        event_names = [e.name for e in events]
        assert "gen_ai.client.inference.operation.details" in event_names
        assert "gen_ai.choice" in event_names

    def test_non_claude_invoke_model(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Non-Claude InvokeModel produces span with model+tokens only."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response_body = json.dumps(
            {
                "results": [{"outputText": "Hello"}],
            }
        ).encode()

        body_mock = MagicMock()
        body_mock.read.return_value = response_body
        client.invoke_model = MagicMock(return_value={"body": body_mock})

        with patch.dict("sys.modules", _mock_botocore_modules()):
            patch_bedrock_client(client, provider)
            client.invoke_model(modelId="amazon.titan-text-express-v1", body="{}")

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        event_names = [e.name for e in spans[0].events]
        assert "gen_ai.choice" not in event_names

    def test_nova_invoke_model(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Nova InvokeModel extracts content from output.message with camelCase tokens."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response_body = json.dumps(
            {
                "output": {
                    "message": {
                        "role": "assistant",
                        "content": [{"text": "Hello from Nova!"}],
                    }
                },
                "stopReason": "end_turn",
                "usage": {"inputTokens": 12, "outputTokens": 6},
            }
        ).encode()

        body_mock = MagicMock()
        body_mock.read.return_value = response_body
        client.invoke_model = MagicMock(return_value={"body": body_mock})

        with patch.dict("sys.modules", _mock_botocore_modules()):
            patch_bedrock_client(client, provider)
            client.invoke_model(
                modelId="us.amazon.nova-2-lite-v1:0",
                body=json.dumps(
                    {
                        "system": [{"text": "Be concise."}],
                        "messages": [{"role": "user", "content": [{"text": "Hello"}]}],
                        "inferenceConfig": {"maxTokens": 128},
                    }
                ),
            )

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        assert attrs["gen_ai.response.model"] == "us.amazon.nova-2-lite-v1:0"
        assert attrs["gen_ai.usage.input_tokens"] == 12
        assert attrs["gen_ai.usage.output_tokens"] == 6
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        events = spans[0].events
        event_names = [e.name for e in events]
        assert "gen_ai.client.inference.operation.details" in event_names
        assert "gen_ai.choice" in event_names

        choice = next(e for e in events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content == [{"text": "Hello from Nova!"}]
        assert choice.attributes["finish_reason"] == "end_turn"


# ---------------------------------------------------------------------------
# InvokeModel Streaming
# ---------------------------------------------------------------------------


class TestInvokeModelStream:
    def test_claude_stream_with_invocation_metrics(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """InvokeModel streaming captures amazon-bedrock-invocationMetrics from message_stop."""
        provider, exporter = tracer_setup
        client = MagicMock()

        # Simulate Claude streaming chunks
        chunks = [
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "message_start",
                            "message": {
                                "model": "claude-3-5-sonnet-20241022",
                                "usage": {"input_tokens": 10},
                            },
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "text", "text": ""},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"text": "Hello!"},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_stop",
                            "index": 0,
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "message_delta",
                            "delta": {"stop_reason": "end_turn"},
                            "usage": {"output_tokens": 5},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "message_stop",
                            "amazon-bedrock-invocationMetrics": {
                                "inputTokenCount": 12,
                                "outputTokenCount": 7,
                                "invocationLatency": 500,
                                "firstByteLatency": 100,
                            },
                        }
                    ).encode()
                }
            },
        ]

        client.invoke_model_with_response_stream = MagicMock(return_value={"body": iter(chunks)})
        patch_bedrock_client(client, provider)

        response = client.invoke_model_with_response_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            body=json.dumps({"messages": [{"role": "user", "content": "Hi"}]}),
        )
        list(response["body"])

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})

        # Bedrock invocation metrics override Claude-level usage
        assert attrs["gen_ai.usage.input_tokens"] == 12
        assert attrs["gen_ai.usage.output_tokens"] == 7
        assert attrs["gen_ai.response.model"] == "claude-3-5-sonnet-20241022"
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        # Verify content was captured
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content[0]["text"] == "Hello!"

    def test_nova_stream_converse_format(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Nova streaming with Converse-style events extracts content and usage."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "chunk": {
                    "bytes": json.dumps(
                        {"contentBlockStart": {"start": {}, "contentBlockIndex": 0}}
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "contentBlockDelta": {
                                "delta": {"text": "Hello from Nova!"},
                                "contentBlockIndex": 0,
                            }
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps({"contentBlockStop": {"contentBlockIndex": 0}}).encode()
                }
            },
            {"chunk": {"bytes": json.dumps({"messageStop": {"stopReason": "end_turn"}}).encode()}},
            {
                "chunk": {
                    "bytes": json.dumps(
                        {"metadata": {"usage": {"inputTokens": 10, "outputTokens": 5}}}
                    ).encode()
                }
            },
        ]

        client.invoke_model_with_response_stream = MagicMock(return_value={"body": iter(chunks)})

        with patch.dict("sys.modules", _mock_botocore_modules()):
            patch_bedrock_client(client, provider)
            response = client.invoke_model_with_response_stream(
                modelId="us.amazon.nova-2-lite-v1:0",
                body=json.dumps(
                    {
                        "system": [{"text": "Be concise."}],
                        "messages": [{"role": "user", "content": [{"text": "Hello"}]}],
                    }
                ),
            )
            list(response["body"])

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.usage.input_tokens"] == 10
        assert attrs["gen_ai.usage.output_tokens"] == 5
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content == [{"text": "Hello from Nova!"}]

    def test_nova_stream_full_response_format(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Nova streaming with single full-response chunk extracts content."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "output": {
                                "message": {
                                    "role": "assistant",
                                    "content": [{"text": "Complete response"}],
                                }
                            },
                            "stopReason": "end_turn",
                            "usage": {"inputTokens": 8, "outputTokens": 3},
                        }
                    ).encode()
                }
            },
        ]

        client.invoke_model_with_response_stream = MagicMock(return_value={"body": iter(chunks)})

        with patch.dict("sys.modules", _mock_botocore_modules()):
            patch_bedrock_client(client, provider)
            response = client.invoke_model_with_response_stream(
                modelId="amazon.nova-pro-v1:0",
                body=json.dumps(
                    {
                        "messages": [{"role": "user", "content": [{"text": "Hello"}]}],
                    }
                ),
            )
            list(response["body"])

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.usage.input_tokens"] == 8
        assert attrs["gen_ai.usage.output_tokens"] == 3

        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content == [{"text": "Complete response"}]

    def test_claude_stream_thinking_with_signature(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Claude streaming captures thinking text and signature from separate delta events."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "message_start",
                            "message": {
                                "model": "claude-3-7-sonnet",
                                "usage": {"input_tokens": 20},
                            },
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "thinking", "thinking": ""},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "thinking_delta", "thinking": "Let me reason..."},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "signature_delta", "signature": "sig_test123"},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_stop",
                            "index": 0,
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_start",
                            "index": 1,
                            "content_block": {"type": "text", "text": ""},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_delta",
                            "index": 1,
                            "delta": {"text": "The answer is 42."},
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "content_block_stop",
                            "index": 1,
                        }
                    ).encode()
                }
            },
            {
                "chunk": {
                    "bytes": json.dumps(
                        {
                            "type": "message_delta",
                            "delta": {"stop_reason": "end_turn"},
                            "usage": {"output_tokens": 30},
                        }
                    ).encode()
                }
            },
            {"chunk": {"bytes": json.dumps({"type": "message_stop"}).encode()}},
        ]

        client.invoke_model_with_response_stream = MagicMock(return_value={"body": iter(chunks)})
        patch_bedrock_client(client, provider)

        response = client.invoke_model_with_response_stream(
            modelId="anthropic.claude-3-7-sonnet-20250219-v1:0",
            body=json.dumps({"messages": [{"role": "user", "content": "Think hard about 42"}]}),
        )
        list(response["body"])

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert len(content) == 2

        # Thinking block with signature from delta events
        reasoning = content[0]["reasoningContent"]["reasoningText"]
        assert reasoning["text"] == "Let me reason..."
        assert reasoning["signature"] == "sig_test123"

        # Text block
        assert content[1]["text"] == "The answer is 42."


# ---------------------------------------------------------------------------
# Resilience — instrumentation must never crash user code
# ---------------------------------------------------------------------------


class TestResilience:
    def test_converse_event_emission_failure_does_not_crash(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Event emission failure is swallowed — user's API call still returns."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "OK"}]}},
            "usage": {"inputTokens": 5, "outputTokens": 2},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        # Monkey-patch _emit_converse_events to throw
        import sideseat.integrations._bedrock.runtime as bedrock_mod

        original_emit = bedrock_mod._emit_converse_events
        bedrock_mod._emit_converse_events = MagicMock(side_effect=RuntimeError("encode boom"))
        try:
            result = client.converse(
                modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
                messages=[{"role": "user", "content": [{"text": "Hi"}]}],
            )
            # User's response is returned despite event emission failure
            assert result is response
        finally:
            bedrock_mod._emit_converse_events = original_emit

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        assert spans[0].status.status_code == StatusCode.OK

    def test_stream_finalize_failure_does_not_crash(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Stream finalize failure is swallowed — iteration ends cleanly."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = _make_stream_chunks()
        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        # Monkey-patch _emit_span_events to throw during finalize
        import sideseat.integrations._bedrock.runtime as bedrock_mod

        original_emit = bedrock_mod._emit_span_events
        bedrock_mod._emit_span_events = MagicMock(side_effect=RuntimeError("serialize boom"))
        try:
            # Iteration should complete without raising
            collected = list(response["stream"])
            assert len(collected) == 5
        finally:
            bedrock_mod._emit_span_events = original_emit

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        assert spans[0].status.status_code == StatusCode.OK

    def test_converse_sync_sets_ok_status(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Sync converse sets OK status on success."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "OK"}]}},
            "usage": {"inputTokens": 5, "outputTokens": 2},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        spans = exporter.get_finished_spans()
        assert spans[0].status.status_code == StatusCode.OK


# ---------------------------------------------------------------------------
# InvokeAgent
# ---------------------------------------------------------------------------


class TestInvokeAgent:
    def test_basic_invoke_agent(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """InvokeAgent produces span with agent attributes."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {"chunk": {"bytes": b"Hello from agent!"}},
        ]
        client.invoke_agent = MagicMock(return_value={"completion": iter(chunks)})
        patch_bedrock_agent_client(client, provider)

        response = client.invoke_agent(
            agentId="AGENT123",
            agentAliasId="ALIAS1",
            sessionId="sess-1",
            inputText="What's the weather?",
        )

        list(response["completion"])

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        span = spans[0]
        assert span.name == "invoke_agent AGENT123"
        attrs = dict(span.attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        assert attrs["gen_ai.operation.name"] == "invoke_agent"
        assert attrs["gen_ai.agent.id"] == "AGENT123"

        user_event = next(e for e in span.events if e.name == "gen_ai.user.message")
        assert user_event.attributes["content"] == "What's the weather?"

        choice = next(e for e in span.events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content[0]["text"] == "Hello from agent!"

    def test_invoke_agent_accumulates_tokens(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Agent tokens are accumulated across multiple model invocations."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            # First model invocation trace (e.g., pre-processing)
            {
                "trace": {
                    "trace": {
                        "preProcessingTrace": {
                            "modelInvocationOutput": {
                                "metadata": {
                                    "usage": {
                                        "inputTokens": 50,
                                        "outputTokens": 20,
                                    },
                                    "foundationModel": "anthropic.claude-3-5-sonnet",
                                }
                            }
                        }
                    }
                }
            },
            # Second model invocation trace (orchestration)
            {
                "trace": {
                    "trace": {
                        "orchestrationTrace": {
                            "modelInvocationOutput": {
                                "metadata": {
                                    "usage": {
                                        "inputTokens": 100,
                                        "outputTokens": 40,
                                    }
                                }
                            }
                        }
                    }
                }
            },
            {"chunk": {"bytes": b"Final answer"}},
        ]
        client.invoke_agent = MagicMock(return_value={"completion": iter(chunks)})
        patch_bedrock_agent_client(client, provider)

        response = client.invoke_agent(agentId="AGENT123", inputText="Hello")
        list(response["completion"])

        spans = exporter.get_finished_spans()
        attrs = dict(spans[0].attributes or {})
        # Tokens should be accumulated: 50+100=150 input, 20+40=60 output
        assert attrs["gen_ai.usage.input_tokens"] == 150
        assert attrs["gen_ai.usage.output_tokens"] == 60
        assert attrs["gen_ai.request.model"] == "anthropic.claude-3-5-sonnet"

    def test_invoke_agent_error(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """InvokeAgent API error produces ERROR span."""
        provider, exporter = tracer_setup
        client = MagicMock()
        client.invoke_agent = MagicMock(side_effect=RuntimeError("agent error"))
        patch_bedrock_agent_client(client, provider)

        with pytest.raises(RuntimeError, match="agent error"):
            client.invoke_agent(agentId="AGENT123", inputText="Hello")

        spans = exporter.get_finished_spans()
        assert spans[0].status.status_code == StatusCode.ERROR


# ---------------------------------------------------------------------------
# instrument_providers integration
# ---------------------------------------------------------------------------
