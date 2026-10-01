"""OpenInference history, tools, retrieval, traces, and sessions."""

from typing import Any

from openinference.instrumentation import (
    Document,
    Message,
    OITracer,
    Tool,
    get_input_attributes,
    get_llm_attributes,
    get_output_attributes,
    get_reranker_attributes,
    get_retriever_attributes,
    get_tool_attributes,
    using_attributes,
)

SYSTEM_PROMPT = "Answer scientific questions in one sentence."
FIRST_QUESTION = "What is the speed of light?"
FIRST_ANSWER = "Light travels at 299,792,458 metres per second in vacuum."
SECOND_QUESTION = "What is the boiling point of water?"
SECOND_ANSWER = "Water boils at 100°C at standard atmospheric pressure."
TOOL_SYSTEM_PROMPT = "Use tools when they are available."
TOOL_QUESTION = "What is the weather in Paris?"
TOOL_RESULT = "Sunny, 22°C, light breeze in Paris."
TOOL_ANSWER = "The weather in Paris is sunny at 22°C with a light breeze."
TOOL_CALL_ID = "call-weather-1"
RETRIEVAL_QUERY = "Which city has the Eiffel Tower?"
RETRIEVAL_ANSWER = "The Eiffel Tower is in Paris."

TOOL_PARAMETERS = {
    "type": "object",
    "properties": {
        "location": {"type": "string"},
    },
    "required": ["location"],
}
TOOL_SCHEMA = {
    "type": "function",
    "function": {
        "name": "get_weather",
        "description": "Get deterministic weather for a city.",
        "parameters": TOOL_PARAMETERS,
    },
}


def _llm_attributes(
    model_id: str,
    input_messages: list[Message],
    output_messages: list[Message],
    *,
    tools: list[Tool] | None = None,
) -> dict[str, Any]:
    """Build one current OpenInference LLM attribute set."""
    return {
        **get_llm_attributes(
            provider="openai",
            system="openai",
            request_model_name=model_id,
            response_model_name=model_id,
            invocation_parameters={"temperature": 0},
            input_messages=input_messages,
            output_messages=output_messages,
            tools=tools,
        ),
        "openinference.sample": "canonical",
    }


def _history(
    model_id: str, owner: Any, tracer: OITracer, attrs: dict[str, str]
) -> None:
    """Emit two model calls whose second request repeats the first turn."""
    session_id = attrs["session.id"]
    user_id = attrs["user.id"]
    first_request: list[Message] = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": FIRST_QUESTION},
    ]
    first_response: list[Message] = [
        {"role": "assistant", "content": FIRST_ANSWER},
    ]
    second_request: list[Message] = [
        *first_request,
        *first_response,
        {"role": "user", "content": SECOND_QUESTION},
    ]
    second_response: list[Message] = [
        {"role": "assistant", "content": SECOND_ANSWER},
    ]

    with (
        using_attributes(session_id=session_id, user_id=user_id),
        owner.trace(
            "openinference-history",
            session_id=session_id,
            user_id=user_id,
        ),
    ):
        with tracer.start_as_current_span(
            "openinference first answer",
            openinference_span_kind="llm",
            attributes=_llm_attributes(
                model_id,
                first_request,
                first_response,
            ),
        ):
            pass
        with tracer.start_as_current_span(
            "openinference second answer",
            openinference_span_kind="llm",
            attributes=_llm_attributes(
                model_id,
                second_request,
                second_response,
            ),
        ):
            pass


