
’∂
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
process.pid≤∫ç≤

logfire.openai_agents4.41.0±Q
†BP⁄UÄ`hO^>•^ÓÊËeuΩ◊Ø"úYô^uF—*+Responses API with {gen_ai.request.model!r}09HhH8ÊõœA0‡ÒÊõœJ'
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
9resp_cs3osytx47ytxvgoceztosepc6rbsjs5m6m4igxl4m5aeczajinaJ«
usageΩ
∫{"requests":1,"input_tokens":138,"output_tokens":221,"total_tokens":359,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":23}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ√ 
response∂ 
≥ {"id":"resp_cs3osytx47ytxvgoceztosepc6rbsjs5m6m4igxl4m5aeczajina","created_at":1787818991.0,"error":null,"incomplete_details":null,"instructions":"You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_8289ae870bd457a0b8b3121f0ede4fa7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv","status":null},{"arguments":"{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}","call_id":"call_b927172290845e3f82cdd316c2f64df1","name":"generate_image","type":"function_call","id":"fc_c2db173d5f5a5076a83d14ac999c11e9","caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}","call_id":"call_cd488973791e5791b0a0df6b09b83697","name":"generate_image","type":"function_call","id":"fc_2f8b45d1f24f5afbb551afe0f2e890ea","caller":null,"namespace":null,"status":"completed"},{"arguments":"{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}","call_id":"call_917368052e0a580e855a2d3aa0b20d2b","name":"generate_image","type":"function_call","id":"fc_07f9fdd9b4ac512483b686c6a6399ae6","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"generate_image","parameters":{"required":["prompt"],"title":"generate_image_args","additionalProperties":false,"properties":{"prompt":{"description":"The image description prompt","title":"Prompt","type":"string"}},"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Generate an image using OpenAI DALL-E.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787818992.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":138,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":221,"output_tokens_details":{"reasoning_tokens":23},"total_tokens":359},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJÁ
	raw_inputŸ
÷[{"content":"Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.","role":"user"}]Jª
events∞
≠[{"event.name":"gen_ai.system.message","content":"You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.","role":"system"},{"event.name":"gen_ai.user.message","content":"Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_8289ae870bd457a0b8b3121f0ede4fa7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_b927172290845e3f82cdd316c2f64df1","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_cd488973791e5791b0a0df6b09b83697","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_917368052e0a580e855a2d3aa0b20d2b","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}"}}]}]J 
gen_ai.usage.input_tokensäJ!
gen_ai.usage.output_tokens›J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'JÃ
logfire.json_schema¥
±{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   π
†BP⁄UÄ`hO^>•^Óag ¬AÅπ"úYô^uF—*Function: {name}09Pí4ÒÊõœA˝∂ÒÊõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
logfire.level_numJ
name
generate_imageJé
inputÑ
Å{"prompt":"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed"}JÌ
output‚
ﬂAn error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.J
mcp_data
nullJ
gen_ai.system
openaiJÜ
error¸
˘{"message":"Error running tool (non-fatal)","data":{"tool_name":"generate_image","error":"Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable."}}JP
logfire.msgA
?Function: generate_image failed: Error running tool (non-fatal)J§
logfire.json_schemaå
â{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{},"error":{"type":"object"}}}Z‡	àÌFÒÊõœ	exception&
exception.type
openai.OpenAIErrorµ
exception.messageü
úMissing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.À
exception.stacktrace≤
ØTraceback (most recent call last):
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 655, in __call__
    return await self._invoke_tool_impl(ctx, input)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 2647, in _on_invoke_tool_impl
    result = await asyncio.to_thread(the_func, *args, **kwargs_dict)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/asyncio/threads.py", line 25, in to_thread
    return await loop.run_in_executor(None, func_call)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/concurrent/futures/thread.py", line 59, in run
    result = self.fn(*self.args, **self.kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/logfire/_internal/integrations/executors.py", line 56, in _run_with_context
    return func()
           ^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/samples/image_gen.py", line 29, in generate_image
    client = OpenAI()
             ^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/openai/_client.py", line 263, in __init__
    raise OpenAIError(
openai.OpenAIError: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.

exception.escaped
Falsez Ö   …
†BP⁄UÄ`hO^>•^ÓÿÉ$Í÷B∫"úYô^uF—*Function: {name}09[8ÒÊõœAòÕ∫ÒÊõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
logfire.level_numJ
name
generate_imageJû
inputî
ë{"prompt":"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork"}JÌ
output‚
ﬂAn error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.J
mcp_data
nullJ
gen_ai.system
openaiJÜ
error¸
˘{"message":"Error running tool (non-fatal)","data":{"tool_name":"generate_image","error":"Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable."}}JP
logfire.msgA
?Function: generate_image failed: Error running tool (non-fatal)J§
logfire.json_schemaå
â{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{},"error":{"type":"object"}}}Z‡	 ;¨ÒÊõœ	exception&
exception.type
openai.OpenAIErrorµ
exception.messageü
úMissing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.À
exception.stacktrace≤
ØTraceback (most recent call last):
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 655, in __call__
    return await self._invoke_tool_impl(ctx, input)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 2647, in _on_invoke_tool_impl
    result = await asyncio.to_thread(the_func, *args, **kwargs_dict)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/asyncio/threads.py", line 25, in to_thread
    return await loop.run_in_executor(None, func_call)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/concurrent/futures/thread.py", line 59, in run
    result = self.fn(*self.args, **self.kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/logfire/_internal/integrations/executors.py", line 56, in _run_with_context
    return func()
           ^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/samples/image_gen.py", line 29, in generate_image
    client = OpenAI()
             ^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/openai/_client.py", line 263, in __init__
    raise OpenAIError(
openai.OpenAIError: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.

exception.escaped
Falsez Ö   ¡
†BP⁄UÄ`hO^>•^Ó4º»&<„G"úYô^uF—*Function: {name}09PÎ:ÒÊõœAàîΩÒÊõœJ'
code.filepath
samples/image_gen.pyJ
code.function
runJ
code.linenouJ*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
logfire.level_numJ
name
generate_imageJñ
inputå
â{"prompt":"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed"}JÌ
output‚
ﬂAn error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.J
mcp_data
nullJ
gen_ai.system
openaiJÜ
error¸
˘{"message":"Error running tool (non-fatal)","data":{"tool_name":"generate_image","error":"Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable."}}JP
logfire.msgA
?Function: generate_image failed: Error running tool (non-fatal)J§
logfire.json_schemaå
â{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{},"error":{"type":"object"}}}Z‡	`ø∞ÒÊõœ	exception&
exception.type
openai.OpenAIErrorµ
exception.messageü
úMissing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.À
exception.stacktrace≤
ØTraceback (most recent call last):
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 655, in __call__
    return await self._invoke_tool_impl(ctx, input)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/agents/tool.py", line 2647, in _on_invoke_tool_impl
    result = await asyncio.to_thread(the_func, *args, **kwargs_dict)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/asyncio/threads.py", line 25, in to_thread
    return await loop.run_in_executor(None, func_call)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/.local/share/uv/python/cpython-3.12.8-macos-aarch64-none/lib/python3.12/concurrent/futures/thread.py", line 59, in run
    result = self.fn(*self.args, **self.kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/logfire/_internal/integrations/executors.py", line 56, in _run_with_context
    return func()
           ^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/samples/image_gen.py", line 29, in generate_image
    client = OpenAI()
             ^^^^^^^^
  File "/Users/pugachev/Desktop/dev/sideseat/misc/samples/python/openai-agents/.venv/lib/python3.12/site-packages/openai/_client.py", line 263, in __init__
    raise OpenAIError(
openai.OpenAIError: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.

exception.escaped
Falsez Ö   √
†BP⁄UÄ`hO^>•^ÓúYô^uF—"¯n™Ùﬂì™µ*Turn {turn} for agent artist09∏Ÿ98ÊõœA0U√ÒÊõœJ'
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

turnJ

agent_name
artistJh
usage_
]{"input_tokens":138,"output_tokens":221,"cached_input_tokens":0,"cache_write_input_tokens":0}J(
logfire.msg
Turn 1 for agent artistJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ét
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
us.openai.gpt-5.6-lunaJ˘
responseÏ
È{"id":"resp_fxhmkm76cofos7kcat6qfl6ip454yep5hpjrszjmajtjejvz7iya","created_at":1787818993.0,"error":null,"incomplete_details":null,"instructions":"You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_67b578f5755259038eedeffb802c5c7b","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJhuP4BIC68M5jqt27gqM97i0B7+RBPvYzc0eNPJC2RtAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBOW5OMjNMRDY4SVRPVWp6QUUzd1R0Q1dMdmREZ2pBQU02Qk9ybU1ROTZXdlhERU84bHdZcUZRbDVOSzBjRlJXcEE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcb/f77IWYTbNnxz/pPdg4PZW+GhIzdp9UC9tCuFrKc+5ls0wRpnX+0wjWt1JcFrVkMFhr1VYZbJYUF24J36JxXNvbPDJru6tkf4JuspBfaoOc/o9TijbMe+DOIlQCAAAQABvZ5fn19KFWhPqhCV0weGliHstIynIyHXX9UchPyluMUO76nmPmAH4OLlMkLYvm2/////8AAAABAAAAAAAAAAAAAAABAAAAxhr0Xp4mRdt6dqeEpyMSe43F5PVuThjGw0jGKXtDYStZGIjsKt6ipvZcX2yhE0o+fQPBX5k1SFIRCLalEOoSSe+ohK9/8T40x/99LNbK67cH8rGFrNhnCQx2DWoNa+6vP/HeU3X9LIFKtf8Yf9K1AX1bcaC72wHp3LXvaQQBii36qLc/OwuT/zrTWT0M681AxVeXMDxRA0tarpE9MJBbDZcQV0by1JBeeooqay8R3ZpKdM11s/5YyQq7irTwfJR+YLZrmUxF4MRc0TKjMKgAphPw56sH4OcAZzBlAjEAzyyMNWlgHdNTawzV+i5kUtoMwVeBpCH6oZ63PJdAeiJwXDVGWJdmosZJtRLn4FpNAjBA596brCq04jHdKvVGWLkzw1s+CXxrhE7pnt+pqZ+d2fnWI8Syf0Yg4ADzq+yhGtE=","status":null},{"id":"msg_fdc4dd7a01ae5531954d0b01d53e159d","content":[{"annotations":[],"text":"Unable to generate the images because image-generation credentials are unavailable.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"generate_image","parameters":{"additionalProperties":false,"title":"generate_image_args","required":["prompt"],"properties":{"prompt":{"description":"The image description prompt","title":"Prompt","type":"string"}},"type":"object"},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Generate an image using OpenAI DALL-E.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787818994.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":547,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":50,"output_tokens_details":{"reasoning_tokens":32},"total_tokens":597},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJú
	raw_inputé
ã[{"content":"Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.","role":"user"},{"id":"rs_8289ae870bd457a0b8b3121f0ede4fa7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv"},{"arguments":"{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}","call_id":"call_b927172290845e3f82cdd316c2f64df1","name":"generate_image","type":"function_call","id":"fc_c2db173d5f5a5076a83d14ac999c11e9","status":"completed"},{"arguments":"{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}","call_id":"call_cd488973791e5791b0a0df6b09b83697","name":"generate_image","type":"function_call","id":"fc_2f8b45d1f24f5afbb551afe0f2e890ea","status":"completed"},{"arguments":"{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}","call_id":"call_917368052e0a580e855a2d3aa0b20d2b","name":"generate_image","type":"function_call","id":"fc_07f9fdd9b4ac512483b686c6a6399ae6","status":"completed"},{"call_id":"call_b927172290845e3f82cdd316c2f64df1","output":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","type":"function_call_output"},{"call_id":"call_cd488973791e5791b0a0df6b09b83697","output":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","type":"function_call_output"},{"call_id":"call_917368052e0a580e855a2d3aa0b20d2b","output":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","type":"function_call_output"}]Jﬂ)
events‘)
—)[{"event.name":"gen_ai.system.message","content":"You are an AI artist. When asked to generate images, use the generate_image tool with varied prompts for each image to create a diverse collection. Return a comma-separated list of the generated image paths.","role":"system"},{"event.name":"gen_ai.user.message","content":"Generate 3 different images of a dog. Vary the style, setting, and mood for each image. Use different prompts like: playful puppy in a park, majestic dog portrait, dog playing in snow.","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_8289ae870bd457a0b8b3121f0ede4fa7","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeMx/wtv+ajIK2/bqF0HUPwK0LhdUKg1MqjtWUAwSyXQfAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcjBCVFFNNlh5eWNaL0NCTXJ2V0lGeHg4UTVqSDl4MVRvTkFMc0FRL2M2b2MwQ1Q2cDArUjhwcm5wRkx6eW04blE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcbz914mljgZpYa4JoQ2TUQLNepz1DtaUK1Ha4tVrKc+5ls0wRpnX+0wjWt1LAa7eunyeWAMz6pYTENz781AJrkD31irBxXaBDghbCWzITqGVzzUy7pQCTgBjMyPMCAAAQAN1oNMNr9GdlSlfi+qFDIUJ71Wp438MfAf8NkM9j+pf5nlE2d0u6yh1j282ueb1aqP////8AAAABAAAAAAAAAAAAAAABAAAAoODQnUoH3noFLuaXQztGz2xnQo/J8BB/seseGwt1Wist3Bn++y99C5/fl/Jw0N0X8Fo/JeZx+qGgaefTEmmfanKqeBLcXogcJlcR+cn8zp6FUcSWbFdM8PHJAKjBSh+mCHeUduYQ4azCr6ztQbqY6fh7KLGRWsAwFITDanfam62zfyuihpnmO1BphnlakBkZ6jTUVLAbr21jVWOHkC0KH+xeTK9k4Ed0/9x6/IjV9CkOAGcwZQIwfUb0OP3a6Gq+dyt4x97zA8QrPOepJF4TUHWWLSjWkKVAQqaCx6N+NZnN+RdKF1G4AjEAjn0J7HuoaFslQy1fIDC4i0RBV7F+BjRtdl954pihZkH82CNyEfEQsKqokhoLW8Lv"}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_b927172290845e3f82cdd316c2f64df1","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A joyful playful golden retriever puppy bounding through a sunny spring park, bright green grass, colorful wildflowers, a red frisbee in the foreground, warm golden-hour light, candid action photography, cheerful energetic mood, highly detailed\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_cd488973791e5791b0a0df6b09b83697","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A majestic adult German shepherd posed in a dramatic studio portrait, rich charcoal and tan fur, dignified expression, deep charcoal background, elegant rim lighting, classical fine-art oil painting style, regal and powerful mood, intricate realistic brushwork\"}"}}]},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_917368052e0a580e855a2d3aa0b20d2b","type":"function","function":{"name":"generate_image","arguments":"{\"prompt\":\"A fluffy Siberian husky joyfully playing in fresh powder snow during a gentle winter snowfall, snow-covered pine forest, cool blue twilight light, dynamic leap with snow flying, cinematic illustration style, adventurous and serene mood, highly detailed\"}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_b927172290845e3f82cdd316c2f64df1","content":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","name":"generate_image"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_cd488973791e5791b0a0df6b09b83697","content":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","name":"generate_image"},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_917368052e0a580e855a2d3aa0b20d2b","content":"An error occurred while running the tool. Please try again. Error: Missing credentials. Please pass an `api_key`, `workload_identity`, `admin_api_key`, or set the `OPENAI_API_KEY` or `OPENAI_ADMIN_KEY` environment variable.","name":"generate_image"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_67b578f5755259038eedeffb802c5c7b","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeJhuP4BIC68M5jqt27gqM97i0B7+RBPvYzc0eNPJC2RtAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBOW5OMjNMRDY4SVRPVWp6QUUzd1R0Q1dMdmREZ2pBQU02Qk9ybU1ROTZXdlhERU84bHdZcUZRbDVOSzBjRlJXcEE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcb/f77IWYTbNnxz/pPdg4PZW+GhIzdp9UC9tCuFrKc+5ls0wRpnX+0wjWt1JcFrVkMFhr1VYZbJYUF24J36JxXNvbPDJru6tkf4JuspBfaoOc/o9TijbMe+DOIlQCAAAQABvZ5fn19KFWhPqhCV0weGliHstIynIyHXX9UchPyluMUO76nmPmAH4OLlMkLYvm2/////8AAAABAAAAAAAAAAAAAAABAAAAxhr0Xp4mRdt6dqeEpyMSe43F5PVuThjGw0jGKXtDYStZGIjsKt6ipvZcX2yhE0o+fQPBX5k1SFIRCLalEOoSSe+ohK9/8T40x/99LNbK67cH8rGFrNhnCQx2DWoNa+6vP/HeU3X9LIFKtf8Yf9K1AX1bcaC72wHp3LXvaQQBii36qLc/OwuT/zrTWT0M681AxVeXMDxRA0tarpE9MJBbDZcQV0by1JBeeooqay8R3ZpKdM11s/5YyQq7irTwfJR+YLZrmUxF4MRc0TKjMKgAphPw56sH4OcAZzBlAjEAzyyMNWlgHdNTawzV+i5kUtoMwVeBpCH6oZ63PJdAeiJwXDVGWJdmosZJtRLn4FpNAjBA596brCq04jHdKvVGWLkzw1s+CXxrhE7pnt+pqZ+d2fnWI8Syf0Yg4ADzq+yhGtE=","status":null}},{"event.name":"gen_ai.assistant.message","content":"Unable to generate the images because image-generation credentials are unavailable.","role":"assistant"}]J 
gen_ai.usage.input_tokens£J 
gen_ai.usage.output_tokens2J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jú
logfire.json_schemaÑ
Å{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ¬
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
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   