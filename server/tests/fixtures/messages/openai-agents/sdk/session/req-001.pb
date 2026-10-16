
î±
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
$812177fb-d3a0-4fc3-b29d-4945b337c454
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
process.pidˆ¼Ğ©

logfire.openai_agents5.1.1ú>
¡	…(Â¨æh¦ÈÚ»ÀL³í‡M"GôA(Ü*+Responses API with {gen_ai.request.model!r}09ˆ[ë­|ÛAÀr…µ|ÛJM
code.filepath<
:/home/user/sideseat/examples/python/harness/harness/cli.pyJ
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
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJJ
response_id;
9resp_gqeob3n6aymhba2cyokzvwk2vtzi3cvvaelnsgyj3hl7zyvrrplqJÅ
usage»
¸{"requests":1,"input_tokens":44,"output_tokens":79,"total_tokens":123,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":29}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJÛ
responseÎ
Ë{"id":"resp_gqeob3n6aymhba2cyokzvwk2vtzi3cvvaelnsgyj3hl7zyvrrplq","access_programs":null,"created_at":1791161609.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"id":"rs_3f4299d8976c536593b5bfd800610711","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeL8WJlF+1yaw3rTDJhVdjw892dWJwY7BU7ZSNclySDp7AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBNkptNGpPamxkU1NYWmNpK2FUNUFwVkE1bXM1aWFkUEdjd1Nxamk1YlNmTUdNK1ZBSzZCNmdodGg0RktoSnB5dGc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcEI3BX9Trlb/vsPZoVbI9Aj1yyaEdoZDuQls5WgTyexPQO0PepR8jRQ3zVYRCTWMV8IUAG3XCehRvsY16eh0RrrZ0UJLKm99VOlg4s0tpsEyRV5pPlNfQvFrf+aYCAAAQAJivI6c4YL2NJ6YpnVcqoD+udTdKPXH45SXA4xlFsr+wJWaYaBeUTMlWi5vLgHWyFf////8AAAABAAAAAAAAAAAAAAABAAABN4hGkdu4c6FDARuSwR6LLlKVMq+l1fmx7CGd4uIDeAsCmVkLy4OEO+SaNiNb+J6ly3W/5q25qgt3zX5fHjkLkCJBkav9Yxhj1olQnWqOOz9HRfhWHjHpp86edtWNdNvaXkPwSCg2oLKz4IN/KRwL/vXv972BH3BSW2lKfJtLHwoLlJAxKIvzuxj76aOjKFhqvlPag7l0p1k5A9QyBe+lczUKvhwM4Z9AIDgseMyJYv2KXKsZjwWp9qI8n8n3hyuUynDfz5E6B2y0HqODSmK6ZopfojvTovzrybylsK2ezRglkOI7RJJ5evqIWU7cc+68k3MBAgv4gHcsQnF6MDs5OI2qHuF6OWPVe7nYOy+hVqbOs1raNBAVGF92ndPI/uxt4xQxMyMdFOZRiZDiEthicoWOXd4gdqBsLhBFmzSr1WygExRV5YlHvgBnMGUCMQD9Vh2Gf6daPWFzMzDf+vfRzVaF1NA5B66CFrp/9sEbkxGO9IvYTEieCPTPVguawT0CMGBtMVBVoUVA4AR8UeCbcnGanIgk2BJ9YKDy6U8ytcFmK34wj29d5Uq6lsR5E5CYvg==","status":null},{"id":"msg_e75d645657fd5041a8838dcccaa6d43e","content":[{"annotations":[],"text":"Visit the **Prado Museum**, home to masterpieces by VelÃ¡zquez, Goya, and El Greco. Donâ€™t miss VelÃ¡zquezâ€™s *Las Meninas*, and allow two to three hours to explore the highlights.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[],"top_p":0.98,"background":false,"completed_at":1791161617.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":44,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":79,"output_tokens_details":{"reasoning_tokens":29},"total_tokens":123},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJS
	raw_inputF
D[{"content":"Suggest one museum to visit in Madrid.","role":"user"}]JÂ
events·
´[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"Suggest one museum to visit in Madrid.","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_3f4299d8976c536593b5bfd800610711","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeL8WJlF+1yaw3rTDJhVdjw892dWJwY7BU7ZSNclySDp7AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBNkptNGpPamxkU1NYWmNpK2FUNUFwVkE1bXM1aWFkUEdjd1Nxamk1YlNmTUdNK1ZBSzZCNmdodGg0RktoSnB5dGc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcEI3BX9Trlb/vsPZoVbI9Aj1yyaEdoZDuQls5WgTyexPQO0PepR8jRQ3zVYRCTWMV8IUAG3XCehRvsY16eh0RrrZ0UJLKm99VOlg4s0tpsEyRV5pPlNfQvFrf+aYCAAAQAJivI6c4YL2NJ6YpnVcqoD+udTdKPXH45SXA4xlFsr+wJWaYaBeUTMlWi5vLgHWyFf////8AAAABAAAAAAAAAAAAAAABAAABN4hGkdu4c6FDARuSwR6LLlKVMq+l1fmx7CGd4uIDeAsCmVkLy4OEO+SaNiNb+J6ly3W/5q25qgt3zX5fHjkLkCJBkav9Yxhj1olQnWqOOz9HRfhWHjHpp86edtWNdNvaXkPwSCg2oLKz4IN/KRwL/vXv972BH3BSW2lKfJtLHwoLlJAxKIvzuxj76aOjKFhqvlPag7l0p1k5A9QyBe+lczUKvhwM4Z9AIDgseMyJYv2KXKsZjwWp9qI8n8n3hyuUynDfz5E6B2y0HqODSmK6ZopfojvTovzrybylsK2ezRglkOI7RJJ5evqIWU7cc+68k3MBAgv4gHcsQnF6MDs5OI2qHuF6OWPVe7nYOy+hVqbOs1raNBAVGF92ndPI/uxt4xQxMyMdFOZRiZDiEthicoWOXd4gdqBsLhBFmzSr1WygExRV5YlHvgBnMGUCMQD9Vh2Gf6daPWFzMzDf+vfRzVaF1NA5B66CFrp/9sEbkxGO9IvYTEieCPTPVguawT0CMGBtMVBVoUVA4AR8UeCbcnGanIgk2BJ9YKDy6U8ytcFmK34wj29d5Uq6lsR5E5CYvg==","status":null}},{"event.name":"gen_ai.assistant.message","content":"Visit the **Prado Museum**, home to masterpieces by VelÃ¡zquez, Goya, and El Greco. Donâ€™t miss VelÃ¡zquezâ€™s *Las Meninas*, and allow two to three hours to explore the highlights.","role":"assistant"}]J
gen_ai.usage.input_tokens,J 
gen_ai.usage.output_tokensOJÃ
gen_ai.usage.raw®
«{"input_tokens":44,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":79,"output_tokens_details":{"reasoning_tokens":29},"total_tokens":123}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'JÓ
logfire.json_schema»
¸{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z …   
¡	…(Â¨æh¦ÈÚ»ÀGôA(Ü"IéÉÅ²¢k]*Turn {turn} for agent assistant09prÓ­|ÛA˜¥µ|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ9
logfire.msg_template!
Turn {turn} for agent assistantJ
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
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
	assistantJf
usage]
[{"input_tokens":44,"output_tokens":79,"cached_input_tokens":0,"cache_write_input_tokens":0}J+
logfire.msg
Turn 1 for agent assistantJ©
logfire.json_schema‘
{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z …   ê
¡	…(Â¨æh¦ÈÚ»ÀIéÉÅ²¢k]"my¼jv£*Agent run: {name!r}09 ÿÏ­|ÛA¨†¬µ|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ
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
Agent run: 'assistant'J
logfire.json_schema†
ƒ{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z …   Û
¡	…(Â¨æh¦ÈÚ»Àmy¼jv£"¦§Í¥Yëñ×*Task: Agent workflow090·­|ÛAp­®µ|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ.
logfire.msg_template
Task: Agent workflowJ
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ
type
customJ
name
Agent workflowJ
gen_ai.system
openaiJ
sdk_span_type
taskJ†
usage}
{{"input_tokens":44,"output_tokens":79,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":1,"total_tokens":123}J%
logfire.msg
Task: Agent workflowJ
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z …   ¼
¡	…(Â¨æh¦ÈÚ»À¦§Í¥Yëñ×"iŒsó8z**OpenAI Agents trace: {name}09XUš­|ÛA °µ|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ
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
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ:
agent_trace_id(
&trace_10599559c8af493ea789f56551751ecdJ©
logfire.json_schema‘
{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z …   ø=
¡	…¹YãL‚»Î$B~6×/V4P;"£)éñ[,Ö*+Responses API with {gen_ai.request.model!r}09ˆ›Úµ|ÛApœ¶|ÛJM
code.filepath<
:/home/user/sideseat/examples/python/harness/harness/cli.pyJ
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
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJJ
response_id;
9resp_hartax2ctb24r52wvtrmgqwm7bgudmbycpn3bsflfdvaz7irashqJÅ
usage»
¸{"requests":1,"input_tokens":44,"output_tokens":64,"total_tokens":108,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":19}}J
gen_ai.system
openaiJ4
gen_ai.response.model
global.openai.gpt-6.1-solJœ
response
Œ{"id":"resp_hartax2ctb24r52wvtrmgqwm7bgudmbycpn3bsflfdvaz7irashq","access_programs":null,"created_at":1791161618.0,"error":null,"incomplete_details":null,"instructions":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","metadata":{},"model":"global.openai.gpt-6.1-sol","object":"response","output":[{"id":"rs_9941b39f16d65a8e81c0c5c5f8037e24","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeJWhGHgqIF/kiQSk0kamBdNUKpC32hrRt9xDrVNBycodAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBdFI5ZWR5OGZDb1o5U2VZL0RtTTFlMTlSbmRuS0hPYmZaSnBJbHBhZHVEWHFGMDN5ZklQS09rRER3MG1BUDQ4enc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBczA+liLqA4gogu92NN6vkUk5MjCJ6b1cgJH1h/gTyexPQO0PepR8jRQ3zVYQy53s7ef7AF3O7PLO83uTQVmt2oKtIHgsnDGsCsBUzAwoDy+GBsiEhZoQksLx3MMACAAAQAMn9KpZ/C1iDgG9tr9OwYolBxCyT50f/6cLnBLmVY96mpFlLoiJ/tH+W6oQ3MhALp/////8AAAABAAAAAAAAAAAAAAABAAABEEdl5fNWp8mF/ssMLX9OaHtNzs2wpaoz1Jr/YjceckDJEtFWVOJYWMYJX2rm1lOu8MthVNyqj2pGyFUZ3a+VSUtSxyhvZDpcVmK5g1hfCeA2c5uwX6NmL+cH6ix6WlbT/Vc5XdYY/XRNX0r+bQh8/htRW4e8C18x9msOlRb+sk1CxUIanVBNhfhS15rGL2yRvLPhtm4cTSrc/bL31whMnPkm/7ndyoI8ATcDB1I+8A4Sxpva0/yCwjLhwrvjM/ZZ9raHI1FlykKGWByuS7+Osig7JYaiYWO33ABgDDAZQxQUq3mIv4JEDILh8Q9ewGgA5bORrBKg0SMpbo941PxPHmLZ38p/lOwCsbCjcGZFiJrg7ea15dy5meSwAIVvXvjwoABnMGUCMCuV93QzKQHAQVsmltZSL3nOK7jh/lEyxjwH/kp5AN3We8miX8htmwDQ3a9lyRQHAwIxAKU2Kc2FK77XpXRIZF/D0bfV31yxvJUaSdSE1rwSaNEkVmSlxQXbG3xiiaAdonD2bQ==","status":null},{"id":"msg_41098d28650c52f4a62559e8d93718da","content":[{"annotations":[],"text":"Visit **El Retiro Park**, a beautiful central Madrid park with shaded paths, gardens, and a lake where you can rent a rowboat. Itâ€™s ideal for a relaxed stroll or picnic.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[],"top_p":0.98,"background":false,"completed_at":1791161629.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_diagnostics":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":44,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":64,"output_tokens_details":{"reasoning_tokens":19},"total_tokens":108},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJQ
	raw_inputD
B[{"content":"Suggest one park to visit in Madrid.","role":"user"}]J
eventsö
ó[{"event.name":"gen_ai.system.message","content":"You are a concise travel assistant. Use the tools for weather and bookings instead of guessing, and answer in at most three sentences.","role":"system"},{"event.name":"gen_ai.user.message","content":"Suggest one park to visit in Madrid.","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_9941b39f16d65a8e81c0c5c5f8037e24","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_rru6BQHgUc/VWS5vRIAlHwIFeJWhGHgqIF/kiQSk0kamBdNUKpC32hrRt9xDrVNBycodAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBdFI5ZWR5OGZDb1o5U2VZL0RtTTFlMTlSbmRuS0hPYmZaSnBJbHBhZHVEWHFGMDN5ZklQS09rRER3MG1BUDQ4enc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBczA+liLqA4gogu92NN6vkUk5MjCJ6b1cgJH1h/gTyexPQO0PepR8jRQ3zVYQy53s7ef7AF3O7PLO83uTQVmt2oKtIHgsnDGsCsBUzAwoDy+GBsiEhZoQksLx3MMACAAAQAMn9KpZ/C1iDgG9tr9OwYolBxCyT50f/6cLnBLmVY96mpFlLoiJ/tH+W6oQ3MhALp/////8AAAABAAAAAAAAAAAAAAABAAABEEdl5fNWp8mF/ssMLX9OaHtNzs2wpaoz1Jr/YjceckDJEtFWVOJYWMYJX2rm1lOu8MthVNyqj2pGyFUZ3a+VSUtSxyhvZDpcVmK5g1hfCeA2c5uwX6NmL+cH6ix6WlbT/Vc5XdYY/XRNX0r+bQh8/htRW4e8C18x9msOlRb+sk1CxUIanVBNhfhS15rGL2yRvLPhtm4cTSrc/bL31whMnPkm/7ndyoI8ATcDB1I+8A4Sxpva0/yCwjLhwrvjM/ZZ9raHI1FlykKGWByuS7+Osig7JYaiYWO33ABgDDAZQxQUq3mIv4JEDILh8Q9ewGgA5bORrBKg0SMpbo941PxPHmLZ38p/lOwCsbCjcGZFiJrg7ea15dy5meSwAIVvXvjwoABnMGUCMCuV93QzKQHAQVsmltZSL3nOK7jh/lEyxjwH/kp5AN3We8miX8htmwDQ3a9lyRQHAwIxAKU2Kc2FK77XpXRIZF/D0bfV31yxvJUaSdSE1rwSaNEkVmSlxQXbG3xiiaAdonD2bQ==","status":null}},{"event.name":"gen_ai.assistant.message","content":"Visit **El Retiro Park**, a beautiful central Madrid park with shaded paths, gardens, and a lake where you can rent a rowboat. Itâ€™s ideal for a relaxed stroll or picnic.","role":"assistant"}]J
gen_ai.usage.input_tokens,J 
gen_ai.usage.output_tokens@JÃ
gen_ai.usage.raw®
«{"input_tokens":44,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":64,"output_tokens_details":{"reasoning_tokens":19},"total_tokens":108}J?
logfire.msg0
.Responses API with 'global.openai.gpt-6.1-sol'JÓ
logfire.json_schema»
¸{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{},"gen_ai.usage.raw":{"type":"object"}}}z …   
¡	…¹YãL‚»Î$B~6£)éñ[,Ö"¥4ˆ®*¯Ô*Turn {turn} for agent assistant09@Éµ|ÛAĞ£*¶|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ9
logfire.msg_template!
Turn {turn} for agent assistantJ
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
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
	assistantJf
usage]
[{"input_tokens":44,"output_tokens":64,"cached_input_tokens":0,"cache_write_input_tokens":0}J+
logfire.msg
Turn 1 for agent assistantJ©
logfire.json_schema‘
{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z …   ê
¡	…¹YãL‚»Î$B~6¥4ˆ®*¯Ô"iÂ¨ñz8®ã*Agent run: {name!r}09eÇµ|ÛA˜¾.¶|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ
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
Agent run: 'assistant'J
logfire.json_schema†
ƒ{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z …   Û
¡	…¹YãL‚»Î$B~6iÂ¨ñz8®ã"Õ™–šß½*Task: Agent workflow09°TÂµ|ÛAĞ0¶|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ.
logfire.msg_template
Task: Agent workflowJ
logfire.span_type
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ
type
customJ
name
Agent workflowJ
gen_ai.system
openaiJ
sdk_span_type
taskJ†
usage}
{{"input_tokens":44,"output_tokens":64,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":1,"total_tokens":108}J%
logfire.msg
Task: Agent workflowJ
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z …   ¼
¡	…¹YãL‚»Î$B~6Õ™–šß½"Âø°ÕÈ ÿ…*OpenAI Agents trace: {name}09hõ¾µ|ÛA™1¶|ÛJ'
code.filepath
scenarios/session.pyJ
code.function
runJ
code.linenoJ
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
spanJ%

session.id
openai-agents-sessionJ
user.id
example-userJ:
agent_trace_id(
&trace_667964ca02404a53ac343da261860442J©
logfire.json_schema‘
{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z …   ©

sideseat2.0.0È
¡	…(Â¨æh¦ÈÚ»ÀiŒsó8z**session-turn-109à!­|ÛA¸ ´µ|ÛJ
logfire.span_type
spanJ
logfire.msg
session-turn-1J%

session.id
openai-agents-sessionJ
user.id
example-userz …   È
¡	…¹YãL‚»Î$B~6Âø°ÕÈ ÿ…*session-turn-209ø¶µ|ÛAÈ4¶|ÛJ
logfire.span_type
spanJ
logfire.msg
session-turn-2J%

session.id
openai-agents-sessionJ
user.id
example-userz …   