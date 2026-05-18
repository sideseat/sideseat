/* Adapted from engagement-mck/solution/site/src/components/chat/tool-call-group.tsx */
import { useCallback, useEffect, useState } from "react";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import type { ToolCallMessage } from "@/api/agui/types";
import { cn } from "@/lib/utils";
import {
  ArgsSection,
  ResultSection,
  ToolCallCard,
  ToolIcon,
} from "./tool-call-card";

export function ToolCallGroup({ tools }: { tools: ToolCallMessage[] }) {
  const [openId, setOpenId] = useState<string | null>(null);
  const openAt = useCallback((id: string) => setOpenId(id), []);
  const close = useCallback(() => setOpenId(null), []);

  useEffect(() => {
    if (!openId) return;
    const raf = requestAnimationFrame(() => {
      const el = document.getElementById(`sheet-tool-${openId}`);
      if (el) el.scrollIntoView({ block: "start", behavior: "auto" });
    });
    return () => cancelAnimationFrame(raf);
  }, [openId]);

  return (
    <>
      <div className="flex flex-wrap items-center gap-1.5">
        {tools.map((t) => (
          <ToolCallCard
            key={t.id}
            toolName={t.toolName}
            args={t.args}
            result={t.result}
            done={t.done}
            scrollTargetId={`card-tool-${t.id}`}
            onOpenFull={() => openAt(t.id)}
          />
        ))}
      </div>

      <Sheet
        open={openId !== null}
        onOpenChange={(o) => {
          if (!o) close();
        }}
      >
        <SheetContent side="right" className="w-full sm:max-w-3xl flex flex-col p-0">
          <SheetHeader className="gap-0 border-b px-5 py-4">
            <SheetTitle className="text-sm font-semibold tracking-tight text-foreground">
              {tools.length === 1 ? "Tool call" : `${tools.length} tool calls`}
            </SheetTitle>
            {tools.length > 1 && (
              <p className="text-[11px] text-muted-foreground">
                Batch — scroll to browse each call.
              </p>
            )}
          </SheetHeader>
          <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
            {tools.map((t, i) => (
              <ToolSheetSection
                key={t.id}
                tool={t}
                index={i + 1}
                total={tools.length}
                active={t.id === openId}
              />
            ))}
          </div>
        </SheetContent>
      </Sheet>
    </>
  );
}

function ToolSheetSection({
  tool,
  index,
  total,
  active,
}: {
  tool: ToolCallMessage;
  index: number;
  total: number;
  active: boolean;
}) {
  return (
    <section
      id={`sheet-tool-${tool.id}`}
      className={cn(
        "scroll-mt-4 border-b px-5 py-5 last:border-b-0",
        active && "bg-primary/5",
      )}
    >
      <header className="mb-3 flex items-center gap-2.5">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
          <ToolIcon toolName={tool.toolName} className="size-3.5" />
        </span>
        <span className="font-mono text-[13px] font-medium text-foreground">{tool.toolName}</span>
        {total > 1 && (
          <span className="ml-auto font-mono text-[10px] tabular-nums text-muted-foreground">
            {index} / {total}
          </span>
        )}
      </header>
      <div className="space-y-4">
        <ArgsSection args={tool.args} truncated={!tool.done} unbounded />
        {tool.result !== null && <ResultSection body={tool.result} unbounded />}
      </div>
    </section>
  );
}
