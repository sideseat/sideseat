// Shared knowledge base for RAG samples - avoids duplication
export const KNOWLEDGE_BASE = [
  {
    id: 'strands',
    content:
      'Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models.',
  },
  {
    id: 'sideseat',
    content:
      'SideSeat is an AI observability toolkit that collects OpenTelemetry traces from AI applications. It normalizes multi-framework data into SideML format and provides a web UI for debugging.',
  },
  {
    id: 'rag',
    content:
      'RAG (Retrieval-Augmented Generation) combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity.',
  },
  {
    id: 'embeddings',
    content:
      'Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings and OpenAI text-embedding-3.',
  },
  {
    id: 'otel',
    content:
      'OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, and gen_ai.usage.input_tokens.',
  },
  {
    id: 'vectors',
    content:
      'Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate.',
  },
];
