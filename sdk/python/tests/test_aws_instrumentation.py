"""Tests for AWS provider instrumentation (Bedrock)."""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import MagicMock, patch

import pytest
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import StatusCode

from sideseat.instrumentation import _instrumented, _lock
from sideseat.instrumentors.aws import AWSInstrumentor
from sideseat.instrumentors.aws.bedrock import _detect_model_family, patch_bedrock_client
from sideseat.instrumentors.aws.bedrock_agent import patch_bedrock_agent_client


@pytest.fixture(autouse=True)
def reset_aws_state():
    """Reset AWSInstrumentor singleton and instrumentation state."""
    AWSInstrumentor._instance = None
    with _lock:
        _instrumented.discard("aws")
    yield
    AWSInstrumentor._instance = None
    with _lock:
        _instrumented.discard("aws")


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


class TestAWSInstrumentor:
    def test_singleton(self) -> None:
        """Second instrument() call is a no-op."""
        mock_wrapt = MagicMock()
        with patch.dict("sys.modules", {"wrapt": mock_wrapt}):
            inst = AWSInstrumentor(tracer_provider=None)
            inst.instrument()
            assert AWSInstrumentor._instance is inst
            assert mock_wrapt.wrap_function_wrapper.call_count == 1

            inst2 = AWSInstrumentor(tracer_provider=None)
            inst2.instrument()
            assert AWSInstrumentor._instance is inst
            assert mock_wrapt.wrap_function_wrapper.call_count == 1

    def test_no_wrapt_graceful(self) -> None:
        """Missing wrapt logs debug and returns without error."""
        with patch("importlib.util.find_spec", return_value=None):
            inst = AWSInstrumentor(tracer_provider=None)
            inst.instrument()
            assert AWSInstrumentor._instance is None

    def test_on_create_client_bedrock_runtime(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """bedrock-runtime client gets patched."""
        provider, _ = tracer_setup
        inst = AWSInstrumentor(tracer_provider=provider)

        client = MagicMock()
        service = MagicMock()
        service.service_name = "bedrock-runtime"
        client._service_model = service
        client.converse = MagicMock()
        client.converse_stream = MagicMock()
        client.invoke_model = MagicMock()
        client.invoke_model_with_response_stream = MagicMock()

        wrapped = MagicMock(return_value=client)
        result = inst._on_create_client(wrapped, None, (), {})
        assert result is client

    def test_on_create_client_other_service(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """Non-Bedrock services are returned unmodified."""
        provider, _ = tracer_setup
        inst = AWSInstrumentor(tracer_provider=provider)

        client = MagicMock()
        service = MagicMock()
        service.service_name = "s3"
        client._service_model = service
        original_put = client.put_object

        wrapped = MagicMock(return_value=client)
        result = inst._on_create_client(wrapped, None, (), {})
        assert result is client
        assert client.put_object is original_put

    def test_on_create_client_agent_runtime(
        self, tracer_setup: tuple[TracerProvider, InMemorySpanExporter]
    ) -> None:
        """bedrock-agent-runtime client gets patched."""
        provider, _ = tracer_setup
        inst = AWSInstrumentor(tracer_provider=provider)

        client = MagicMock()
        service = MagicMock()
        service.service_name = "bedrock-agent-runtime"
        client._service_model = service

        wrapped = MagicMock(return_value=client)
        result = inst._on_create_client(wrapped, None, (), {})
        assert result is client


# ---------------------------------------------------------------------------
# Model family detection
# ---------------------------------------------------------------------------


class TestModelFamilyDetection:
    def test_claude_models(self) -> None:
        assert _detect_model_family("anthropic.claude-3-5-sonnet-20241022-v2:0") == "claude"
        assert _detect_model_family("us.anthropic.claude-3-7-sonnet-20250219-v1:0") == "claude"
        assert _detect_model_family("anthropic.claude-v2") == "claude"

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


class TestInstrumentProviders:
    def test_idempotent(self) -> None:
        """instrument_providers is idempotent via _instrumented set."""
        from sideseat.instrumentation import instrument_providers

        with patch("sideseat.instrumentation._try_instrument_aws") as mock_try:
            instrument_providers(None, ("bedrock",))
            instrument_providers(None, ("bedrock",))
            assert mock_try.call_count == 2

    def test_no_providers_skips_aws(self) -> None:
        """Empty providers list skips AWS instrumentation."""
        from sideseat.instrumentation import instrument_providers

        with patch("sideseat.instrumentation._try_instrument_aws") as mock_try:
            instrument_providers(None)
            instrument_providers(None, ())
            assert mock_try.call_count == 0

    def test_no_botocore(self) -> None:
        """No botocore installed -> silent return."""
        from sideseat.instrumentation import _try_instrument_aws

        with patch.dict("sys.modules", {"botocore": None}):
            _try_instrument_aws(None)
            assert "aws" not in _instrumented

    def test_with_botocore(self) -> None:
        """With botocore -> AWSInstrumentor.instrument() called."""
        from sideseat.instrumentation import _try_instrument_aws

        mock_botocore = MagicMock()
        with (
            patch.dict("sys.modules", {"botocore": mock_botocore}),
            patch("sideseat.instrumentors.aws.AWSInstrumentor") as mock_cls,
        ):
            _try_instrument_aws(None)
            mock_cls.assert_called_once_with(tracer_provider=None)
            mock_cls.return_value.instrument.assert_called_once()
            assert "aws" in _instrumented

    def test_instrument_failure_cleans_up(self) -> None:
        """Failed instrumentation removes 'aws' from _instrumented."""
        from sideseat.instrumentation import _try_instrument_aws

        mock_botocore = MagicMock()
        with (
            patch.dict("sys.modules", {"botocore": mock_botocore}),
            patch(
                "sideseat.instrumentors.aws.AWSInstrumentor",
                side_effect=RuntimeError("boom"),
            ),
        ):
            _try_instrument_aws(None)
            assert "aws" not in _instrumented


# ---------------------------------------------------------------------------
# _try_instrument_provider (OpenAI, Anthropic, VertexAI)
# ---------------------------------------------------------------------------


class TestTryInstrumentProvider:
    @pytest.fixture(autouse=True)
    def reset_provider_state(self) -> None:
        with _lock:
            _instrumented.discard("openai")
            _instrumented.discard("anthropic")
            _instrumented.discard("vertex_ai")
        yield
        with _lock:
            _instrumented.discard("openai")
            _instrumented.discard("anthropic")
            _instrumented.discard("vertex_ai")

    def test_openai_provider_instruments(self) -> None:
        """OpenAI provider calls OpenInference instrumentor when SDK is available."""
        from sideseat.instrumentation import _try_instrument_provider

        mock_openai = MagicMock()
        mock_instrumentor = MagicMock()
        mock_mod = MagicMock()
        mock_mod.OpenAIInstrumentor.return_value = mock_instrumentor

        with (
            patch.dict("sys.modules", {"openai": mock_openai}),
            patch("sideseat.instrumentation._instrument_openinference") as mock_oi,
        ):
            _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", None)
            mock_oi.assert_called_once_with("openai", "OpenAIInstrumentor", None)
            assert "openai" in _instrumented

    def test_provider_skips_without_sdk(self) -> None:
        """No SDK installed -> no-op."""
        from sideseat.instrumentation import _try_instrument_provider

        with patch("sideseat._utils._module_available", return_value=False):
            _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", None)
            assert "openai" not in _instrumented

    def test_provider_idempotent(self) -> None:
        """Second call for same provider is a no-op."""
        from sideseat.instrumentation import _try_instrument_provider

        mock_openai = MagicMock()
        with (
            patch.dict("sys.modules", {"openai": mock_openai}),
            patch("sideseat.instrumentation._instrument_openinference") as mock_oi,
        ):
            _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", None)
            _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", None)
            assert mock_oi.call_count == 1

    def test_unknown_provider_ignored(self) -> None:
        """Unknown provider string in providers tuple is harmless."""
        from sideseat.instrumentation import instrument_providers

        instrument_providers(None, ("unknown_provider",))
        assert "unknown_provider" not in _instrumented

    def test_provider_failure_cleans_up(self) -> None:
        """Failed OpenInference instrumentation removes name from _instrumented."""
        from sideseat.instrumentation import _try_instrument_provider

        mock_openai = MagicMock()
        with (
            patch.dict("sys.modules", {"openai": mock_openai}),
            patch(
                "sideseat.instrumentation._instrument_openinference",
                side_effect=RuntimeError("missing dep"),
            ),
        ):
            _try_instrument_provider("openai", "openai", "openai", "OpenAIInstrumentor", None)
            assert "openai" not in _instrumented
