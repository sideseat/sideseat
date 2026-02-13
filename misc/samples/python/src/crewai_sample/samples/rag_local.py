"""RAG sample demonstrating embeddings, vector search, and retrieval-augmented generation.

This sample shows how to:
1. Generate embeddings using Amazon Bedrock Titan Embeddings
2. Store vectors in memory with cosine similarity search
3. Retrieve relevant context based on semantic similarity
4. Use retrieved context to augment LLM responses

Prerequisites:
- AWS credentials with bedrock permissions
- AWS_REGION environment variable (default: us-east-1)
"""

import json
import os

import boto3
import numpy as np
from crewai import Agent, Crew, Process, Task
from crewai.tools import tool
from opentelemetry import trace

AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))
EMBEDDING_MODEL = "amazon.titan-embed-text-v2:0"

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

    def __init__(self, bedrock_client):
        self.bedrock = bedrock_client
        self.documents: list[dict] = []
        self.embeddings: list[np.ndarray] = []

    def _embed(self, text: str) -> np.ndarray:
        """Generate embedding via Bedrock Titan."""
        response = self.bedrock.invoke_model(
            modelId=EMBEDDING_MODEL,
            body=json.dumps({"inputText": text}),
            contentType="application/json",
        )
        result = json.loads(response["body"].read())
        return np.array(result["embedding"], dtype=np.float32)

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
        scores = [self._cosine_similarity(query_embedding, emb) for emb in self.embeddings]
        ranked = sorted(enumerate(scores), key=lambda x: x[1], reverse=True)[:k]
        return [{"document": self.documents[i], "score": score} for i, score in ranked]


# Global knowledge base instance (set in run())
_kb: RAGKnowledgeBase | None = None


@tool("search_knowledge")
def search_knowledge(query: str, num_results: int = 3) -> str:
    """Search the knowledge base for information relevant to the query.

    Args:
        query: The search query to find relevant information
        num_results: Number of results to return (default: 3)
    """
    if _kb is None:
        return "Knowledge base not initialized."

    results = _kb.search(query, k=num_results)

    if not results:
        return "No relevant information found in the knowledge base."

    # Format results for the LLM
    context_parts = []
    for i, result in enumerate(results, 1):
        doc = result["document"]
        score = result["score"]
        context_parts.append(f"[{i}] (relevance: {score:.2f}) {doc['content']}")

    return "\n\n".join(context_parts)


def run(llm, trace_attrs: dict):
    """Run the RAG sample."""
    global _kb

    tracer = trace.get_tracer(__name__)

    # Initialize Bedrock client
    boto_session = boto3.Session(region_name=AWS_REGION)
    bedrock = boto_session.client("bedrock-runtime")

    # Create and populate knowledge base
    print("Initializing RAG knowledge base...")
    _kb = RAGKnowledgeBase(bedrock)

    print(f"Indexing {len(KNOWLEDGE_BASE)} documents...")
    _kb.index(KNOWLEDGE_BASE)
    print("Knowledge base ready")

    # Create agent with search tool
    print("\nCreating RAG agent...")

    rag_agent = Agent(
        role="Knowledge Assistant",
        goal="Answer questions using the knowledge base",
        backstory=SYSTEM_PROMPT,
        llm=llm,
        tools=[search_knowledge],
        verbose=False,
    )

    # Test queries that exercise the RAG pipeline
    queries = [
        "What is SideSeat and how does it help with AI observability?",
        "How does RAG work and what are its key components?",
        "What embedding models are available for vector search?",
    ]

    with tracer.start_as_current_span(
        "crewai.session",
        attributes=trace_attrs,
    ):
        for i, query in enumerate(queries, 1):
            print(f"\n{'=' * 60}")
            print(f"Query {i}: {query}")
            print("-" * 60)

            task = Task(
                description=query,
                expected_output="A comprehensive answer based on the knowledge base",
                agent=rag_agent,
            )

            crew = Crew(
                agents=[rag_agent],
                tasks=[task],
                process=Process.sequential,
                verbose=False,
                share_crew=False,
            )

            result = crew.kickoff()
            print(f"Answer: {result.raw}")
