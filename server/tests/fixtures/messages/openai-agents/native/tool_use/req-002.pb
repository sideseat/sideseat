
£i
Ω
!
process.runtime.name	
cpython
#
process.runtime.version
3.14.7
U
process.runtime.description6
43.14.7 (main, Sep  1 2026, 14:05:28) [Clang 22.1.3 ]

	host.name
bcd074a10ac4

	host.arch
arm64

os.type
darwin


os.version
25.6.0
=
service.instance.id&
$4c03ddf0-3ad2-49c6-a5e2-d691ec7750de
=
service.version*
(f8af17bcb0ac2eeb4dfaf0a06f069688dd669b9c
"
telemetry.sdk.language
python
%
telemetry.sdk.name
opentelemetry
!
telemetry.sdk.version
1.44.0

service.name
openai-agents

logfire.version
5.1.1

process.pidŒÆíc

logfire.openai_agents5.1.1≥N
°	åÛA˜Ø)^"2Bf"⁄∏‹Ô÷¡÷µ∑"≤ıﬂÔ‹„vÓ*+Responses API with {gen_ai.request.model!r}090VZ¸{€Aò·Ë˝{€J(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J3
gen_ai.request.model
global.openai.gpt-6.1-solJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_v3th36v7afqqeduwnmqmmd6gqnkqnawtn7lybu4pv763riy5jypqJ≈
usageª
∏{"requests":1,"input_tokens":407,"output_tokens":80,"total_tokens":487,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJ∞
response£
†{"id":"resp_v3th36v7afqqeduwnmqmmd6gqnkqnawtn7lybu4pv763riy5jypq","access_programs":null,"created_at":1791161601.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"id":"msg_4ca7b4b6b0eb557e844034226f652814","content":[{"annotations":[],"text":"Paris: sunny over the next two days, with highs of 21¬∞C and 22¬∞C; an umbrella probably isn‚Äôt necessary, with just a 10% chance of rain tomorrow.  \nTokyo: rain on the first day and sunshine on the second, with highs of 21¬∞C and 22¬∞C; pack an umbrella, as tomorrow‚Äôs rain chance is 80%.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"get_weather","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"},"days":{"default":1,"description":"How many days to forecast, from 1 to 7.","title":"Days","type":"integer"}},"required":["city","days"],"title":"get_weather_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the weather forecast for a city.","output_schema":null},{"name":"get_precipitation","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"}},"required":["city"],"title":"get_precipitation_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the chance of rain in a city for tomorrow.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1791161603.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":407,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":80,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":487},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJë
	raw_inputÉ
Ä[{"content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"},{"arguments":"{\"city\":\"Paris\",\"days\":2}","call_id":"call_f38f9a61cea95939ac9265381def150e","name":"get_weather","type":"function_call","id":"fc_5a4905ff48ff5942b1f5553f4104db43","status":"completed"},{"arguments":"{\"city\":\"Tokyo\",\"days\":2}","call_id":"call_af7698eac407506c94cd91c2591d3747","name":"get_weather","type":"function_call","id":"fc_9f0852d5bfe956bd843ba3fdb6a338fb","status":"completed"},{"arguments":"{\"city\":\"Paris\"}","call_id":"call_0bf0682b250b5469bfe0a2c41bee11ba","name":"get_precipitation","type":"function_call","id":"fc_cdc5b4e6b79f5bf3b6be41d2b7ba6b70","status":"completed"},{"arguments":"{\"city\":\"Tokyo\"}","call_id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","name":"get_precipitation","type":"function_call","id":"fc_403596f667255477a193ce70316cf95b","status":"completed"},{"call_id":"call_f38f9a61cea95939ac9265381def150e","output":"{'city': 'Paris', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","type":"function_call_output"},{"call_id":"call_af7698eac407506c94cd91c2591d3747","output":"{'city': 'Tokyo', 'forecast': [{'day': 1, 'condition': 'rain', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","type":"function_call_output"},{"call_id":"call_0bf0682b250b5469bfe0a2c41bee11ba","output":"10% chance of rain in Paris tomorrow.","type":"function_call_output"},{"call_id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","output":"80% chance of rain in Tokyo tomorrow.","type":"function_call_output"}]J¬
events∑
¥[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_f38f9a61cea95939ac9265381def150e","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_af7698eac407506c94cd91c2591d3747","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Tokyo\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_0bf0682b250b5469bfe0a2c41bee11ba","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Paris\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Tokyo\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_f38f9a61cea95939ac9265381def150e","content":"{'city': 'Paris', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","name":"get_weather"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_af7698eac407506c94cd91c2591d3747","content":"{'city': 'Tokyo', 'forecast': [{'day': 1, 'condition': 'rain', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","name":"get_weather"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_0bf0682b250b5469bfe0a2c41bee11ba","content":"10% chance of rain in Paris tomorrow.","name":"get_precipitation"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","content":"80% chance of rain in Tokyo tomorrow.","name":"get_precipitation"},{"event.name":"gen_ai.assistant.message","content":"Paris: sunny over the next two days, with highs of 21¬∞C and 22¬∞C; an umbrella probably isn‚Äôt necessary, with just a 10% chance of rain tomorrow.  \nTokyo: rain on the first day and sunshine on the second, with highs of 21¬∞C and 22¬∞C; pack an umbrella, as tomorrow‚Äôs rain chance is 80%.","role":"assistant"}]J 
gen_ai.usage.input_tokensóJ 
gen_ai.usage.output_tokensPJ√
gen_ai.usage.rawÆ
´{"input_tokens":407,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":80,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":487}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'J„
logfire.json_schemaÀ
»{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z Ö   œ
°	åÛA˜Ø)^"2Bf"⁄≤ıﬂÔ‹„vÓ"<ÂÂz$Ë*Turn {turn} for agent assistant09»?Z¸{€AËÂp˝{€J(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ9
logfire.msg_template!
Turn {turn} for agent assistantJ
logfire.span_type
spanJ
type
customJ
name
turnJ
gen_ai.system
openaiJ
sdk_span_type
turnJ

turnJ

agent_name
	assistantJg
usage^
\{"input_tokens":407,"output_tokens":80,"cached_input_tokens":0,"cache_write_input_tokens":0}J+
logfire.msg
Turn 2 for agent assistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö    
°	åÛA˜Ø)^"2Bf"⁄<ÂÂz$Ë"5ßr:2≥*Agent run: {name!r}098%Ç˚˙{€A–≥y˝{€J(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
	assistantJ
handoffs
[]J.
tools%
#["get_weather","get_precipitation"]J
output_type
strJ
gen_ai.system
openaiJ'
logfire.msg
Agent run: 'assistant'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   ú
°	åÛA˜Ø)^"2Bf"⁄5ßr:2≥"AË¥*º˘hÃ*Task: Agent workflow09†Üh˚˙{€Ax	|˝{€J(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ.
logfire.msg_template
Task: Agent workflowJ
logfire.span_type
spanJ
type
customJ
name
Agent workflowJ
gen_ai.system
openaiJ
sdk_span_type
taskJà
usage
}{"input_tokens":567,"output_tokens":168,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":2,"total_tokens":735}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˚
°	åÛA˜Ø)^"2Bf"⁄AË¥*º˘hÃ"|XÃ¯!ÿ‘*OpenAI Agents trace: {name}09x¨G˚˙{€A`}˝{€J(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ
name
Agent workflowJ
group_id
nullJ
metadata
nullJ
tracing
nullJ5
logfire.msg_template
OpenAI Agents trace: {name}J4
logfire.msg%
#OpenAI Agents trace: Agent workflowJ
logfire.span_type
spanJ:
agent_trace_id(
&trace_3b1d290b1a8b4c87895fc54291e49a1bJ©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   À
	
exampleΩ
°	åÛA˜Ø)^"2Bf"⁄|XÃ¯!ÿ‘*tool-use09¿S¬˙˙{€Aò9Å˝{€J&

session.id
openai-agents-tool_useJ
user.id
example-userJ
logfire.span_type
spanJ
logfire.msg

tool-usez Ö   