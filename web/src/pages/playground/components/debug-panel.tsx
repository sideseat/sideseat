import {
  AlertCircle,
  ChevronRight,
  Cpu,
  Database,
  FileText,
  Flag,
  MessageSquare,
  Milestone,
  Search,
  Sparkles,
  Trash2,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Input } from "@/components/ui/input";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { JsonContent } from "@/components/thread/content/json-content";
import type { BaseEvent, ChatState } from "@/api/agui/types";
import { cn } from "@/lib/utils";

interface Props {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  state: ChatState;
}

interface EventGroup {
  id: string;
  type: string;
  category: EventCategory;
  count: number;
  /** Index of the latest event in the source list (for time delta and key). */
  lastIdx: number;
  /** Latest payload — what we show when expanded. */
  payload: BaseEvent;
}

type EventCategory =
  | "lifecycle"
  | "step"
  | "text"
  | "reasoning"
  | "tool"
  | "state"
  | "messages"
  | "custom"
  | "raw"
  | "other";

const CATEGORY_META: Record<
  EventCategory,
  { label: string; icon: LucideIcon; tint: string; ring: string }
> = {
  lifecycle: {
    label: "Lifecycle",
    icon: Flag,
    tint: "text-emerald-700 dark:text-emerald-400 bg-emerald-500/10",
    ring: "ring-emerald-500/30",
  },
  step: {
    label: "Step",
    icon: Milestone,
    tint: "text-violet-700 dark:text-violet-400 bg-violet-500/10",
    ring: "ring-violet-500/30",
  },
  text: {
    label: "Text",
    icon: MessageSquare,
    tint: "text-sky-700 dark:text-sky-400 bg-sky-500/10",
    ring: "ring-sky-500/30",
  },
  reasoning: {
    label: "Reasoning",
    icon: Cpu,
    tint: "text-fuchsia-700 dark:text-fuchsia-400 bg-fuchsia-500/10",
    ring: "ring-fuchsia-500/30",
  },
  tool: {
    label: "Tool",
    icon: Wrench,
    tint: "text-amber-700 dark:text-amber-400 bg-amber-500/10",
    ring: "ring-amber-500/30",
  },
  state: {
    label: "State",
    icon: Database,
    tint: "text-indigo-700 dark:text-indigo-400 bg-indigo-500/10",
    ring: "ring-indigo-500/30",
  },
  messages: {
    label: "Snapshot",
    icon: FileText,
    tint: "text-slate-700 dark:text-slate-300 bg-slate-500/10",
    ring: "ring-slate-500/30",
  },
  custom: {
    label: "Custom",
    icon: Sparkles,
    tint: "text-rose-700 dark:text-rose-400 bg-rose-500/10",
    ring: "ring-rose-500/30",
  },
  raw: {
    label: "Raw",
    icon: AlertCircle,
    tint: "text-zinc-700 dark:text-zinc-400 bg-zinc-500/10",
    ring: "ring-zinc-500/30",
  },
  other: {
    label: "Other",
    icon: AlertCircle,
    tint: "text-muted-foreground bg-muted",
    ring: "ring-muted",
  },
};

function categorize(type: string): EventCategory {
  if (type.startsWith("RUN_")) return "lifecycle";
  if (type.startsWith("STEP_")) return "step";
  if (type.startsWith("TEXT_MESSAGE_")) return "text";
  if (type.startsWith("REASONING_") || type.startsWith("THINKING_")) return "reasoning";
  if (type.startsWith("TOOL_CALL_")) return "tool";
  if (type === "STATE_SNAPSHOT" || type === "STATE_DELTA") return "state";
  if (type === "MESSAGES_SNAPSHOT") return "messages";
  if (type === "CUSTOM") return "custom";
  if (type === "RAW") return "raw";
  return "other";
}

const CATEGORY_ORDER: EventCategory[] = [
  "lifecycle",
  "step",
  "text",
  "reasoning",
  "tool",
  "state",
  "messages",
  "custom",
  "raw",
  "other",
];

