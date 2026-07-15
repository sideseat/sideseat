
π—
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
$ab43331f-f500-422c-88cd-a753594f56b1
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
process.pid¨ºÒÃ

logfire.openai_agents4.41.0˙E
†BQ+ΩD…BÛÕÕÃImÊr‚%j˜Ã•"îÉg∑˝;±ã*+Responses API with {gen_ai.request.model!r}09ÿ∂gÎõœA0èÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_hq2pg7yfgmuojhvp2bgzht5foly2h6lfzpyjgpsk7wkmdllkbk6qJ∆
usageº
π{"requests":1,"input_tokens":224,"output_tokens":52,"total_tokens":276,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":33}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJö 
responseç 
ä {"id":"resp_hq2pg7yfgmuojhvp2bgzht5foly2h6lfzpyjgpsk7wkmdllkbk6q","created_at":1787819012.0,"error":null,"incomplete_details":null,"instructions":"You are a project planner. Your role is to:\n1. Break down complex tasks into steps\n2. Identify which specialist should handle each step\n3. Hand off to the appropriate agent (researcher, coder, reviewer)\n4. Coordinate the overall workflow\n\nStart by handing off to the researcher for initial research.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_533767ec27465a1d8a3616caac2e7047","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeB/B3g3jeRtnruPoCBDwQGk/tE3a8Dna8PLsNiQgKMZ8AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBM3R2VjVVNWdQLzA4OGhYdGVnaDIwL1FoSm9YaWtxb3U5SXlIYkNSUGxzamdFRnIzYTdFbGdQUlRqTHdmbktHS3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBctl1oSA75btolsJXidXX8UYX1NSNxMfWobIuKhlrKc+5ls0wRpnX+0wjWt1LPLJeUEJctU5fmTq6Kb8geg7KWwxwPJvXTpLRGPuQM7jq0ccUD6zRI4mg7mikWuMsCAAAQAJZYAgjpL5xL09LADQini6S+s+B6rOVIa2iIEUMwJZ/m00+wN3oaF4CocC8v1xsxMP////8AAAABAAAAAAAAAAAAAAABAAAAwuQHuPtR2+07vlWe5Dr5g7clFiKCxROJds4YIkq8FyHsswdaHvjjg3p89MwoSwNCEMCBXTRUAsedUEPEnEz9Pw+5ay/T58WrsyvUZYtFzMt8uOl9aarMX6wCQP9CH1+AhzGmwFa9ARYEzKOoootPfMq/U1+fRthsomZgfnBmHDTYuqm833CUNfANgK+aI+GHSQRfbpU56pELgNsMABcl/Dy6ck5NLPENPeI1gjrDwK09g4RAE+Tr9HhMBF+VOuwqFvfl084lGmL+0ErVE4dzBWRl8gBnMGUCMD4SL0xKasXXoRzHr1pgWCjcTnrgztintsbaWAIWhTYRiNbljAQBfPnGJa029E0c0wIxAJHVeoPgM4Tn/eAgDrReCPyQzbuhCSydkeUvuYW1SAcqG2V9n3wrkgoaUEIXEUMa7g==","status":null},{"arguments":"{}","call_id":"call_33f0e1e44dde54948a5a764835e91b8f","name":"transfer_to_researcher","type":"function_call","id":"fc_7380dd6e6d7e54ea88491452c133dc51","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"calculator","parameters":{"additionalProperties":false,"title":"calculator_args","required":["operation","a","b"],"type":"object","properties":{"operation":{"description":"The operation to perform (add, subtract, multiply, divide)","title":"Operation","type":"string"},"a":{"description":"First number","title":"A","type":"number"},"b":{"description":"Second number","title":"B","type":"number"}}},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Perform basic arithmetic operations.","output_schema":null},{"name":"transfer_to_researcher","parameters":{"additionalProperties":false,"properties":{},"required":[],"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Handoff to the researcher agent to handle the request. ","output_schema":null},{"name":"transfer_to_coder","parameters":{"additionalProperties":false,"required":[],"properties":{},"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Handoff to the coder agent to handle the request. ","output_schema":null},{"name":"transfer_to_reviewer","parameters":{"additionalProperties":false,"required":[],"properties":{},"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Handoff to the reviewer agent to handle the request. ","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819012.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":224,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":52,"output_tokens_details":{"reasoning_tokens":33},"total_tokens":276},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJÅ
	raw_inputt
r[{"content":"Create a simple plan to build a weather app that shows forecasts for multiple cities","role":"user"}]J 
eventsø
º[{"event.name":"gen_ai.system.message","content":"You are a project planner. Your role is to:\n1. Break down complex tasks into steps\n2. Identify which specialist should handle each step\n3. Hand off to the appropriate agent (researcher, coder, reviewer)\n4. Coordinate the overall workflow\n\nStart by handing off to the researcher for initial research.","role":"system"},{"event.name":"gen_ai.user.message","content":"Create a simple plan to build a weather app that shows forecasts for multiple cities","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_533767ec27465a1d8a3616caac2e7047","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeB/B3g3jeRtnruPoCBDwQGk/tE3a8Dna8PLsNiQgKMZ8AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBM3R2VjVVNWdQLzA4OGhYdGVnaDIwL1FoSm9YaWtxb3U5SXlIYkNSUGxzamdFRnIzYTdFbGdQUlRqTHdmbktHS3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBctl1oSA75btolsJXidXX8UYX1NSNxMfWobIuKhlrKc+5ls0wRpnX+0wjWt1LPLJeUEJctU5fmTq6Kb8geg7KWwxwPJvXTpLRGPuQM7jq0ccUD6zRI4mg7mikWuMsCAAAQAJZYAgjpL5xL09LADQini6S+s+B6rOVIa2iIEUMwJZ/m00+wN3oaF4CocC8v1xsxMP////8AAAABAAAAAAAAAAAAAAABAAAAwuQHuPtR2+07vlWe5Dr5g7clFiKCxROJds4YIkq8FyHsswdaHvjjg3p89MwoSwNCEMCBXTRUAsedUEPEnEz9Pw+5ay/T58WrsyvUZYtFzMt8uOl9aarMX6wCQP9CH1+AhzGmwFa9ARYEzKOoootPfMq/U1+fRthsomZgfnBmHDTYuqm833CUNfANgK+aI+GHSQRfbpU56pELgNsMABcl/Dy6ck5NLPENPeI1gjrDwK09g4RAE+Tr9HhMBF+VOuwqFvfl084lGmL+0ErVE4dzBWRl8gBnMGUCMD4SL0xKasXXoRzHr1pgWCjcTnrgztintsbaWAIWhTYRiNbljAQBfPnGJa029E0c0wIxAJHVeoPgM4Tn/eAgDrReCPyQzbuhCSydkeUvuYW1SAcqG2V9n3wrkgoaUEIXEUMa7g==","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_33f0e1e44dde54948a5a764835e91b8f","type":"function","function":{"name":"transfer_to_researcher","arguments":"{}"}}]}]J 
gen_ai.usage.input_tokens‡J 
gen_ai.usage.output_tokens4J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jö
logfire.json_schemaÇ
ˇ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   Ç
†BQ+ΩD…BÛÕÕÃImqÅÉ¡∑ÑB^"îÉg∑˝;±ã*$Handoff: {from_agent} ‚Üí {to_agent}09¯ƒHèÎõœAhAMèÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ>
logfire.msg_template&
$Handoff: {from_agent} ‚Üí {to_agent}J
logfire.span_type
spanJ

from_agent	
plannerJ
to_agent

researcherJ
gen_ai.system
openaiJ0
logfire.msg!
Handoff: planner ‚Üí researcherJj
logfire.json_schemaS
Q{"type":"object","properties":{"from_agent":{},"to_agent":{},"gen_ai.system":{}}}z Ö   √
†BQ+ΩD…BÛÕÕÃImîÉg∑˝;±ã"s¢îsí√ﬂÑ*Turn {turn} for agent planner09®èZÎõœAÿ…OèÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ7
logfire.msg_template
Turn {turn} for agent plannerJ
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

turnJ

agent_name	
plannerJg
usage^
\{"input_tokens":224,"output_tokens":52,"cached_input_tokens":0,"cache_write_input_tokens":0}J)
logfire.msg
Turn 1 for agent plannerJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   Ã
†BQ+ΩD…BÛÕÕÃIms¢îsí√ﬂÑ"Ç ¶:1ì|m*Agent run: {name!r}09Ä~WÎõœA0ŸQèÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name	
plannerJ/
handoffs#
!["researcher","coder","reviewer"]J
tools
["calculator"]J
output_type
strJ
gen_ai.system
openaiJ%
logfire.msg
Agent run: 'planner'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   —a
†BQ+ΩD…BÛÕÕÃImëèOãÓO8"TÀÜ¨¶>*+Responses API with {gen_ai.request.model!r}09dèÎõœAò˛ÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_pgfcbxw4ygddfowlyrd4dhnpggt5n4s2pchr5pda4q3ugqoyadgqJ«
usageΩ
∫{"requests":1,"input_tokens":266,"output_tokens":145,"total_tokens":411,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":66}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJò"
responseã"
à"{"id":"resp_pgfcbxw4ygddfowlyrd4dhnpggt5n4s2pchr5pda4q3ugqoyadgq","created_at":1787819013.0,"error":null,"incomplete_details":null,"instructions":"You are a research specialist. Your role is to:\n1. Gather information on topics\n2. Provide factual, well-sourced answers\n3. Hand off to coder if implementation is needed","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_77b8ac3b998d5ed68e2e47e384098b44","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeK/+Pt5GaXQtRp4KpIK2hLEW48vKNMbYz/7QdKqFn6DPAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBanNGZ2E5VDcwT0ZhYjE3UlZHVU5WcGFGVk9valJPUEFOY2k0NFlSUnN4OWluUmIzMDhuZHYxdXV6U0tQSTJ3MkE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcORQbPdEZ5LS4rIok1yDCUW9X4mzd3DJDMrOEQlrKc+5ls0wRpnX+0wjWt1I1CztJ1jvQvPX8DAfew6YhJO6ukMdhm39b7o342nUQPBg2GWzPKtxRHNnlHLM0iBACAAAQANQ+iqFF2ZDjtjXvOGid3Bhcc5tHBRt97uPoCw1VDosY6bmp2R7ePOLahD1lDvMtEv////8AAAABAAAAAAAAAAAAAAABAAABOhFKQ6wlVdxmy3MiZ/NH+xnaySBx5yxe2vjoU66eqw6luLXQ950Nh2FUIX7lPHf+Tybkj1wjANwkEzQnILR8AzNU5tMTyH94BQiDxIpR/Mwh2INd+pRQFGJOU3BnrirEykQM76GYwODGKIeGI0Asw/x1JLoDHOMfGjWd9QNUzrFIoDk6Psu3xfutN/mQ0534ghB2gbATteIqmrT6Tu/b0cErzejRzTR+goDPn+uK+Vh8kQv3GOWv1EZbmqR4EmVWvB7WS/ODMyhLozFhnXNaDk20hFR5/hoC+gSfYehJb9xFmHaY6ACuqNx8EPk7oIq+7s8nM4NYjbQyz4CV+37GbY9LUAS4sKdeCKsIfZQ4HH8vwiUuScJg6l2IVxk5GoihWa9ZWuiEhHUFlGsAS5IWkJ14yvyDSmq9vZVd++68jAFzP8zCNesY4TZBngBnMGUCMQDDbUZPevhu4u86C1yYvoSxrMAfFTRHcFZ6cMHjsTFUY3XX1siq3tQd9d6pNk5ywp8CMGHMKuXBTwtpoCJkxPwWxaltfXkaKmhsc5v4deR8jzS8Hn3vjFyR7fVHS8TZbonC1w==","status":null},{"arguments":"{\"query\":\"Open-Meteo forecast API documentation geocoding API official\",\"max_results\":3}","call_id":"call_f7b176ff07da57218c5c1af3404c82ea","name":"web_search","type":"function_call","id":"fc_92f75eac1a3655efb643fcca24569b5a","caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"query\":\"MDN Fetch API official documentation localStorage official\",\"max_results\":3}","call_id":"call_3dfd279b76b35b578d0c81f998e35812","name":"web_search","type":"function_call","id":"fc_c9b1ee20060f5ce3a472bc6337f66c79","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"web_search","parameters":{"required":["query","max_results"],"type":"object","properties":{"query":{"description":"Search query string","title":"Query","type":"string"},"max_results":{"default":5,"description":"Maximum number of results to return","title":"Max Results","type":"integer"}},"title":"web_search_args","additionalProperties":false},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Search the web and return results.","output_schema":null},{"name":"weather_forecast","parameters":{"title":"weather_forecast_args","additionalProperties":false,"type":"object","properties":{"city":{"description":"The name of the city","title":"City","type":"string"},"days":{"default":3,"description":"Number of days for the forecast","title":"Days","type":"integer"}},"required":["city","days"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Get weather forecast for a city.","output_schema":null},{"name":"transfer_to_coder","parameters":{"type":"object","required":[],"additionalProperties":false,"properties":{}},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Handoff to the coder agent to handle the request. ","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819014.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":266,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":145,"output_tokens_details":{"reasoning_tokens":66},"total_tokens":411},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ≠
	raw_inputü
ú[{"content":"Create a simple plan to build a weather app that shows forecasts for multiple cities","role":"user"},{"id":"rs_533767ec27465a1d8a3616caac2e7047","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeB/B3g3jeRtnruPoCBDwQGk/tE3a8Dna8PLsNiQgKMZ8AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBM3R2VjVVNWdQLzA4OGhYdGVnaDIwL1FoSm9YaWtxb3U5SXlIYkNSUGxzamdFRnIzYTdFbGdQUlRqTHdmbktHS3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBctl1oSA75btolsJXidXX8UYX1NSNxMfWobIuKhlrKc+5ls0wRpnX+0wjWt1LPLJeUEJctU5fmTq6Kb8geg7KWwxwPJvXTpLRGPuQM7jq0ccUD6zRI4mg7mikWuMsCAAAQAJZYAgjpL5xL09LADQini6S+s+B6rOVIa2iIEUMwJZ/m00+wN3oaF4CocC8v1xsxMP////8AAAABAAAAAAAAAAAAAAABAAAAwuQHuPtR2+07vlWe5Dr5g7clFiKCxROJds4YIkq8FyHsswdaHvjjg3p89MwoSwNCEMCBXTRUAsedUEPEnEz9Pw+5ay/T58WrsyvUZYtFzMt8uOl9aarMX6wCQP9CH1+AhzGmwFa9ARYEzKOoootPfMq/U1+fRthsomZgfnBmHDTYuqm833CUNfANgK+aI+GHSQRfbpU56pELgNsMABcl/Dy6ck5NLPENPeI1gjrDwK09g4RAE+Tr9HhMBF+VOuwqFvfl084lGmL+0ErVE4dzBWRl8gBnMGUCMD4SL0xKasXXoRzHr1pgWCjcTnrgztintsbaWAIWhTYRiNbljAQBfPnGJa029E0c0wIxAJHVeoPgM4Tn/eAgDrReCPyQzbuhCSydkeUvuYW1SAcqG2V9n3wrkgoaUEIXEUMa7g=="},{"arguments":"{}","call_id":"call_33f0e1e44dde54948a5a764835e91b8f","name":"transfer_to_researcher","type":"function_call","id":"fc_7380dd6e6d7e54ea88491452c133dc51","status":"completed"},{"call_id":"call_33f0e1e44dde54948a5a764835e91b8f","output":"{\"assistant\": \"researcher\"}","type":"function_call_output"}]Jú
eventsë
é[{"event.name":"gen_ai.system.message","content":"You are a research specialist. Your role is to:\n1. Gather information on topics\n2. Provide factual, well-sourced answers\n3. Hand off to coder if implementation is needed","role":"system"},{"event.name":"gen_ai.user.message","content":"Create a simple plan to build a weather app that shows forecasts for multiple cities","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_533767ec27465a1d8a3616caac2e7047","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeB/B3g3jeRtnruPoCBDwQGk/tE3a8Dna8PLsNiQgKMZ8AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBM3R2VjVVNWdQLzA4OGhYdGVnaDIwL1FoSm9YaWtxb3U5SXlIYkNSUGxzamdFRnIzYTdFbGdQUlRqTHdmbktHS3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBctl1oSA75btolsJXidXX8UYX1NSNxMfWobIuKhlrKc+5ls0wRpnX+0wjWt1LPLJeUEJctU5fmTq6Kb8geg7KWwxwPJvXTpLRGPuQM7jq0ccUD6zRI4mg7mikWuMsCAAAQAJZYAgjpL5xL09LADQini6S+s+B6rOVIa2iIEUMwJZ/m00+wN3oaF4CocC8v1xsxMP////8AAAABAAAAAAAAAAAAAAABAAAAwuQHuPtR2+07vlWe5Dr5g7clFiKCxROJds4YIkq8FyHsswdaHvjjg3p89MwoSwNCEMCBXTRUAsedUEPEnEz9Pw+5ay/T58WrsyvUZYtFzMt8uOl9aarMX6wCQP9CH1+AhzGmwFa9ARYEzKOoootPfMq/U1+fRthsomZgfnBmHDTYuqm833CUNfANgK+aI+GHSQRfbpU56pELgNsMABcl/Dy6ck5NLPENPeI1gjrDwK09g4RAE+Tr9HhMBF+VOuwqFvfl084lGmL+0ErVE4dzBWRl8gBnMGUCMD4SL0xKasXXoRzHr1pgWCjcTnrgztintsbaWAIWhTYRiNbljAQBfPnGJa029E0c0wIxAJHVeoPgM4Tn/eAgDrReCPyQzbuhCSydkeUvuYW1SAcqG2V9n3wrkgoaUEIXEUMa7g=="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_33f0e1e44dde54948a5a764835e91b8f","type":"function","function":{"name":"transfer_to_researcher","arguments":"{}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_33f0e1e44dde54948a5a764835e91b8f","content":"{\"assistant\": \"researcher\"}","name":"transfer_to_researcher"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_77b8ac3b998d5ed68e2e47e384098b44","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeK/+Pt5GaXQtRp4KpIK2hLEW48vKNMbYz/7QdKqFn6DPAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBanNGZ2E5VDcwT0ZhYjE3UlZHVU5WcGFGVk9valJPUEFOY2k0NFlSUnN4OWluUmIzMDhuZHYxdXV6U0tQSTJ3MkE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcORQbPdEZ5LS4rIok1yDCUW9X4mzd3DJDMrOEQlrKc+5ls0wRpnX+0wjWt1I1CztJ1jvQvPX8DAfew6YhJO6ukMdhm39b7o342nUQPBg2GWzPKtxRHNnlHLM0iBACAAAQANQ+iqFF2ZDjtjXvOGid3Bhcc5tHBRt97uPoCw1VDosY6bmp2R7ePOLahD1lDvMtEv////8AAAABAAAAAAAAAAAAAAABAAABOhFKQ6wlVdxmy3MiZ/NH+xnaySBx5yxe2vjoU66eqw6luLXQ950Nh2FUIX7lPHf+Tybkj1wjANwkEzQnILR8AzNU5tMTyH94BQiDxIpR/Mwh2INd+pRQFGJOU3BnrirEykQM76GYwODGKIeGI0Asw/x1JLoDHOMfGjWd9QNUzrFIoDk6Psu3xfutN/mQ0534ghB2gbATteIqmrT6Tu/b0cErzejRzTR+goDPn+uK+Vh8kQv3GOWv1EZbmqR4EmVWvB7WS/ODMyhLozFhnXNaDk20hFR5/hoC+gSfYehJb9xFmHaY6ACuqNx8EPk7oIq+7s8nM4NYjbQyz4CV+37GbY9LUAS4sKdeCKsIfZQ4HH8vwiUuScJg6l2IVxk5GoihWa9ZWuiEhHUFlGsAS5IWkJ14yvyDSmq9vZVd++68jAFzP8zCNesY4TZBngBnMGUCMQDDbUZPevhu4u86C1yYvoSxrMAfFTRHcFZ6cMHjsTFUY3XX1siq3tQd9d6pNk5ywp8CMGHMKuXBTwtpoCJkxPwWxaltfXkaKmhsc5v4deR8jzS8Hn3vjFyR7fVHS8TZbonC1w==","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_f7b176ff07da57218c5c1af3404c82ea","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"Open-Meteo forecast API documentation geocoding API official\",\"max_results\":3}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_3dfd279b76b35b578d0c81f998e35812","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"MDN Fetch API official documentation localStorage official\",\"max_results\":3}"}}]}]J 
gen_ai.usage.input_tokensäJ!
gen_ai.usage.output_tokensëJ<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'JÛ
logfire.json_schema€
ÿ{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   À
†BQ+ΩD…BÛÕÕÃImö@kED€ö"TÀÜ¨¶>*Function: {name}09h}˛ÎõœAx8,˛ÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name

web_searchJc
inputZ
X{"query":"Open-Meteo forecast API documentation geocoding API official","max_results":3}JÓ
output„
‡{"status":"success","content":[{"json":{"query":"Open-Meteo forecast API documentation geocoding API official","results":[{"title":"Result 1 for 'Open-Meteo forecast API documentation geocoding API official'","url":"https://example.com/0"},{"title":"Result 2 for 'Open-Meteo forecast API documentation geocoding API official'","url":"https://example.com/1"},{"title":"Result 3 for 'Open-Meteo forecast API documentation geocoding API official'","url":"https://example.com/2"}]}}]}J
mcp_data
nullJ
gen_ai.system
openaiJ%
logfire.msg
Function: web_searchJò
logfire.json_schemaÄ
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   ¡
†BQ+ΩD…BÛÕÕÃImÀ∫(qô"TÀÜ¨¶>*Function: {name}09ò]˛ÎõœA¯j.˛ÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name

web_searchJa
inputX
V{"query":"MDN Fetch API official documentation localStorage official","max_results":3}JÊ
output€
ÿ{"status":"success","content":[{"json":{"query":"MDN Fetch API official documentation localStorage official","results":[{"title":"Result 1 for 'MDN Fetch API official documentation localStorage official'","url":"https://example.com/0"},{"title":"Result 2 for 'MDN Fetch API official documentation localStorage official'","url":"https://example.com/1"},{"title":"Result 3 for 'MDN Fetch API official documentation localStorage official'","url":"https://example.com/2"}]}}]}J
mcp_data
nullJ
gen_ai.system
openaiJ%
logfire.msg
Function: web_searchJò
logfire.json_schemaÄ
~{"type":"object","properties":{"name":{},"input":{},"output":{"type":"object"},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   –
†BQ+ΩD…BÛÕÕÃImTÀÜ¨¶>"ú ¿RÜc* Turn {turn} for agent researcher09`ßXèÎõœA˙2˛ÎõœJ#
code.filepath
samples/swarm.pyJ
code.function
runJ
code.linenoöJ:
logfire.msg_template"
 Turn {turn} for agent researcherJ
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

turnJ

agent_name

researcherJh
usage_
]{"input_tokens":266,"output_tokens":145,"cached_input_tokens":0,"cache_write_input_tokens":0}J,
logfire.msg
Turn 2 for agent researcherJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   