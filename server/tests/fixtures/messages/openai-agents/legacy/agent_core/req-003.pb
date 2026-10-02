
ü∏
¡
!
process.runtime.name	
cpython
#
process.runtime.version
3.12.8
U
process.runtime.description6
43.12.8 (main, Dec 19 2024, 14:22:58) [Clang 18.1.8 ]
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
25.5.0
=
service.instance.id&
$82467b55-ca64-4226-9142-b7478e24cb45
=
service.version*
(c40efef761da8af0e63dc5103afd1e0f01d0cc18
"
telemetry.sdk.language
python
%
telemetry.sdk.name
opentelemetry
!
telemetry.sdk.version
1.44.0
"
service.name
telemetry-sample

logfire.version
4.41.0

process.pidΩª‹±

logfire.openai_agents4.41.0€G
†BQÒ´ô£Fe	Pì2∞ ò∫G¬"'&•t>Q÷£*+Responses API with {gen_ai.request.model!r}09hﬂ›—ËõœA Ê|FÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_tllf6gta4ta4fuzcnw4x7r4xfhx3hsf2s67x7j47nv3f4y6tfrnaJ∆
usageº
π{"requests":1,"input_tokens":239,"output_tokens":41,"total_tokens":280,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":16}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJÒ
response‰
·{"id":"resp_tllf6gta4ta4fuzcnw4x7r4xfhx3hsf2s67x7j47nv3f4y6tfrna","created_at":1787819002.0,"error":null,"incomplete_details":null,"instructions":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw=","status":null},{"arguments":"{\"preference_type\":\"favorite_number\"}","call_id":"call_bba135355c155eedbf2f98c7fed08616","name":"retrieve_user_preference","type":"function_call","id":"fc_8e1daad0b6585321922cf80575d84bc9","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"execute_python_code","parameters":{"properties":{"code":{"description":"Python code to execute","title":"Code","type":"string"}},"type":"object","title":"execute_python_code_args","additionalProperties":false,"required":["code"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Execute Python code and return the result.","output_schema":null},{"name":"retrieve_user_preference","parameters":{"properties":{"preference_type":{"description":"Type of preference to retrieve (e.g., 'favorite_number', 'name')","title":"Preference Type","type":"string"}},"additionalProperties":false,"type":"object","required":["preference_type"],"title":"retrieve_user_preference_args"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Retrieve a user preference from memory.","output_schema":null},{"name":"store_user_preference","parameters":{"additionalProperties":false,"properties":{"preference_type":{"description":"Type of preference to store","title":"Preference Type","type":"string"},"value":{"description":"The preference value to store","title":"Value","type":"string"}},"title":"store_user_preference_args","type":"object","required":["preference_type","value"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Store a user preference in memory.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819002.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":239,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":41,"output_tokens_details":{"reasoning_tokens":16},"total_tokens":280},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJé
	raw_inputÄ
˝[{"content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"}]J√
events∏
µ[{"event.name":"gen_ai.system.message","content":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","role":"system"},{"event.name":"gen_ai.user.message","content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw=","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_bba135355c155eedbf2f98c7fed08616","type":"function","function":{"name":"retrieve_user_preference","arguments":"{\"preference_type\":\"favorite_number\"}"}}]}]J 
gen_ai.usage.input_tokensÔJ 
gen_ai.usage.output_tokens)J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jö
logfire.json_schemaÇ
ˇ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ∆
†BQÒ´ô£Fe	Pì:‚µ08ˇ"'&•t>Q÷£*Function: {name}09∞µïFÈõœA`§°FÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ"
name
retrieve_user_preferenceJ0
input'
%{"preference_type":"favorite_number"}J
output
7J
mcp_data
nullJ
gen_ai.system
openaiJ3
logfire.msg$
"Function: retrieve_user_preferenceJà
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   ˇ
†BQÒ´ô£Fe	Pì'&•t>Q÷£"êOœØUNy*+Turn {turn} for agent memory_code_assistant09PÃÃ—ËõœA¯¿¶FÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJE
logfire.msg_template-
+Turn {turn} for agent memory_code_assistantJ
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

turnJ%

agent_name
memory_code_assistantJg
usage^
\{"input_tokens":239,"output_tokens":41,"cached_input_tokens":0,"cache_write_input_tokens":0}J7
logfire.msg(
&Turn 1 for agent memory_code_assistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   Ö]
†BQÒ´ô£Fe	Pìb©TUS¿€"”k≠πôÓ*+Responses API with {gen_ai.request.model!r}09¿∞∆FÈõœA@FwÖÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_oxeyj6pjcm52vz23iot7khdhco557xwty57w4skm5nzdjfdpmexaJ∆
usageº
π{"requests":1,"input_tokens":294,"output_tokens":55,"total_tokens":349,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":10}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJí 
responseÖ 
Ç {"id":"resp_oxeyj6pjcm52vz23iot7khdhco557xwty57w4skm5nzdjfdpmexa","created_at":1787819003.0,"error":null,"incomplete_details":null,"instructions":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_8e0e94834542537ab5277e61f66eb19f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJWwLKVwMun+I41Y93S15fDOh1tHECOKdCbLTNdMIHCsAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBc0MwVkFVSWxNbU1mWVlINU1FVHRZYVk5RkVLWXdGcnNIY0ZwWXcrSXVSUExrQ0o3K25MNW14T2thT1ZEQjZGMnc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBco56qabbI4d6zpqUdCUj/KH3TfsO15V4kQWwD0lrKc+5ls0wRpnX+0wjWt1LXn0FXrQnxMuVE3QVZg4l47teZmpV9hZyTHjQUab5TDxHkFDgSkvcRNG7fkEOnTUgCAAAQAD3ebSIBvbx9uJeC3Pc5ZCb+YHHySY79kgLpYLYuvQ1G3oYH60vfAU6FqSf3OgHE8P////8AAAABAAAAAAAAAAAAAAABAAAAb2IEUNLOFNTqmOkZzG4ih3GYRSTWiOPsQ1HbbKcV0ua4sSNcnzMB3/NcLY/4hsbNTIIDPkPkiFgPxLrZusfI+t9zCcho7sdYJdOo65Pmc8o6KPzuKvAbYKiP1POHwCJgTpltEw3LSJQA0/6cAJBXjqes/PXvU5FKTn/fYLxUpCEAZzBlAjEAjJv8GNoOb+jjWdcnG9UQBWtA00aWZq4VfJcPJoYZOaC+USaR+BjNkVr6UbwVRys9AjBaOKPcifseLQhozlIYxa9hUATM+LKEMo5g0um00d7b+2SRv1WQvZvie7SEPEe0z2E=","status":null},{"arguments":"{\"code\":\"favorite_number = 7\\nsquared = favorite_number ** 2\\nassert squared == 49\\nprint(squared)\"}","call_id":"call_5ea33b13c1f058cf99a1e724661cccae","name":"execute_python_code","type":"function_call","id":"fc_ced8b8bea69f54b6a6fb1dbff6733407","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"execute_python_code","parameters":{"required":["code"],"additionalProperties":false,"properties":{"code":{"description":"Python code to execute","title":"Code","type":"string"}},"type":"object","title":"execute_python_code_args"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Execute Python code and return the result.","output_schema":null},{"name":"retrieve_user_preference","parameters":{"required":["preference_type"],"properties":{"preference_type":{"description":"Type of preference to retrieve (e.g., 'favorite_number', 'name')","title":"Preference Type","type":"string"}},"type":"object","title":"retrieve_user_preference_args","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Retrieve a user preference from memory.","output_schema":null},{"name":"store_user_preference","parameters":{"additionalProperties":false,"type":"object","properties":{"preference_type":{"description":"Type of preference to store","title":"Preference Type","type":"string"},"value":{"description":"The preference value to store","title":"Value","type":"string"}},"required":["preference_type","value"],"title":"store_user_preference_args"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Store a user preference in memory.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819004.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":294,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":55,"output_tokens_details":{"reasoning_tokens":10},"total_tokens":349},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJÔ
	raw_input·
ﬁ[{"content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"},{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw="},{"arguments":"{\"preference_type\":\"favorite_number\"}","call_id":"call_bba135355c155eedbf2f98c7fed08616","name":"retrieve_user_preference","type":"function_call","id":"fc_8e1daad0b6585321922cf80575d84bc9","status":"completed"},{"call_id":"call_bba135355c155eedbf2f98c7fed08616","output":"7","type":"function_call_output"}]JÎ
events‡
›[{"event.name":"gen_ai.system.message","content":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","role":"system"},{"event.name":"gen_ai.user.message","content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_bba135355c155eedbf2f98c7fed08616","type":"function","function":{"name":"retrieve_user_preference","arguments":"{\"preference_type\":\"favorite_number\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_bba135355c155eedbf2f98c7fed08616","content":"7","name":"retrieve_user_preference"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_8e0e94834542537ab5277e61f66eb19f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJWwLKVwMun+I41Y93S15fDOh1tHECOKdCbLTNdMIHCsAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBc0MwVkFVSWxNbU1mWVlINU1FVHRZYVk5RkVLWXdGcnNIY0ZwWXcrSXVSUExrQ0o3K25MNW14T2thT1ZEQjZGMnc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBco56qabbI4d6zpqUdCUj/KH3TfsO15V4kQWwD0lrKc+5ls0wRpnX+0wjWt1LXn0FXrQnxMuVE3QVZg4l47teZmpV9hZyTHjQUab5TDxHkFDgSkvcRNG7fkEOnTUgCAAAQAD3ebSIBvbx9uJeC3Pc5ZCb+YHHySY79kgLpYLYuvQ1G3oYH60vfAU6FqSf3OgHE8P////8AAAABAAAAAAAAAAAAAAABAAAAb2IEUNLOFNTqmOkZzG4ih3GYRSTWiOPsQ1HbbKcV0ua4sSNcnzMB3/NcLY/4hsbNTIIDPkPkiFgPxLrZusfI+t9zCcho7sdYJdOo65Pmc8o6KPzuKvAbYKiP1POHwCJgTpltEw3LSJQA0/6cAJBXjqes/PXvU5FKTn/fYLxUpCEAZzBlAjEAjJv8GNoOb+jjWdcnG9UQBWtA00aWZq4VfJcPJoYZOaC+USaR+BjNkVr6UbwVRys9AjBaOKPcifseLQhozlIYxa9hUATM+LKEMo5g0um00d7b+2SRv1WQvZvie7SEPEe0z2E=","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_5ea33b13c1f058cf99a1e724661cccae","type":"function","function":{"name":"execute_python_code","arguments":"{\"code\":\"favorite_number = 7\\nsquared = favorite_number ** 2\\nassert squared == 49\\nprint(squared)\"}"}}]}]J 
gen_ai.usage.input_tokens¶J 
gen_ai.usage.output_tokens7J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jö
logfire.json_schemaÇ
ˇ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ˝
†BQÒ´ô£Fe	Pì÷⁄◊€ΩLÿ"”k≠πôÓ*Function: {name}09‡°îÖÈõœA∞¿•ÖÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
execute_python_codeJo
inputf
d{"code":"favorite_number = 7\nsquared = favorite_number ** 2\nassert squared == 49\nprint(squared)"}J
output
49
J
mcp_data
nullJ
gen_ai.system
openaiJ.
logfire.msg
Function: execute_python_codeJà
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   ˇ
†BQÒ´ô£Fe	Pì”k≠πôÓ"êOœØUNy*+Turn {turn} for agent memory_code_assistant09 ïªFÈõœA¿…™ÖÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJE
logfire.msg_template-
+Turn {turn} for agent memory_code_assistantJ
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

turnJ%

agent_name
memory_code_assistantJg
usage^
\{"input_tokens":294,"output_tokens":55,"cached_input_tokens":0,"cache_write_input_tokens":0}J7
logfire.msg(
&Turn 2 for agent memory_code_assistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ÿa
†BQÒ´ô£Fe	PìÕWKè¶ÎÈ‰"‹`µÏØ*+Responses API with {gen_ai.request.model!r}09 |—ÖÈõœAÏ\∑ÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_reotzacb3xuvmorr2ws5jojiu3rsojet43e4ogj2usvs66uvtetqJ≈
usageª
∏{"requests":1,"input_tokens":363,"output_tokens":21,"total_tokens":384,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ¿
response≥
∞{"id":"resp_reotzacb3xuvmorr2ws5jojiu3rsojet43e4ogj2usvs66uvtetq","created_at":1787819004.0,"error":null,"incomplete_details":null,"instructions":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"msg_632b9800819756e5b079dff6057fa859","content":[{"annotations":[],"text":"Your favorite number is **7**. Squared, it is **49**.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"execute_python_code","parameters":{"properties":{"code":{"description":"Python code to execute","title":"Code","type":"string"}},"title":"execute_python_code_args","additionalProperties":false,"required":["code"],"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Execute Python code and return the result.","output_schema":null},{"name":"retrieve_user_preference","parameters":{"properties":{"preference_type":{"description":"Type of preference to retrieve (e.g., 'favorite_number', 'name')","title":"Preference Type","type":"string"}},"required":["preference_type"],"type":"object","additionalProperties":false,"title":"retrieve_user_preference_args"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Retrieve a user preference from memory.","output_schema":null},{"name":"store_user_preference","parameters":{"required":["preference_type","value"],"title":"store_user_preference_args","additionalProperties":false,"type":"object","properties":{"preference_type":{"description":"Type of preference to store","title":"Preference Type","type":"string"},"value":{"description":"The preference value to store","title":"Value","type":"string"}}},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Store a user preference in memory.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819005.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":363,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":21,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":384},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJÙ
	raw_inputÊ
„[{"content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"},{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw="},{"arguments":"{\"preference_type\":\"favorite_number\"}","call_id":"call_bba135355c155eedbf2f98c7fed08616","name":"retrieve_user_preference","type":"function_call","id":"fc_8e1daad0b6585321922cf80575d84bc9","status":"completed"},{"call_id":"call_bba135355c155eedbf2f98c7fed08616","output":"7","type":"function_call_output"},{"id":"rs_8e0e94834542537ab5277e61f66eb19f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJWwLKVwMun+I41Y93S15fDOh1tHECOKdCbLTNdMIHCsAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBc0MwVkFVSWxNbU1mWVlINU1FVHRZYVk5RkVLWXdGcnNIY0ZwWXcrSXVSUExrQ0o3K25MNW14T2thT1ZEQjZGMnc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBco56qabbI4d6zpqUdCUj/KH3TfsO15V4kQWwD0lrKc+5ls0wRpnX+0wjWt1LXn0FXrQnxMuVE3QVZg4l47teZmpV9hZyTHjQUab5TDxHkFDgSkvcRNG7fkEOnTUgCAAAQAD3ebSIBvbx9uJeC3Pc5ZCb+YHHySY79kgLpYLYuvQ1G3oYH60vfAU6FqSf3OgHE8P////8AAAABAAAAAAAAAAAAAAABAAAAb2IEUNLOFNTqmOkZzG4ih3GYRSTWiOPsQ1HbbKcV0ua4sSNcnzMB3/NcLY/4hsbNTIIDPkPkiFgPxLrZusfI+t9zCcho7sdYJdOo65Pmc8o6KPzuKvAbYKiP1POHwCJgTpltEw3LSJQA0/6cAJBXjqes/PXvU5FKTn/fYLxUpCEAZzBlAjEAjJv8GNoOb+jjWdcnG9UQBWtA00aWZq4VfJcPJoYZOaC+USaR+BjNkVr6UbwVRys9AjBaOKPcifseLQhozlIYxa9hUATM+LKEMo5g0um00d7b+2SRv1WQvZvie7SEPEe0z2E="},{"arguments":"{\"code\":\"favorite_number = 7\\nsquared = favorite_number ** 2\\nassert squared == 49\\nprint(squared)\"}","call_id":"call_5ea33b13c1f058cf99a1e724661cccae","name":"execute_python_code","type":"function_call","id":"fc_ced8b8bea69f54b6a6fb1dbff6733407","status":"completed"},{"call_id":"call_5ea33b13c1f058cf99a1e724661cccae","output":"49\n","type":"function_call_output"}]JË
events›
⁄[{"event.name":"gen_ai.system.message","content":"You are an AI assistant that validates answers through code execution.\nWhen asked about code, algorithms, or calculations, write Python code to verify your answers.\nWhen asked about user preferences or personal information (like favorite numbers, names, etc.),\nalways check your memory first to retrieve any stored information.","role":"system"},{"event.name":"gen_ai.user.message","content":"Calculate my favorite number squared. First retrieve my favorite number using the retrieve_user_preference tool with 'favorite_number' as the argument. Then use the execute_python_code tool to compute and verify the square.","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_839c76a1d3175c00a722fcaa7c8245f7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNzo+cXo8OBIsl0Z3gvuBI9yOl6mmS1n/v/PpQdBgR1tAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBeFYrOXB4eFR4VkZHRG5oL20rN2NsamdTbHJEMTkvWHVJRWNnQThOVWdmbVRhckFIaWdhQWdSaWVzOC9kN1o4L3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcxgff59aX8A3spN+J1banO522c/mVQXXyfNrXm1rKc+5ls0wRpnX+0wjWt1LeB36W6Fj92kV+94AUk16tQbMJSkTI4a3Z5NzomKvMX0SPWqAwd0TpAMiOBvwdBrgCAAAQALWy+n2HtR6+aGdWMT5aUfty3UNJvJpFL2MBqyDqBeuZdbbYLW3IJsWFEH7vz9ICp/////8AAAABAAAAAAAAAAAAAAABAAAAhP1oEESPfEdgL404qNm4QE+DAleJSrSZQnDOL1tlSu2w7CtDEw/2PTt0uz6c4anVF8RNS6QRHWadv2sGh6Q6+KvbkctbPsGIRgj9Zh1EZgqv9WKTY5HE3GLDfP1Zt/l4hNBHLKrf9kNNKIKhbPf9qkTirnURm+oOuOC8r6UHKdiSxG9bEW5U8Hu5kPmUDxu/Co3/8PQAZzBlAjEAh0QiMsIbzplSdaQhuNcJnMoNRi5JQeU+yiaxma9yK68M69+qo48j4R4ajgJk7JuOAjAye3IU1pwKFZVaOoNWSDH1GZqsHzdgpjT5GQpF0A/c+aCSVNLBsKcPedojRCpUCrw="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_bba135355c155eedbf2f98c7fed08616","type":"function","function":{"name":"retrieve_user_preference","arguments":"{\"preference_type\":\"favorite_number\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_bba135355c155eedbf2f98c7fed08616","content":"7","name":"retrieve_user_preference"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_8e0e94834542537ab5277e61f66eb19f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJWwLKVwMun+I41Y93S15fDOh1tHECOKdCbLTNdMIHCsAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBc0MwVkFVSWxNbU1mWVlINU1FVHRZYVk5RkVLWXdGcnNIY0ZwWXcrSXVSUExrQ0o3K25MNW14T2thT1ZEQjZGMnc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBco56qabbI4d6zpqUdCUj/KH3TfsO15V4kQWwD0lrKc+5ls0wRpnX+0wjWt1LXn0FXrQnxMuVE3QVZg4l47teZmpV9hZyTHjQUab5TDxHkFDgSkvcRNG7fkEOnTUgCAAAQAD3ebSIBvbx9uJeC3Pc5ZCb+YHHySY79kgLpYLYuvQ1G3oYH60vfAU6FqSf3OgHE8P////8AAAABAAAAAAAAAAAAAAABAAAAb2IEUNLOFNTqmOkZzG4ih3GYRSTWiOPsQ1HbbKcV0ua4sSNcnzMB3/NcLY/4hsbNTIIDPkPkiFgPxLrZusfI+t9zCcho7sdYJdOo65Pmc8o6KPzuKvAbYKiP1POHwCJgTpltEw3LSJQA0/6cAJBXjqes/PXvU5FKTn/fYLxUpCEAZzBlAjEAjJv8GNoOb+jjWdcnG9UQBWtA00aWZq4VfJcPJoYZOaC+USaR+BjNkVr6UbwVRys9AjBaOKPcifseLQhozlIYxa9hUATM+LKEMo5g0um00d7b+2SRv1WQvZvie7SEPEe0z2E="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_5ea33b13c1f058cf99a1e724661cccae","type":"function","function":{"name":"execute_python_code","arguments":"{\"code\":\"favorite_number = 7\\nsquared = favorite_number ** 2\\nassert squared == 49\\nprint(squared)\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_5ea33b13c1f058cf99a1e724661cccae","content":"49\n","name":"execute_python_code"},{"event.name":"gen_ai.assistant.message","content":"Your favorite number is **7**. Squared, it is **49**.","role":"assistant"}]J 
gen_ai.usage.input_tokensÎJ 
gen_ai.usage.output_tokensJ<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jæ
logfire.json_schema¶
£{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ˇ
†BQÒ´ô£Fe	Pì‹`µÏØ"êOœØUNy*+Turn {turn} for agent memory_code_assistant09à◊≈ÖÈõœA@2r∑ÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJE
logfire.msg_template-
+Turn {turn} for agent memory_code_assistantJ
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

turnJ%

agent_name
memory_code_assistantJg
usage^
\{"input_tokens":363,"output_tokens":21,"cached_input_tokens":0,"cache_write_input_tokens":0}J7
logfire.msg(
&Turn 3 for agent memory_code_assistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   â
†BQÒ´ô£Fe	PìêOœØUNy"Óg(›;q*Agent run: {name!r}09®Ç»—ËõœAXòä∑ÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
memory_code_assistantJ
handoffs
[]JU
toolsL
J["execute_python_code","retrieve_user_preference","store_user_preference"]J
output_type
strJ
gen_ai.system
openaiJ3
logfire.msg$
"Agent run: 'memory_code_assistant'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   û
†BQÒ´ô£Fe	PìÓg(›;q"îö5?û*‡*Task: Agent workflow09¯#•—ËõœA¯ç∑ÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJ.
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
taskJä
usageÄ
~{"input_tokens":896,"output_tokens":117,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":3,"total_tokens":1013}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˚
†BQÒ´ô£Fe	Pìîö5?û*‡"π÷H˘äÉ*OpenAI Agents trace: {name}09‡˘Ç—ËõœA@é∑ÈõœJ(
code.filepath
samples/agent_core.pyJ
code.function
runJ
code.linenorJ
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
&trace_7937584346b44b7ca65d1825412e6c2eJ©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   ¯

samples.agent_coreﬂ
†BQÒ´ô£Fe	Pìπ÷H˘äÉ*openai_agents.session09(¢Ë–ËõœAÄëë∑ÈõœJ1

session.id#
!openai-agents-agent_core-f841d130J
user.id
	demo-userJ
logfire.span_type
spanJ&
logfire.msg
openai_agents.sessionz Ö   