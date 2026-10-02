"""Tests for AWS provider instrumentation (Bedrock)."""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import MagicMock

import pytest
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import StatusCode

from sideseat.integrations._bedrock.runtime import _detect_model_family, patch_bedrock_client


@pytest.fixture
def tracer_setup() -> tuple[TracerProvider, InMemorySpanExporter]:
    """Create a TracerProvider with in-memory exporter for assertions."""
    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    return provider, exporter


# ---------------------------------------------------------------------------
# AWSInstrumentor
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Model family detection
# ---------------------------------------------------------------------------


class TestModelFamilyDetection:
    def test_claude_models(self) -> None:
        assert _detect_model_family("anthropic.claude-3-5-sonnet-20241022-v2:0") == "claude"
        assert _detect_model_family("us.anthropic.claude-3-7-sonnet-20250219-v1:0") == "claude"
        assert _detect_model_family("anthropic.claude-v2") == "claude"

    def test_nova_models(self) -> None:
        assert _detect_model_family("us.amazon.nova-2-lite-v1:0") == "nova"
        assert _detect_model_family("amazon.nova-pro-v1:0") == "nova"
        assert _detect_model_family("amazon.nova-lite-v1:0") == "nova"

    def test_non_claude(self) -> None:
        assert _detect_model_family("amazon.titan-text-express-v1") is None
        assert _detect_model_family("meta.llama3-70b-instruct-v1:0") is None
        assert _detect_model_family("mistral.mistral-large-2407-v1:0") is None

    def test_unknown(self) -> None:
        assert _detect_model_family("some-custom-model") is None


# ---------------------------------------------------------------------------
# Converse sync
# ---------------------------------------------------------------------------


