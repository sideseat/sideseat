/**
 * Trace utilities for sample runs.
 *
 * Note: Strands JS SDK doesn't have native trace_attributes like Python.
 * This just creates session/user IDs for logging purposes.
 */

export interface TraceAttributes {
  'session.id': string;
  'user.id': string;
}

/**
 * Create trace attributes for a sample run.
 * Matches Python's create_trace_attributes() format.
 */
export function createTraceAttributes(sampleName: string): TraceAttributes {
  const sessionId = `strands-${sampleName}-${randomHex(8)}`;
  return {
    'session.id': sessionId,
    'user.id': 'demo-user',
  };
}

function randomHex(length: number): string {
  const chars = '0123456789abcdef';
  let result = '';
  for (let i = 0; i < length; i++) {
    result += chars[Math.floor(Math.random() * chars.length)];
  }
  return result;
}
