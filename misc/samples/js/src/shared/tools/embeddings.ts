import { InvokeModelCommand } from '@aws-sdk/client-bedrock-runtime';
import { getBedrockClient } from '../aws-client.js';
import { config } from '../config.js';

export interface EmbeddingResult {
  embedding: number[];
  inputTextTokenCount: number;
}

export async function generateEmbedding(text: string): Promise<EmbeddingResult> {
  const client = getBedrockClient();
  const command = new InvokeModelCommand({
    modelId: config.models.embedding,
    contentType: 'application/json',
    body: JSON.stringify({ inputText: text }),
  });

  const response = await client.send(command);
  const result = JSON.parse(new TextDecoder().decode(response.body));
  return {
    embedding: result.embedding,
    inputTextTokenCount: result.inputTextTokenCount,
  };
}

export function cosineSimilarity(a: number[], b: number[]): number {
  let dotProduct = 0;
  let normA = 0;
  let normB = 0;
  for (let i = 0; i < a.length; i++) {
    dotProduct += a[i] * b[i];
    normA += a[i] * a[i];
    normB += b[i] * b[i];
  }
  const denominator = Math.sqrt(normA) * Math.sqrt(normB);
  // Return 0 for zero vectors to avoid NaN
  return denominator === 0 ? 0 : dotProduct / denominator;
}

export interface Document {
  id: string;
  content: string;
}

export interface SearchResult {
  document: Document;
  score: number;
}

// RAG helper class for vector similarity search
export class VectorStore {
  private documents: Document[] = [];
  private embeddings: number[][] = [];

  /**
   * Index documents by generating embeddings.
   * Uses batched parallel processing for better performance while
   * respecting API rate limits.
   */
  async index(docs: Document[], batchSize = 3): Promise<void> {
    // Process in batches to balance speed and API rate limits
    for (let i = 0; i < docs.length; i += batchSize) {
      const batch = docs.slice(i, i + batchSize);
      const results = await Promise.all(
        batch.map(async (doc) => {
          const { embedding } = await generateEmbedding(doc.content);
          return { doc, embedding };
        })
      );
      for (const { doc, embedding } of results) {
        this.documents.push(doc);
        this.embeddings.push(embedding);
      }
    }
  }

  async search(query: string, k = 3): Promise<SearchResult[]> {
    const { embedding: queryEmb } = await generateEmbedding(query);
    const scored = this.embeddings.map((emb, i) => ({
      document: this.documents[i],
      score: cosineSimilarity(queryEmb, emb),
    }));
    return scored.sort((a, b) => b.score - a.score).slice(0, k);
  }
}
