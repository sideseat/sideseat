import { API_BASE_URL } from "../api-client";
import type { BaseEvent, RunAgentInput } from "./types";

export class RunError extends Error {
  code: string;
  status: number;

  constructor(code: string, message: string, status: number) {
    super(message);
    this.name = "RunError";
    this.code = code;
    this.status = status;
  }
}

interface RunAgentStreamOptions {
  baseUrl?: string;
  projectId: string;
  agentName: string;
  input: RunAgentInput;
  signal: AbortSignal;
}

/**
 * Async iterator over the AG-UI run endpoint's SSE response. The endpoint
 * emits `data: <json>\n\n` frames with no `event:` line.
 */
export async function* runAgentStream(opts: RunAgentStreamOptions): AsyncGenerator<BaseEvent> {
  const baseUrl = opts.baseUrl ?? API_BASE_URL;
  const url = `${baseUrl}/project/${opts.projectId}/agents/${opts.agentName}/runs`;

  let res: Response;
  try {
    res = await fetch(url, {
      method: "POST",
      credentials: "include",
      signal: opts.signal,
      headers: {
        "Content-Type": "application/json",
        Accept: "text/event-stream",
      },
      body: JSON.stringify(opts.input),
    });
  } catch (e) {
    if (opts.signal.aborted) {
      throw new RunError("client_aborted", "Cancelled by user", 0);
    }
    throw new RunError(
      "network_error",
      e instanceof Error ? e.message : String(e),
      0,
    );
  }

  if (!res.ok) {
    let code = "unknown";
    let message = res.statusText;
    try {
      const body = (await res.json()) as { error?: string; code?: string; message?: string };
      code = body.code ?? body.error ?? "unknown";
      message = body.message ?? res.statusText;
    } catch {
      // Empty / non-JSON body: keep defaults.
    }
    throw new RunError(code, message, res.status);
  }

  if (!res.body) {
    throw new RunError("no_body", "missing response body", 0);
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";

  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });

      let split = findBlankLine(buf);
      while (split) {
        const frame = buf.slice(0, split.idx);
        buf = buf.slice(split.idx + split.len);

        const data = frame
          .split(/\r?\n/)
          .filter((l) => l.startsWith("data:"))
          .map((l) => l.slice(5).replace(/^ /, ""))
          .join("\n");

        if (data) {
          try {
            yield JSON.parse(data) as BaseEvent;
          } catch {
            // Skip malformed frame.
          }
        }
        split = findBlankLine(buf);
      }
    }
  } finally {
    try {
      reader.releaseLock();
    } catch {
      // Reader may already be released by abort; ignore.
    }
  }
}

function findBlankLine(s: string): { idx: number; len: number } | null {
  const a = s.indexOf("\n\n");
  const b = s.indexOf("\r\n\r\n");
  if (a < 0 && b < 0) return null;
  if (a < 0) return { idx: b, len: 4 };
  if (b < 0 || a < b) return { idx: a, len: 2 };
  return { idx: b, len: 4 };
}
