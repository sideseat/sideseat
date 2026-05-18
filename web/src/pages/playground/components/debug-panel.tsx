import { useMemo } from "react";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { ChatState } from "@/api/agui/types";

interface Props {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  state: ChatState;
}

export function DebugPanel({ open, onOpenChange, state }: Props) {
  const eventsRev = useMemo(() => [...state.eventLog].reverse(), [state.eventLog]);
  const customsRev = useMemo(
    () => [...state.customEvents].reverse(),
    [state.customEvents],
  );
  const rawsRev = useMemo(() => [...state.rawEvents].reverse(), [state.rawEvents]);

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="sm:max-w-2xl w-full p-0 flex flex-col">
        <SheetHeader className="px-4 py-3 border-b">
          <SheetTitle>Debug</SheetTitle>
        </SheetHeader>
        <Tabs defaultValue="events" className="flex-1 min-h-0 flex flex-col">
          <TabsList className="mx-4 mt-3 self-start">
            <TabsTrigger value="events">Events</TabsTrigger>
            <TabsTrigger value="state">State</TabsTrigger>
            <TabsTrigger value="custom">Custom</TabsTrigger>
            <TabsTrigger value="raw">Raw</TabsTrigger>
          </TabsList>
          <TabsContent value="events" className="flex-1 min-h-0 overflow-auto px-4 py-3 space-y-1">
            {eventsRev.length === 0 && (
              <div className="text-xs text-muted-foreground">No events yet.</div>
            )}
            {eventsRev.map((ev, idx) => (
              <details
                key={`${state.eventLog.length - idx}-${String(ev.type)}`}
                className="rounded border bg-card px-2 py-1"
              >
                <summary className="cursor-pointer text-xs font-mono">
                  {String(ev.type)}
                </summary>
                <pre className="mt-1 max-h-40 overflow-auto text-[11px] whitespace-pre-wrap">
                  {prettyJson(ev)}
                </pre>
              </details>
            ))}
          </TabsContent>
          <TabsContent value="state" className="flex-1 min-h-0 overflow-auto px-4 py-3">
            <pre className="text-[11px] whitespace-pre-wrap">{prettyJson(state.latestState)}</pre>
          </TabsContent>
          <TabsContent value="custom" className="flex-1 min-h-0 overflow-auto px-4 py-3 space-y-1">
            {customsRev.length === 0 && (
              <div className="text-xs text-muted-foreground">No custom events yet.</div>
            )}
            {customsRev.map((c) => (
              <details key={c.id} className="rounded border bg-card px-2 py-1">
                <summary className="cursor-pointer text-xs font-mono">{c.name}</summary>
                <pre className="mt-1 max-h-40 overflow-auto text-[11px] whitespace-pre-wrap">
                  {prettyJson(c.value)}
                </pre>
              </details>
            ))}
          </TabsContent>
          <TabsContent value="raw" className="flex-1 min-h-0 overflow-auto px-4 py-3 space-y-1">
            {rawsRev.length === 0 && (
              <div className="text-xs text-muted-foreground">No raw events yet.</div>
            )}
            {rawsRev.map((r) => (
              <details key={r.id} className="rounded border bg-card px-2 py-1">
                <summary className="cursor-pointer text-xs font-mono">RAW</summary>
                <pre className="mt-1 max-h-40 overflow-auto text-[11px] whitespace-pre-wrap">
                  {prettyJson(r.raw)}
                </pre>
              </details>
            ))}
          </TabsContent>
        </Tabs>
      </SheetContent>
    </Sheet>
  );
}

function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v ?? "");
  }
}
