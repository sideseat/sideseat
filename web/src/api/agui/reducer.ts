/**
 * Pure reducer: AG-UI events -> rendered Message[].
 *
 * Adapted from engagement-mck/solution/site/src/lib/agui/reducer.ts.
 * Trimmed to the events SideSeat agents emit; unknown events are
 * preserved in `eventLog` / `customEvents` / `rawEvents` so debug
 * panels surface everything.
 */

import { uuid } from "@/lib/utils";
import type {
  BaseEvent,
  ChatState,
  CustomEventRecord,
  ErrorMessage,
  Message,
  RawEventRecord,
  ReasoningMessage,
  RunStatusMessage,
  StepMessage,
  TextMessage,
  ToolCallMessage,
} from "./types";

const EVENT_LOG_CAP = 500;

export type Action =
  | { type: "event"; event: BaseEvent }
  | { type: "append_user"; content: string; id: string }
  | { type: "reset"; threadId: string }
  | { type: "client_aborted" };

export function initialState(threadId: string): ChatState {
  return {
    messages: [],
    runState: "idle",
    latestState: null,
    customEvents: [],
    rawEvents: [],
    eventLog: [],
    threadId,
    tokenUsage: undefined,
  };
}

export function reduce(state: ChatState, action: Action): ChatState {
  switch (action.type) {
    case "reset":
      return initialState(action.threadId);
    case "append_user":
      return pushMessage(state, {
        kind: "text",
        id: action.id,
        role: "user",
        content: action.content,
        streaming: false,
      });
    case "client_aborted":
      return {
        ...state,
        runState: "errored",
        messages: [
          ...state.messages,
          {
            kind: "error",
            id: `err-${uuid()}`,
            code: "client_aborted",
            message: "Cancelled by user",
          } satisfies ErrorMessage,
        ],
      };
    case "event":
      return applyEvent(logEvent(state, action.event), action.event);
  }
}

function logEvent(state: ChatState, event: BaseEvent): ChatState {
  const next = [...state.eventLog, event];
  if (next.length > EVENT_LOG_CAP) next.splice(0, next.length - EVENT_LOG_CAP);
  return { ...state, eventLog: next };
}