class TestConverse:
    def test_basic_converse(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Converse produces span with correct attributes and events."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "Hello!"}],
                }
            },
            "usage": {"inputTokens": 10, "outputTokens": 5},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        result = client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        assert result is response
        spans = exporter.get_finished_spans()
        assert len(spans) == 1

        span = spans[0]
        assert span.name == "chat anthropic.claude-3-5-sonnet-20241022-v2:0"
        attrs = dict(span.attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        assert attrs["gen_ai.operation.name"] == "chat"
        assert attrs["gen_ai.request.model"] == "anthropic.claude-3-5-sonnet-20241022-v2:0"
        assert attrs["gen_ai.usage.input_tokens"] == 10
        assert attrs["gen_ai.usage.output_tokens"] == 5
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        events = span.events
        event_names = [e.name for e in events]
        assert "gen_ai.client.inference.operation.details" in event_names
        assert "gen_ai.choice" in event_names

        details_event = next(
            e for e in events if e.name == "gen_ai.client.inference.operation.details"
        )
        input_msgs = json.loads(details_event.attributes["gen_ai.input.messages"])
        assert len(input_msgs) == 1
        assert input_msgs[0]["role"] == "user"

        output_msgs = json.loads(details_event.attributes["gen_ai.output.messages"])
        assert len(output_msgs) == 1
        assert output_msgs[0]["role"] == "assistant"

    def test_converse_reports_the_cache_counters_aws_actually_sends(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Converse's TokenUsage names them cacheReadInputTokens / cacheWriteInputTokens.

        The `...TokenCount` spellings read here previously exist nowhere in the API, so a cached
        Converse call reported no cache usage and was priced as a full fresh prompt - silently, and
        in the expensive direction.
        """
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "Hi"}]}},
            "usage": {
                "inputTokens": 10,
                "outputTokens": 5,
                "cacheReadInputTokens": 1200,
                "cacheWriteInputTokens": 3400,
            },
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        attrs = dict(exporter.get_finished_spans()[0].attributes or {})
        assert attrs["gen_ai.usage.cache_read_input_tokens"] == 1200
        assert attrs["gen_ai.usage.cache_write_input_tokens"] == 3400

    def test_converse_zero_cache_tokens_are_reported_not_dropped(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """A reported 0 is a fact about the call, distinct from the field being absent."""
        provider, exporter = tracer_setup
        client = MagicMock()

        client.converse = MagicMock(
            return_value={
                "output": {"message": {"role": "assistant", "content": [{"text": "Hi"}]}},
                "usage": {
                    "inputTokens": 10,
                    "outputTokens": 5,
                    "cacheWriteInputTokens": 0,
                },
            }
        )
        patch_bedrock_client(client, provider)
        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        attrs = dict(exporter.get_finished_spans()[0].attributes or {})
        assert attrs["gen_ai.usage.cache_write_input_tokens"] == 0
        assert "gen_ai.usage.cache_read_input_tokens" not in attrs

    def test_converse_with_system_prompt(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """System prompt is included in input messages."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "OK"}]}},
            "usage": {"inputTokens": 20, "outputTokens": 3},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            system=[{"text": "You are helpful."}],
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        spans = exporter.get_finished_spans()
        details = next(
            e for e in spans[0].events if e.name == "gen_ai.client.inference.operation.details"
        )
        input_msgs = json.loads(details.attributes["gen_ai.input.messages"])
        assert input_msgs[0]["role"] == "system"
        assert input_msgs[1]["role"] == "user"

    def test_converse_with_tools(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Tool config is captured as gen_ai.tool.definitions."""
        provider, exporter = tracer_setup
        client = MagicMock()

        tool_spec = {
            "toolSpec": {
                "name": "get_weather",
                "description": "Get weather",
                "inputSchema": {"json": {"type": "object", "properties": {}}},
            }
        }
        response = {
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "toolUse": {
                                "toolUseId": "tu_123",
                                "name": "get_weather",
                                "input": {"city": "NYC"},
                            }
                        }
                    ],
                }
            },
            "usage": {"inputTokens": 15, "outputTokens": 20},
            "stopReason": "tool_use",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Weather?"}]}],
            toolConfig={"tools": [tool_spec]},
        )

        spans = exporter.get_finished_spans()
        attrs = dict(spans[0].attributes or {})
        tool_defs = json.loads(attrs["gen_ai.tool.definitions"])
        assert len(tool_defs) == 1
        assert tool_defs[0]["toolSpec"]["name"] == "get_weather"

    def test_converse_with_tool_results(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Tool results from input messages are bundled in choice event."""
        provider, exporter = tracer_setup
        client = MagicMock()

        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "NYC is sunny."}]}},
            "usage": {"inputTokens": 30, "outputTokens": 10},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[
                {"role": "user", "content": [{"text": "Weather?"}]},
                {
                    "role": "user",
                    "content": [
                        {
                            "toolResult": {
                                "toolUseId": "tu_123",
                                "content": [{"text": "Sunny, 72F"}],
                            }
                        }
                    ],
                },
            ],
        )

        spans = exporter.get_finished_spans()
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        assert "tool.result" in choice.attributes
        tool_results = json.loads(choice.attributes["tool.result"])
        assert len(tool_results) == 1
        assert tool_results[0]["toolResult"]["toolUseId"] == "tu_123"

    def test_converse_error(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """API errors produce ERROR span and re-raise."""
        provider, exporter = tracer_setup
        client = MagicMock()
        client.converse = MagicMock(side_effect=RuntimeError("throttled"))
        patch_bedrock_client(client, provider)

        with pytest.raises(RuntimeError, match="throttled"):
            client.converse(modelId="anthropic.claude-3-5-sonnet-20241022-v2:0")

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        assert spans[0].status.status_code == StatusCode.ERROR

    def test_converse_inference_config(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Inference config params are captured."""
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
            inferenceConfig={
                "temperature": 0.7,
                "topP": 0.9,
                "maxTokens": 1024,
            },
        )

        spans = exporter.get_finished_spans()
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.request.temperature"] == 0.7
        assert attrs["gen_ai.request.top_p"] == 0.9
        assert attrs["gen_ai.request.max_tokens"] == 1024

    def test_converse_missing_usage(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Missing usage is handled gracefully."""
        provider, exporter = tracer_setup
        client = MagicMock()
        response = {
            "output": {"message": {"role": "assistant", "content": [{"text": "OK"}]}},
            "stopReason": "end_turn",
        }
        client.converse = MagicMock(return_value=response)
        patch_bedrock_client(client, provider)

        client.converse(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert "gen_ai.usage.input_tokens" not in attrs


# ---------------------------------------------------------------------------
# ConverseStream
# ---------------------------------------------------------------------------


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


class TestConverseStream:
    def test_basic_stream(self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]) -> None:
        """Stream wrapper accumulates text and emits events on exhaustion."""
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

        collected = list(response["stream"])
        assert len(collected) == 5

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        span = spans[0]
        attrs = dict(span.attributes or {})
        assert attrs["gen_ai.system"] == "aws_bedrock"
        assert attrs["gen_ai.usage.input_tokens"] == 10
        assert attrs["gen_ai.usage.output_tokens"] == 5
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        choice = next(e for e in span.events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content[0]["text"] == "Hello!"

    def test_stream_error(self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]) -> None:
        """Stream error produces ERROR span."""
        provider, exporter = tracer_setup
        client = MagicMock()

        def error_stream() -> Any:
            yield {
                "contentBlockStart": {
                    "contentBlockIndex": 0,
                    "start": {},
                }
            }
            raise ConnectionError("stream broken")

        stream_response = {"stream": error_stream()}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )

        with pytest.raises(ConnectionError):
            list(response["stream"])

        spans = exporter.get_finished_spans()
        assert spans[0].status.status_code == StatusCode.ERROR

    def test_stream_tool_use(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Stream wrapper accumulates tool use blocks with JSON parsing."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "contentBlockStart": {
                    "contentBlockIndex": 0,
                    "start": {"toolUse": {"toolUseId": "tu_1", "name": "calc"}},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"toolUse": {"input": '{"x":'}},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"toolUse": {"input": "42}"}},
                }
            },
            {"contentBlockStop": {"contentBlockIndex": 0}},
            {"messageStop": {"stopReason": "tool_use"}},
            {"metadata": {"usage": {"inputTokens": 10, "outputTokens": 15}}},
        ]

        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "calc 42"}]}],
        )
        list(response["stream"])

        spans = exporter.get_finished_spans()
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert content[0]["toolUse"]["name"] == "calc"
        assert content[0]["toolUse"]["input"] == {"x": 42}

    def test_stream_reasoning_with_signature(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Stream wrapper captures reasoning text and signature."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "contentBlockStart": {
                    "contentBlockIndex": 0,
                    "start": {"reasoningContent": {}},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"reasoningContent": {"text": "Let me think..."}},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"reasoningContent": {"signature": "sig_abc123"}},
                }
            },
            {"contentBlockStop": {"contentBlockIndex": 0}},
            {
                "contentBlockStart": {
                    "contentBlockIndex": 1,
                    "start": {},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 1,
                    "delta": {"text": "The answer is 42."},
                }
            },
            {"contentBlockStop": {"contentBlockIndex": 1}},
            {"messageStop": {"stopReason": "end_turn"}},
            {"metadata": {"usage": {"inputTokens": 20, "outputTokens": 30}}},
        ]

        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "Think"}]}],
        )
        list(response["stream"])

        spans = exporter.get_finished_spans()
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert len(content) == 2

        # Reasoning block with signature
        reasoning = content[0]["reasoningContent"]["reasoningText"]
        assert reasoning["text"] == "Let me think..."
        assert reasoning["signature"] == "sig_abc123"

        # Text block
        assert content[1]["text"] == "The answer is 42."

    def test_stream_unknown_block_type_preserved(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Unknown block types (guard, citations) pass through verbatim."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {
                "contentBlockStart": {
                    "contentBlockIndex": 0,
                    "start": {
                        "guardContent": {
                            "type": "BLOCKED",
                            "text": "Content filtered",
                        }
                    },
                }
            },
            {"contentBlockStop": {"contentBlockIndex": 0}},
            {"messageStop": {"stopReason": "guardrail_intervened"}},
            {"metadata": {"usage": {"inputTokens": 5, "outputTokens": 1}}},
        ]

        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="anthropic.claude-3-5-sonnet-20241022-v2:0",
            messages=[{"role": "user", "content": [{"text": "test"}]}],
        )
        list(response["stream"])

        spans = exporter.get_finished_spans()
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert len(content) == 1
        # Block preserved verbatim — no spurious "text" key added
        assert "guardContent" in content[0]
        assert "text" not in content[0]
        assert content[0]["guardContent"]["type"] == "BLOCKED"

    def test_stream_without_content_block_start(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Stream that skips contentBlockStart still captures text.

        Some Bedrock endpoints (global inference profiles) send contentBlockDelta
        directly after messageStart without a contentBlockStart event.
        """
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {"messageStart": {"role": "assistant"}},
            {"contentBlockDelta": {"contentBlockIndex": 0, "delta": {"text": "Hello "}}},
            {"contentBlockDelta": {"contentBlockIndex": 0, "delta": {"text": "world!"}}},
            {"contentBlockStop": {"contentBlockIndex": 0}},
            {"messageStop": {"stopReason": "end_turn"}},
            {"metadata": {"usage": {"inputTokens": 10, "outputTokens": 5}}},
        ]

        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="global.anthropic.claude-haiku-4-5-20251001-v1:0",
            messages=[{"role": "user", "content": [{"text": "Hi"}]}],
        )
        collected = list(response["stream"])
        assert len(collected) == 6

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
        attrs = dict(spans[0].attributes or {})
        assert attrs["gen_ai.usage.input_tokens"] == 10
        assert attrs["gen_ai.usage.output_tokens"] == 5
        assert attrs["gen_ai.response.finish_reasons"] == ("end_turn",)

        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert len(content) == 1
        assert content[0]["text"] == "Hello world!"

    def test_stream_without_content_block_start_reasoning(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Reasoning stream without contentBlockStart correctly types as reasoning block."""
        provider, exporter = tracer_setup
        client = MagicMock()

        chunks = [
            {"messageStart": {"role": "assistant"}},
            # Reasoning block — no contentBlockStart
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"reasoningContent": {"text": "Let me think..."}},
                }
            },
            {
                "contentBlockDelta": {
                    "contentBlockIndex": 0,
                    "delta": {"reasoningContent": {"signature": "sig_abc"}},
                }
            },
            {"contentBlockStop": {"contentBlockIndex": 0}},
            # Text block — no contentBlockStart
            {"contentBlockDelta": {"contentBlockIndex": 1, "delta": {"text": "The answer is 42."}}},
            {"contentBlockStop": {"contentBlockIndex": 1}},
            {"messageStop": {"stopReason": "end_turn"}},
            {"metadata": {"usage": {"inputTokens": 20, "outputTokens": 30}}},
        ]

        stream_response = {"stream": iter(chunks)}
        client.converse_stream = MagicMock(return_value=stream_response)
        patch_bedrock_client(client, provider)

        response = client.converse_stream(
            modelId="global.anthropic.claude-haiku-4-5-20251001-v1:0",
            messages=[{"role": "user", "content": [{"text": "Think"}]}],
        )
        list(response["stream"])

        spans = exporter.get_finished_spans()
        choice = next(e for e in spans[0].events if e.name == "gen_ai.choice")
        content = json.loads(choice.attributes["message"])
        assert len(content) == 2

        # Reasoning block with signature
        reasoning = content[0]["reasoningContent"]["reasoningText"]
        assert reasoning["text"] == "Let me think..."
        assert reasoning["signature"] == "sig_abc"

        # Text block
        assert content[1]["text"] == "The answer is 42."

    def test_stream_close_mid_stream(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Closing stream mid-iteration finalizes span."""
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

        stream = response["stream"]
        next(stream)
        stream.close()

        spans = exporter.get_finished_spans()
        assert len(spans) == 1
