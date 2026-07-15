
∞Â
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
process.pid≤∫îﬁ

logfire.openai_agents4.41.0·r
†BP⁄UÄ`hO^>•^Ó≠ﬁ7å∑◊"n¯›zm
G†*+Responses API with {gen_ai.request.model!r}09(ù”ÒÊõœA®∫3ÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_fxhmkm76cofos7kcat6qfl6ip454yep5hpjrszjmajtjejvz7iyaJ∆
usageº
π{"requests":1,"input_tokens":547,"output_tokens":50,"total_tokens":597,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":32}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJÊ
responseŸ
÷{"id": "resp_fxhmkm76cofos7kcat6qfl6ip454yep5hpjrszjmajtjejvz7iya", "created_at": 1787818993.0, "error": null, "incomplete_details": null, "instructions": "You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.", "metadata": {}, "model": "us.openai.gpt-5.6-luna", "object": "response", "output": [{"id": "rs_67b578f5755259038eedeffb802c5c7b", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeJhuP4BIC68M5jqt27gqM97i0B7+RBPvYzc0eNPJC2RtAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBOW5OMjNMRDY4SVRPVWp6QUUzd1R0Q1dMdmREZ2pBQU02Qk9ybU1ROTZXdlhERU84bHdZcUZRbDVOSzBjRlJXcEE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcb/f77IWYTbNnxz/pPdg4PZW+GhIzdp9UC9tCuFrKc+5ls0wRpnX+0wjWt1JcFrVkMFhr1VYZbJYUF24J36JxXNvbPDJru6tkf4JuspBfaoOc/o9TijbMe+DOIlQCAAAQABvZ5fn19KFWhPqhCV0weGliHstIynIyHXX9UchPyluMUO76nmPmAH4OLlMkLYvm2/////8AAAABAAAAAAAAAAAAAAABAAAAxhr0Xp4mRdt6dqeEpyMSe43F5PVuThjGw0jGKXtDYStZGIjsKt6ipvZcX2yhE0o+fQPBX5k1SFIRCLalEOoSSe+ohK9/8T40x/99LNbK67cH8rGFrNhnCQx2DWoNa+6vP/HeU3X9LIFKtf8Yf9K1AX1bcaC72wHp3LXvaQQBii36qLc/OwuT/zrTWT0M681AxVeXMDxRA0tarpE9MJBbDZcQV0by1JBeeooqay8R3ZpKdM11s/5YyQq7irTwfJR+YLZrmUxF4MRc0TKjMKgAphPw56sH4OcAZzBlAjEAzyyMNWlgHdNTawzV+i5kUtoMwVeBpCH6oZ63PJdAeiJwXDVGWJdmosZJtRLn4FpNAjBA596brCq04jHdKvVGWLkzw1s+CXxrhE7pnt+pqZ+d2fnWI8Syf0Yg4ADzq+yhGtE=", "status": null}, {"id": "msg_fdc4dd7a01ae5531954d0b01d53e159d", "content": [{"annotations": [], "text": "[Scrubbed due to 'credential']", "type": "output_text", "logprobs": []}], "role": "assistant", "status": "completed", "type": "message", "phase": "final_answer"}], "parallel_tool_calls": true, "temperature": 1.0, "tool_choice": "auto", "tools": [{"name": "generate_image", "parameters": {"additionalProperties": false, "title": "generate_image_args", "required": ["prompt"], "properties": {"prompt": {"description": "The image description prompt", "title": "Prompt", "type": "string"}}, "type": "object"}, "strict": true, "type": "function", "allowed_callers": null, "defer_loading": null, "description": "Generate an image using OpenAI DALL-E.", "output_schema": null}], "top_p": 0.98, "background": false, "completed_at": 1787818994.0, "conversation": null, "max_output_tokens": null, "max_tool_calls": null, "moderation": null, "previous_response_id": null, "prompt": null, "prompt_cache_key": null, "prompt_cache_options": null, "prompt_cache_retention": "in_memory", "reasoning": {"context": "all_turns", "effort": "medium", "generate_summary": null, "mode": null, "summary": null}, "safety_identifier": null, "service_tier": "default", "status": "completed", "text": {"format": {"type": "text"}, "verbosity": "medium"}, "top_logprobs": 0, "truncation": "disabled", "usage": {"input_tokens": 547, "input_tokens_details": {"cache_write_tokens": 0, "cached_tokens": 0}, "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 32}, "total_tokens": 597}, "user": null, "billing": {"payer": "developer"}, "frequency_penalty": 0.0, "presence_penalty": 0.0, "store": true}J
gen_ai.operation.name
chatJú
	raw_inputé
ã[{"content": "Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.", "role": "user"}, {"id": "rs_8289ae870bd457a0b8b3121f0ede4fa7", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv"}, {"arguments": "{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}", "call_id": "call_b927172290845e3f82cdd316c2f64df1", "name": "generate_image", "type": "function_call", "id": "fc_c2db173d5f5a5076a83d14ac999c11e9", "status": "completed"}, {"arguments": "{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}", "call_id": "call_cd488973791e5791b0a0df6b09b83697", "name": "generate_image", "type": "function_call", "id": "fc_2f8b45d1f24f5afbb551afe0f2e890ea", "status": "completed"}, {"arguments": "{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}", "call_id": "call_917368052e0a580e855a2d3aa0b20d2b", "name": "generate_image", "type": "function_call", "id": "fc_07f9fdd9b4ac512483b686c6a6399ae6", "status": "completed"}, {"call_id": "call_b927172290845e3f82cdd316c2f64df1", "output": "[Scrubbed due to 'credential']", "type": "function_call_output"}, {"call_id": "call_cd488973791e5791b0a0df6b09b83697", "output": "[Scrubbed due to 'credential']", "type": "function_call_output"}, {"call_id": "call_917368052e0a580e855a2d3aa0b20d2b", "output": "[Scrubbed due to 'credential']", "type": "function_call_output"}]J‰%
eventsŸ%
÷%[{"event.name": "gen_ai.system.message", "content": "You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.", "role": "system"}, {"event.name": "gen_ai.user.message", "content": "Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.", "role": "user"}, {"event.name": "gen_ai.unknown", "role": "unknown", "content": "reasoning\n\nSee JSON for details", "data": {"id": "rs_8289ae870bd457a0b8b3121f0ede4fa7", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv"}}, {"event.name": "gen_ai.assistant.message", "role": "assistant", "tool_calls": [{"id": "call_b927172290845e3f82cdd316c2f64df1", "type": "function", "function": {"name": "generate_image", "arguments": "{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}"}}]}, {"event.name": "gen_ai.assistant.message", "role": "assistant", "tool_calls": [{"id": "call_cd488973791e5791b0a0df6b09b83697", "type": "function", "function": {"name": "generate_image", "arguments": "{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}"}}]}, {"event.name": "gen_ai.assistant.message", "role": "assistant", "tool_calls": [{"id": "call_917368052e0a580e855a2d3aa0b20d2b", "type": "function", "function": {"name": "generate_image", "arguments": "{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}"}}]}, {"event.name": "gen_ai.tool.message", "role": "tool", "id": "call_b927172290845e3f82cdd316c2f64df1", "content": "[Scrubbed due to 'credential']", "name": "generate_image"}, {"event.name": "gen_ai.tool.message", "role": "tool", "id": "call_cd488973791e5791b0a0df6b09b83697", "content": "[Scrubbed due to 'credential']", "name": "generate_image"}, {"event.name": "gen_ai.tool.message", "role": "tool", "id": "call_917368052e0a580e855a2d3aa0b20d2b", "content": "[Scrubbed due to 'credential']", "name": "generate_image"}, {"event.name": "gen_ai.unknown", "role": "assistant", "content": "reasoning\n\nSee JSON for details", "data": {"id": "rs_67b578f5755259038eedeffb802c5c7b", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeJhuP4BIC68M5jqt27gqM97i0B7+RBPvYzc0eNPJC2RtAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBOW5OMjNMRDY4SVRPVWp6QUUzd1R0Q1dMdmREZ2pBQU02Qk9ybU1ROTZXdlhERU84bHdZcUZRbDVOSzBjRlJXcEE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcb/f77IWYTbNnxz/pPdg4PZW+GhIzdp9UC9tCuFrKc+5ls0wRpnX+0wjWt1JcFrVkMFhr1VYZbJYUF24J36JxXNvbPDJru6tkf4JuspBfaoOc/o9TijbMe+DOIlQCAAAQABvZ5fn19KFWhPqhCV0weGliHstIynIyHXX9UchPyluMUO76nmPmAH4OLlMkLYvm2/////8AAAABAAAAAAAAAAAAAAABAAAAxhr0Xp4mRdt6dqeEpyMSe43F5PVuThjGw0jGKXtDYStZGIjsKt6ipvZcX2yhE0o+fQPBX5k1SFIRCLalEOoSSe+ohK9/8T40x/99LNbK67cH8rGFrNhnCQx2DWoNa+6vP/HeU3X9LIFKtf8Yf9K1AX1bcaC72wHp3LXvaQQBii36qLc/OwuT/zrTWT0M681AxVeXMDxRA0tarpE9MJBbDZcQV0by1JBeeooqay8R3ZpKdM11s/5YyQq7irTwfJR+YLZrmUxF4MRc0TKjMKgAphPw56sH4OcAZzBlAjEAzyyMNWlgHdNTawzV+i5kUtoMwVeBpCH6oZ63PJdAeiJwXDVGWJdmosZJtRLn4FpNAjBA596brCq04jHdKvVGWLkzw1s+CXxrhE7pnt+pqZ+d2fnWI8Syf0Yg4ADzq+yhGtE=", "status": null}}, {"event.name": "gen_ai.assistant.message", "content": "[Scrubbed due to 'credential']", "role": "assistant"}]J 
gen_ai.usage.input_tokens£J 
gen_ai.usage.output_tokens2J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jú
logfire.json_schemaÑ
Å{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}Jﬁ
logfire.scrubbed…
∆[{"path": ["attributes", "response", "output", 1, "content", 0, "text"], "matched_substring": "credential"}, {"path": ["attributes", "raw_input", 5, "output"], "matched_substring": "credential"}, {"path": ["attributes", "raw_input", 6, "output"], "matched_substring": "credential"}, {"path": ["attributes", "raw_input", 7, "output"], "matched_substring": "credential"}, {"path": ["attributes", "events", 6, "content"], "matched_substring": "credential"}, {"path": ["attributes", "events", 7, "content"], "matched_substring": "credential"}, {"path": ["attributes", "events", 8, "content"], "matched_substring": "credential"}, {"path": ["attributes", "events", 10, "content"], "matched_substring": "credential"}]z Ö   ¬
†BP⁄UÄ`hO^>•^Ón¯›zm
G†"¯n™Ùﬂì™µ*Turn {turn} for agent artist09 9…ÒÊõœA»Ó63ÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ6
logfire.msg_template
Turn {turn} for agent artistJ
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

