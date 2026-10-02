
Ï]
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
$bb617d97-204c-4b3f-a51e-160a09ba0aef
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
process.pid≤∫¨W

logfire.openai_agents4.41.0ÚB
†BP⁄UÄ`hO^>•^ÓBô°~,Àó"(“{7∏Ç·*+Responses API with {gen_ai.request.model!r}09X€_3ÁõœA¿sBÇÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenoÉJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_wm3ucghqabszmyyawx4wvpreparxttmm5ymmps2ow2sqg4ory3waJ«
usageΩ
∫{"requests":1,"input_tokens":124,"output_tokens":103,"total_tokens":227,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":59}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJé
responseÅ
˛{"id":"resp_wm3ucghqabszmyyawx4wvpreparxttmm5ymmps2ow2sqg4ory3wa","created_at":1787818994.0,"error":null,"incomplete_details":null,"instructions":"You are an experienced art critic. Use the read_image tool to load and visually inspect each image. Evaluate based on creativity, composition, and appeal. Your final line must include: FINAL DECISION: [path to best image]","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_e148c009f9145151b36ab8764075624f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeODv5/Kh1xxLPv7r2M8NGjW31sqeI+xcQZAXcjNpg50SAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBMmFaRWh2c2VFcXV6dzh4bDNYL1ZvYk1NMXZEYW1yV21EZ2N2T1F4ekIvanJlL2o2M3pWV3NwUnpLdGJmbkRxN3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcVMB14rvs8ijeAhvhyNK7YM/jLifM6OMWBX/cKFrKc+5ls0wRpnX+0wjWt1Kv29iLrutAmA7JP4sQaSl03sakWHpAxoIGIOhj8n6xk55tCFuGoNB+Z9+Mimhz/foCAAAQAL63J68tS+YVHASR4MySbJEuyZ37hlGYv7lCPnJkO4rdNhMWH444J7d+EWfLvUcWmv////8AAAABAAAAAAAAAAAAAAABAAABInSLdj7ptHoQBPIfUALQ2pfgQzv7K7A6do78Ex0+9yP0e94x0yWPWv7CUMUs0IvwlQWlpYkp2n9j69z1A16cd5ZKNGgyWyXjoRLKk9NxkeXSPe+gs3v8rIwuzcHYH7GKmizT4FMeWlfdqf0LAYbsd6zGw62D183IxvXVClMZbLnHV+25kyhsxFpTMc3447mCszPeTkGbgjh17qD/T/APvImG5Bvr8pGyGzS7zkgUCPTz+X0xm99bha2L7QhbXf7fDvLUyD9HMCKWelIhmhSYMvvm8HcEq37udXYNI8a+TvYKXziLqJRUUsbNb8p15TckULiNP0YsszUkkdiatLiz17lWEzulHiCyl0DQM1QhSleqvJ7TCdOM3y2E2zrSXnqbRfoUCaXt7CRdbqedWB8hoiNwTwBnMGUCMB1pEt+fCGuGF4kIMFouMfBmL0kr6Gt6+a/XvIAtqHnubmk31KGIeh4SXPe/I8uMgAIxAIeP+c+JbVzAREEZus5lI8u0AiGvuBzObquCTzAJqxD06dBUURKac+JjIr+sizRIbw==","status":null},{"id":"msg_d9477cefe627508780192fee7fd74293","content":[{"annotations":[],"text":"No images were provided for evaluation; image generation failed because credentials were unavailable. Therefore, I cannot compare creativity, composition, or appeal.\n\nFINAL DECISION: No image available (generation failed)","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"read_image","parameters":{"title":"read_image_args","type":"object","additionalProperties":false,"properties":{"file_path":{"description":"Path to the image file","title":"File Path","type":"string"}},"required":["file_path"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Read an image file for visual inspection.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787818995.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":124,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":103,"output_tokens_details":{"reasoning_tokens":59},"total_tokens":227},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJº
	raw_inputÆ
´[{"content":"Evaluate these generated images and select the best one:\nUnable to generate the images because image-generation credentials are unavailable.","role":"user"}]Jç
eventsÇ
ˇ[{"event.name":"gen_ai.system.message","content":"You are an experienced art critic. Use the read_image tool to load and visually inspect each image. Evaluate based on creativity, composition, and appeal. Your final line must include: FINAL DECISION: [path to best image]","role":"system"},{"event.name":"gen_ai.user.message","content":"Evaluate these generated images and select the best one:\nUnable to generate the images because image-generation credentials are unavailable.","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_e148c009f9145151b36ab8764075624f","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeODv5/Kh1xxLPv7r2M8NGjW31sqeI+xcQZAXcjNpg50SAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBMmFaRWh2c2VFcXV6dzh4bDNYL1ZvYk1NMXZEYW1yV21EZ2N2T1F4ekIvanJlL2o2M3pWV3NwUnpLdGJmbkRxN3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcVMB14rvs8ijeAhvhyNK7YM/jLifM6OMWBX/cKFrKc+5ls0wRpnX+0wjWt1Kv29iLrutAmA7JP4sQaSl03sakWHpAxoIGIOhj8n6xk55tCFuGoNB+Z9+Mimhz/foCAAAQAL63J68tS+YVHASR4MySbJEuyZ37hlGYv7lCPnJkO4rdNhMWH444J7d+EWfLvUcWmv////8AAAABAAAAAAAAAAAAAAABAAABInSLdj7ptHoQBPIfUALQ2pfgQzv7K7A6do78Ex0+9yP0e94x0yWPWv7CUMUs0IvwlQWlpYkp2n9j69z1A16cd5ZKNGgyWyXjoRLKk9NxkeXSPe+gs3v8rIwuzcHYH7GKmizT4FMeWlfdqf0LAYbsd6zGw62D183IxvXVClMZbLnHV+25kyhsxFpTMc3447mCszPeTkGbgjh17qD/T/APvImG5Bvr8pGyGzS7zkgUCPTz+X0xm99bha2L7QhbXf7fDvLUyD9HMCKWelIhmhSYMvvm8HcEq37udXYNI8a+TvYKXziLqJRUUsbNb8p15TckULiNP0YsszUkkdiatLiz17lWEzulHiCyl0DQM1QhSleqvJ7TCdOM3y2E2zrSXnqbRfoUCaXt7CRdbqedWB8hoiNwTwBnMGUCMB1pEt+fCGuGF4kIMFouMfBmL0kr6Gt6+a/XvIAtqHnubmk31KGIeh4SXPe/I8uMgAIxAIeP+c+JbVzAREEZus5lI8u0AiGvuBzObquCTzAJqxD06dBUURKac+JjIr+sizRIbw==","status":null}},{"event.name":"gen_ai.assistant.message","content":"No images were provided for evaluation; image generation failed because credentials were unavailable. Therefore, I cannot compare creativity, composition, or appeal.\n\nFINAL DECISION: No image available (generation failed)","role":"assistant"}]J
gen_ai.usage.input_tokens|J 
gen_ai.usage.output_tokensgJ<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jú
logfire.json_schemaÑ
Å{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ƒ
†BP⁄UÄ`hO^>•^Ó(“{7∏Ç·"ÎáHQeRS**Turn {turn} for agent critic09 ÖT3ÁõœAòOWÇÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenoÉJ6
logfire.msg_template
Turn {turn} for agent criticJ
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

turnJ

agent_name
criticJh
usage_
]{"input_tokens":124,"output_tokens":103,"cached_input_tokens":0,"cache_write_input_tokens":0}J(
logfire.msg
Turn 1 for agent criticJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   Ø
†BP⁄UÄ`hO^>•^ÓÎáHQeRS*"]ˆVÙÈ¬ñ*Agent run: {name!r}09@∂Q3ÁõœA(£ZÇÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenoÉJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
criticJ
handoffs
[]J
tools
["read_image"]J
output_type
strJ
gen_ai.system
openaiJ$
logfire.msg
Agent run: 'critic'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   ú
†BP⁄UÄ`hO^>•^Ó]ˆVÙÈ¬ñ"’üo∂§àp*Task: Agent workflow09®•J3ÁõœA0Ô[ÇÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenoÉJ.
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
}{"input_tokens":124,"output_tokens":103,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":1,"total_tokens":227}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˚
†BP⁄UÄ`hO^>•^Ó’üo∂§àp"tU9™Fëª^*OpenAI Agents trace: {name}09ËYG3ÁõœAÿÕ\ÇÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenoÉJ
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
&trace_7003a7ca0b034195b5c48f022ef122e2J©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   ˆ

samples.image_genﬁ
†BP⁄UÄ`hO^>•^ÓtU9™Fëª^*openai_agents.session09p¨á7ÊõœAPÆ^ÇÁõœJ0

session.id"
 openai-agents-image_gen-39f4d081J
user.id
	demo-userJ
logfire.span_type
spanJ&
logfire.msg
openai_agents.sessionz Ö   