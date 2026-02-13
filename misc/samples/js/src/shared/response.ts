/**
 * Utilities for extracting content from SDK responses.
 *
 * Strands SDK responses have varying shapes depending on the operation.
 * These utilities normalize response extraction across samples.
 */

/**
 * Extract text content from a Strands agent response message.
 * Handles both string and content block array formats.
 */
export function extractTextFromMessage(message: unknown): string {
  if (!message || typeof message !== 'object') return String(message);

  const msg = message as Record<string, unknown>;
  const content = msg.content;

  if (typeof content === 'string') return content;

  if (Array.isArray(content)) {
    const texts: string[] = [];
    for (const block of content) {
      if (typeof block === 'string') {
        texts.push(block);
      } else if (typeof block === 'object' && block !== null) {
        const b = block as Record<string, unknown>;
        if (b.type === 'text' && typeof b.text === 'string') {
          texts.push(b.text);
        }
      }
    }
    if (texts.length > 0) return texts.join('\n');
  }

  return String(message);
}

/**
 * Extract text content from a full Strands agent response.
 * Unwraps the message wrapper if present.
 */
export function extractTextFromResponse(response: unknown): string {
  if (!response || typeof response !== 'object') return String(response);
  if (typeof response === 'string') return response;

  const resp = response as Record<string, unknown>;

  // Try to unwrap message wrapper
  if (resp.message && typeof resp.message === 'object') {
    return extractTextFromMessage(resp.message);
  }

  // Try direct message extraction
  return extractTextFromMessage(response);
}

/**
 * Extract thinking/reasoning content from a Strands response if available.
 * Returns null if no thinking content found.
 */
export function extractThinkingFromResponse(response: unknown): string | null {
  if (!response || typeof response !== 'object') return null;

  const resp = response as Record<string, unknown>;
  if (!resp.message || typeof resp.message !== 'object') return null;

  const message = resp.message as Record<string, unknown>;
  const content = message.content;
  if (!Array.isArray(content)) return null;

  for (const block of content) {
    if (typeof block === 'object' && block !== null) {
      const b = block as Record<string, unknown>;
      // Anthropic thinking block format
      if (b.type === 'thinking') {
        return (b.thinking ?? b.text) as string;
      }
      // Alternative reasoning format
      if (b.type === 'reasoning') {
        return (b.reasoning ?? b.text) as string;
      }
    }
  }
  return null;
}
