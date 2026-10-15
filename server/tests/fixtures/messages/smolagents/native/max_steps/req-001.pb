
›^
Ë
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
$e37fa1f2-8a0c-4dc5-9665-4a90ab641a4d

service.name

smolagentsÊ\
2
(openinference.instrumentation.smolagents0.1.42ØR
z9®+ k±+­À#µ¾ü%ú®ÙÊn/oY"øt {î¼*LiteLLMModel.generate_stream090ÄÂ0´bÞAP4»´bÞJ%
input.mime_type
application/jsonJï#
input.valueß#
Ü#{"messages": ["ChatMessage(role=<MessageRole.SYSTEM: 'system'>, content=[{'type': 'text', 'text': 'You are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.\\nTo do so, you have been given access to some tools.\\n\\nThe tool call you write is an action: after the tool is executed, you will get the result of the tool call as an \"observation\".\\nThis Action/Observation can repeat N times, you should take several steps when needed.\\n\\nYou can use the result of the previous action as input for the next action.\\nThe observation will always be a string: it can represent a file, like \"image_1.jpg\".\\nThen you can use it as input for the next action. You can do it for instance as follows:\\n\\nObservation: \"image_1.jpg\"\\n\\nAction:\\n{\\n  \"name\": \"image_transformer\",\\n  \"arguments\": {\"image\": \"image_1.jpg\"}\\n}\\n\\nTo provide the final answer to the task, use an action blob with \"name\": \"final_answer\" tool. It is the only way to complete the task, else you will be stuck on a loop. So your final output should look like this:\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": {\"answer\": \"insert your final answer here\"}\\n}\\n\\n\\nHere are a few examples using notional tools:\\n---\\nTask: \"Generate an image of the oldest person in this document.\"\\n\\nAction:\\n{\\n  \"name\": \"document_qa\",\\n  \"arguments\": {\"document\": \"document.pdf\", \"question\": \"Who is the oldest person mentioned?\"}\\n}\\nObservation: \"The oldest person in the document is John Doe, a 55 year old lumberjack living in Newfoundland.\"\\n\\nAction:\\n{\\n  \"name\": \"image_generator\",\\n  \"arguments\": {\"prompt\": \"A portrait of John Doe, a 55-year-old man living in Canada.\"}\\n}\\nObservation: \"image.png\"\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"image.png\"\\n}\\n\\n---\\nTask: \"What is the result of the following operation: 5 + 3 + 1294.678?\"\\n\\nAction:\\n{\\n    \"name\": \"python_interpreter\",\\n    \"arguments\": {\"code\": \"5 + 3 + 1294.678\"}\\n}\\nObservation: 1302.678\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"1302.678\"\\n}\\n\\n---\\nTask: \"Which city has the highest population , Guangzhou or Shanghai?\"\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Guangzhou\"\\n}\\nObservation: [\\'Guangzhou has a population of 15 million inhabitants as of 2021.\\']\\n\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Shanghai\"\\n}\\nObservation: \\'26 million (2019)\\'\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"Shanghai\"\\n}\\n\\nAbove example were using notional tools that might not exist for you. You only have access to these tools:\\n- get_weather: Get the weather forecast for a city.\\n    Takes inputs: {\\'city\\': {\\'type\\': \\'string\\', \\'description\\': \\'The city name.\\'}, \\'days\\': {\\'type\\': \\'integer\\', \\'nullable\\': True, \\'description\\': \\'How many days to forecast, from 1 to 7.\\'}}\\n    Returns an output of type: object\\n- final_answer: Provides a final answer to the given problem.\\n    Takes inputs: {\\'answer\\': {\\'type\\': \\'any\\', \\'description\\': \\'The final answer to the problem\\'}}\\n    Returns an output of type: any\\nYou are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.\\n\\nHere are the rules you should always follow to solve your task:\\n1. ALWAYS provide a tool call, else you will fail.\\n2. Always use the right arguments for the tools. Never use variable names as the action arguments, use the value instead.\\n3. Call a tool only when needed: do not call the search agent if you do not need information, try to solve the task yourself. If no tool call is needed, use final_answer tool to return your answer.\\n4. Never re-do a tool call that you previously did with the exact same parameters.\\n\\nNow Begin!'}], tool_calls=None, raw=None, token_usage=None)", "ChatMessage(role=<MessageRole.USER: 'user'>, content=[{'type': 'text', 'text': 'New task:\\nCheck the weather in Paris and tell me whether to take an umbrella.'}], tool_calls=None, raw=None, token_usage=None)"], "stop_sequences": ["Observation:", "Calling tools:"], "response_format": null, "tools_to_call_from": ["<smolagents.tools.tool.<locals>.SimpleTool object at 0x109626f90>", "<smolagents.default_tools.FinalAnswerTool object at 0x1100a12b0>"], "kwargs": {}}Jk
llm.invocation_parametersN
L{"aws_region_name": "us-west-2", "max_tokens": 16000, "tool_choice": "auto"}J-
!llm.input_messages.0.message.role
systemJF
<llm.input_messages.0.message.contents.0.message_content.type
textJ«
<llm.input_messages.0.message.contents.0.message_content.textê
çYou are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.
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
- get_weather: Get the weather forecast for a city.
    Takes inputs: {'city': {'type': 'string', 'description': 'The city name.'}, 'days': {'type': 'integer', 'nullable': True, 'description': 'How many days to forecast, from 1 to 7.'}}
    Returns an output of type: object
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
textJ
<llm.input_messages.1.message.contents.0.message_content.textO
MNew task:
Check the weather in Paris and tell me whether to take an umbrella.J
llm.token_count.promptœJ 
llm.token_count.completion2J
llm.token_count.totalÎJ>
llm.model_name,
*bedrock/global.anthropic.claude-sonnet-5-5J1
"llm.output_messages.0.message.role
	assistantJG
=llm.output_messages.0.message.contents.0.message_content.type
textJC
=llm.output_messages.0.message.contents.0.message_content.text
 J[
7llm.output_messages.0.message.tool_calls.0.tool_call.id 
tooluse_RKoGWA8l2IX0s8dZ0GLqN5JS
Bllm.output_messages.0.message.tool_calls.0.tool_call.function.name
get_weatherJ^
Gllm.output_messages.0.message.tool_calls.0.tool_call.function.arguments
{"city": "Paris"}J‚
llm.tools.0.tool.json_schemaá
Þ{"type": "function", "function": {"name": "get_weather", "description": "Get the weather forecast for a city.", "parameters": {"type": "object", "properties": {"city": {"type": "string", "description": "The city name."}, "days": {"type": "integer", "nullable": true, "description": "How many days to forecast, from 1 to 7."}}, "required": ["city"]}}}J¸
llm.tools.1.tool.json_schema—
”{"type": "function", "function": {"name": "final_answer", "description": "Provides a final answer to the given problem.", "parameters": {"type": "object", "properties": {"answer": {"type": "string", "description": "The final answer to the problem"}}, "required": ["answer"]}}}J&
output.mime_type
application/jsonJÅ
output.value´
±{"role": "assistant", "content": "", "tool_calls": [{"function": {"arguments": "{\"city\": \"Paris\"}", "name": "get_weather", "description": null}, "id": "tooluse_RKoGWA8l2IX0s8dZ0GLqN5", "type": "function"}], "raw": null, "token_usage": {"input_tokens": 1820, "output_tokens": 50, "total_tokens": 1870}}J 
openinference.span.kind
LLMz…   ó
z9®+ k±+­À#µ¾ü%¹»|{?ì"øt {î¼*
SimpleTool09X78»´bÞA(-;»´bÞJ[
input.valueL
J{"args": [], "sanitize_inputs_outputs": true, "kwargs": {"city": "Paris"}}J
	tool.name
get_weatherJ:
tool.description&
$Get the weather forecast for a city.J½
tool.parameters©
¦{"city": {"type": "string", "description": "The city name."}, "days": {"type": "integer", "nullable": true, "description": "How many days to forecast, from 1 to 7."}}Ja
output.valueQ
O{"city": "Paris", "forecast": [{"day": 1, "condition": "sunny", "high_c": 21}]}J&
output.mime_type
application/jsonJ!
openinference.span.kind
TOOLz…   Â
z9®+ k±+­À#µ¾ü%øt {î¼"êBS¦È*Step 109Ð˜·0´bÞAHi>»´bÞJì
input.valueÜ
Ù{"memory_step": "ActionStep(step_number=1, timing=Timing(start_time=1791978227.790515, end_time=None, duration=None), model_input_messages=None, tool_calls=None, error=None, model_output_message=None, model_output=None, code_action=None, observations=None, observations_images=None, action_output=None, token_usage=None, is_final_answer=False)"}Ja
output.valueQ
O{'city': 'Paris', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}]}J"
openinference.span.kind
CHAINz…   