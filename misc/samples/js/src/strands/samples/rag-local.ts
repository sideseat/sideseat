/**
 * RAG sample demonstrating embeddings, vector search, and retrieval-augmented generation.
 *
 * This sample shows how to:
 * 1. Generate embeddings using Amazon Bedrock Titan Embeddings
 * 2. Store vectors in memory with cosine similarity search
 * 3. Retrieve relevant context based on semantic similarity
 * 4. Use retrieved context to augment LLM responses
 *
 * Prerequisites:
 * - AWS credentials with bedrock permissions
 * - AWS_REGION environment variable (default: us-east-1)
 */

import { Agent, tool } from '@strands-agents/sdk';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';
import { extractTextFromResponse } from '../../shared/response.js';
import { VectorStore } from '../../shared/tools/embeddings.js';
import { KNOWLEDGE_BASE } from '../../shared/tools/knowledge-base.js';

const SYSTEM_PROMPT = `You are a helpful AI assistant with access to a technical knowledge base about AI frameworks and observability.

When answering questions:
1. ALWAYS use the search_knowledge tool first to find relevant information
2. Base your answers on the retrieved context
3. If the knowledge base doesn't have relevant information, say so clearly
4. Be concise but thorough in your responses

The knowledge base contains information about Strands Agents, SideSeat, RAG, embeddings, OpenTelemetry, and vector search.`;

// Test queries that exercise the RAG pipeline
const QUERIES = [
  'What is SideSeat and how does it help with AI observability?',
  'How does RAG work and what are its key components?',
  'What embedding models are available for vector search?',
];

export async function run(modelId: string) {
  // Create and populate knowledge base
  console.log('Initializing RAG knowledge base...');
  const store = new VectorStore();

  console.log(`Indexing ${KNOWLEDGE_BASE.length} documents...`);
  await store.index(KNOWLEDGE_BASE);
  console.log('Knowledge base ready');

  // Create search tool bound to the knowledge base
  const searchTool = tool({
    name: 'search_knowledge',
    description: 'Search the knowledge base for information relevant to the query.',
    inputSchema: z.object({
      query: z.string().describe('The search query to find relevant information'),
      num_results: z.number().default(3).describe('Number of results to return'),
    }),
    callback: async ({ query, num_results }) => {
      const results = await store.search(query, num_results);

      if (results.length === 0) {
        return 'No relevant information found in the knowledge base.';
      }

      // Format results for the LLM
      return results
        .map((r, i) => `[${i + 1}] (relevance: ${r.score.toFixed(2)}) ${r.document.content}`)
        .join('\n\n');
    },
  });

  // Create agent with search tool
  console.log('\nCreating RAG agent...');
  const agent = new Agent({
    model: resolveModel(modelId),
    tools: [searchTool],
    systemPrompt: SYSTEM_PROMPT,
  });

  // Run test queries
  for (let i = 0; i < QUERIES.length; i++) {
    const query = QUERIES[i];
    console.log(`\n${'='.repeat(60)}`);
    console.log(`Query ${i + 1}: ${query}`);
    console.log('-'.repeat(60));

    const response = await agent.invoke(query);
    const answer = extractTextFromResponse(response);
    console.log(`Answer: ${answer}`);
  }
}
