export type EventType =
  | "RUN_STARTED"
  | "RUN_FINISHED"
  | "RUN_ERROR"
  | "STEP_STARTED"
  | "STEP_FINISHED"
  | "TEXT_MESSAGE_START"
  | "TEXT_MESSAGE_CONTENT"
  | "TEXT_MESSAGE_CHUNK"
  | "TEXT_MESSAGE_END"
  | "REASONING_START"
  | "REASONING_CONTENT"
  | "REASONING_END"
  | "THINKING_START"
  | "THINKING_END"
  | "THINKING_TEXT_MESSAGE_START"
  | "THINKING_TEXT_MESSAGE_CONTENT"
  | "THINKING_TEXT_MESSAGE_END"
  | "TOOL_CALL_START"
  | "TOOL_CALL_ARGS"
  | "TOOL_CALL_END"
  | "TOOL_CALL_RESULT"
  | "STATE_SNAPSHOT"
  | "STATE_DELTA"
  | "MESSAGES_SNAPSHOT"
  | "CUSTOM"
  | "RAW";

export interface BaseEvent {
  type: EventType | string;
  timestamp?: number;
  rawEvent?: unknown;
  [k: string]: unknown;
}

export interface RunAgentInputMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
}

export interface RunAgentInput {
  thread_id: string;
  run_id: string;
  messages: RunAgentInputMessage[];
  tools: unknown[];
  context: unknown[];
  state: Record<string, unknown>;
  forwarded_props: Record<string, unknown>;
}

export interface TextMessage {
  kind: "text";
  id: string;
  role: "user" | "assistant";
  content: string;
  streaming: boolean;
}

export interface ReasoningMessage {
  kind: "reasoning";
  id: string;
  content: string;
  streaming: boolean;
}

export interface ToolCallMessage {
  kind: "tool_call";
  id: string;
  toolName: string;
  args: string;
  result: string | null;
  done: boolean;
}

export interface StepMessage {
  kind: "step";
  id: string;
  stepName: string;
}

export interface ErrorMessage {
  kind: "error";
  id: string;
  code: string;
  message: string;
}

export interface RunStatusMessage {
  kind: "run_status";
  id: string;
  phase: "started" | "finished";
}

export interface StateMessage {
  kind: "state";
  id: string;
  payload: unknown;
}

export type Message =
  | TextMessage
  | ReasoningMessage
  | ToolCallMessage
  | StepMessage
  | ErrorMessage
  | RunStatusMessage
  | StateMessage;

export interface CustomEventRecord {
  id: string;
  name: string;
  value: unknown;
  timestamp: number;
}

export interface RawEventRecord {
  id: string;
  raw: unknown;
  timestamp: number;
}

export interface ChatState {
  messages: Message[];
  runState: "idle" | "running" | "errored" | "finished";
  latestState: unknown | null;
  pendingMessagesSnapshot: BaseEvent | null;
  customEvents: CustomEventRecord[];
  rawEvents: RawEventRecord[];
  eventLog: BaseEvent[];
  threadId: string;
  tokenUsage?: { input?: number; output?: number; total?: number };
}
