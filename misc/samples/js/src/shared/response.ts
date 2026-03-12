/**
 * Utilities for extracting content from SDK responses.
 *
 * Handles both legacy response shapes and the AgentResult type from SDK 0.6.0+.
 * AgentResult: { type: 'agentResult', lastMessage: { content: ContentBlock[] } }
 * ContentBlock types: { type: 'textBlock', text: string } | { type: 'reasoningBlock', text?: string }
 */

type Block = { type: string; text?: string };

function getContentBlocks(response: unknown): Block[] {
  if (!response || typeof response !== 'object') return [];
  const r = response as Record<string, unknown>;

  // AgentResult from SDK 0.6.0+
  if (r.type === 'agentResult' && r.lastMessage) {
    const msg = r.lastMessage as Record<string, unknown>;
    if (Array.isArray(msg.content)) return msg.content as Block[];
  }

  // Legacy format: { message: { content: [...] } }
  const msg = r.message;
  if (msg && typeof msg === 'object') {
    const m = msg as Record<string, unknown>;
    if (Array.isArray(m.content)) return m.content as Block[];
  }

  return [];
}

/**
 * Extract text content from a Strands agent response.
 */
export function extractTextFromResponse(response: unknown): string {
  if (typeof response === 'string') return response;

  const blocks = getContentBlocks(response);
  const texts = blocks
    .filter((b) => b.type === 'textBlock' && typeof b.text === 'string')
    .map((b) => b.text as string);

  if (texts.length > 0) return texts.join('\n');

  // Fallback: try toString()
  if (response && typeof (response as Record<string, unknown>).toString === 'function') {
    const str = String(response);
    if (str !== '[object Object]') return str;
  }

  return String(response);
}

/**
 * Extract thinking/reasoning content from a Strands response if available.
 * Returns null if no thinking content found.
 */
export function extractThinkingFromResponse(response: unknown): string | null {
  const blocks = getContentBlocks(response);

  for (const block of blocks) {
    if (block.type === 'reasoningBlock' && typeof block.text === 'string') {
      return block.text;
    }
    // Legacy thinking block format
    if (block.type === 'thinking' && typeof block.text === 'string') {
      return block.text;
    }
  }

  return null;
}