turnJ

agent_name
artistJg
usage^
\{"input_tokens":547,"output_tokens":50,"cached_input_tokens":0,"cache_write_input_tokens":0}J(
logfire.msg
Turn 2 for agent artistJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ≤
†BP⁄UÄ`hO^>•^Ó¯n™Ùﬂì™µ"1˛ê±áOÈ*Agent run: {name!r}09P,68ÊõœA∞‘;3ÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
artistJ
handoffs
[]J
tools
["generate_image"]J
output_type
strJ
gen_ai.system
openaiJ$
logfire.msg
Agent run: 'artist'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   õ
†BP⁄UÄ`hO^>•^Ó1˛ê±áOÈ"î?∂óã*Task: Agent workflow09P8ÊõœAP[=3ÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ.
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
}{"input_tokens":685,"output_tokens":271,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":2,"total_tokens":956}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˙
†BP⁄UÄ`hO^>•^Óî?∂óã"tU9™Fëª^*OpenAI Agents trace: {name}09òÌˇ7ÊõœAÄM>3ÁõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ
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
&trace_a86379c7206c4cb48771c3bb8e391a59J©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   ·B
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
us.openai.gpt-5.6-lunaJÔ
response‚
ﬂ{"id": "resp_wm3ucghqabszmyyawx4wvpreparxttmm5ymmps2ow2sqg4ory3wa", "created_at": 1787818994.0, "error": null, "incomplete_details": null, "instructions": "You are an experienced art critic. Use the read_image tool to load and visually inspect each image. Evaluate based on creativity, composition, and appeal. Your final line must include: FINAL DECISION: [path to best image]", "metadata": {}, "model": "us.openai.gpt-5.6-luna", "object": "response", "output": [{"id": "rs_e148c009f9145151b36ab8764075624f", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeODv5/Kh1xxLPv7r2M8NGjW31sqeI+xcQZAXcjNpg50SAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBMmFaRWh2c2VFcXV6dzh4bDNYL1ZvYk1NMXZEYW1yV21EZ2N2T1F4ekIvanJlL2o2M3pWV3NwUnpLdGJmbkRxN3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcVMB14rvs8ijeAhvhyNK7YM/jLifM6OMWBX/cKFrKc+5ls0wRpnX+0wjWt1Kv29iLrutAmA7JP4sQaSl03sakWHpAxoIGIOhj8n6xk55tCFuGoNB+Z9+Mimhz/foCAAAQAL63J68tS+YVHASR4MySbJEuyZ37hlGYv7lCPnJkO4rdNhMWH444J7d+EWfLvUcWmv////8AAAABAAAAAAAAAAAAAAABAAABInSLdj7ptHoQBPIfUALQ2pfgQzv7K7A6do78Ex0+9yP0e94x0yWPWv7CUMUs0IvwlQWlpYkp2n9j69z1A16cd5ZKNGgyWyXjoRLKk9NxkeXSPe+gs3v8rIwuzcHYH7GKmizT4FMeWlfdqf0LAYbsd6zGw62D183IxvXVClMZbLnHV+25kyhsxFpTMc3447mCszPeTkGbgjh17qD/T/APvImG5Bvr8pGyGzS7zkgUCPTz+X0xm99bha2L7QhbXf7fDvLUyD9HMCKWelIhmhSYMvvm8HcEq37udXYNI8a+TvYKXziLqJRUUsbNb8p15TckULiNP0YsszUkkdiatLiz17lWEzulHiCyl0DQM1QhSleqvJ7TCdOM3y2E2zrSXnqbRfoUCaXt7CRdbqedWB8hoiNwTwBnMGUCMB1pEt+fCGuGF4kIMFouMfBmL0kr6Gt6+a/XvIAtqHnubmk31KGIeh4SXPe/I8uMgAIxAIeP+c+JbVzAREEZus5lI8u0AiGvuBzObquCTzAJqxD06dBUURKac+JjIr+sizRIbw==", "status": null}, {"id": "msg_d9477cefe627508780192fee7fd74293", "content": [{"annotations": [], "text": "[Scrubbed due to 'credential']", "type": "output_text", "logprobs": []}], "role": "assistant", "status": "completed", "type": "message", "phase": "final_answer"}], "parallel_tool_calls": true, "temperature": 1.0, "tool_choice": "auto", "tools": [{"name": "read_image", "parameters": {"title": "read_image_args", "type": "object", "additionalProperties": false, "properties": {"file_path": {"description": "Path to the image file", "title": "File Path", "type": "string"}}, "required": ["file_path"]}, "strict": true, "type": "function", "allowed_callers": null, "defer_loading": null, "description": "Read an image file for visual inspection.", "output_schema": null}], "top_p": 0.98, "background": false, "completed_at": 1787818995.0, "conversation": null, "max_output_tokens": null, "max_tool_calls": null, "moderation": null, "previous_response_id": null, "prompt": null, "prompt_cache_key": null, "prompt_cache_options": null, "prompt_cache_retention": "in_memory", "reasoning": {"context": "all_turns", "effort": "medium", "generate_summary": null, "mode": null, "summary": null}, "safety_identifier": null, "service_tier": "default", "status": "completed", "text": {"format": {"type": "text"}, "verbosity": "medium"}, "top_logprobs": 0, "truncation": "disabled", "usage": {"input_tokens": 124, "input_tokens_details": {"cache_write_tokens": 0, "cached_tokens": 0}, "output_tokens": 103, "output_tokens_details": {"reasoning_tokens": 59}, "total_tokens": 227}, "user": null, "billing": {"payer": "developer"}, "frequency_penalty": 0.0, "presence_penalty": 0.0, "store": true}J
gen_ai.operation.name
chatJN
	raw_inputA
?[{"content": "[Scrubbed due to 'credential']", "role": "user"}]JÅ
eventsˆ
Û[{"event.name": "gen_ai.system.message", "content": "You are an experienced art critic. Use the read_image tool to load and visually inspect each image. Evaluate based on creativity, composition, and appeal. Your final line must include: FINAL DECISION: [path to best image]", "role": "system"}, {"event.name": "gen_ai.user.message", "content": "[Scrubbed due to 'credential']", "role": "user"}, {"event.name": "gen_ai.unknown", "role": "assistant", "content": "reasoning\n\nSee JSON for details", "data": {"id": "rs_e148c009f9145151b36ab8764075624f", "summary": [], "type": "reasoning", "content": [], "encrypted_content": "rsn_5ZVrif4J0bXIqmWdledj7QIFeODv5/Kh1xxLPv7r2M8NGjW31sqeI+xcQZAXcjNpg50SAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBMmFaRWh2c2VFcXV6dzh4bDNYL1ZvYk1NMXZEYW1yV21EZ2N2T1F4ekIvanJlL2o2M3pWV3NwUnpLdGJmbkRxN3c9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcVMB14rvs8ijeAhvhyNK7YM/jLifM6OMWBX/cKFrKc+5ls0wRpnX+0wjWt1Kv29iLrutAmA7JP4sQaSl03sakWHpAxoIGIOhj8n6xk55tCFuGoNB+Z9+Mimhz/foCAAAQAL63J68tS+YVHASR4MySbJEuyZ37hlGYv7lCPnJkO4rdNhMWH444J7d+EWfLvUcWmv////8AAAABAAAAAAAAAAAAAAABAAABInSLdj7ptHoQBPIfUALQ2pfgQzv7K7A6do78Ex0+9yP0e94x0yWPWv7CUMUs0IvwlQWlpYkp2n9j69z1A16cd5ZKNGgyWyXjoRLKk9NxkeXSPe+gs3v8rIwuzcHYH7GKmizT4FMeWlfdqf0LAYbsd6zGw62D183IxvXVClMZbLnHV+25kyhsxFpTMc3447mCszPeTkGbgjh17qD/T/APvImG5Bvr8pGyGzS7zkgUCPTz+X0xm99bha2L7QhbXf7fDvLUyD9HMCKWelIhmhSYMvvm8HcEq37udXYNI8a+TvYKXziLqJRUUsbNb8p15TckULiNP0YsszUkkdiatLiz17lWEzulHiCyl0DQM1QhSleqvJ7TCdOM3y2E2zrSXnqbRfoUCaXt7CRdbqedWB8hoiNwTwBnMGUCMB1pEt+fCGuGF4kIMFouMfBmL0kr6Gt6+a/XvIAtqHnubmk31KGIeh4SXPe/I8uMgAIxAIeP+c+JbVzAREEZus5lI8u0AiGvuBzObquCTzAJqxD06dBUURKac+JjIr+sizRIbw==", "status": null}}, {"event.name": "gen_ai.assistant.message", "content": "[Scrubbed due to 'credential']", "role": "assistant"}]J
gen_ai.usage.input_tokens|J 
gen_ai.usage.output_tokensgJ<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jú
logfire.json_schemaÑ
Å{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}JÜ
logfire.scrubbedÒ
Ó[{"path": ["attributes", "response", "output", 1, "content", 0, "text"], "matched_substring": "credential"}, {"path": ["attributes", "raw_input", 0, "content"], "matched_substring": "credential"}, {"path": ["attributes", "events", 1, "content"], "matched_substring": "credential"}, {"path": ["attributes", "events", 3, "content"], "matched_substring": "credential"}]z Ö   ƒ
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
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   —

samples.image_genπ
†BP⁄UÄ`hO^>•^ÓtU9™Fëª^*openai_agents.session09p¨á7ÊõœAPÆ^ÇÁõœJ+

session.id
[Scrubbed due to 'session']J
user.id
	demo-userJ
logfire.span_type
spanJ&
logfire.msg
openai_agents.sessionJ^
logfire.scrubbedJ
H[{"path": ["attributes", "session.id"], "matched_substring": "session"}]z Ö   