function applyEvent(state: ChatState, event: BaseEvent): ChatState {
  const t = String(event.type ?? "");

  // Run lifecycle.
  if (t === "RUN_STARTED") {
    return pushMessage(
      { ...state, runState: "running" },
      { kind: "run_status", id: `run-started-${field(event, "run_id") ?? uuid()}`, phase: "started" } satisfies RunStatusMessage,
    );
  }
  if (t === "RUN_FINISHED") {
    const usage = parseUsage(event);
    return pushMessage(
      {
        ...state,
        runState: "finished",
        tokenUsage: usage ? mergeUsage(state.tokenUsage, usage) : state.tokenUsage,
      },
      { kind: "run_status", id: `run-finished-${field(event, "run_id") ?? uuid()}`, phase: "finished" } satisfies RunStatusMessage,
    );
  }
  if (t === "RUN_ERROR") {
    const code = String(field(event, "code") ?? "internal");
    const message = String(field(event, "message") ?? "Run failed");
    return {
      ...state,
      runState: "errored",
      messages: [
        ...state.messages,
        { kind: "error", id: `err-${uuid()}`, code, message } satisfies ErrorMessage,
      ],
    };
  }

  // Steps.
  if (t === "STEP_STARTED") {
    const stepName = String(field(event, "step_name") ?? field(event, "stepName") ?? "step");
    return pushMessage(state, {
      kind: "step",
      id: `step-${stepName}-${state.messages.length}`,
      stepName,
    } satisfies StepMessage);
  }
  if (t === "STEP_FINISHED") return state;

  // Text streaming.
  if (t === "TEXT_MESSAGE_START") {
    const id = String(field(event, "message_id") ?? field(event, "messageId") ?? uuid());
    const role = (field(event, "role") as "user" | "assistant" | undefined) ?? "assistant";
    return pushMessage(state, {
      kind: "text",
      id,
      role,
      content: "",
      streaming: true,
    } satisfies TextMessage);
  }
  if (t === "TEXT_MESSAGE_CONTENT" || t === "TEXT_MESSAGE_CHUNK") {
    const id = String(field(event, "message_id") ?? field(event, "messageId") ?? "");
    const delta = String(field(event, "delta") ?? "");
    if (!delta) return state;
    return patchById(state, id, (m) =>
      m.kind === "text" ? { ...m, content: m.content + delta } : m,
    );
  }
  if (t === "TEXT_MESSAGE_END") {
    const id = String(field(event, "message_id") ?? field(event, "messageId") ?? "");
    return patchById(state, id, (m) =>
      m.kind === "text" ? { ...m, streaming: false } : m,
    );
  }

  // Reasoning / thinking (treated identically).
  if (
    t === "REASONING_START" ||
    t === "THINKING_START" ||
    t === "THINKING_TEXT_MESSAGE_START"
  ) {
    return openReasoning(state, event);
  }
  if (
    t === "REASONING_CONTENT" ||
    t === "THINKING_TEXT_MESSAGE_CONTENT"
  ) {
    const delta = String(field(event, "delta") ?? "");
    if (!delta) return state;
    return appendToLastReasoning(state, delta);
  }
  if (
    t === "REASONING_END" ||
    t === "THINKING_END" ||
    t === "THINKING_TEXT_MESSAGE_END"
  ) {
    return finalizeLastReasoning(state);
  }

  // Tool calls.
  if (t === "TOOL_CALL_START") {
    const id = String(field(event, "tool_call_id") ?? field(event, "toolCallId") ?? uuid());
    const toolName = String(
      field(event, "tool_call_name") ??
        field(event, "toolCallName") ??
        field(event, "name") ??
        "tool",
    );
    return pushMessage(state, {
      kind: "tool_call",
      id,
      toolName,
      args: "",
      result: null,
      done: false,
    } satisfies ToolCallMessage);
  }
  if (t === "TOOL_CALL_ARGS") {
    const id = String(field(event, "tool_call_id") ?? field(event, "toolCallId") ?? "");
    const delta = String(field(event, "delta") ?? "");
    if (!delta) return state;
    return patchById(state, id, (m) =>
      m.kind === "tool_call" ? { ...m, args: m.args + delta } : m,
    );
  }
  if (t === "TOOL_CALL_END") {
    const id = String(field(event, "tool_call_id") ?? field(event, "toolCallId") ?? "");
    return patchById(state, id, (m) =>
      m.kind === "tool_call" ? { ...m, done: true } : m,
    );
  }
  if (t === "TOOL_CALL_RESULT") {
    const id = String(field(event, "tool_call_id") ?? field(event, "toolCallId") ?? "");
    const content = field(event, "content");
    const result = content === undefined ? "" : typeof content === "string" ? content : JSON.stringify(content);
    return patchById(state, id, (m) =>
      m.kind === "tool_call" ? { ...m, result, done: true } : m,
    );
  }

  // State.
  if (t === "STATE_SNAPSHOT") {
    return { ...state, latestState: field(event, "snapshot") ?? field(event, "state") ?? null };
  }
  if (t === "STATE_DELTA") {
    return { ...state, latestState: field(event, "delta") ?? state.latestState };
  }

  // MESSAGES_SNAPSHOT carries the agent's view of full history (including
  // tool-result rows and tool-use-only assistant rows with non-string
  // `content`). The streaming event stream already drives every visible
  // message, so applying the snapshot only causes duplicates and bogus
  // "Thinking…" bubbles. We log it for the debug panel but never mutate
  // `messages`. Re-enable selectively if a future agent depends on it.
  if (t === "MESSAGES_SNAPSHOT") {
    return state;
  }

  // Custom.
  if (t === "CUSTOM") {
    const name = String(field(event, "name") ?? "custom");
    const value = field(event, "value");
    const record: CustomEventRecord = {
      id: `c-${uuid()}`,
      name,
      value,
      timestamp: nowMs(event),
    };
    let next: ChatState = { ...state, customEvents: [...state.customEvents, record] };
    if (name === "TOKEN_USAGE") {
      const usage = parseUsage({ ...(value as object), type: "TOKEN_USAGE" });
      if (usage) next = { ...next, tokenUsage: mergeUsage(next.tokenUsage, usage) };
    }
    return next;
  }

  // Raw fallback.
  if (t === "RAW") {
    const record: RawEventRecord = {
      id: `r-${uuid()}`,
      raw: field(event, "event") ?? event,
      timestamp: nowMs(event),
    };
    return { ...state, rawEvents: [...state.rawEvents, record] };
  }

  return state;
}

