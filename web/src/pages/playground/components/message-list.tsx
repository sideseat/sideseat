/* Adapted from engagement-mck/solution/site/src/components/chat/message-list.tsx */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ChatState, Message, ToolCallMessage } from "@/api/agui/types";
import { MessageItem } from "./message-item";
import { PulseDot } from "./pulse-dot";
import { ToolCallGroup } from "./tool-call-group";

interface Props {
  state: ChatState;
  isStreaming: boolean;
}

const PIN_THRESHOLD_PX = 80;

export function MessageList({ state, isStreaming }: Props) {
  const messages = state.messages;
  const live = isStreaming;

  // Page-level auto-scroll: pin the WINDOW to the bottom while the user
  // is near the bottom; otherwise leave them where they are.
  const atBottomRef = useRef(true);
  useEffect(() => {
    const onScroll = () => {
      const scrollTop = window.scrollY;
      const fromBottom = document.documentElement.scrollHeight - scrollTop - window.innerHeight;
      atBottomRef.current = fromBottom < PIN_THRESHOLD_PX;
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  const tail = messages[messages.length - 1];
  const tailSignal =
    tail && (tail.kind === "text" || tail.kind === "reasoning") ? tail.content.length : 0;

  useLayoutEffect(() => {
    if (atBottomRef.current) {
      window.scrollTo({ top: document.documentElement.scrollHeight });
    }
  }, [messages.length, tailSignal, live]);

  const sections = useMemo(() => groupSections(messages), [messages]);

  return (
    <div className="mx-auto w-full max-w-4xl flex-1 px-3 py-4 md:px-6 md:py-6">
      {sections.map((section, idx) => (
        <section
          key={section.stepId ?? `prelude-${idx}`}
          data-step-id={section.stepId ?? undefined}
          className="flex flex-col gap-3 scroll-mt-4"
        >
          {section.stepName ? <StickyStepHeader name={section.stepName} /> : null}
          {renderItems(section.items)}
        </section>
      ))}
      {live ? <ActivityIndicator tail={tail} tailSignal={tailSignal} /> : null}
      <div className="h-6" />
    </div>
  );
}

interface Section {
  stepId: string | null;
  stepName: string | null;
  items: Message[];
}

function groupSections(messages: Message[]): Section[] {
  const out: Section[] = [];
  let cur: Section = { stepId: null, stepName: null, items: [] };
  out.push(cur);
  for (const m of messages) {
    if (m.kind === "step") {
      cur = { stepId: m.id, stepName: m.stepName, items: [] };
      out.push(cur);
    } else {
      cur.items.push(m);
    }
  }
  if (out.length > 1 && out[0].items.length === 0) out.shift();
  return out;
}

function renderItems(items: Message[]) {
  const out: React.ReactElement[] = [];
  let buf: ToolCallMessage[] = [];
  const flush = () => {
    if (buf.length === 0) return;
    const group = buf;
    buf = [];
    out.push(<ToolCallGroup key={`grp-${group[0].id}`} tools={group} />);
  };
  for (const m of items) {
    if (m.kind === "tool_call") {
      buf.push(m);
      continue;
    }
    flush();
    out.push(<MessageItem key={m.id} message={m} />);
  }
  flush();
  return out;
}

function StickyStepHeader({ name }: { name: string }) {
  return (
    <div
      className="sticky z-10 -mx-3 flex items-center gap-3 bg-background/85 px-3 py-1.5 backdrop-blur supports-backdrop-filter:bg-background/70 md:-mx-6 md:px-6"
      style={{ top: "var(--header-height)" }}
    >
      <span className="h-px flex-1 bg-border" />
      <span className="inline-flex items-center gap-2 rounded-full border bg-card px-2.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.12em] text-muted-foreground shadow-xs">
        <span className="size-1.5 rounded-full bg-primary" />
        {name}
      </span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

/**
 * Trailing live indicator with seconds-since-last-stream-update so a long
 * silent tool call doesn't look frozen.
 */
function ActivityIndicator({
  tail,
  tailSignal,
}: {
  tail: Message | undefined;
  tailSignal: number;
}) {
  const activity = useMemo(() => {
    if (!tail) return "Working";
    if (tail.kind === "tool_call" && !tail.done) return `Running ${tail.toolName}`;
    if (tail.kind === "reasoning" && tail.streaming) return "Thinking";
    if (tail.kind === "text" && tail.streaming && tail.role === "assistant") return "Writing";
    return "Working";
  }, [tail]);

  const tailId = tail?.id ?? "__none__";
  const [since, setSince] = useState(0);
  useEffect(() => {
    setSince(0);
    const start = Date.now();
    const t = setInterval(() => {
      setSince(Math.floor((Date.now() - start) / 1000));
    }, 1000);
    return () => clearInterval(t);
  }, [tailId, tailSignal]);

  const longPause = since >= 10;
  const label = longPause ? `Still ${activity.toLowerCase()}…` : `${activity}…`;

  return (
    <div
      aria-live="polite"
      aria-label={`${activity}, ${since} seconds`}
      className="mt-3 flex items-center gap-2.5 rounded-md bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
    >
      <PulseDot />
      <span className="font-medium text-foreground/85">{label}</span>
      {since > 0 && (
        <span className="ml-auto font-mono tabular-nums text-[11px] text-muted-foreground/80">
          {formatElapsed(since)}
        </span>
      )}
    </div>
  );
}

function formatElapsed(s: number): string {
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}m ${r.toString().padStart(2, "0")}s`;
}
