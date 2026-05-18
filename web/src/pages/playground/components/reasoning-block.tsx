import { Brain, Loader2 } from "lucide-react";
import type { ReasoningMessage } from "@/api/agui/types";

interface Props {
  message: ReasoningMessage;
}

export function ReasoningBlock({ message }: Props) {
  const lastLine = lastNonEmptyLine(message.content);
  const summary = message.streaming ? lastLine || "Thinking…" : "Reasoning";
  return (
    <details className="rounded-md border bg-muted/30 px-3 py-2 text-sm">
      <summary className="flex cursor-pointer items-center gap-2 text-muted-foreground">
        <Brain className="size-4" />
        <span className="flex-1 truncate">{summary}</span>
        {message.streaming && <Loader2 className="size-3 animate-spin" />}
      </summary>
      <pre className="mt-2 whitespace-pre-wrap text-xs text-muted-foreground">
        {message.content || "—"}
      </pre>
    </details>
  );
}

function lastNonEmptyLine(s: string): string {
  const lines = s.split("\n");
  for (let i = lines.length - 1; i >= 0; i--) {
    const l = lines[i].trim();
    if (l) return l;
  }
  return "";
}