// ---------- helpers ----------

function field(event: BaseEvent, name: string): unknown {
  return (event as Record<string, unknown>)[name];
}

function nowMs(event: BaseEvent): number {
  const ts = event.timestamp;
  if (typeof ts === "number") return ts;
  return Date.now();
}

function pushMessage(state: ChatState, msg: Message): ChatState {
  if (state.messages.some((m) => m.id === msg.id)) return state;
  return { ...state, messages: [...state.messages, msg] };
}

function patchById(
  state: ChatState,
  id: string,
  patch: (m: Message) => Message,
): ChatState {
  if (!id) return state;
  const idx = state.messages.findIndex((m) => m.id === id);
  if (idx < 0) return state;
  const next = state.messages.slice();
  next[idx] = patch(next[idx]);
  return { ...state, messages: next };
}

function openReasoning(state: ChatState, event: BaseEvent): ChatState {
  const id = String(field(event, "message_id") ?? field(event, "messageId") ?? `reason-${uuid()}`);
  const last = state.messages[state.messages.length - 1];
  if (last && last.kind === "reasoning") {
    // Reopen the existing reasoning block instead of creating a staircase.
    const next = state.messages.slice();
    next[next.length - 1] = { ...last, streaming: true } satisfies ReasoningMessage;
    return { ...state, messages: next };
  }
  return pushMessage(state, {
    kind: "reasoning",
    id,
    content: "",
    streaming: true,
  } satisfies ReasoningMessage);
}

function appendToLastReasoning(state: ChatState, delta: string): ChatState {
  const last = state.messages[state.messages.length - 1];
  if (!last || last.kind !== "reasoning") return state;
  const next = state.messages.slice();
  next[next.length - 1] = { ...last, content: last.content + delta } satisfies ReasoningMessage;
  return { ...state, messages: next };
}

function finalizeLastReasoning(state: ChatState): ChatState {
  const last = state.messages[state.messages.length - 1];
  if (!last || last.kind !== "reasoning") return state;
  const next = state.messages.slice();
  next[next.length - 1] = { ...last, streaming: false } satisfies ReasoningMessage;
  return { ...state, messages: next };
}

function parseUsage(event: unknown): { input?: number; output?: number; total?: number } | null {
  if (!event || typeof event !== "object") return null;
  const obj = event as Record<string, unknown>;
  const usage = (obj.usage as Record<string, unknown> | undefined) ?? obj;
  const num = (v: unknown) => (typeof v === "number" ? v : undefined);
  const input = num(usage.input_tokens) ?? num(usage.prompt_tokens);
  const output = num(usage.output_tokens) ?? num(usage.completion_tokens);
  const total = num(usage.total_tokens) ?? (input !== undefined && output !== undefined ? input + output : undefined);
  if (input === undefined && output === undefined && total === undefined) return null;
  return { input, output, total };
}

function mergeUsage(
  prev: { input?: number; output?: number; total?: number } | undefined,
  next: { input?: number; output?: number; total?: number },
): { input?: number; output?: number; total?: number } {
  if (!prev) return next;
  return {
    input: next.input ?? prev.input,
    output: next.output ?? prev.output,
    total: next.total ?? prev.total,
  };
}
