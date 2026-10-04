
˜k
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
$5b45518e-48a3-4b1f-bed0-1d5ffa01fc78

service.name

smolagentséi
2
(openinference.instrumentation.smolagents0.1.42ÚV
d8˝˛&›#Œt0¢Wòzûÿd¸X∏q"^q}Eƒ*LiteLLMModel.generate09»U}P±s€A®P∆±s€J%
input.mime_type
application/jsonJ» 
input.value∏ 
µ {"messages": ["ChatMessage(role=<MessageRole.SYSTEM: 'system'>, content=[{'type': 'text', 'text': 'You are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.\\nTo do so, you have been given access to some tools.\\n\\nThe tool call you write is an action: after the tool is executed, you will get the result of the tool call as an \"observation\".\\nThis Action/Observation can repeat N times, you should take several steps when needed.\\n\\nYou can use the result of the previous action as input for the next action.\\nThe observation will always be a string: it can represent a file, like \"image_1.jpg\".\\nThen you can use it as input for the next action. You can do it for instance as follows:\\n\\nObservation: \"image_1.jpg\"\\n\\nAction:\\n{\\n  \"name\": \"image_transformer\",\\n  \"arguments\": {\"image\": \"image_1.jpg\"}\\n}\\n\\nTo provide the final answer to the task, use an action blob with \"name\": \"final_answer\" tool. It is the only way to complete the task, else you will be stuck on a loop. So your final output should look like this:\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": {\"answer\": \"insert your final answer here\"}\\n}\\n\\n\\nHere are a few examples using notional tools:\\n---\\nTask: \"Generate an image of the oldest person in this document.\"\\n\\nAction:\\n{\\n  \"name\": \"document_qa\",\\n  \"arguments\": {\"document\": \"document.pdf\", \"question\": \"Who is the oldest person mentioned?\"}\\n}\\nObservation: \"The oldest person in the document is John Doe, a 55 year old lumberjack living in Newfoundland.\"\\n\\nAction:\\n{\\n  \"name\": \"image_generator\",\\n  \"arguments\": {\"prompt\": \"A portrait of John Doe, a 55-year-old man living in Canada.\"}\\n}\\nObservation: \"image.png\"\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"image.png\"\\n}\\n\\n---\\nTask: \"What is the result of the following operation: 5 + 3 + 1294.678?\"\\n\\nAction:\\n{\\n    \"name\": \"python_interpreter\",\\n    \"arguments\": {\"code\": \"5 + 3 + 1294.678\"}\\n}\\nObservation: 1302.678\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"1302.678\"\\n}\\n\\n---\\nTask: \"Which city has the highest population , Guangzhou or Shanghai?\"\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Guangzhou\"\\n}\\nObservation: [\\'Guangzhou has a population of 15 million inhabitants as of 2021.\\']\\n\\n\\nAction:\\n{\\n    \"name\": \"web_search\",\\n    \"arguments\": \"Population Shanghai\"\\n}\\nObservation: \\'26 million (2019)\\'\\n\\nAction:\\n{\\n  \"name\": \"final_answer\",\\n  \"arguments\": \"Shanghai\"\\n}\\n\\nAbove example were using notional tools that might not exist for you. You only have access to these tools:\\n- final_answer: Provides a final answer to the given problem.\\n    Takes inputs: {\\'answer\\': {\\'type\\': \\'any\\', \\'description\\': \\'The final answer to the problem\\'}}\\n    Returns an output of type: any\\nYou are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.\\n\\nHere are the rules you should always follow to solve your task:\\n1. ALWAYS provide a tool call, else you will fail.\\n2. Always use the right arguments for the tools. Never use variable names as the action arguments, use the value instead.\\n3. Call a tool only when needed: do not call the search agent if you do not need information, try to solve the task yourself. If no tool call is needed, use final_answer tool to return your answer.\\n4. Never re-do a tool call that you previously did with the exact same parameters.\\n\\nNow Begin!'}], tool_calls=None, raw=None, token_usage=None)", "ChatMessage(role=<MessageRole.USER: 'user'>, content=[{'type': 'text', 'text': 'New task:\\nSuggest one museum to visit in Madrid.'}], tool_calls=None, raw=None, token_usage=None)"], "stop_sequences": ["Observation:", "Calling tools:"], "response_format": null, "tools_to_call_from": ["<smolagents.default_tools.FinalAnswerTool object at 0x1144a3380>"], "kwargs": {}}Jk
llm.invocation_parametersN
L{"aws_region_name": "us-west-2", "max_tokens": 16000, "tool_choice": "auto"}J-
!llm.input_messages.0.message.role
systemJF
<llm.input_messages.0.message.contents.0.message_content.type
textJò
<llm.input_messages.0.message.contents.0.message_content.text◊
‘You are an expert assistant who can solve any task using tool calls. You will be given a task to solve as best you can.
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
textJr
<llm.input_messages.1.message.contents.0.message_content.text2
0New task:
Suggest one museum to visit in Madrid.J
llm.token_count.prompt´J 
llm.token_count.completion~J
llm.token_count.total©J>
llm.model_name,
*bedrock/global.anthropic.claude-sonnet-5-5J1
"llm.output_messages.0.message.role
	assistantJG
=llm.output_messages.0.message.contents.0.message_content.type
textJC
=llm.output_messages.0.message.contents.0.message_content.text
 J[
7llm.output_messages.0.message.tool_calls.0.tool_call.id 
tooluse_gDbink0F6t2hj4oYQCSEQbJT
Bllm.output_messages.0.message.tool_calls.0.tool_call.function.name
final_answerJ¶
Gllm.output_messages.0.message.tool_calls.0.tool_call.function.arguments⁄
◊{"answer": "Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel\u00e1zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two."}J∏
llm.tools.0.tool.json_schemaó
î{"type": "function", "function": {"name": "final_answer", "description": "Provides a final answer to the given problem.", "parameters": {"type": "object", "properties": {"answer": {"type": "string", "description": "The final answer to the problem"}}, "required": ["answer"]}}}J&
output.mime_type
application/jsonJ˘
output.valueË
Â{"role": "assistant", "content": "", "tool_calls": [{"function": {"arguments": "{\"answer\": \"Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel\\u00e1zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two.\"}", "name": "final_answer", "description": null}, "id": "tooluse_gDbink0F6t2hj4oYQCSEQb", "type": "function"}], "raw": "ModelResponse(id='chatcmpl-e9600415-f2fe-405e-a754-10d88a86aa2b', created=1791152484, model='global.anthropic.claude-sonnet-5-5', object='chat.completion', system_fingerprint=None, choices=[Choices(finish_reason='tool_calls', index=0, message=Message(content='', role='assistant', tool_calls=[ChatCompletionMessageToolCall(index=0, function=Function(arguments='{\"answer\": \"Visit the Museo del Prado, Madrid\\'s premier art museum, home to masterpieces by Vel\\\\u00e1zquez, Goya, and El Greco. It\\'s located on Paseo del Prado, near Retiro Park, so you can easily combine the two.\"}', name='final_answer'), id='tooluse_gDbink0F6t2hj4oYQCSEQb', type='function')], function_call=None, provider_specific_fields=None))], usage=Usage(completion_tokens=126, prompt_tokens=1579, total_tokens=1705, completion_tokens_details=CompletionTokensDetailsWrapper(accepted_prediction_tokens=None, audio_tokens=None, reasoning_tokens=0, rejected_prediction_tokens=None, text_tokens=126, image_tokens=None, video_tokens=None), prompt_tokens_details=PromptTokensDetailsWrapper(audio_tokens=None, cache_write_tokens=0, cached_tokens=0, text_tokens=1579, image_tokens=None, video_tokens=None, cache_creation_tokens=0), cache_creation_input_tokens=0, cache_read_input_tokens=0))", "token_usage": {"input_tokens": 1579, "output_tokens": 126, "total_tokens": 1705}}J 
openinference.span.kind
LLMzÖ   ﬁ
d8˝˛&›#Œt0¢Wò‡êFu ≥^"^q}Eƒ*FinalAnswerTool09ò6o∆±s€Ax_p∆±s€Jü
input.valueè
å{"args": [], "sanitize_inputs_outputs": true, "kwargs": {"answer": "Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel√°zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two."}}J
	tool.name
final_answerJC
tool.description/
-Provides a final answer to the given problem.Jb
tool.parametersO
M{"answer": {"type": "any", "description": "The final answer to the problem"}}JŸ
output.value»
≈Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel√°zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two.J 
output.mime_type

text/plainJ!
openinference.span.kind
TOOLzÖ   ª
d8˝˛&›#Œt0¢Wò^q}Eƒ"yúÕJp;∏*Step 109P{zP±s€A?x∆±s€JÏ
input.value‹
Ÿ{"memory_step": "ActionStep(step_number=1, timing=Timing(start_time=1791152482.206187, end_time=None, duration=None), model_input_messages=None, tool_calls=None, error=None, model_output_message=None, model_output=None, code_action=None, observations=None, observations_images=None, action_output=None, token_usage=None, is_final_answer=False)"}JŸ
output.value»
≈Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel√°zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two.J"
openinference.span.kind
CHAINzÖ   √
d8˝˛&›#Œt0¢WòyúÕJp;∏"<qh¶ŒVß*ToolCallingAgent.run09¿<5P±s€A¯U|∆±s€JΩ
input.value≠
™{"task": "Suggest one museum to visit in Madrid.", "stream": false, "reset": true, "images": null, "additional_args": null, "max_steps": null, "return_full_result": null}J
smolagents.max_stepsJ,
smolagents.tools_names*

final_answerJŸ
output.value»
≈Visit the Museo del Prado, Madrid's premier art museum, home to masterpieces by Vel√°zquez, Goya, and El Greco. It's located on Paseo del Prado, near Retiro Park, so you can easily combine the two.J
llm.token_count.prompt´J 
llm.token_count.completion~J
llm.token_count.total©J"
openinference.span.kind
AGENTzÖ   ï
	
exampleá
d8˝˛&›#Œt0¢Wò<qh¶ŒVß*session-turn-109PC0P±s€A§|∆±s€J"

session.id
smolagents-sessionJ
user.id
example-userz Ö   