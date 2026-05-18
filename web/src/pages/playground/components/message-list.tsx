import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import type { ChatState } from "@/api/agui/types";
import { MessageItem } from "./message-item";
import { ScrollToBottom } from "./scroll-to-bottom";

interface Props {
  state: ChatState;
  isStreaming: boolean;
}

const PIN_THRESHOLD_PX = 80;

export function MessageList({ state, isStreaming }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const [showScrollButton, setShowScrollButton] = useState(false);

  const scrollToBottom = useCallback((smooth = true) => {
    const el = containerRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" });
    pinnedRef.current = true;
    setShowScrollButton(false);
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onScroll = () => {
      const fromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      pinnedRef.current = fromBottom < PIN_THRESHOLD_PX;
      setShowScrollButton(!pinnedRef.current && isStreaming);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [isStreaming]);

  // Auto-scroll if pinned. Triggers on any reducer-state change so streaming
  // text deltas keep the view glued to the bottom.
  useEffect(() => {
    if (pinnedRef.current && containerRef.current) {
      const el = containerRef.current;
      el.scrollTop = el.scrollHeight;
    }
  }, [state.messages, state.runState]);

  // Keep the scroll-to-latest button hidden when not streaming.
  useEffect(() => {
    if (!isStreaming) setShowScrollButton(false);
  }, [isStreaming]);

  return (
    <div className="relative flex-1 min-h-0">
      <div
        ref={containerRef}
        className="absolute inset-0 overflow-y-auto px-1 py-4 space-y-3"
      >
        {state.messages.map((m) => (
          <MessageItem key={m.id} message={m} />
        ))}
        {isStreaming && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground py-1">
            <Loader2 className="size-3 animate-spin" />
            <span>Streaming…</span>
            {state.tokenUsage && (
              <span className="ml-2">
                {state.tokenUsage.input ?? "?"} in / {state.tokenUsage.output ?? "?"} out
              </span>
            )}
          </div>
        )}
      </div>
      {showScrollButton && <ScrollToBottom onClick={() => scrollToBottom(true)} />}
    </div>
  );
}
