
ÔÈ
ë
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
$a04459f5-ca80-4cc0-b56d-488e737b9860
"
telemetry.sdk.language
python
 
telemetry.sdk.name

sideseat
 
telemetry.sdk.version
2.0.0

service.name
openai-agents

service.version
0.23.1
%
sideseat.framework
openai-agents
-
sideseat.integrations
('openai-agents',)

logfire.version
5.1.1

process.pid“´ŒÂ

logfire.openai_agents5.1.1–=
¡	!?KÿVpò,ƒ÷ËÊàÚ>ošC"šr8Áòú7 *+Responses API with {gen_ai.request.model!r}09[e¹ı{ÛAóWÁı{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J
model_settingsı
ú{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J3
gen_ai.request.model
global.openai.gpt-6.1-solJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJJ
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
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z …   È
¡	!?KÿVpò,ƒ÷V^V@‡Éóz"šr8Áòú7 *Function: {name}09Øu‚Áı{ÛAĞQ¤Áı{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   ğ
¡	!?KÿVpò,ƒ÷R¨‹Ø-2ë"šr8Áòú7 *Function: {name}09ğ¾ŠÁı{ÛA˜õ¦Áı{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   É
¡	!?KÿVpò,ƒ÷BºÆ÷ïğ"šr8Áòú7 *Function: {name}09H:{Áı{ÛApÀ©Áı{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   ğ
¡	!?KÿVpò,ƒ÷œ¯bbu“"šr8Áòú7 *Function: {name}09ğÖ†Áı{ÛAğò«Áı{ÛJˆ
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.lineno–J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z …   ’
¡	!?KÿVpò,ƒ÷šr8Áòú7 "©(ïéy‰*Turn {turn} for agent assistant09X¡M¹ı{ÛA8@²Áı{ÛJ(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ9
logfire.msg_template!
Turn {turn} for agent assistantJ
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z …   öN
¡	!?KÿVpò,ƒ÷ë‹šì²f"¦òbÑNG*+Responses API with {gen_ai.request.model!r}09ø´ÃÁı{ÛA ~BÂı{ÛJ(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ
model_settingsı
ú{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J3
gen_ai.request.model
global.openai.gpt-6.1-solJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJJ
response_id;
9resp_v3th36v7afqqeduwnmqmmd6gqnkqnawtn7lybu4pv763riy5jypqJÅ
usage»
¸{"requests":1,"input_tokens":407,"output_tokens":80,"total_tokens":487,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJ°
response£
 {"id":"resp_v3th36v7afqqeduwnmqmmd6gqnkqnawtn7lybu4pv763riy5jypq","access_programs":null,"created_at":1791161601.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"id":"msg_4ca7b4b6b0eb557e844034226f652814","content":[{"annotations":[],"text":"Paris: sunny over the next two days, with highs of 21Â°C and 22Â°C; an umbrella probably isnâ€™t necessary, with just a 10% chance of rain tomorrow.  \nTokyo: rain on the first day and sunshine on the second, with highs of 21Â°C and 22Â°C; pack an umbrella, as tomorrowâ€™s rain chance is 80%.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"get_weather","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"},"days":{"default":1,"description":"How many days to forecast, from 1 to 7.","title":"Days","type":"integer"}},"required":["city","days"],"title":"get_weather_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the weather forecast for a city.","output_schema":null},{"name":"get_precipitation","parameters":{"properties":{"city":{"description":"The city name.","title":"City","type":"string"}},"required":["city"],"title":"get_precipitation_args","type":"object","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"async_":null,"defer_loading":null,"description":"Get the chance of rain in a city for tomorrow.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1791161603.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":407,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":80,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":487},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ‘
	raw_inputƒ
€[{"content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"},{"arguments":"{\"city\":\"Paris\",\"days\":2}","call_id":"call_f38f9a61cea95939ac9265381def150e","name":"get_weather","type":"function_call","id":"fc_5a4905ff48ff5942b1f5553f4104db43","status":"completed"},{"arguments":"{\"city\":\"Tokyo\",\"days\":2}","call_id":"call_af7698eac407506c94cd91c2591d3747","name":"get_weather","type":"function_call","id":"fc_9f0852d5bfe956bd843ba3fdb6a338fb","status":"completed"},{"arguments":"{\"city\":\"Paris\"}","call_id":"call_0bf0682b250b5469bfe0a2c41bee11ba","name":"get_precipitation","type":"function_call","id":"fc_cdc5b4e6b79f5bf3b6be41d2b7ba6b70","status":"completed"},{"arguments":"{\"city\":\"Tokyo\"}","call_id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","name":"get_precipitation","type":"function_call","id":"fc_403596f667255477a193ce70316cf95b","status":"completed"},{"call_id":"call_f38f9a61cea95939ac9265381def150e","output":"{'city': 'Paris', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","type":"function_call_output"},{"call_id":"call_af7698eac407506c94cd91c2591d3747","output":"{'city': 'Tokyo', 'forecast': [{'day': 1, 'condition': 'rain', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","type":"function_call_output"},{"call_id":"call_0bf0682b250b5469bfe0a2c41bee11ba","output":"10% chance of rain in Paris tomorrow.","type":"function_call_output"},{"call_id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","output":"80% chance of rain in Tokyo tomorrow.","type":"function_call_output"}]JÂ
events·
´[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"What will the weather be in Paris and in Tokyo over the next two days, and should I pack an umbrella for either city?","role":"user"},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_f38f9a61cea95939ac9265381def150e","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_af7698eac407506c94cd91c2591d3747","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Tokyo\",\"days\":2}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_0bf0682b250b5469bfe0a2c41bee11ba","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Paris\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","type":"function","function":{"name":"get_precipitation","arguments":"{\"city\":\"Tokyo\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_f38f9a61cea95939ac9265381def150e","content":"{'city': 'Paris', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","name":"get_weather"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_af7698eac407506c94cd91c2591d3747","content":"{'city': 'Tokyo', 'forecast': [{'day': 1, 'condition': 'rain', 'high_c': 21}, {'day': 2, 'condition': 'sunny', 'high_c': 22}]}","name":"get_weather"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_0bf0682b250b5469bfe0a2c41bee11ba","content":"10% chance of rain in Paris tomorrow.","name":"get_precipitation"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_22fc63ebe3ed5b6fa85ddf289831ac5b","content":"80% chance of rain in Tokyo tomorrow.","name":"get_precipitation"},{"event.name":"gen_ai.assistant.message","content":"Paris: sunny over the next two days, with highs of 21Â°C and 22Â°C; an umbrella probably isnâ€™t necessary, with just a 10% chance of rain tomorrow.  \nTokyo: rain on the first day and sunshine on the second, with highs of 21Â°C and 22Â°C; pack an umbrella, as tomorrowâ€™s rain chance is 80%.","role":"assistant"}]J 
gen_ai.usage.input_tokens—J 
gen_ai.usage.output_tokensPJÃ
gen_ai.usage.raw®
«{"input_tokens":407,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":80,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":487}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'Jã
logfire.json_schemaË
È{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z …   ’
¡	!?KÿVpò,ƒ÷¦òbÑNG"©(ïéy‰*Turn {turn} for agent assistant09 Ø¶Áı{ÛAø\Âı{ÛJ(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ9
logfire.msg_template!
Turn {turn} for agent assistantJ
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
logfire.json_schema‘
{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z …   
¡	!?KÿVpò,ƒ÷©(ïéy‰"ÈlÌŠ¥Õ5*Agent run: {name!r}09ÈMJ¹ı{ÛAè>bÂı{ÛJ(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
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
Agent run: 'assistant'J
logfire.json_schema†
ƒ{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z …   ß
¡	!?KÿVpò,ƒ÷ÈlÌŠ¥Õ5" 2×˜ı„!6*Task: Agent workflow090—4¹ı{ÛA`œdÂı{ÛJ(
code.filepath
scenarios/tool_use.pyJ
code.function
runJ
code.linenoJ.
logfire.msg_template
Task: Agent workflowJ
logfire.span_type
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ
type
customJ
name
Agent workflowJ
gen_ai.system
openaiJ
sdk_span_type
taskJˆ
usage
}{"input_tokens":567,"output_tokens":168,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":2,"total_tokens":735}J%
logfire.msg
Task: Agent workflowJ
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z …   ¾
¡	!?KÿVpò,ƒ÷ 2×˜ı„!6"-HCÈÜJ*OpenAI Agents trace: {name}09Ø¹ı{ÛA(ÉeÂı{ÛJ(
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
spanJ&

session.id
openai-agents-tool_useJ
user.id
example-userJ:
agent_trace_id(
&trace_d1f6d363e8a24f3a98654cb0f1d34496J©
logfire.json_schema‘
{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z …   Ó

sideseat2.0.0½
¡	!?KÿVpò,ƒ÷-HCÈÜJ*tool-use09Ğl—¸ı{ÛAÈCiÂı{ÛJ
logfire.span_type
spanJ
logfire.msg

tool-useJ&

session.id
openai-agents-tool_useJ
user.id
example-userz …   