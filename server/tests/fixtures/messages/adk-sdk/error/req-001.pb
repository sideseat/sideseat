
í¶
«
"
telemetry.sdk.language
python
 
telemetry.sdk.name

sideseat
 
telemetry.sdk.version
1.0.8

service.name

google-adk

service.version
2.10.0
"
sideseat.framework

google-adkƒ§

gcp.vertex.agent2.10.0ÍC
uQ√ãÃO]≠ µpÿ„[HùÕoü"™„FÅ¶¬˛*2generate_content openai/nonexistent-model-id-1234509Xg∫ ‘ŸA∆Ê£ ‘ŸJ
gen_ai.system
openaiJ+
gen_ai.operation.name
generate_contentJ;
gen_ai.request.model#
!openai/nonexistent-model-id-12345J 
gen_ai.agent.name
	assistantJ.
gen_ai.conversation.id
adk-error-b0812599JC
gcp.vertex.agent.event_id&
$b20122bc-4101-4c77-a8d8-278dceab2c81JJ
gcp.vertex.agent.invocation_id(
&e-468f705d-ace0-47d3-b045-0f6e0ff79475Z∫?	0EÊ£ ‘Ÿ	exception6
exception.type$
"litellm.exceptions.BadRequestErrorQ
exception.message<
:litellm.BadRequestError: OpenAIException - model not found˙=
exception.stacktrace·=
ﬁ=Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 948, in acompletion
    headers, response = await self.make_openai_chat_completion_request(
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/logging_utils.py", line 340, in async_wrapper
    result: Final = await func(*args, **kwargs)
                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 505, in make_openai_chat_completion_request
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 481, in make_openai_chat_completion_request
    raw_response = await openai_aclient.chat.completions.with_raw_response.create(**data, timeout=timeout)
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_legacy_response.py", line 386, in wrapped
    return cast(LegacyAPIResponse[R], await func(*args, **kwargs))
                                      ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/resources/chat/completions/completions.py", line 2907, in create
    return await self._post(
           ^^^^^^^^^^^^^^^^^
    ...<55 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1992, in post
    return await self.request(cast_to, opts, stream=stream, stream_cls=stream_cls)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1777, in request
    raise self._make_status_error_from_response(err.response) from None
openai.NotFoundError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 658, in acompletion
    response = await _resolve_dispatched_chat_response(init_response)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 723, in _resolve_dispatched_chat_response
    return await pending
           ^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 1008, in acompletion
    raise OpenAIError(
    ...<4 lines>...
    )
litellm.llms.openai.common_utils.OpenAIError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 608, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/sdk/trace/__init__.py", line 1177, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 443, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/tracing.py", line 1135, in _use_native_generate_content_span_stable_semconv
    yield gc_span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/tracing.py", line 1152, in _use_native_generate_content_span
    yield gc_span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/tracing.py", line 1005, in use_inference_span
    yield gc_span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/_instrumentation.py", line 662, in record_inference_telemetry
    yield tel_ctx
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 308, in run_and_handle_error
    async for llm_response in agen:
      tel_ctx.record_llm_response(invocation_context, llm_response)
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 3883, in generate_content_async
    response = await self.llm_client.acompletion(**completion_args)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 923, in acompletion
    return await acompletion(
           ^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/chat_completions/dispatch.py", line 114, in acompletion
    return await _ADISPATCH.arun(
           ^^^^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/rust_bridge/dispatch.py", line 114, in arun
    return await python(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2271, in wrapper_async
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2082, in wrapper_async
    result = await original_function(*args, **call_kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 711, in acompletion
    raise exception_type(
          ~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<3 lines>...
        extra_kwargs=kwargs,
        ^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2719, in exception_type
    raise e  # it's already mapped
    ^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2466, in exception_type
    _map_openai_exception(
    ~~~~~~~~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<5 lines>...
        extra_information=extra_information,
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 374, in _map_openai_exception
    raise BadRequestError(
    ...<6 lines>...
    )
litellm.exceptions.BadRequestError: litellm.BadRequestError: OpenAIException - model not found

exception.escaped
FalsezOKBadRequestError: litellm.BadRequestError: OpenAIException - model not foundÖ   ¯?
uQ√ãÃO]≠ µpÿ™„FÅ¶¬˛"Dÿ?,ÇŸQ¯*call_llm09¯∑ ‘ŸA∞£4§ ‘ŸZ⁄>	i4§ ‘Ÿ	exception6
exception.type$
"litellm.exceptions.BadRequestErrorQ
exception.message<
:litellm.BadRequestError: OpenAIException - model not foundö=
exception.stacktraceÅ=
˛<Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 948, in acompletion
    headers, response = await self.make_openai_chat_completion_request(
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/logging_utils.py", line 340, in async_wrapper
    result: Final = await func(*args, **kwargs)
                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 505, in make_openai_chat_completion_request
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 481, in make_openai_chat_completion_request
    raw_response = await openai_aclient.chat.completions.with_raw_response.create(**data, timeout=timeout)
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_legacy_response.py", line 386, in wrapped
    return cast(LegacyAPIResponse[R], await func(*args, **kwargs))
                                      ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/resources/chat/completions/completions.py", line 2907, in create
    return await self._post(
           ^^^^^^^^^^^^^^^^^
    ...<55 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1992, in post
    return await self.request(cast_to, opts, stream=stream, stream_cls=stream_cls)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1777, in request
    raise self._make_status_error_from_response(err.response) from None
openai.NotFoundError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 658, in acompletion
    response = await _resolve_dispatched_chat_response(init_response)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 723, in _resolve_dispatched_chat_response
    return await pending
           ^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 1008, in acompletion
    raise OpenAIError(
    ...<4 lines>...
    )
litellm.llms.openai.common_utils.OpenAIError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 608, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/sdk/trace/__init__.py", line 1177, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 443, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_model_call.py", line 248, in _call_llm_with_tracing
    async for llm_response in agen:
    ...<20 lines>...
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 679, in _run_and_handle_error
    async for response in agen:
      yield response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 331, in run_and_handle_error
    raise model_error
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 308, in run_and_handle_error
    async for llm_response in agen:
      tel_ctx.record_llm_response(invocation_context, llm_response)
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 3883, in generate_content_async
    response = await self.llm_client.acompletion(**completion_args)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 923, in acompletion
    return await acompletion(
           ^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/chat_completions/dispatch.py", line 114, in acompletion
    return await _ADISPATCH.arun(
           ^^^^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/rust_bridge/dispatch.py", line 114, in arun
    return await python(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2271, in wrapper_async
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2082, in wrapper_async
    result = await original_function(*args, **call_kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 711, in acompletion
    raise exception_type(
          ~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<3 lines>...
        extra_kwargs=kwargs,
        ^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2719, in exception_type
    raise e  # it's already mapped
    ^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2466, in exception_type
    _map_openai_exception(
    ~~~~~~~~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<5 lines>...
        extra_information=extra_information,
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 374, in _map_openai_exception
    raise BadRequestError(
    ...<6 lines>...
    )
litellm.exceptions.BadRequestError: litellm.BadRequestError: OpenAIException - model not found

exception.escaped
FalsezOKBadRequestError: litellm.BadRequestError: OpenAIException - model not foundÖ   ¿O
uQ√ãÃO]≠ µpÿDÿ?,ÇŸQ¯"IÕ;ÿ·Ó4*invoke_agent assistant09X˛àÛ‘ŸA ë§ ‘ŸJ'
gen_ai.operation.name
invoke_agentJ
gen_ai.agent.description
 J 
gen_ai.agent.name
	assistantJ.
gen_ai.conversation.id
adk-error-b0812599Z˘L	¯‡ê§ ‘Ÿ	exception6
exception.type$
"litellm.exceptions.BadRequestErrorQ
exception.message<
:litellm.BadRequestError: OpenAIException - model not foundπK
exception.stacktrace†K
ùKTraceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 948, in acompletion
    headers, response = await self.make_openai_chat_completion_request(
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/logging_utils.py", line 340, in async_wrapper
    result: Final = await func(*args, **kwargs)
                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 505, in make_openai_chat_completion_request
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 481, in make_openai_chat_completion_request
    raw_response = await openai_aclient.chat.completions.with_raw_response.create(**data, timeout=timeout)
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_legacy_response.py", line 386, in wrapped
    return cast(LegacyAPIResponse[R], await func(*args, **kwargs))
                                      ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/resources/chat/completions/completions.py", line 2907, in create
    return await self._post(
           ^^^^^^^^^^^^^^^^^
    ...<55 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1992, in post
    return await self.request(cast_to, opts, stream=stream, stream_cls=stream_cls)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/openai/_base_client.py", line 1777, in request
    raise self._make_status_error_from_response(err.response) from None
openai.NotFoundError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 658, in acompletion
    response = await _resolve_dispatched_chat_response(init_response)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 723, in _resolve_dispatched_chat_response
    return await pending
           ^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/llms/openai/openai.py", line 1008, in acompletion
    raise OpenAIError(
    ...<4 lines>...
    )
litellm.llms.openai.common_utils.OpenAIError: Error code: 404 - {'error': {'message': 'model not found', 'type': 'invalid_request_error'}}

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 608, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/sdk/trace/__init__.py", line 1177, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 443, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/_instrumentation.py", line 561, in record_agent_invocation
    yield scope
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/base_agent.py", line 392, in _run
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/llm_agent.py", line 632, in _run_async_impl
    async for event in agen:
    ...<8 lines>...
        should_pause = True
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 359, in run_async
    async for event in agen:
      last_event = event
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 437, in _run_one_step_async
    async for llm_response in agen:
    ...<23 lines>...
          yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 629, in _call_llm_async
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_model_call.py", line 284, in call_llm_async
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/utils/_runner_utils.py", line 42, in _with_caller_context
    async for item in a:
    ...<4 lines>...
        context.detach(token)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_model_call.py", line 248, in _call_llm_with_tracing
    async for llm_response in agen:
    ...<20 lines>...
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 679, in _run_and_handle_error
    async for response in agen:
      yield response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 331, in run_and_handle_error
    raise model_error
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 308, in run_and_handle_error
    async for llm_response in agen:
      tel_ctx.record_llm_response(invocation_context, llm_response)
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 3883, in generate_content_async
    response = await self.llm_client.acompletion(**completion_args)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 923, in acompletion
    return await acompletion(
           ^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/chat_completions/dispatch.py", line 114, in acompletion
    return await _ADISPATCH.arun(
           ^^^^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/rust_bridge/dispatch.py", line 114, in arun
    return await python(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2271, in wrapper_async
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2082, in wrapper_async
    result = await original_function(*args, **call_kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 711, in acompletion
    raise exception_type(
          ~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<3 lines>...
        extra_kwargs=kwargs,
        ^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2719, in exception_type
    raise e  # it's already mapped
    ^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2466, in exception_type
    _map_openai_exception(
    ~~~~~~~~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<5 lines>...
        extra_information=extra_information,
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 374, in _map_openai_exception
    raise BadRequestError(
    ...<6 lines>...
    )
litellm.exceptions.BadRequestError: litellm.BadRequestError: OpenAIException - model not found

exception.escaped
FalsezOKBadRequestError: litellm.BadRequestError: OpenAIException - model not foundÖ   —P
uQ√ãÃO]≠ µpÿIÕ;ÿ·Ó4*
invocation09–òÔÚ‘ŸAÄSê• ‘ŸZªO	X0ê• ‘Ÿ	exception6
exception.type$
"litellm.exceptions.BadRequestErrorQ
exception.message<
:litellm.BadRequestError: OpenAIException - model not found˚M
exception.stacktrace‚M
ﬂMTraceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner_utils.py", line 241, in _drive_root_node
    await root_ctx._run_node_internal(  # pylint: disable=protected-access
    ...<3 lines>...
    )
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/context.py", line 515, in _run_node_internal
    return await _dynamic_node_scheduler.run_node_internal(
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<12 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_dynamic_node_scheduler.py", line 738, in run_node_internal
    raise DynamicNodeFailError(
    ...<3 lines>...
    )
google.adk.workflow._errors.DynamicNodeFailError: Dynamic node assistant failed

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 608, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/sdk/trace/__init__.py", line 1177, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/opentelemetry/trace/__init__.py", line 443, in start_as_current_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/telemetry/_instrumentation.py", line 159, in record_invocation
    yield
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner_utils.py", line 274, in _run
    await runner._cleanup_root_task(task, runner.agent.name)  # pylint: disable=protected-access
    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/runners.py", line 812, in _cleanup_root_task
    await task
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner_utils.py", line 250, in _drive_root_node
    raise e.error
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner.py", line 138, in run
    await self._execute_node(ctx, node_input)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner.py", line 304, in _execute_node
    await self._run_node_loop(ctx, node_input)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_node_runner.py", line 318, in _run_node_loop
    async for event in agen:
      self._track_event_in_context(event, ctx)
      await self._enqueue_event(event, ctx)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_base_node.py", line 190, in run
    async for item in agen:
    ...<12 lines>...
        yield Event(output=validated)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/llm_agent.py", line 683, in _run_impl
    async for event in agen:
    ...<4 lines>...
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/workflow/_llm_agent_wrapper.py", line 482, in run_llm_agent_as_node
    async for event in run_iter:
    ...<31 lines>...
        break
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/base_agent.py", line 328, in run_async
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/base_agent.py", line 422, in _run_with_lifecycle
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/utils/_runner_utils.py", line 42, in _with_caller_context
    async for item in a:
    ...<4 lines>...
        context.detach(token)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/base_agent.py", line 392, in _run
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/agents/llm_agent.py", line 632, in _run_async_impl
    async for event in agen:
    ...<8 lines>...
        should_pause = True
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 359, in run_async
    async for event in agen:
      last_event = event
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 437, in _run_one_step_async
    async for llm_response in agen:
    ...<23 lines>...
          yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 629, in _call_llm_async
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_model_call.py", line 284, in call_llm_async
    async for event in agen:
      yield event
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/utils/_runner_utils.py", line 42, in _with_caller_context
    async for item in a:
    ...<4 lines>...
        context.detach(token)
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_model_call.py", line 248, in _call_llm_with_tracing
    async for llm_response in agen:
    ...<20 lines>...
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/base_llm_flow.py", line 679, in _run_and_handle_error
    async for response in agen:
      yield response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 331, in run_and_handle_error
    raise model_error
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/flows/llm_flows/core/_finalizer.py", line 308, in run_and_handle_error
    async for llm_response in agen:
      tel_ctx.record_llm_response(invocation_context, llm_response)
      yield llm_response
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 3883, in generate_content_async
    response = await self.llm_client.acompletion(**completion_args)
               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/google/adk/models/lite_llm.py", line 923, in acompletion
    return await acompletion(
           ^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/chat_completions/dispatch.py", line 114, in acompletion
    return await _ADISPATCH.arun(
           ^^^^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/rust_bridge/dispatch.py", line 114, in arun
    return await python(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2271, in wrapper_async
    raise e
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/utils.py", line 2082, in wrapper_async
    result = await original_function(*args, **call_kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/main.py", line 711, in acompletion
    raise exception_type(
          ~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<3 lines>...
        extra_kwargs=kwargs,
        ^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2719, in exception_type
    raise e  # it's already mapped
    ^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 2466, in exception_type
    _map_openai_exception(
    ~~~~~~~~~~~~~~~~~~~~~^
        model=model,
        ^^^^^^^^^^^^
    ...<5 lines>...
        extra_information=extra_information,
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "/Users/sideseat/Desktop/dev/sideseat/examples/python/adk/.venv/lib/python3.13/site-packages/litellm/litellm_core_utils/exception_mapping_utils.py", line 374, in _map_openai_exception
    raise BadRequestError(
    ...<6 lines>...
    )
litellm.exceptions.BadRequestError: litellm.BadRequestError: OpenAIException - model not found

exception.escaped
FalsezOKBadRequestError: litellm.BadRequestError: OpenAIException - model not foundÖ   'https://opentelemetry.io/schemas/1.36.0