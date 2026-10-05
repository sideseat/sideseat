
ı[
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
$df65d805-0d45-45fe-9b5a-72bc23aabe66
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
process.pid»™≤W

logfire.openai_agents5.1.1ÌB
°	åoûw˜¬f73–Ï´˝c‹[˜π"ò∑ˇ$„úz1*+Responses API with {gen_ai.request.model!r}09àÌQÙ{€A∏bÓC˜{€Jà
code.filepathw
u/Users/sideseat/Desktop/dev/sideseat/.claude/worktrees/agent-a19a68f1975c89c48/examples/python/harness/harness/cli.pyJ
code.function	
_invokeJ
code.linenoñJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J3
gen_ai.request.model
global.openai.gpt-6.1-solJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_bljkblrggox24zo42gyqxzznuqyjeq64fhepcmsjryrvcuzr2xmaJ≈
usageª
∏{"requests":1,"input_tokens":128,"output_tokens":36,"total_tokens":164,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJã
response˛
˚{"id":"resp_bljkblrggox24zo42gyqxzznuqyjeq64fhepcmsjryrvcuzr2xma","access_programs":null,"created_at":1791161577.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"id":"msg_785bfe0e98225bd4a9226b460cfeba94","content":[{"annotations":[],"text":"Try **bacalhau √† Br√°s**‚Äîshredded salt cod mixed with thin fried potatoes, onions, and eggs, usually topped with black olives.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[],"top_p":0.98,"background":false,"completed_at":1791161578.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":128,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":36,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":164},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ˛
	raw_input
Ì[{"role":"user","content":"I am planning a weekend in Lisbon. Name one neighbourhood to stay in."},{"id":"rs_67cfd2c8e9b8517c9cc9e7b8ec25c906","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeLunq96oGga8bTv8fMxlBR029Ssmj/Fg0Vn3oZw9tQQiAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFhLY2VXOVhwZTNBQWUzRituMExlWUxFV3JsZ01uTmtlSmhidHcyZUFZVGg3WUFTc0lyRFM2NGx2MVFzTTFTWXc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcfSbn3guzjOnC6GDZcYz8HuRMXAp8FRYqDbUuQATyexPQO0PepR8jRQ3zVYSKJ7otDaj4UmGSg6bDY40K9a3etr9/ppwE9P0YtBUyfbQaO8xSAo5UaqUGtkqGrScCAAAQAHuGT2s89ICuLJjJB+BUP7HbZbfoRxAOZMBkDXSK4RZvfO5KcKXozwoNNmM1XvkOJf////8AAAABAAAAAAAAAAAAAAABAAABLPFJFhCXty0ra6Y1+z2J0UruHFYc3KfI29N2PIbTwA4TYAlmI8sc7kr4wO+mw1hI2sUJIHyXcOMbiuihsbY3TeBZBOd80+nNlbFDTVJeP0HG2HjafyCyTCWss6Goxl3FWCmqeghz1nJvIrIikXQGMS0UIOoliaUlHOIpgHZc3iyK0vTX8dMCPWugtZqChGcezkPadvo76fInu6cnJ9etE6SC5VlhokN8GNsAgM2DrSal8qwmqlSRkzfeIaJEVNxVHE/UTXc4D1msfxQIWLhiwNxygMfOsh4L4dfd7PwmBbyX++g7t+1O2n998PoutJIYUUEvWzTa9Uxr58ZLhcDv3F4Qrj0k9p0e2y/8N6nQ2Rd+gXDSKfPWYbloTrIp7XH6aoufWQRiGidYBRBfqr/Q9mLkRmr8FCb7U0sbcsgAZzBlAjEA0VM1VHRmcujTEm2lgxZUR1T6qbOxjvHzCxlrSUzTnBjXgRFFxhOKhI66s3viSQAIAjBU4ZJ2u7Q+d+TJGvw5hJWiXnu4sKmaKuxQ9hDLW/8lpXs9ZuyCu4E1/siT5rlc5Tw="},{"id":"msg_2d83296656c8571596b5607e45859057","content":[{"annotations":[],"text":"Stay in **Chiado**‚Äîit‚Äôs central, walkable, and full of caf√©s and restaurants, making it a great base for a weekend in Lisbon.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"},{"role":"user","content":"What is one dish I should try there?"}]Jº
events±
Æ[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"I am planning a weekend in Lisbon. Name one neighbourhood to stay in.","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_67cfd2c8e9b8517c9cc9e7b8ec25c906","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeLunq96oGga8bTv8fMxlBR029Ssmj/Fg0Vn3oZw9tQQiAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFhLY2VXOVhwZTNBQWUzRituMExlWUxFV3JsZ01uTmtlSmhidHcyZUFZVGg3WUFTc0lyRFM2NGx2MVFzTTFTWXc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcfSbn3guzjOnC6GDZcYz8HuRMXAp8FRYqDbUuQATyexPQO0PepR8jRQ3zVYSKJ7otDaj4UmGSg6bDY40K9a3etr9/ppwE9P0YtBUyfbQaO8xSAo5UaqUGtkqGrScCAAAQAHuGT2s89ICuLJjJB+BUP7HbZbfoRxAOZMBkDXSK4RZvfO5KcKXozwoNNmM1XvkOJf////8AAAABAAAAAAAAAAAAAAABAAABLPFJFhCXty0ra6Y1+z2J0UruHFYc3KfI29N2PIbTwA4TYAlmI8sc7kr4wO+mw1hI2sUJIHyXcOMbiuihsbY3TeBZBOd80+nNlbFDTVJeP0HG2HjafyCyTCWss6Goxl3FWCmqeghz1nJvIrIikXQGMS0UIOoliaUlHOIpgHZc3iyK0vTX8dMCPWugtZqChGcezkPadvo76fInu6cnJ9etE6SC5VlhokN8GNsAgM2DrSal8qwmqlSRkzfeIaJEVNxVHE/UTXc4D1msfxQIWLhiwNxygMfOsh4L4dfd7PwmBbyX++g7t+1O2n998PoutJIYUUEvWzTa9Uxr58ZLhcDv3F4Qrj0k9p0e2y/8N6nQ2Rd+gXDSKfPWYbloTrIp7XH6aoufWQRiGidYBRBfqr/Q9mLkRmr8FCb7U0sbcsgAZzBlAjEA0VM1VHRmcujTEm2lgxZUR1T6qbOxjvHzCxlrSUzTnBjXgRFFxhOKhI66s3viSQAIAjBU4ZJ2u7Q+d+TJGvw5hJWiXnu4sKmaKuxQ9hDLW/8lpXs9ZuyCu4E1/siT5rlc5Tw="}},{"event.name":"gen_ai.assistant.message","content":"Stay in **Chiado**‚Äîit‚Äôs central, walkable, and full of caf√©s and restaurants, making it a great base for a weekend in Lisbon.","role":"assistant"},{"event.name":"gen_ai.user.message","content":"What is one dish I should try there?","role":"user"},{"event.name":"gen_ai.assistant.message","content":"Try **bacalhau √† Br√°s**‚Äîshredded salt cod mixed with thin fried potatoes, onions, and eggs, usually topped with black olives.","role":"assistant"}]J 
gen_ai.usage.input_tokensÄJ 
gen_ai.usage.output_tokens$J√
gen_ai.usage.rawÆ
´{"input_tokens":128,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":36,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":164}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'Jı

logfire.json_schema›

⁄
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z Ö   —
°	åoûw˜¬f73–Ïò∑ˇ$„úz1"˘Á24[Q*Turn {turn} for agent assistant09Ÿ∆QÙ{€AP7ˇC˜{€J*
code.filepath
scenarios/multi_turn.pyJ
code.function
runJ
code.linenoJ9
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
\{"input_tokens":128,"output_tokens":36,"cached_input_tokens":0,"cache_write_input_tokens":0}J+
logfire.msg
Turn 1 for agent assistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ´
°	åoûw˜¬f73–Ï˘Á24[Q"¨€$~->ne*Agent run: {name!r}09»ûƒQÙ{€A D˜{€J*
code.filepath
scenarios/multi_turn.pyJ
code.function
runJ
code.linenoJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
	assistantJ
handoffs
[]J
tools
[]J
output_type
strJ
gen_ai.system
openaiJ'
logfire.msg
Agent run: 'assistant'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   ù
°	åoûw˜¬f73–Ï¨€$~->ne"ÃÌ∞v}Ûn@*Task: Agent workflow09∏õæQÙ{€A(sD˜{€J*
code.filepath
scenarios/multi_turn.pyJ
code.function
runJ
code.linenoJ.
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
taskJá
usage~
|{"input_tokens":128,"output_tokens":36,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":1,"total_tokens":164}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˝
°	åoûw˜¬f73–ÏÃÌ∞v}Ûn@"¿Ó®ô#Ω*OpenAI Agents trace: {name}09∞∏QÙ{€A JD˜{€J*
code.filepath
scenarios/multi_turn.pyJ
code.function
runJ
code.linenoJ
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
&trace_309ed1dcbc2245df82428df5b4c258b7J©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   