export function DebugPanel({ open, onOpenChange, state }: Props) {
  const [filter, setFilter] = useState("");
  const [activeCats, setActiveCats] = useState<Set<EventCategory>>(() => new Set(CATEGORY_ORDER));

  // Counts per category for the chip filter row.
  const counts = useMemo(() => {
    const c: Record<EventCategory, number> = {
      lifecycle: 0,
      step: 0,
      text: 0,
      reasoning: 0,
      tool: 0,
      state: 0,
      messages: 0,
      custom: 0,
      raw: 0,
      other: 0,
    };
    for (const ev of state.eventLog) c[categorize(String(ev.type ?? ""))] += 1;
    return c;
  }, [state.eventLog]);

  // Group consecutive same-type events into a single row with a count.
  const groups: EventGroup[] = useMemo(() => {
    const out: EventGroup[] = [];
    state.eventLog.forEach((ev, idx) => {
      const type = String(ev.type ?? "");
      const cat = categorize(type);
      const last = out[out.length - 1];
      if (last && last.type === type) {
        last.count += 1;
        last.lastIdx = idx;
        last.payload = ev;
        return;
      }
      out.push({
        id: `${type}-${idx}`,
        type,
        category: cat,
        count: 1,
        lastIdx: idx,
        payload: ev,
      });
    });
    return out.reverse();
  }, [state.eventLog]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return groups.filter((g) => {
      if (!activeCats.has(g.category)) return false;
      if (q && !g.type.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [groups, filter, activeCats]);

  const totalEvents = state.eventLog.length;

  // Reset filter on each new run.
  useEffect(() => {
    if (state.runState === "running") setFilter("");
  }, [state.runState]);

  const toggleCat = (cat: EventCategory) =>
    setActiveCats((prev) => {
      const next = new Set(prev);
      if (next.has(cat)) next.delete(cat);
      else next.add(cat);
      return next;
    });

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="flex w-full flex-col gap-0 p-0 sm:max-w-2xl">
        <Tabs defaultValue="events" className="flex flex-1 min-h-0 flex-col">
          {/* HEADER: title + tabs side-by-side, like a real toolbar. */}
          <header className="flex shrink-0 items-center gap-3 border-b px-4 py-2.5">
            <SheetTitle className="shrink-0 text-sm font-semibold">Debug</SheetTitle>
            <TabsList className="h-8">
              <TabsTrigger value="events" className="gap-1.5 text-xs">
                Events
                <span className="rounded bg-muted px-1 py-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                  {totalEvents}
                </span>
              </TabsTrigger>
              <TabsTrigger value="state" className="text-xs">
                State
              </TabsTrigger>
              <TabsTrigger value="custom" className="gap-1.5 text-xs">
                Custom
                {state.customEvents.length > 0 && (
                  <span className="rounded bg-muted px-1 py-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                    {state.customEvents.length}
                  </span>
                )}
              </TabsTrigger>
              <TabsTrigger value="raw" className="gap-1.5 text-xs">
                Raw
                {state.rawEvents.length > 0 && (
                  <span className="rounded bg-muted px-1 py-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                    {state.rawEvents.length}
                  </span>
                )}
              </TabsTrigger>
            </TabsList>
          </header>

          {/* EVENTS TAB */}
          <TabsContent value="events" className="flex flex-1 min-h-0 flex-col gap-0">
            {/* Filter row: search + category chips */}
            <div className="shrink-0 space-y-2 border-b px-4 py-2.5">
              <div className="relative">
                <Search className="absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  placeholder="Filter events…"
                  className="h-8 pl-7 pr-7 text-xs"
                />
                {filter && (
                  <button
                    type="button"
                    onClick={() => setFilter("")}
                    aria-label="Clear filter"
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                )}
              </div>
              <div className="flex flex-wrap gap-1">
                {CATEGORY_ORDER.map((cat) => {
                  const meta = CATEGORY_META[cat];
                  const active = activeCats.has(cat);
                  const n = counts[cat];
                  if (n === 0) return null;
                  const Icon = meta.icon;
                  return (
                    <button
                      key={cat}
                      type="button"
                      onClick={() => toggleCat(cat)}
                      className={cn(
                        "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-colors",
                        active
                          ? `${meta.tint} border-transparent`
                          : "border-border bg-background text-muted-foreground hover:bg-muted",
                      )}
                    >
                      <Icon className="size-3" />
                      <span>{meta.label}</span>
                      <span className="font-mono tabular-nums opacity-70">{n}</span>
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Timeline list */}
            <div className="flex-1 min-h-0 overflow-auto">
              {filtered.length === 0 ? (
                <EmptyHint
                  message={totalEvents === 0 ? "No events yet." : "No events match the filter."}
                />
              ) : (
                <ul className="divide-y">
                  {filtered.map((g) => (
                    <EventRow key={g.id} group={g} />
                  ))}
                </ul>
              )}
            </div>
          </TabsContent>

          {/* STATE TAB */}
          <TabsContent value="state" className="flex-1 min-h-0 overflow-auto p-4">
            {state.latestState === null ? (
              <EmptyHint message="No state snapshot yet." />
            ) : (
              <div className="rounded-md border bg-card p-3">
                <JsonContent data={state.latestState} />
              </div>
            )}
          </TabsContent>

          {/* CUSTOM TAB */}
          <TabsContent value="custom" className="flex-1 min-h-0 overflow-auto p-4 space-y-2">
            {state.customEvents.length === 0 ? (
              <EmptyHint message="No custom events yet." />
            ) : (
              [...state.customEvents].reverse().map((c) => (
                <details key={c.id} className="group rounded-md border bg-card">
                  <summary className="flex cursor-pointer select-none items-center gap-2 px-3 py-2 text-xs">
                    <ChevronRight className="size-3 shrink-0 transition-transform group-open:rotate-90" />
                    <Sparkles className="size-3.5 text-rose-500" />
                    <span className="font-mono font-medium">{c.name}</span>
                    <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                      {formatTimestamp(c.timestamp)}
                    </span>
                  </summary>
                  <div className="border-t bg-muted/30 px-3 py-2">
                    <JsonContent data={c.value} />
                  </div>
                </details>
              ))
            )}
          </TabsContent>

          {/* RAW TAB */}
          <TabsContent value="raw" className="flex-1 min-h-0 overflow-auto p-4 space-y-2">
            {state.rawEvents.length === 0 ? (
              <EmptyHint message="No raw events yet." />
            ) : (
              [...state.rawEvents].reverse().map((r) => (
                <details key={r.id} className="group rounded-md border bg-card">
                  <summary className="flex cursor-pointer select-none items-center gap-2 px-3 py-2 text-xs">
                    <ChevronRight className="size-3 shrink-0 transition-transform group-open:rotate-90" />
                    <AlertCircle className="size-3.5 text-zinc-500" />
                    <span className="font-mono font-medium">RAW</span>
                    <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                      {formatTimestamp(r.timestamp)}
                    </span>
                  </summary>
                  <div className="border-t bg-muted/30 px-3 py-2">
                    <JsonContent data={r.raw} />
                  </div>
                </details>
              ))
            )}
          </TabsContent>
        </Tabs>
      </SheetContent>
    </Sheet>
  );
}

function EventRow({ group }: { group: EventGroup }) {
  const meta = CATEGORY_META[group.category];
  const Icon = meta.icon;
  const summary = useMemo(() => oneLineSummary(group.payload), [group.payload]);

  return (
    <li>
      <details className="group">
        <summary className="flex cursor-pointer select-none items-center gap-2 px-4 py-2 hover:bg-muted/40">
          <span
            className={cn("flex size-6 shrink-0 items-center justify-center rounded-md", meta.tint)}
          >
            <Icon className="size-3.5" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block font-mono text-[12px] font-medium leading-tight">
              {group.type}
              {group.count > 1 && (
                <span className="ml-1.5 rounded bg-muted px-1 py-0 align-middle font-mono text-[10px] tabular-nums text-muted-foreground">
                  ×{group.count}
                </span>
              )}
            </span>
            {summary && (
              <span className="block truncate text-[11px] text-muted-foreground">{summary}</span>
            )}
          </span>
          <ChevronRight className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-90" />
        </summary>
        <div className="border-t bg-muted/30 px-4 py-2">
          <JsonContent data={group.payload} collapsed={2} />
        </div>
      </details>
    </li>
  );
}

function EmptyHint({ message }: { message: string }) {
  return (
    <div className="flex h-full items-center justify-center p-8 text-xs text-muted-foreground">
      {message}
    </div>
  );
}

function formatTimestamp(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

/** One-line teaser to make collapsed event rows scannable. */
function oneLineSummary(ev: BaseEvent): string {
  const type = String(ev.type ?? "");
  const get = (...keys: string[]) => {
    for (const k of keys) {
      const v = (ev as Record<string, unknown>)[k];
      if (typeof v === "string" && v) return v;
      if (typeof v === "number") return String(v);
    }
    return "";
  };

  if (type === "TEXT_MESSAGE_CONTENT" || type === "TEXT_MESSAGE_CHUNK") {
    const d = get("delta");
    return d ? truncate(d, 80) : "";
  }
  if (type === "TOOL_CALL_START" || type === "TOOL_CALL_END" || type === "TOOL_CALL_RESULT") {
    return get("toolCallName", "tool_call_name", "name", "toolCallId", "tool_call_id");
  }
  if (type === "TOOL_CALL_ARGS") {
    const d = get("delta");
    return d ? truncate(d.replace(/\s+/g, " "), 80) : "";
  }
  if (type === "STEP_STARTED" || type === "STEP_FINISHED") {
    return get("stepName", "step_name");
  }
  if (type === "RUN_STARTED" || type === "RUN_FINISHED") {
    return get("threadId", "thread_id");
  }
  if (type === "RUN_ERROR") {
    const code = get("code");
    const msg = get("message");
    return code ? (msg ? `${code} — ${truncate(msg, 60)}` : code) : truncate(msg, 80);
  }
  if (type === "CUSTOM") {
    return get("name");
  }
  return "";
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s;
  return `${s.slice(0, max - 1)}…`;
}
