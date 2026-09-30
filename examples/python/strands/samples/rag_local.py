"""RAG sample demonstrating local embeddings and retrieval-augmented generation.

This sample shows how to:
1. Generate deterministic embeddings locally
2. Store vectors in memory with cosine similarity search
3. Retrieve relevant context based on semantic similarity
4. Use retrieved context to augment LLM responses
"""

import hashlib
import re

import numpy as np

from strands import Agent, tool

EMBEDDING_DIMENSIONS = 64

# Self-contained knowledge base for the demo
KNOWLEDGE_BASE = [
    {
        "id": "strands",
        "content": "Strands Agents is an AI framework for building agent applications. It supports tools, multi-agent swarms, and structured outputs. It integrates with AWS Bedrock, OpenAI, Anthropic, and Google models. Agents can use tools defined with the @tool decorator.",
    },
    {
        "id": "sideseat",
        "content": "SideSeat is an AI observability toolkit that collects OpenTelemetry traces from AI applications. It normalizes multi-framework data into SideML format and provides a web UI for debugging LLM calls, analyzing costs, and measuring latency.",
    },
    {
        "id": "rag",
        "content": "RAG (Retrieval-Augmented Generation) is a pattern that combines vector search with LLM generation. Documents are embedded into vectors, stored in a vector database, and retrieved based on semantic similarity to enhance LLM context and reduce hallucinations.",
    },
    {
        "id": "embeddings",
        "content": "Embeddings are dense vector representations of text that capture semantic meaning. Similar texts have similar embeddings. Common models include Amazon Titan Embeddings, OpenAI text-embedding-3, and Cohere Embed. Typical dimensions range from 256 to 3072.",
    },
    {
        "id": "otel",
        "content": "OpenTelemetry GenAI semantic conventions define standard attributes for AI/LLM observability. Key attributes include gen_ai.operation.name, gen_ai.request.model, gen_ai.usage.input_tokens, and gen_ai.usage.output_tokens.",
    },
    {
        "id": "vectors",
        "content": "Vector search finds similar items by comparing embedding vectors using distance metrics like cosine similarity or L2 distance. Popular vector databases include FAISS, Pinecone, Chroma, and Weaviate. Cosine similarity measures the angle between vectors.",
    },
]

SYSTEM_PROMPT = """You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.

When answering questions:
1. ALWAYS use the search_knowledge tool first to find relevant information
2. Base your answers on the retrieved context
3. If the knowledge base doesn't have relevant information, say so clearly
4. Be concise but thorough in your responses

The knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search."""


class RAGKnowledgeBase:
    """Encapsulated RAG system with embeddings and vector search."""

    def __init__(self):
        self.documents: list[dict] = []
        self.embeddings: list[np.ndarray] = []

    def _embed(self, text: str) -> np.ndarray:
        """Generate a stable local feature-hashing embedding."""
        embedding = np.zeros(EMBEDDING_DIMENSIONS, dtype=np.float32)
        for token in re.findall(r"[a-z0-9]+", text.lower()):
            digest = hashlib.sha256(token.encode()).digest()
            index = int.from_bytes(digest[:4], "big") % EMBEDDING_DIMENSIONS
            embedding[index] += 1.0 if digest[4] % 2 == 0 else -1.0
        norm = np.linalg.norm(embedding)
        return embedding if norm == 0 else embedding / norm

    def _cosine_similarity(self, a: np.ndarray, b: np.ndarray) -> float:
        """Compute cosine similarity between two vectors."""
        return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b)))

    def index(self, documents: list[dict]):
        """Index documents by generating and storing their embeddings."""
        for doc in documents:
            embedding = self._embed(doc["content"])
            self.documents.append(doc)
            self.embeddings.append(embedding)

    def search(self, query: str, k: int = 3) -> list[dict]:
        """Search for similar documents using cosine similarity."""
        query_embedding = self._embed(query)
        scores = [
            self._cosine_similarity(query_embedding, emb) for emb in self.embeddings
        ]
        ranked = sorted(enumerate(scores), key=lambda x: x[1], reverse=True)[:k]
        return [{"document": self.documents[i], "score": score} for i, score in ranked]


def create_search_tool(kb: RAGKnowledgeBase):
    """Create a search tool bound to the knowledge base instance."""

    @tool
    def search_knowledge(query: str, num_results: int = 3) -> str:
        """Search the knowledge base for information relevant to the query.

        Args:
            query: The search query to find relevant information
            num_results: Number of results to return (default: 3)
        """
        results = kb.search(query, k=num_results)

        if not results:
            return "No relevant information found in the knowledge base."

        # Format results for the LLM
        context_parts = []
        for i, result in enumerate(results, 1):
            doc = result["document"]
            score = result["score"]
            context_parts.append(f"[{i}] (relevance: {score:.2f}) {doc['content']}")

        return "\n\n".join(context_parts)

    return search_knowledge


def run(model, trace_attrs: dict):
    """Run the RAG sample."""
    # Create and populate knowledge base
    print("Initializing RAG knowledge base...")
    kb = RAGKnowledgeBase()

    print(f"Indexing {len(KNOWLEDGE_BASE)} documents...")
    kb.index(KNOWLEDGE_BASE)
    print("Knowledge base ready")

    # Create agent with search tool
    print("\nCreating RAG agent...")
    agent = Agent(
        model=model,
        tools=[create_search_tool(kb)],
        system_prompt=SYSTEM_PROMPT,
        trace_attributes=trace_attrs,
    )

    # Test queries that exercise the RAG pipeline
    queries = [
        "What is SideSeat and how does it help with AI observability?",
        "How does RAG work and what are its key components?",
        "What embedding models are available for vector search?",
    ]

    for i, query in enumerate(queries, 1):
        print(f"\n{'=' * 60}")
        print(f"Query {i}: {query}")
        print("-" * 60)
        response = agent(query)
        print(f"Answer: {response.message['content'][0]['text']}")