def _tool_roundtrip(
    model_id: str,
    owner: Any,
    tracer: OITracer,
    attrs: dict[str, str],
) -> None:
    """Emit a model call, one tool execution, and the final model answer."""
    session_id = attrs["session.id"]
    user_id = attrs["user.id"]
    request: list[Message] = [
        {"role": "system", "content": TOOL_SYSTEM_PROMPT},
        {"role": "user", "content": TOOL_QUESTION},
    ]
    tool_call: Message = {
        "role": "assistant",
        "tool_calls": [
            {
                "id": TOOL_CALL_ID,
                "function": {
                    "name": "get_weather",
                    "arguments": {"location": "Paris"},
                },
            }
        ],
    }
    tool_result: Message = {
        "role": "tool",
        "tool_call_id": TOOL_CALL_ID,
        "content": TOOL_RESULT,
    }
    final_answer: list[Message] = [
        {"role": "assistant", "content": TOOL_ANSWER},
    ]
    tools: list[Tool] = [{"json_schema": TOOL_SCHEMA}]

    with (
        using_attributes(session_id=session_id, user_id=user_id),
        owner.trace(
            "openinference-tool-roundtrip",
            session_id=session_id,
            user_id=user_id,
        ),
    ):
        with tracer.start_as_current_span(
            "openinference tool request",
            openinference_span_kind="llm",
            attributes=_llm_attributes(
                model_id,
                request,
                [tool_call],
                tools=tools,
            ),
        ):
            pass

        with tracer.start_as_current_span(
            "get_weather",
            openinference_span_kind="tool",
            attributes={
                **get_tool_attributes(
                    name="get_weather",
                    description="Get deterministic weather for a city.",
                    parameters=TOOL_PARAMETERS,
                ),
                **get_input_attributes({"location": "Paris"}),
                **get_output_attributes(TOOL_RESULT),
                "tool.id": TOOL_CALL_ID,
            },
        ):
            pass

        with tracer.start_as_current_span(
            "openinference tool answer",
            openinference_span_kind="llm",
            attributes=_llm_attributes(
                model_id,
                [*request, tool_call, tool_result],
                final_answer,
                tools=tools,
            ),
        ):
            pass


def _retrieval(
    model_id: str,
    owner: Any,
    tracer: OITracer,
    attrs: dict[str, str],
) -> None:
    """Emit retrieval and reranking before a grounded model answer."""
    session_id = attrs["session.id"]
    user_id = attrs["user.id"]
    documents: list[Document] = [
        {
            "id": "doc-london",
            "content": "Big Ben is in London.",
            "score": 0.61,
        },
        {
            "id": "doc-paris",
            "content": "The Eiffel Tower is in Paris.",
            "score": 0.98,
        },
    ]
    reranked = [documents[1], documents[0]]

    with (
        using_attributes(session_id=session_id, user_id=user_id),
        owner.trace(
            "openinference-retrieval",
            session_id=session_id,
            user_id=user_id,
        ),
    ):
        with tracer.start_as_current_span(
            "openinference retrieve",
            openinference_span_kind="retriever",
            attributes={
                **get_input_attributes(RETRIEVAL_QUERY),
                **get_retriever_attributes(documents=documents),
                "openinference.sample": "canonical",
            },
        ):
            pass

        with tracer.start_as_current_span(
            "openinference rerank",
            openinference_span_kind="reranker",
            attributes={
                **get_reranker_attributes(
                    query=RETRIEVAL_QUERY,
                    model_name="deterministic-reranker",
                    input_documents=documents,
                    output_documents=reranked,
                    top_k=2,
                ),
                "openinference.sample": "canonical",
            },
        ):
            pass

        with tracer.start_as_current_span(
            "openinference grounded answer",
            openinference_span_kind="llm",
            attributes=_llm_attributes(
                model_id,
                [{"role": "user", "content": RETRIEVAL_QUERY}],
                [{"role": "assistant", "content": RETRIEVAL_ANSWER}],
            ),
        ):
            pass


def run(
    model_id: str,
    trace_attrs: dict[str, str],
    owner: Any,
    tracer: OITracer,
) -> None:
    """Emit three traces sharing one session and cover the production rubric."""
    _history(model_id, owner, tracer, trace_attrs)
    _tool_roundtrip(model_id, owner, tracer, trace_attrs)
    _retrieval(model_id, owner, tracer, trace_attrs)
