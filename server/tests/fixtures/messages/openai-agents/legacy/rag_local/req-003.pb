
ö
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
process.pid˛Ω“Î

logfire.openai_agents4.41.0‘B
†BQq∏ÌÅÅ\Éc‹ÔPÄX:î‹Û"Ê®¢ª•¯€*+Responses API with {gen_ai.request.model!r}09à70õœA–≥‘nõœJ'
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
9resp_dlbccf3ka5rvwil4makks26tvjhf7zcfqpppf6vyp3zqx7hq4xdqJ∆
usageº
π{"requests":1,"input_tokens":194,"output_tokens":55,"total_tokens":249,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":10}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ¿
response≥
∞{"id":"resp_dlbccf3ka5rvwil4makks26tvjhf7zcfqpppf6vyp3zqx7hq4xdq","created_at":1787819033.0,"error":null,"incomplete_details":null,"instructions":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"rs_4823637b666e58be9433f57145388668","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeLydn1jFhs7f1dZby7B2jLi62qz5pMKuocZ2Pv3ZbTR5AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBbmEvY0g0elV5SGdVVFpjSm50RWQvd0hQMTE4cTFwd2pXS2dpZ0ZsNzRVQ2E1Y0hYc01VVGpNcUpsZGNtRndJekE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcnilHOYt6zcu8/Xq4Gua14bjW1lTe7JDdX2xXBFrKc+5ls0wRpnX+0wjWt1I6G+dsZOfsoL+IGKJlgvItHATBnVJ5waleAyAtmuO5AxYPFafdD8uOdTjNRlgi03oCAAAQAN+hrtsWE0tzuyEBC1amnLRqMs6XAtYv6ctAORYGC27wAMK9eRHaffLpQg97PDMfXP////8AAAABAAAAAAAAAAAAAAABAAAAZXQRGXRPgjAo09I1sCK6aEkE5bSBiULALkcKK81hoDb0nYK4oOT7kxBChCnWKmrMgKrXEm+qizsK9yr6cfJiMEW3PAzTH6164aS/z3ko47uHVJdeP0R5i+CPmd73Bok7pwUdidSDM+cDqlodzLlJc+/o6TACjABnMGUCMQDsRa9Xz6lTLNkJuXoayup7+DOQPOytltYe8CgWsNXnZc4Qjj42a3na6E62myQmyh8CMEIvwUJ+WXtiovU0uIOE+w1vWarnfCCg8afRlOZZ88dbBePilWgZpnSPnHlx1bLf5w==","status":null},{"arguments":"{\"query\":\"RAG retrieval augmented generation how it works key components retrieval embeddings vector database chunking reranking generation context\",\"num_results\":5}","call_id":"call_830505c14beb55dd9b4fe7c1a64b83e8","name":"search_knowledge","type":"function_call","id":"fc_567b7563d68659928e60c39fcd97183b","caller":null,"namespace":null,"status":"completed"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"search_knowledge","parameters":{"type":"object","additionalProperties":false,"properties":{"query":{"description":"The search query to find relevant information","title":"Query","type":"string"},"num_results":{"default":3,"description":"Number of results to return (default: 3)","title":"Num Results","type":"integer"}},"title":"search_knowledge_args","required":["query","num_results"]},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Search the knowledge base for information relevant to the query.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819033.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":194,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":55,"output_tokens_details":{"reasoning_tokens":10},"total_tokens":249},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJ_
	raw_inputR
P[{"content":"How does RAG work and what are its key components?","role":"user"}]Jù
eventsí
è[{"event.name":"gen_ai.system.message","content":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","role":"system"},{"event.name":"gen_ai.user.message","content":"How does RAG work and what are its key components?","role":"user"},{"event.name":"gen_ai.unknown","role":"assistant","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_4823637b666e58be9433f57145388668","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeLydn1jFhs7f1dZby7B2jLi62qz5pMKuocZ2Pv3ZbTR5AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBbmEvY0g0elV5SGdVVFpjSm50RWQvd0hQMTE4cTFwd2pXS2dpZ0ZsNzRVQ2E1Y0hYc01VVGpNcUpsZGNtRndJekE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcnilHOYt6zcu8/Xq4Gua14bjW1lTe7JDdX2xXBFrKc+5ls0wRpnX+0wjWt1I6G+dsZOfsoL+IGKJlgvItHATBnVJ5waleAyAtmuO5AxYPFafdD8uOdTjNRlgi03oCAAAQAN+hrtsWE0tzuyEBC1amnLRqMs6XAtYv6ctAORYGC27wAMK9eRHaffLpQg97PDMfXP////8AAAABAAAAAAAAAAAAAAABAAAAZXQRGXRPgjAo09I1sCK6aEkE5bSBiULALkcKK81hoDb0nYK4oOT7kxBChCnWKmrMgKrXEm+qizsK9yr6cfJiMEW3PAzTH6164aS/z3ko47uHVJdeP0R5i+CPmd73Bok7pwUdidSDM+cDqlodzLlJc+/o6TACjABnMGUCMQDsRa9Xz6lTLNkJuXoayup7+DOQPOytltYe8CgWsNXnZc4Qjj42a3na6E62myQmyh8CMEIvwUJ+WXtiovU0uIOE+w1vWarnfCCg8afRlOZZ88dbBePilWgZpnSPnHlx1bLf5w==","status":null}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_830505c14beb55dd9b4fe7c1a64b83e8","type":"function","function":{"name":"search_knowledge","arguments":"{\"query\":\"RAG retrieval augmented generation how it works key components retrieval embeddings vector database chunking reranking generation context\",\"num_results\":5}"}}]}]J 
gen_ai.usage.input_tokens¬J 
gen_ai.usage.output_tokens7J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jö
logfire.json_schemaÇ
ˇ
{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","prefixItems":[{"type":"object","title":"ResponseReasoningItem","x-python-datatype":"PydanticModel"},{"type":"object","title":"ResponseFunctionToolCall","x-python-datatype":"PydanticModel"}]},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   Ü
†BQq∏ÌÅÅ\Éc‹ÔP6g1¿‹¶Íí"Ê®¢ª•¯€*Function: {name}09êôËnõœA¯ÜiâõœJ'
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
search_knowledgeJ≤
input®
•{"query":"RAG retrieval augmented generation how it works key components retrieval embeddings vector database chunking reranking generation context","num_results":5}JŸ

outputŒ

À
[1] (relevance: 0.71) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.

[2] (relevance: 0.19) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.

[3] (relevance: 0.17) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.

[4] (relevance: 0.16) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.

[5] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.J
mcp_data
nullJ
gen_ai.system
openaiJ+
logfire.msg
Function: search_knowledgeJà
logfire.json_schemaq
o{"type":"object","properties":{"name":{},"input":{},"output":{},"mcp_data":{"type":"null"},"gen_ai.system":{}}}z Ö   €
†BQq∏ÌÅÅ\Éc‹ÔPÊ®¢ª•¯€"·.‹∏\'µ*"Turn {turn} for agent RAGAssistant09ò-0õœAêrâõœJ'
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
\{"input_tokens":194,"output_tokens":55,"cached_input_tokens":0,"cache_write_input_tokens":0}J.
logfire.msg
Turn 1 for agent RAGAssistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   Æ~
†BQq∏ÌÅÅ\Éc‹ÔPjÎ∆“u5Rñ"uìs¿Uf*+Responses API with {gen_ai.request.model!r}09hÉâõœAH©ÿ3ÒõœJ'
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
9resp_g4a5qmfw4lyctvv5nmzo24f3dvgmmbsa5uvw375rnr62e75xijzqJ«
usageΩ
∫{"requests":1,"input_tokens":566,"output_tokens":451,"total_tokens":1017,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}J
gen_ai.system
openaiJ1
gen_ai.response.model
us.openai.gpt-5.6-lunaJ±$
response§$
°${"id":"resp_g4a5qmfw4lyctvv5nmzo24f3dvgmmbsa5uvw375rnr62e75xijzq","created_at":1787819034.0,"error":null,"incomplete_details":null,"instructions":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","metadata":{},"model":"us.openai.gpt-5.6-luna","object":"response","output":[{"id":"msg_4a5129ff98745378b14325ab93a855ef","content":[{"annotations":[],"text":"RAG (Retrieval-Augmented Generation) combines **information retrieval** with **LLM generation**. Instead of relying only on the model‚Äôs training data, it retrieves relevant information from an external knowledge base and provides it to the model as context.\n\n### How RAG works\n\n1. **Ingest documents**\n   - Collect source content such as PDFs, web pages, databases, or internal documents.\n   - Clean and split the content into smaller chunks.\n\n2. **Create embeddings**\n   - Convert each text chunk into a dense numerical vector using an embedding model.\n   - Similar meanings produce similar vectors.\n\n3. **Store vectors**\n   - Save the embeddings, along with the original text and metadata, in a vector database or index.\n\n4. **Process a user query**\n   - Convert the user‚Äôs question into an embedding using the same or a compatible embedding model.\n\n5. **Retrieve relevant content**\n   - Use vector search to find chunks whose embeddings are most similar to the query.\n   - Similarity is commonly measured with cosine similarity or L2 distance.\n   - Systems may also use metadata filters, keyword search, or reranking.\n\n6. **Generate an answer**\n   - Add the retrieved chunks to the LLM‚Äôs prompt as context.\n   - The LLM generates a response grounded in that retrieved information.\n\n### Key components\n\n- **Document ingestion and preprocessing:** Loading, cleaning, and chunking source data.\n- **Embedding model:** Converts text into semantic vectors.\n- **Vector database/index:** Stores and searches embeddings. Examples include FAISS, Pinecone, Chroma, and Weaviate.\n- **Retriever:** Finds relevant passages, often using similarity search.\n- **Reranker, optionally:** Reorders retrieved results to improve relevance.\n- **Prompt template:** Combines the user question and retrieved context.\n- **Generative LLM:** Produces the final answer.\n- **Metadata and filtering:** Restricts retrieval by attributes such as source, date, user, or document type.\n\nRAG can improve factuality and enable access to current or private information, but its quality depends heavily on chunking, embedding quality, retrieval accuracy, and the relevance of the context supplied to the LLM.","type":"output_text","logprobs":[]}],"role":"assistant","status":"completed","type":"message","phase":"final_answer"}],"parallel_tool_calls":true,"temperature":1.0,"tool_choice":"auto","tools":[{"name":"search_knowledge","parameters":{"type":"object","additionalProperties":false,"required":["query","num_results"],"title":"search_knowledge_args","properties":{"query":{"description":"The search query to find relevant information","title":"Query","type":"string"},"num_results":{"default":3,"description":"Number of results to return (default: 3)","title":"Num Results","type":"integer"}}},"strict":true,"type":"function","allowed_callers":null,"defer_loading":null,"description":"Search the knowledge base for information relevant to the query.","output_schema":null}],"top_p":0.98,"background":false,"completed_at":1787819037.0,"conversation":null,"max_output_tokens":null,"max_tool_calls":null,"moderation":null,"previous_response_id":null,"prompt":null,"prompt_cache_key":null,"prompt_cache_options":null,"prompt_cache_retention":"in_memory","reasoning":{"context":"all_turns","effort":"medium","generate_summary":null,"mode":null,"summary":null},"safety_identifier":null,"service_tier":"default","status":"completed","text":{"format":{"type":"text"},"verbosity":"medium"},"top_logprobs":0,"truncation":"disabled","usage":{"input_tokens":566,"input_tokens_details":{"cache_write_tokens":0,"cached_tokens":0},"output_tokens":451,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":1017},"user":null,"billing":{"payer":"developer"},"frequency_penalty":0.0,"presence_penalty":0.0,"store":true}J
gen_ai.operation.name
chatJÊ
	raw_inputÿ
’[{"content":"How does RAG work and what are its key components?","role":"user"},{"id":"rs_4823637b666e58be9433f57145388668","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeLydn1jFhs7f1dZby7B2jLi62qz5pMKuocZ2Pv3ZbTR5AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBbmEvY0g0elV5SGdVVFpjSm50RWQvd0hQMTE4cTFwd2pXS2dpZ0ZsNzRVQ2E1Y0hYc01VVGpNcUpsZGNtRndJekE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcnilHOYt6zcu8/Xq4Gua14bjW1lTe7JDdX2xXBFrKc+5ls0wRpnX+0wjWt1I6G+dsZOfsoL+IGKJlgvItHATBnVJ5waleAyAtmuO5AxYPFafdD8uOdTjNRlgi03oCAAAQAN+hrtsWE0tzuyEBC1amnLRqMs6XAtYv6ctAORYGC27wAMK9eRHaffLpQg97PDMfXP////8AAAABAAAAAAAAAAAAAAABAAAAZXQRGXRPgjAo09I1sCK6aEkE5bSBiULALkcKK81hoDb0nYK4oOT7kxBChCnWKmrMgKrXEm+qizsK9yr6cfJiMEW3PAzTH6164aS/z3ko47uHVJdeP0R5i+CPmd73Bok7pwUdidSDM+cDqlodzLlJc+/o6TACjABnMGUCMQDsRa9Xz6lTLNkJuXoayup7+DOQPOytltYe8CgWsNXnZc4Qjj42a3na6E62myQmyh8CMEIvwUJ+WXtiovU0uIOE+w1vWarnfCCg8afRlOZZ88dbBePilWgZpnSPnHlx1bLf5w=="},{"arguments":"{\"query\":\"RAG retrieval augmented generation how it works key components retrieval embeddings vector database chunking reranking generation context\",\"num_results\":5}","call_id":"call_830505c14beb55dd9b4fe7c1a64b83e8","name":"search_knowledge","type":"function_call","id":"fc_567b7563d68659928e60c39fcd97183b","status":"completed"},{"call_id":"call_830505c14beb55dd9b4fe7c1a64b83e8","output":"[1] (relevance: 0.71) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.\n\n[2] (relevance: 0.19) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.\n\n[3] (relevance: 0.17) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.\n\n[4] (relevance: 0.16) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.\n\n[5] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.","type":"function_call_output"}]Jÿ-
eventsÕ-
 -[{"event.name":"gen_ai.system.message","content":"You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.\n\nWhen answering questions:\n1. ALWAYS use the search_knowledge tool first to find relevant information\n2. Base your answers on the retrieved context\n3. If the knowledge base doesn't have relevant information, say so clearly\n4. Be concise but thorough in your responses\n\nThe knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.","role":"system"},{"event.name":"gen_ai.user.message","content":"How does RAG work and what are its key components?","role":"user"},{"event.name":"gen_ai.unknown","role":"unknown","content":"reasoning\n\nSee JSON for details","data":{"id":"rs_4823637b666e58be9433f57145388668","summary":[],"type":"reasoning","content":[],"encrypted_content":"rsn_5ZVrif4J0bXIqmWdledj7QIFeLydn1jFhs7f1dZby7B2jLi62qz5pMKuocZ2Pv3ZbTR5AF8AAQAVYXdzLWNyeXB0by1wdWJsaWMta2V5AERBbmEvY0g0elV5SGdVVFpjSm50RWQvd0hQMTE4cTFwd2pXS2dpZ0ZsNzRVQ2E1Y0hYc01VVGpNcUpsZGNtRndJekE9PQABABFhd3Mta21zLWhpZXJhcmNoeQAKYmVybS1zdG9yZQBcnilHOYt6zcu8/Xq4Gua14bjW1lTe7JDdX2xXBFrKc+5ls0wRpnX+0wjWt1I6G+dsZOfsoL+IGKJlgvItHATBnVJ5waleAyAtmuO5AxYPFafdD8uOdTjNRlgi03oCAAAQAN+hrtsWE0tzuyEBC1amnLRqMs6XAtYv6ctAORYGC27wAMK9eRHaffLpQg97PDMfXP////8AAAABAAAAAAAAAAAAAAABAAAAZXQRGXRPgjAo09I1sCK6aEkE5bSBiULALkcKK81hoDb0nYK4oOT7kxBChCnWKmrMgKrXEm+qizsK9yr6cfJiMEW3PAzTH6164aS/z3ko47uHVJdeP0R5i+CPmd73Bok7pwUdidSDM+cDqlodzLlJc+/o6TACjABnMGUCMQDsRa9Xz6lTLNkJuXoayup7+DOQPOytltYe8CgWsNXnZc4Qjj42a3na6E62myQmyh8CMEIvwUJ+WXtiovU0uIOE+w1vWarnfCCg8afRlOZZ88dbBePilWgZpnSPnHlx1bLf5w=="}},{"event.name":"gen_ai.assistant.message","role":"assistant","tool_calls":[{"id":"call_830505c14beb55dd9b4fe7c1a64b83e8","type":"function","function":{"name":"search_knowledge","arguments":"{\"query\":\"RAG retrieval augmented generation how it works key components retrieval embeddings vector database chunking reranking generation context\",\"num_results\":5}"}}]},{"event.name":"gen_ai.tool.message","role":"tool","id":"call_830505c14beb55dd9b4fe7c1a64b83e8","content":"[1] (relevance: 0.71) RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.\n\n[2] (relevance: 0.19) Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.\n\n[3] (relevance: 0.17) Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.\n\n[4] (relevance: 0.16) Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.\n\n[5] (relevance: 0.12) OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.","name":"search_knowledge"},{"event.name":"gen_ai.assistant.message","content":"RAG (Retrieval-Augmented Generation) combines **information retrieval** with **LLM generation**. Instead of relying only on the model‚Äôs training data, it retrieves relevant information from an external knowledge base and provides it to the model as context.\n\n### How RAG works\n\n1. **Ingest documents**\n   - Collect source content such as PDFs, web pages, databases, or internal documents.\n   - Clean and split the content into smaller chunks.\n\n2. **Create embeddings**\n   - Convert each text chunk into a dense numerical vector using an embedding model.\n   - Similar meanings produce similar vectors.\n\n3. **Store vectors**\n   - Save the embeddings, along with the original text and metadata, in a vector database or index.\n\n4. **Process a user query**\n   - Convert the user‚Äôs question into an embedding using the same or a compatible embedding model.\n\n5. **Retrieve relevant content**\n   - Use vector search to find chunks whose embeddings are most similar to the query.\n   - Similarity is commonly measured with cosine similarity or L2 distance.\n   - Systems may also use metadata filters, keyword search, or reranking.\n\n6. **Generate an answer**\n   - Add the retrieved chunks to the LLM‚Äôs prompt as context.\n   - The LLM generates a response grounded in that retrieved information.\n\n### Key components\n\n- **Document ingestion and preprocessing:** Loading, cleaning, and chunking source data.\n- **Embedding model:** Converts text into semantic vectors.\n- **Vector database/index:** Stores and searches embeddings. Examples include FAISS, Pinecone, Chroma, and Weaviate.\n- **Retriever:** Finds relevant passages, often using similarity search.\n- **Reranker, optionally:** Reorders retrieved results to improve relevance.\n- **Prompt template:** Combines the user question and retrieved context.\n- **Generative LLM:** Produces the final answer.\n- **Metadata and filtering:** Restricts retrieval by attributes such as source, date, user, or document type.\n\nRAG can improve factuality and enable access to current or private information, but its quality depends heavily on chunking, embedding quality, retrieval accuracy, and the relevance of the context supplied to the LLM.","role":"assistant"}]J 
gen_ai.usage.input_tokens∂J!
gen_ai.usage.output_tokens√J<
logfire.msg-
+Responses API with 'us.openai.gpt-5.6-luna'Jæ
logfire.json_schema¶
£{"type":"object","properties":{"response_id":{},"usage":{"type":"object"},"gen_ai.system":{},"model_settings":{"type":"object","title":"ModelSettings","x-python-datatype":"dataclass"},"gen_ai.request.model":{},"gen_ai.response.model":{},"response":{"type":"object","title":"Response","x-python-datatype":"PydanticModel","properties":{"output":{"type":"array","items":{"type":"object","title":"ResponseOutputMessage","x-python-datatype":"PydanticModel","properties":{"content":{"type":"array","items":{"type":"object","title":"ResponseOutputText","x-python-datatype":"PydanticModel"}}}}},"tools":{"type":"array","items":{"type":"object","title":"FunctionTool","x-python-datatype":"PydanticModel"}},"reasoning":{"type":"object","title":"Reasoning","x-python-datatype":"PydanticModel"},"text":{"type":"object","title":"ResponseTextConfig","x-python-datatype":"PydanticModel","properties":{"format":{"type":"object","title":"ResponseFormatText","x-python-datatype":"PydanticModel"}}},"usage":{"type":"object","title":"ResponseUsage","x-python-datatype":"PydanticModel","properties":{"input_tokens_details":{"type":"object","title":"InputTokensDetails","x-python-datatype":"PydanticModel"},"output_tokens_details":{"type":"object","title":"OutputTokensDetails","x-python-datatype":"PydanticModel"}}}}},"gen_ai.operation.name":{},"raw_input":{"type":"array"},"events":{"type":"array"},"gen_ai.usage.input_tokens":{},"gen_ai.usage.output_tokens":{}}}z Ö   ‹
†BQq∏ÌÅÅ\Éc‹ÔPuìs¿Uf"·.‹∏\'µ*"Turn {turn} for agent RAGAssistant09–	xâõœAò/¯3ÒõœJ'
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
RAGAssistantJh
usage_
]{"input_tokens":566,"output_tokens":451,"cached_input_tokens":0,"cache_write_input_tokens":0}J.
logfire.msg
Turn 2 for agent RAGAssistantJ©
logfire.json_schemaë
é{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"turn":{},"agent_name":{},"usage":{"type":"object"}}}z Ö   ¡
†BQq∏ÌÅÅ\Éc‹ÔP·.‹∏\'µ"1Õ“}«cÏV*Agent run: {name!r}09Ë±*0õœA(w˝3ÒõœJ'
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
É{"type":"object","properties":{"name":{},"handoffs":{"type":"array"},"tools":{"type":"array"},"output_type":{},"gen_ai.system":{}}}z Ö   û
†BQq∏ÌÅÅ\Éc‹ÔP1Õ“}«cÏV" ≈4?Íí*Task: Agent workflow09‡!0õœA∞~ˇ3ÒõœJ'
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
taskJä
usageÄ
~{"input_tokens":760,"output_tokens":506,"cached_input_tokens":0,"cache_write_input_tokens":0,"requests":2,"total_tokens":1266}J%
logfire.msg
Task: Agent workflowJç
logfire.json_schemav
t{"type":"object","properties":{"type":{},"name":{},"gen_ai.system":{},"sdk_span_type":{},"usage":{"type":"object"}}}z Ö   ˚
†BQq∏ÌÅÅ\Éc‹ÔP ≈4?Íí" 9g≠ÈÄ¨Õ*OpenAI Agents trace: {name}09¶0õœAX] 4ÒõœJ'
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
&trace_b346530e901f42ec9736e05fd8b5e015J©
logfire.json_schemaë
é{"type":"object","properties":{"name":{},"agent_trace_id":{},"group_id":{"type":"null"},"metadata":{"type":"null"},"tracing":{"type":"null"}}}z Ö   