import { ChevronRight, Wrench } from "lucide-react";
import { useState } from "react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import type { ToolCallMessage } from "@/api/agui/types";
import { RunStatusPill } from "./run-status-pill";

interface Props {
  message: ToolCallMessage;
}

export function ToolCallCard({ message }: Props) {
  const [argsOpen, setArgsOpen] = useState(!message.done);
  const [resultOpen, setResultOpen] = useState(false);

  const argsPretty = prettyJson(message.args);
  const resultPretty = message.result === null ? null : prettyJson(message.result);

  return (
    <div className="rounded-md border bg-card text-card-foreground">
      <div className="flex items-center gap-2 border-b px-3 py-2">
        <Wrench className="size-4 text-muted-foreground" />
        <span className="font-mono text-sm">{message.toolName}</span>
        <span className="ml-auto">
          <RunStatusPill variant={message.done ? "done" : "streaming"} />
        </span>
      </div>
      <div className="p-3 space-y-2">
        <Collapsible open={argsOpen} onOpenChange={setArgsOpen}>
          <CollapsibleTrigger className="flex w-full items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
            <ChevronRight className={cn("size-3 transition-transform", argsOpen && "rotate-90")} />
            <span>Arguments</span>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <pre className="mt-1 max-h-64 overflow-auto rounded bg-muted px-2 py-1 text-xs">
              {argsPretty || "—"}
            </pre>
          </CollapsibleContent>
        </Collapsible>
        {resultPretty !== null && (
          <Collapsible open={resultOpen} onOpenChange={setResultOpen}>
            <CollapsibleTrigger className="flex w-full items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
              <ChevronRight className={cn("size-3 transition-transform", resultOpen && "rotate-90")} />
              <span>Result</span>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <pre className="mt-1 max-h-64 overflow-auto rounded bg-muted px-2 py-1 text-xs">
                {resultPretty || "—"}
              </pre>
            </CollapsibleContent>
          </Collapsible>
        )}
      </div>
    </div>
  );
}

function prettyJson(raw: string): string {
  if (!raw) return "";
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
