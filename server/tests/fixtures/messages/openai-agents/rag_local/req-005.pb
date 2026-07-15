
ë‘
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
$c05b59a7-3c4d-4522-aab3-e846f53980e2
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
process.pid˛ΩıÃ

logfire.openai_agents4.41.0ÿA
†BQq∏ÌÅÅ\Éc‹ÔP—∏ˇ?Œ">ßGwı:Bw*+Responses API with {gen_ai.request.model!r}09¯¶ 4ÒõœAp@Ñ¥ÒõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±Jê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_zioh5ihnji2pdqqvegibj4lpqww7wbjecva6umeh75safrhfw66aJ∆
usageº
π{"requests":1,"input_tokens":191,"output_tokens":44,"total_tokens":235,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":10}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ˛
responseÒ
Ó{"id":"resp_zioh5ihnji2pdqqvegibj4lpqww7wbjecva6umeh75safrhfw66a","created_at":1787819037.0,"error":null,"incomplete_details":null,"instructions":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_914154ba03b05045bbd7eb04a37a19c1","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNvPfnlM/5Wa9i+gUBJoqAG2m5q1Ze1wjTylJprgvQ4TAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcUp1alZ1eHBWc3FjMzg4WENpWU5DZllDRmEwUkVSR2FtZTF6MlJ0WjQxU0tGcytCN1pTS3h2clU2R3pIbnZHTmc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBc9QUZ346RRK5JyLanT8xsCmyAzlEdwQKyiLWZDVrKc+5ls0wRpnX+0wjWt1KsAxpD5HortypASUGs486jY8q96iWPlKG2DORsbToX5dEuPDUq/5AIZV9ts46zzMkCAAAQAJAjXnfYimCxcLUIVAqCBu/bbppjo9JWWh0TdqwPFe8rdMO1tHTb54jO7C/Hs1bUb/////8AAAABAAAAAAAAAAAAAAABAAAAZTrw+I2X6slmcse9vR9D40k4C2zR1zWjSSb2JrdbMEEG02ZpCk2Bj/74dvUe42NT+ep1RF/grnYB3jIcuCm20iVOAezeWPtKze2bir8n0qjHWu/yZwRLexg82HHUdKZWW3n7GaIqisdOMf8blG7Ox5j91ImLSwBnMGUCMHAfjur08foroZ0SLsHkBc6A7Ji8reAHuss4Kc7/bviz+BFwF3c11bGno4lVfH2wcgIxAO7gLLk56PQlGRuCCrioS36tmH+ZUyqjXD9YjrJUjmIUmYyvvF6QtYBYyw+QwOgmOg==","status":null},{"arguments":"{\"query\":\"available embedding models for vector search supported embedding models\",\"num_results\":5}","call_id":"call_941a9b5a77db5076b652b1b5c6a0da2d","name":"search_knowledge","type":"function_call","id":"fc_323a30806f645c77b966fe83700125a9","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"search_knowledge","parameters":{"type":"object","additionalProperties":false,"required":["query","num_results"],"title":"search_knowledge_args","properties":{"query":{"description":"The search query to find relevant information","title":"Query","type":"string"},"num_results":{"default":3,"description":"Number of results to return (default: 3)","title":"Num Results","type":"integer"}}},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Search the knowledge base for information relevant to the query.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819039.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":191,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":44,"output_tokens_details":{"reasoning_tokens":10},"total_tokens":235},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJc
	raw_inputV
T[{"content":"What embedding models are available for vector search?","role":"user"}]Jﬂ
events‘
—[{"event.name":"gen_ai.system.message","content":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","role":"system"},{"event.name":"gen_ai.user.message","content":"What embedding models are available for vector search?","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_914154ba03b05045bbd7eb04a37a19c1","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNvPfnlM/5Wa9i+gUBJoqAG2m5q1Ze1wjTylJprgvQ4TAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcUp1alZ1eHBWc3FjMzg4WENpWU5DZllDRmEwUkVSR2FtZTF6MlJ0WjQxU0tGcytCN1pTS3h2clU2R3pIbnZHTmc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBc9QUZ346RRK5JyLanT8xsCmyAzlEdwQKyiLWZDVrKc+5ls0wRpnX+0wjWt1KsAxpD5HortypASUGs486jY8q96iWPlKG2DORsbToX5dEuPDUq/5AIZV9ts46zzMkCAAAQAJAjXnfYimCxcLUIVAqCBu/bbppjo9JWWh0TdqwPFe8rdMO1tHTb54jO7C/Hs1bUb/////8AAAABAAAAAAAAAAAAAAABAAAAZTrw+I2X6slmcse9vR9D40k4C2zR1zWjSSb2JrdbMEEG02ZpCk2Bj/74dvUe42NT+ep1RF/grnYB3jIcuCm20iVOAezeWPtKze2bir8n0qjHWu/yZwRLexg82HHUdKZWW3n7GaIqisdOMf8blG7Ox5j91ImLSwBnMGUCMHAfjur08foroZ0SLsHkBc6A7Ji8reAHuss4Kc7/bviz+BFwF3c11bGno4lVfH2wcgIxAO7gLLk56PQlGRuCCrioS36tmH+ZUyqjXD9YjrJUjmIUmYyvvF6QtYBYyw+QwOgmOg==","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_941a9b5a77db5076b652b1b5c6a0da2d","type":"function","function":{"name":"search_knowledge","arguments":"{\"query\":\"available embedding models for vector search supported embedding models\",\"num_results\":5}"}}]}]J 
gen_ai.usage.input_tokensøJ 
gen_ai.usage.output_tokens,J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jö
logfire.json_schemaÇ
ˇ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ¡
†BQq∏ÌÅÅ\Éc‹ÔPÓÁvl0">ßGwı:Bw*Function: {name}09P˝ñ¥ÒõœA „–ŒÒõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J*
logfire.msg_template
Function: {name}J
logfire.span_type
spanJ
name
search_knowledgeJn
inpute
c{"query":"available embedding models for vector search supported embedding models","num_results":5}JŸ

outputŒ

À
[1] (relevance: 0.52) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.

[2] (relevance: 0.51) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.

[3] (relevance: 0.12) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.

[4] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.

[5] (relevance: 0.09) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.J
mcp_data
nullJ
gen_ai.system
openaiJ+
logfire.msg
Function: search_knowledgeJà
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   €
†BQq∏ÌÅÅ\Éc‹ÔP>ßGwı:Bw"€â≥.óÎˆ*"Turn {turn} for agent RAGAssistant09ﬂ4ÒõœA‡ŸŒÒõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J<
logfire.msg_template$
"Turn {turn} for agent RAGAssistantJ
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

turnJ

agent_name
RAGAssistantJg
usage^
\{"input_tokens":191,"output_tokens":44,"cached_input_tokens":0,"cache_write_input_tokens":0}J.
logfire.msg
Turn 1 for agent RAGAssistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ïa
†BQq∏ÌÅÅ\Éc‹ÔP*‹ﬁi3Í≤"á0–c°o*+Responses API with {gen_ai.request.model!r}09¯¶ÈŒÒõœAqÿÚõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±Jê
model_settings˝
˙{"temperature":null,"top_p":null,"frequency_penalty":null,"presence_penalty":null,"tool_choice":null,"parallel_tool_calls":null,"truncation":null,"max_tokens":null,"reasoning":null,"verbosity":null,"metadata":null,"store":null,"prompt_cache_retention":null,"include_usage":null,"response_include":null,"top_logprobs":null,"extra_query":null,"extra_body":null,"extra_headers":null,"extra_args":null,"retry":null,"context_management":null,"prompt_cache_options":null,"preserve_raw_usage":null,"timeout":null}J0
gen_ai.request.model
us.openai.gpt-5.6-lunaJE
logfire.msg_template-
+Responses API with {gen_ai.request.model!r}J
logfire.span_type
spanJJ
response_id;
9resp_hsv3xptcfeaf5xajjjttibf7vhjsrcbvwa2n7f3653pt4wliouhqJ≈
usageª
∏{"requests":1,"input_tokens":552,"output_tokens":97,"total_tokens":649,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ£
responseñ
ì{"id":"resp_hsv3xptcfeaf5xajjjttibf7vhjsrcbvwa2n7f3653pt4wliouhq","created_at":1787819040.0,"error":null,"incomplete_details":null,"instructions":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"msg_baae4b09338d5c6a94f9ce1d47262855","content":[{"annotations":[],"text":"Common embedding models available for vector search include:\n\n- **Amazon Titan Embeddings**\n- **OpenAI `text-embedding-3`** models\n- **Cohere Embed**\n\nEmbedding dimensions typically range from **256 to 3,072**, depending on the model. The resulting vectors can be stored in vector databases such as **FAISS, Pinecone, Chroma, or Weaviate** and compared using metrics like cosine similarity or L2 distance.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"search_knowledge","parameters":{"properties":{"query":{"description":"The search query to find relevant information","title":"Query","type":"string"},"num_results":{"default":3,"description":"Number of results to return (default: 3)","title":"Num Results","type":"integer"}},"title":"search_knowledge_args","type":"object","additionalProperties":false,"required":["query","num_results"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Search the knowledge base for information relevant to the query.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819040.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":552,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":97,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":649},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ®
	raw_inputö
ó[{"content":"What embedding models are available for vector search?","role":"user"},{"id":"rs_914154ba03b05045bbd7eb04a37a19c1","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNvPfnlM/5Wa9i+gUBJoqAG2m5q1Ze1wjTylJprgvQ4TAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcUp1alZ1eHBWc3FjMzg4WENpWU5DZllDRmEwUkVSR2FtZTF6MlJ0WjQxU0tGcytCN1pTS3h2clU2R3pIbnZHTmc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBc9QUZ346RRK5JyLanT8xsCmyAzlEdwQKyiLWZDVrKc+5ls0wRpnX+0wjWt1KsAxpD5HortypASUGs486jY8q96iWPlKG2DORsbToX5dEuPDUq/5AIZV9ts46zzMkCAAAQAJAjXnfYimCxcLUIVAqCBu/bbppjo9JWWh0TdqwPFe8rdMO1tHTb54jO7C/Hs1bUb/////8AAAABAAAAAAAAAAAAAAABAAAAZTrw+I2X6slmcse9vR9D40k4C2zR1zWjSSb2JrdbMEEG02ZpCk2Bj/74dvUe42NT+ep1RF/grnYB3jIcuCm20iVOAezeWPtKze2bir8n0qjHWu/yZwRLexg82HHUdKZWW3n7GaIqisdOMf8blG7Ox5j91ImLSwBnMGUCMHAfjur08foroZ0SLsHkBc6A7Ji8reAHuss4Kc7/bviz+BFwF3c11bGno4lVfH2wcgIxAO7gLLk56PQlGRuCCrioS36tmH+ZUyqjXD9YjrJUjmIUmYyvvF6QtYBYyw+QwOgmOg=="},{"arguments":"{\"query\":\"available embedding models for vector search supported embedding models\",\"num_results\":5}","call_id":"call_941a9b5a77db5076b652b1b5c6a0da2d","name":"search_knowledge","type":"function_call","id":"fc_323a30806f645c77b966fe83700125a9","status":"completed"},{"call_id":"call_941a9b5a77db5076b652b1b5c6a0da2d","output":"[1] (relevance: 0.52) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.\n\n[2] (relevance: 0.51) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.\n\n[3] (relevance: 0.12) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.\n\n[4] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.\n\n[5] (relevance: 0.09) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.","type":"function_call_output"}]Jé
eventsÉ
Ä[{"event.name":"gen_ai.system.message","content":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","role":"system"},{"event.name":"gen_ai.user.message","content":"What embedding models are available for vector search?","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_914154ba03b05045bbd7eb04a37a19c1","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeNvPfnlM/5Wa9i+gUBJoqAG2m5q1Ze1wjTylJprgvQ4TAF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBcUp1alZ1eHBWc3FjMzg4WENpWU5DZllDRmEwUkVSR2FtZTF6MlJ0WjQxU0tGcytCN1pTS3h2clU2R3pIbnZHTmc9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBc9QUZ346RRK5JyLanT8xsCmyAzlEdwQKyiLWZDVrKc+5ls0wRpnX+0wjWt1KsAxpD5HortypASUGs486jY8q96iWPlKG2DORsbToX5dEuPDUq/5AIZV9ts46zzMkCAAAQAJAjXnfYimCxcLUIVAqCBu/bbppjo9JWWh0TdqwPFe8rdMO1tHTb54jO7C/Hs1bUb/////8AAAABAAAAAAAAAAAAAAABAAAAZTrw+I2X6slmcse9vR9D40k4C2zR1zWjSSb2JrdbMEEG02ZpCk2Bj/74dvUe42NT+ep1RF/grnYB3jIcuCm20iVOAezeWPtKze2bir8n0qjHWu/yZwRLexg82HHUdKZWW3n7GaIqisdOMf8blG7Ox5j91ImLSwBnMGUCMHAfjur08foroZ0SLsHkBc6A7Ji8reAHuss4Kc7/bviz+BFwF3c11bGno4lVfH2wcgIxAO7gLLk56PQlGRuCCrioS36tmH+ZUyqjXD9YjrJUjmIUmYyvvF6QtYBYyw+QwOgmOg=="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_941a9b5a77db5076b652b1b5c6a0da2d","type":"function","function":{"name":"search_knowledge","arguments":"{\"query\":\"available embedding models for vector search supported embedding models\",\"num_results\":5}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_941a9b5a77db5076b652b1b5c6a0da2d","content":"[1] (relevance: 0.52) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.\n\n[2] (relevance: 0.51) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.\n\n[3] (relevance: 0.12) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.\n\n[4] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.\n\n[5] (relevance: 0.09) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.","name":"search_knowledge"},{"event.name":"gen_ai.assistant.message","content":"Common embedding models available for vector search include:\n\n- **Amazon Titan Embeddings**\n- **OpenAI `text-embedding-3`** models\n- **Cohere Embed**\n\nEmbedding dimensions typically range from **256 to 3,072**, depending on the model. The resulting vectors can be stored in vector databases such as **FAISS, Pinecone, Chroma, or Weaviate** and compared using metrics like cosine similarity or L2 distance.","role":"assistant"}]J 
gen_ai.usage.input_tokens®J 
gen_ai.usage.output_tokensaJ<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jæ
logfire.json_schema¶
£{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   €
†BQq∏ÌÅÅ\Éc‹ÔPá0–c°o"€â≥.óÎˆ*"Turn {turn} for agent RAGAssistant09∏FﬂŒÒõœAòIÔÚõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J<
logfire.msg_template$
"Turn {turn} for agent RAGAssistantJ
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

turnJ

agent_name
RAGAssistantJg
usage^
\{"input_tokens":552,"output_tokens":97,"cached_input_tokens":0,"cache_write_input_tokens":0}J.
logfire.msg
Turn 2 for agent RAGAssistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ¡
†BQq∏ÌÅÅ\Éc‹ÔP€â≥.óÎˆ"±zQõ±É‚V*Agent run: {name!r}09x∂4ÒõœAvÚÚõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J-
logfire.msg_template
Agent run: {name!r}J
logfire.span_type
spanJ
name
RAGAssistantJ
handoffs
[]J
tools
["search_knowledge"]J
output_type
strJ
gen_ai.system
openaiJ*
logfire.msg
Agent run: 'RAGAssistant'Jû
logfire.json_schemaÜ
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   ú
†BQq∏ÌÅÅ\Éc‹ÔP±zQõ±É‚V"´ΩM¨≠M¡*Task: Agent workflow09Ëz4ÒõœA…ÛÚõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J.
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
}{"input_tokens":743,"output_tokens":141,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":2,"total_tokens":884}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˚
†BQq∏ÌÅÅ\Éc‹ÔP´ΩM¨≠M¡" 9g≠ÈÄ¨Õ*OpenAI Agents trace: {name}09(/	4ÒõœAïÙÚõœJ'
code.filepath
samples/rag_local.pyJ
code.function
runJ
code.lineno±J
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
&trace_be70bd06c785487baa5db23e5a13f2caJ©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   —

samples.rag_localπ
†BQq∏ÌÅÅ\Éc‹ÔP 9g≠ÈÄ¨Õ*openai_agents.session09√Ä=ÔõœA+¯ÚõœJ+

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