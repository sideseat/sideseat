
ü]
½
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
process.pidÎ®¹Y

logfire.openai_agents5.1.1Ó<
¡	ŒóA÷¯)^"2Bf"Ú„}@¯Þ–Ä—"eÔÛ+õ¦Èu*+Responses API with {gen_ai.request.model!r}09ø¨ûú{ÛAÐ(¹Yü{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J
model_settingsý
ú{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J3
gen_ai.request.model
global.openai.gpt-6.1-solJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_xh72xb2d4gdkl5u2diqzqfgnuydzi43mydwaze556axeaheh3wxqJÅ
usage»
¸{"requests":1,"input_tokens":160,"output_tokens":88,"total_tokens":248,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJ£
response–
“{"id":"resp_xh72xb2d4gdkl5u2diqzqfgnuydzi43mydwaze556axeaheh3wxq","access_programs":null,"created_at":1791161595.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"arguments":"{\"city\":\"Paris\",\"days\":2}","call_id":"call_f38f9a61cea95939ac9265381def150e","name":"get_weather","type":"function_call","id":"fc_5a4905ff48ff5942b1f5553f4104db43","async_":null,"caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"city\":\"Tokyo\",\"days\":2}","call_id":"call_af7698eac407506c94cd91c2591d3747","name":"get_weather","type":"function_call","id":"fc_9f0852d5bfe956bd843ba3fdb6a338fb","async_":null,"caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"city\":\"Paris\"}","call_id":"call_0bf0682b250b5469bfe0a2c41bee11ba","name":"get_precipitation","type":"function_call","id":"fc_cdc5b4e6b79f5bf3b6be41d2b7ba6b70","async_":null,"caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"city\":\"Tokyo\"}","call_id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","name":"get_precipitation","type":"function_call","id":"fc_403596f667255477a193ce70316cf95b","async_":null,"caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"get_weather","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"},"days":{"default":1,"description":"How many days to forecast, from 1 to 7.","title":"Days","type":"integer"}},"required":["city","days"],"title":"get_weather_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the weather forecast for a city.","output_schema":null},{"name":"get_precipitation","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"}},"required":["city"],"title":"get_precipitation_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the chance of rain in a city for tomorrow.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1791161600.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":160,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":88,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":248},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ¤
	raw_input–
“[{"content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"}]Jø	
eventsí	
ê	[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_f38f9a61cea95939ac9265381def150e","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_af7698eac407506c94cd91c2591d3747","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Tokyo\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_0bf0682b250b5469bfe0a2c41bee11ba","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Paris\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Tokyo\"}"}}]}]J 
gen_ai.usage.input_tokens J 
gen_ai.usage.output_tokensXJÃ
gen_ai.usage.raw®
«{"input_tokens":160,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":88,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":248}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'Já

logfire.json_schemaÉ

Æ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z …   …
¡	ŒóA÷¯)^"2Bf"Úî¤¢l•ùX"eÔÛ+õ¦Èu*Function: {name}09 ŠóYü{ÛAX–'Zü{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
get_weatherJ$
input
{"city":"Tokyo","days":2}J|
outputr
p{"city":"Tokyo","forecast":[{"day":1,"condition":"rain","high_c":21},{"day":2,"condition":"sunny","high_c":22}]}J
mcp_data
nullJ
gen_ai.system
openaiJ&
logfire.msg
Function: get_weatherJ˜
logfire.json_schema€
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   ­
¡	ŒóA÷¯)^"2Bf"Ú1èm³§™”ó"eÔÛ+õ¦Èu*Function: {name}09ÀcüYü{ÛA û)Zü{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
get_precipitationJ
input
{"city":"Tokyo"}J1
output'
%80% chance of rain in Tokyo tomorrow.J
mcp_data
nullJ
gen_ai.system
openaiJ,
logfire.msg
Function: get_precipitationJˆ
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   †
¡	ŒóA÷¯)^"2Bf"Ú @l_-ÐÿS"eÔÛ+õ¦Èu*Function: {name}09(¡èYü{ÛAXr-Zü{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
get_weatherJ$
input
{"city":"Paris","days":2}J}
outputs
q{"city":"Paris","forecast":[{"day":1,"condition":"sunny","high_c":21},{"day":2,"condition":"sunny","high_c":22}]}J
mcp_data
nullJ
gen_ai.system
openaiJ&
logfire.msg
Function: get_weatherJ˜
logfire.json_schema€
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   ­
¡	ŒóA÷¯)^"2Bf"ÚF	‹Fý€"eÔÛ+õ¦Èu*Function: {name}09°TøYü{ÛA`;/Zü{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
get_precipitationJ
input
{"city":"Paris"}J1
output'
%10% chance of rain in Paris tomorrow.J
mcp_data
nullJ
gen_ai.system
openaiJ,
logfire.msg
Function: get_precipitationJˆ
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   Ï
¡	ŒóA÷¯)^"2Bf"ÚeÔÛ+õ¦Èu"<ååz$è*Turn {turn} for agent assistant09@â…ûú{ÛA¨p9Zü{ÛJ(
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

turnJ

agent_name
	assistantJg
usage^
\{"input_tokens":160,"output_tokens":88,"cached_input_tokens":0,"cache_write_input_tokens":0}J+
logfire.msg
Turn 1 for agent assistantJ©
logfire.json_schema‘
Ž{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z …   