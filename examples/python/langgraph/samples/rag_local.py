"""RAG sample demonstrating local embeddings and retrieval-augmented generation.

Demonstrates:
- Deterministic local feature-hashing embeddings
- In-memory vector store with cosine similarity search
- RAG pattern: retrieve context before generation
- Tool-based knowledge retrieval in ReAct agent
"""

import hashlib
import re

import numpy as np
from langchain_core.messages import AIMessage, SystemMessage
from langchain_core.tools import tool

from langgraph.prebuilt import create_react_agent

DEFAULT_TOP_K = 3
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
    """In-memory RAG system with embeddings and vector search."""

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
        """Compute cosine similarity between two vectors.

        Args:
            a: First vector
            b: Second vector

        Returns:
            Cosine similarity score (0-1), or 0 if vectors are invalid
        """
        norm_a = np.linalg.norm(a)
        norm_b = np.linalg.norm(b)

        if norm_a == 0 or norm_b == 0:
            return 0.0

        return float(np.dot(a, b) / (norm_a * norm_b))

    def index(self, documents: list[dict]) -> int:
        """Index documents by generating and storing their embeddings.

        Args:
            documents: List of dicts with 'id' and 'content' keys

        Returns:
            Number of documents successfully indexed
        """
        indexed = 0
        for doc in documents:
            embedding = self._embed(doc["content"])
            self.documents.append(doc)
            self.embeddings.append(embedding)
            indexed += 1
        return indexed

    def search(self, query: str, k: int = DEFAULT_TOP_K) -> list[dict]:
        """Search for similar documents using cosine similarity.

        Args:
            query: Search query text
            k: Number of results to return

        Returns:
            List of dicts with 'document' and 'score' keys
        """
        query_embedding = self._embed(query)
        scores = [
            self._cosine_similarity(query_embedding, emb) for emb in self.embeddings
        ]
        ranked = sorted(enumerate(scores), key=lambda x: x[1], reverse=True)[:k]

        return [{"document": self.documents[i], "score": score} for i, score in ranked]


def create_search_tool(kb: RAGKnowledgeBase):
    """Create a search tool bound to the knowledge base instance.

    Args:
        kb: RAGKnowledgeBase instance

    Returns:
        LangChain tool function
    """

    @tool
    def search_knowledge(query: str, num_results: int = DEFAULT_TOP_K) -> str:
        """Search the knowledge base for information relevant to the query.

        Args:
            query: The search query to find relevant information
            num_results: Number of results to return (1-5, default: 3)

        Returns:
            Formatted search results with relevance scores
        """
        num_results = max(1, min(num_results, 5))
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


def extract_response(result: dict) -> str:
    """Extract the final text response from agent result."""
    messages = result.get("messages", [])
    for msg in reversed(messages):
        if isinstance(msg, AIMessage) and msg.content:
            if isinstance(msg.content, str):
                return msg.content
            if isinstance(msg.content, list):
                for block in msg.content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        return block.get("text", "")
    return "[No response generated]"


def run(model, trace_attrs: dict):
    """Run the RAG sample demonstrating retrieval-augmented generation.

    This sample shows:
    - Deterministic local embedding generation
    - In-memory vector store implementation
    - Cosine similarity search
    - ReAct agent with knowledge retrieval tool

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    # Create and populate knowledge base
    print("Initializing RAG knowledge base...")
    kb = RAGKnowledgeBase()

    print(f"Indexing {len(KNOWLEDGE_BASE)} documents...")
    indexed = kb.index(KNOWLEDGE_BASE)
    print(f"Knowledge base ready ({indexed} documents indexed)")

    # Create agent with search tool
    print("\nCreating RAG agent...")
    agent = create_react_agent(
        model=model,
        tools=[create_search_tool(kb)],
        prompt=SystemMessage(content=SYSTEM_PROMPT),
    )

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

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

        result = agent.invoke({"messages": [("user", query)]}, config=config)
        print(f"Answer: {extract_response(result)}")
