
Á∑
À
"
telemetry.sdk.language
python
%
telemetry.sdk.name
opentelemetry
!
telemetry.sdk.version
1.45.0
=
service.instance.id&
$a0f9bf1a-717d-42d0-8e4d-90f7eb50bd45

service.name

smolagentsï∂
2
(openinference.instrumentation.smolagents0.1.42û\
z˛˛
‚g¡ÌtıeêöQÖØ8¥H#"mT¶«–M≤*LiteLLMModel.generate09ËÅôµs€A0ÆñI∂s€J%
input.mime_type
application/jsonJÒ#
input.value·#
ﬁ#{"messages": ["ChatMessage(role=<MessageRole.SYSTEM: 'system'>, content=[{'type': 'text', 'text': 'You are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.\\nTo do so, you have been given access to some tools.\\n\\nThe tool call you write is an action: after the tool is executed, you will get the result of the tool call as an \"observation\".\\nThis Action/Observation can repeat N times, you should take several steps when needed.\\n\\nYou can use the result of the previous action as input for the next action.\\nThe observation will always be a string: it can represent a file, like \"image_1.jpg\".\\nThen you can use it as input for the next action. You can do it for instance as follows:\\n\\nObservation: \"image_1.jpg\"\\n\\nAction:\\n{\\n  \"name\": \"image_transformer\",\\n  \"arguments\": {\"image\": \"image_1.jpg\"}\\n}\\n\\nTo provide the final answer to the task, use an action blob with \"name\": \"final_answer\" tool. It is the only way to complete the task, else you will be stuck on a loop. So your final output should look like this:\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": {\"answer\": \"insert your final answer here\"}\\n}\\n\\n\\nHere are a few examples using notional tools:\\n---\\nTask: \"Generate an image of the oldest person in this document.\"\\n\\nAction:\\n{\\n  \"name\": \"document_qa\",\\n  \"arguments\": {\"document\": \"document.pdf\", \"question\": \"Who is the oldest person mentioned?\"}\\n}\\nObservation: \"The oldest person in the document is John Doe, a 55 year old lumberjack living in Newfoundland.\"\\n\\nAction:\\n{\\n  \"name\": \"image_generator\",\\n  \"arguments\": {\"prompt\": \"A portrait of John Doe, a 55-year-old man living in Canada.\"}\\n}\\nObservation: \"image.png\"\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"image.png\"\\n}\\n\\n---\\nTask: \"What is the result of the following operation: 5 + 3 + 1294.678?\"\\n\\nAction:\\n{\\n    \"name\": \"python_interpreter\",\\n    \"arguments\": {\"code\": \"5 + 3 + 1294.678\"}\\n}\\nObservation: 1302.678\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"1302.678\"\\n}\\n\\n---\\nTask: \"Which city has the highest population , Guangzhou or Shanghai?\"\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Guangzhou\"\\n}\\nObservation: [\\'Guangzhou has a population of 15 million inhabitants as of 2021.\\']\\n\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Shanghai\"\\n}\\nObservation: \\'26 million (2019)\\'\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"Shanghai\"\\n}\\n\\nAbove example were using notional tools that might not exist for you. You only have access to these tools:\\n- book_flight: Book a flight.\\n    Takes inputs: {\\'origin\\': {\\'type\\': \\'string\\', \\'description\\': \\'Departure city.\\'}, \\'destination\\': {\\'type\\': \\'string\\', \\'description\\': \\'Arrival city.\\'}, \\'date\\': {\\'type\\': \\'string\\', \\'description\\': \\'Travel date.\\'}}\\n    Returns an output of type: string\\n- final_answer: Provides a final answer to the given problem.\\n    Takes inputs: {\\'answer\\': {\\'type\\': \\'any\\', \\'description\\': \\'The final answer to the problem\\'}}\\n    Returns an output of type: any\\nYou are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.\\n\\nHere are the rules you should always follow to solve your task:\\n1. ALWAYS provide a tool call, else you will fail.\\n2. Always use the right arguments for the tools. Never use variable names as the action arguments, use the value instead.\\n3. Call a tool only when needed: do not call the search agent if you do not need information, try to solve the task yourself. If no tool call is needed, use final_answer tool to return your answer.\\n4. Never re-do a tool call that you previously did with the exact same parameters.\\n\\nNow Begin!'}], tool_calls=None, raw=None, token_usage=None)", "ChatMessage(role=<MessageRole.USER: 'user'>, content=[{'type': 'text', 'text': 'New task:\\nBook me a flight from London to Oslo on 2026-11-14.'}], tool_calls=None, raw=None, token_usage=None)"], "stop_sequences": ["Observation:", "Calling tools:"], "response_format": null, "tools_to_call_from": ["<smolagents.tools.tool.<locals>.SimpleTool object at 0x109c57770>", "<smolagents.default_tools.FinalAnswerTool object at 0x11127f770>"], "kwargs": {}}Jk
llm.invocation_parametersN
L{"aws_region_name": "us-west-2", "max_tokens": 16000, "tool_choice": "auto"}J-
!llm.input_messages.0.message.role
systemJF
<llm.input_messages.0.message.contents.0.message_content.type
textJ≠
<llm.input_messages.0.message.contents.0.message_content.textÏ
ÈYou are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.
To do so, you have been given access to some tools.

The tool call you write is an action: after the tool is executed, you will get the result of the tool call as an "observation".
This Action/Observation can repeat N times, you should take several steps when needed.

You can use the result of the previous action as input for the next action.
The observation will always be a string: it can represent a file, like "image_1.jpg".
Then you can use it as input for the next action. You can do it for instance as follows:

Observation: "image_1.jpg"

Action:
{
  "name": "image_transformer",
  "arguments": {"image": "image_1.jpg"}
}

To provide the final answer to the task, use an action blob with "name": "final_answer" tool. It is the only way to complete the task, else you will be stuck on a loop. So your final output should look like this:
Action:
{
  "name": "final_answer",
  "arguments": {"answer": "insert your final answer here"}
}


Here are a few examples using notional tools:
---
Task: "Generate an image of the oldest person in this document."

Action:
{
  "name": "document_qa",
  "arguments": {"document": "document.pdf", "question": "Who is the oldest person mentioned?"}
}
Observation: "The oldest person in the document is John Doe, a 55 year old lumberjack living in Newfoundland."

Action:
{
  "name": "image_generator",
  "arguments": {"prompt": "A portrait of John Doe, a 55-year-old man living in Canada."}
}
Observation: "image.png"

Action:
{
  "name": "final_answer",
  "arguments": "image.png"
}

---
Task: "What is the result of the following operation: 5 + 3 + 1294.678?"

Action:
{
    "name": "python_interpreter",
    "arguments": {"code": "5 + 3 + 1294.678"}
}
Observation: 1302.678

Action:
{
  "name": "final_answer",
  "arguments": "1302.678"
}

---
Task: "Which city has the highest population , Guangzhou or Shanghai?"

Action:
{
    "name": "web_search",
    "arguments": "Population Guangzhou"
}
Observation: ['Guangzhou has a population of 15 million inhabitants as of 2021.']


Action:
{
    "name": "web_search",
    "arguments": "Population Shanghai"
}
Observation: '26 million (2019)'

Action:
{
  "name": "final_answer",
  "arguments": "Shanghai"
}

Above example were using notional tools that might not exist for you. You only have access to these tools:
- book_flight: Book a flight.
    Takes inputs: {'origin': {'type': 'string', 'description': 'Departure city.'}, 'destination': {'type': 'string', 'description': 'Arrival city.'}, 'date': {'type': 'string', 'description': 'Travel date.'}}
    Returns an output of type: string
- final_answer: Provides a final answer to the given problem.
    Takes inputs: {'answer': {'type': 'any', 'description': 'The final answer to the problem'}}
    Returns an output of type: any
You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.

Here are the rules you should always follow to solve your task:
1. ALWAYS provide a tool call, else you will fail.
2. Always use the right arguments for the tools. Never use variable names as the action arguments, use the value instead.
3. Call a tool only when needed: do not call the search agent if you do not need information, try to solve the task yourself. If no tool call is needed, use final_answer tool to return your answer.
4. Never re-do a tool call that you previously did with the exact same parameters.

Now Begin!J+
!llm.input_messages.1.message.role
userJF
<llm.input_messages.1.message.contents.0.message_content.type
textJ
<llm.input_messages.1.message.contents.0.message_content.text?
=New task:
Book me a flight from London to Oslo on 2026-11-14.J
llm.token_count.prompt®J 
llm.token_count.completiondJ
llm.token_count.totalåJ>
llm.model_name,
*bedrock/global.anthropic.claude-sonnet-5-5J1
"llm.output_messages.0.message.role
	assistantJG
=llm.output_messages.0.message.contents.0.message_content.type
textJC
=llm.output_messages.0.message.contents.0.message_content.text
 J[
7llm.output_messages.0.message.tool_calls.0.tool_call.id 
tooluse_u9zL60LC34rxhkCRhkfTRVJS
Bllm.output_messages.0.message.tool_calls.0.tool_call.function.name
book_flightJé
Gllm.output_messages.0.message.tool_calls.0.tool_call.function.argumentsC
A{"origin": "London", "destination": "Oslo", "date": "2026-11-14"}Jù
llm.tools.0.tool.json_schema¸
˘{"type": "function", "function": {"name": "book_flight", "description": "Book a flight.", "parameters": {"type": "object", "properties": {"origin": {"type": "string", "description": "Departure city."}, "destination": {"type": "string", "description": "Arrival city."}, "date": {"type": "string", "description": "Travel date."}}, "required": ["origin", "destination", "date"]}}}J∏
llm.tools.1.tool.json_schemaó
î{"type": "function", "function": {"name": "final_answer", "description": "Provides a final answer to the given problem.", "parameters": {"type": "object", "properties": {"answer": {"type": "string", "description": "The final answer to the problem"}}, "required": ["answer"]}}}J&
output.mime_type
application/jsonJ”
output.value¬
ø{"role": "assistant", "content": "", "tool_calls": [{"function": {"arguments": "{\"origin\": \"London\", \"destination\": \"Oslo\", \"date\": \"2026-11-14\"}", "name": "book_flight", "description": null}, "id": "tooluse_u9zL60LC34rxhkCRhkfTRV", "type": "function"}], "raw": "ModelResponse(id='chatcmpl-19073265-856c-4231-b417-4afbd02dfc95', created=1791152503, model='global.anthropic.claude-sonnet-5-5', object='chat.completion', system_fingerprint=None, choices=[Choices(finish_reason='tool_calls', index=0, message=Message(content='', role='assistant', tool_calls=[ChatCompletionMessageToolCall(index=0, function=Function(arguments='{\"origin\": \"London\", \"destination\": \"Oslo\", \"date\": \"2026-11-14\"}', name='book_flight'), id='tooluse_u9zL60LC34rxhkCRhkfTRV', type='function')], function_call=None, provider_specific_fields=None))], usage=Usage(completion_tokens=100, prompt_tokens=1832, total_tokens=1932, completion_tokens_details=CompletionTokensDetailsWrapper(accepted_prediction_tokens=None, audio_tokens=None, reasoning_tokens=0, rejected_prediction_tokens=None, text_tokens=100, image_tokens=None, video_tokens=None), prompt_tokens_details=PromptTokensDetailsWrapper(audio_tokens=None, cache_write_tokens=0, cached_tokens=0, text_tokens=1832, image_tokens=None, video_tokens=None, cache_creation_tokens=0), cache_creation_input_tokens=0, cache_read_input_tokens=0))", "token_usage": {"input_tokens": 1832, "output_tokens": 100, "total_tokens": 1932}}J 
openinference.span.kind
LLMzÖ   ˆ
z˛˛
‚g¡Ìtıeêèx
Çù™-"mT¶«–M≤*
SimpleTool09–¡øI∂s€Aô&J∂s€Jã
input.value|
z{"args": [], "sanitize_inputs_outputs": true, "kwargs": {"origin": "London", "destination": "Oslo", "date": "2026-11-14"}}J
	tool.name
book_flightJ$
tool.description
Book a flight.J’
tool.parameters¡
æ{"origin": {"type": "string", "description": "Departure city."}, "destination": {"type": "string", "description": "Arrival city."}, "date": {"type": "string", "description": "Travel date."}}J!
openinference.span.kind
TOOLZ¯	êØ%J∂s€	exception6
exception.type$
"harness.content.BookingUnavailablea
exception.messageL
JNo seats from London to Oslo on 2026-11-14: the booking system is offline.®
exception.stacktraceè
åTraceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/opentelemetry/trace/__init__.py", line 602, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/_tracers.py", line 142, in start_as_current_span
    yield cast(OpenInferenceSpan, current_span)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/smolagents/_wrappers.py", line 667, in __call__
    response = wrapped(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 246, in __call__
    outputs = self.forward(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 1098, in wrapped_function
    return tool_function(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/harness/harness/content.py", line 111, in book_flight
    raise BookingUnavailable(
        f"No seats from {origin} to {destination} on {date}: the booking system is offline."
    )
harness.content.BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.

exception.escaped
Falsezb^BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.Ö   ƒE
z˛˛
‚g¡ÌtıeêmT¶«–M≤"Åö/¥‹*Step 1098Ú}ôµs€A–‹vJ∂s€JÏ
input.value‹
Ÿ{"memory_step": "ActionStep(step_number=1, timing=Timing(start_time=1791152500.610872, end_time=None, duration=None), model_input_messages=None, tool_calls=None, error=None, model_output_message=None, model_output=None, code_action=None, observations=None, observations_images=None, action_output=None, token_usage=None, is_final_answer=False)"}J"
openinference.span.kind
CHAINZÌ	»“jJ∂s€	exception<
exception.type*
(smolagents.utils.AgentToolExecutionErrorë
exception.message˚
¯Error executing tool 'book_flight' with arguments {'origin': 'London', 'destination': 'Oslo', 'date': '2026-11-14'}: BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.
Please try again or use another toolÊ
exception.stacktraceÕ
 Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1486, in execute_tool_call
    return tool(**arguments) if is_managed_agent else tool(**arguments, sanitize_inputs_outputs=True)
                                                      ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/smolagents/_wrappers.py", line 667, in __call__
    response = wrapped(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 246, in __call__
    outputs = self.forward(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 1098, in wrapped_function
    return tool_function(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/harness/harness/content.py", line 111, in book_flight
    raise BookingUnavailable(
        f"No seats from {origin} to {destination} on {date}: the booking system is offline."
    )
harness.content.BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/smolagents/_wrappers.py", line 323, in __call__
    yield from wrapped(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1336, in _step_stream
    for output in self.process_tool_calls(chat_message, memory_step):
                  ~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1421, in process_tool_calls
    tool_output = process_single_tool_call(tool_call)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1390, in process_single_tool_call
    tool_call_result = self.execute_tool_call(tool_name, tool_arguments)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1502, in execute_tool_call
    raise AgentToolExecutionError(error_msg, self.logger) from e
smolagents.utils.AgentToolExecutionError: Error executing tool 'book_flight' with arguments {'origin': 'London', 'destination': 'Oslo', 'date': '2026-11-14'}: BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.
Please try again or use another tool

exception.escaped
FalseZ›!	 ¶vJ∂s€	exception<
exception.type*
(smolagents.utils.AgentToolExecutionErrorë
exception.message˚
¯Error executing tool 'book_flight' with arguments {'origin': 'London', 'destination': 'Oslo', 'date': '2026-11-14'}: BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.
Please try again or use another tool÷
exception.stacktraceΩ
∫Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1486, in execute_tool_call
    return tool(**arguments) if is_managed_agent else tool(**arguments, sanitize_inputs_outputs=True)
                                                      ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/smolagents/_wrappers.py", line 667, in __call__
    response = wrapped(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 246, in __call__
    outputs = self.forward(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/tools.py", line 1098, in wrapped_function
    return tool_function(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/harness/harness/content.py", line 111, in book_flight
    raise BookingUnavailable(
        f"No seats from {origin} to {destination} on {date}: the booking system is offline."
    )
harness.content.BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.

The above exception was the direct cause of the following exception:

Traceback (most recent call last):
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/opentelemetry/trace/__init__.py", line 602, in use_span
    yield span
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/_tracers.py", line 142, in start_as_current_span
    yield cast(OpenInferenceSpan, current_span)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/openinference/instrumentation/smolagents/_wrappers.py", line 323, in __call__
    yield from wrapped(*args, **kwargs)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1336, in _step_stream
    for output in self.process_tool_calls(chat_message, memory_step):
                  ~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1421, in process_tool_calls
    tool_output = process_single_tool_call(tool_call)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1390, in process_single_tool_call
    tool_call_result = self.execute_tool_call(tool_name, tool_arguments)
  File "/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a5917963f2590a710/examples/python/smolagents/.venv/lib/python3.14/site-packages/smolagents/agents.py", line 1502, in execute_tool_call
    raise AgentToolExecutionError(error_msg, self.logger) from e
smolagents.utils.AgentToolExecutionError: Error executing tool 'book_flight' with arguments {'origin': 'London', 'destination': 'Oslo', 'date': '2026-11-14'}: BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.
Please try again or use another tool

exception.escaped
FalsezñëAgentToolExecutionError: Error executing tool 'book_flight' with arguments {'origin': 'London', 'destination': 'Oslo', 'date': '2026-11-14'}: BookingUnavailable: No seats from London to Oslo on 2026-11-14: the booking system is offline.
Please try again or use another toolÖ   