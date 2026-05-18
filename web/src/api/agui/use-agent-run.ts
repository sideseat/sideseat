import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { uuid } from "@/lib/utils";
import { initialState, reduce } from "./reducer";
import { RunError, runAgentStream } from "./run-stream";
import type { BaseEvent, ChatState, RunAgentInput } from "./types";

export interface UseAgentRunResult {
  state: ChatState;
  send: (prompt: string) => void;
  cancel: () => void;
  clear: () => void;
  isStreaming: boolean;
  error: { code: string; message: string } | null;
}

interface UseAgentRunArgs {
  projectId: string;
  agentName: string | null;
}

export function useAgentRun({ projectId, agentName }: UseAgentRunArgs): UseAgentRunResult {
  const [state, dispatch] = useReducer(reduce, undefined, () => initialState(uuid()));
  const controllerRef = useRef<AbortController | null>(null);

  // Switching agent or unmounting must abort an in-flight stream so its
  // events cannot bleed into the next agent's reducer.
  useEffect(() => {
    return () => {
      controllerRef.current?.abort();
      controllerRef.current = null;
    };
  }, []);

  // When the user selects a different agent, reset the conversation.
  useEffect(() => {
    controllerRef.current?.abort();
    controllerRef.current = null;
    dispatch({ type: "reset", threadId: uuid() });
    // We intentionally only reset on agentName change.
  }, [agentName]);

  const isStreaming = state.runState === "running";

  const send = useCallback(
    (prompt: string) => {
      const trimmed = prompt.trim();
      if (!trimmed || !agentName) return;
      if (controllerRef.current) return; // already streaming

      const messageId = `u-${uuid()}`;
      dispatch({ type: "append_user", content: trimmed, id: messageId });

      // Synthesise a STEP_STARTED tagged with the registered agent so the
      // stream is always grouped under a sticky header naming whoever is
      // running. Real STEP_STARTED events from swarms/graphs interleave
      // naturally and can switch the header mid-run for sub-agents.
      dispatch({
        type: "event",
        event: {
          type: "STEP_STARTED",
          stepName: agentName,
        } as BaseEvent,
      });

      const input: RunAgentInput = {
        thread_id: state.threadId,
        run_id: uuid(),
        messages: [{ id: messageId, role: "user", content: trimmed }],
        tools: [],
        context: [],
        state: {},
        forwarded_props: {},
      };

      const controller = new AbortController();
      controllerRef.current = controller;

      void (async () => {
        try {
          for await (const event of runAgentStream({
            projectId,
            agentName,
            input,
            signal: controller.signal,
          })) {
            dispatch({ type: "event", event });
          }
        } catch (e) {
          if (e instanceof RunError && e.code === "client_aborted") {
            dispatch({ type: "client_aborted" });
          } else if (e instanceof RunError) {
            const ev: BaseEvent = {
              type: "RUN_ERROR",
              code: e.code,
              message: e.message,
              status: e.status,
            };
            dispatch({ type: "event", event: ev });
          } else {
            const ev: BaseEvent = {
              type: "RUN_ERROR",
              code: "internal",
              message: e instanceof Error ? e.message : String(e),
            };
            dispatch({ type: "event", event: ev });
          }
        } finally {
          if (controllerRef.current === controller) {
            controllerRef.current = null;
          }
        }
      })();
    },
    [agentName, projectId, state.threadId],
  );

  const cancel = useCallback(() => {
    controllerRef.current?.abort();
  }, []);

  const clear = useCallback(() => {
    controllerRef.current?.abort();
    controllerRef.current = null;
    dispatch({ type: "reset", threadId: uuid() });
  }, []);

  const error = useMemo<{ code: string; message: string } | null>(() => {
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const m = state.messages[i];
      if (m.kind === "error") return { code: m.code, message: m.message };
    }
    return null;
  }, [state.messages]);

  return { state, send, cancel, clear, isStreaming, error };
}
