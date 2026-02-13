"""RAG sample demonstrating embeddings, vector search, and retrieval-augmented generation.

Demonstrates:
- Embedding generation using Amazon Bedrock Titan Embeddings
- In-memory vector store with cosine similarity search
- RAG pattern: retrieve context before generation
- Tool-based knowledge retrieval in ReAct agent

Prerequisites:
- AWS credentials with bedrock permissions
- AWS_REGION environment variable (default: us-east-1)
"""

import json
import os
from typing import Optional

import boto3
import numpy as np
from langchain_core.messages import AIMessage, SystemMessage
from langchain_core.tools import tool
from langgraph.prebuilt import create_react_agent

# Constants
AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))
EMBEDDING_MODEL = "amazon.titan-embed-text-v2:0"
DEFAULT_TOP_K = 3

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

    def __init__(self, bedrock_client):
        """Initialize with Bedrock client for embeddings.

        Args:
            bedrock_client: boto3 bedrock-runtime client
        """
        self.bedrock = bedrock_client
        self.documents: list[dict] = []
        self.embeddings: list[np.ndarray] = []

    def _embed(self, text: str) -> Optional[np.ndarray]:
        """Generate embedding via Bedrock Titan.

        Args:
            text: Text to embed

        Returns:
            Embedding vector as numpy array, or None on error
        """
        try:
            response = self.bedrock.invoke_model(
                modelId=EMBEDDING_MODEL,
                body=json.dumps({"inputText": text}),
                contentType="application/json",
            )
            result = json.loads(response["body"].read())
            return np.array(result["embedding"], dtype=np.float32)
        except Exception as e:
            print(f"[Embedding Error: {e}]")
            return None

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
            if embedding is not None:
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
        if query_embedding is None:
            return []

        scores = [self._cosine_similarity(query_embedding, emb) for emb in self.embeddings]
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
    - Embedding generation with Bedrock Titan
    - In-memory vector store implementation
    - Cosine similarity search
    - ReAct agent with knowledge retrieval tool

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    # Initialize Bedrock client
    try:
        boto_session = boto3.Session(region_name=AWS_REGION)
        bedrock = boto_session.client("bedrock-runtime")
    except Exception as e:
        print(f"[Error creating Bedrock client: {e}]")
        return

    # Create and populate knowledge base
    print("Initializing RAG knowledge base...")
    kb = RAGKnowledgeBase(bedrock)

    print(f"Indexing {len(KNOWLEDGE_BASE)} documents...")
    indexed = kb.index(KNOWLEDGE_BASE)
    print(f"Knowledge base ready ({indexed} documents indexed)")

    if indexed == 0:
        print("[Error: No documents indexed, check Bedrock access]")
        return

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

        try:
            result = agent.invoke({"messages": [("user", query)]}, config=config)
            print(f"Answer: {extract_response(result)}")
        except Exception as e:
            print(f"[Error: {e}]")
