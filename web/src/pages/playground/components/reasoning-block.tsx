/* Adapted from engagement-mck/solution/site/src/components/chat/reasoning-block.tsx */
import { Brain, ChevronRight } from "lucide-react";
import { useMemo } from "react";
import { cn } from "@/lib/utils";
import { PulseDot } from "./pulse-dot";

interface Props {
  content: string;
  streaming: boolean;
}

export function ReasoningBlock({ content, streaming }: Props) {
  const hasContent = content.trim().length > 0;
  const preview = useMemo(() => {
    if (!content) return streaming ? "Thinking…" : "";
    const lines = content
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean);
    return lines[lines.length - 1] ?? "";
  }, [content, streaming]);

  return (
    <details className="group rounded-lg border border-border/80 bg-muted/30 text-xs transition-colors hover:bg-muted/50">
      <summary className="flex cursor-pointer select-none items-center gap-2 px-3 py-2 text-muted-foreground">
        <ChevronRight className="size-3 shrink-0 transition-transform duration-200 ease-out group-open:rotate-90" />
        <Brain
          className={cn(
            "size-3.5 shrink-0 transition-colors",
            streaming ? "text-primary" : "text-muted-foreground",
          )}
        />
        <span className="shrink-0 font-medium text-foreground/80">
          {streaming ? "Thinking" : "Reasoning"}
        </span>
        {streaming ? <PulseDot /> : null}
        {preview && hasContent ? (
          <span className="min-w-0 flex-1 truncate italic text-muted-foreground/80">{preview}</span>
        ) : null}
      </summary>
      {hasContent ? (
        <div className="border-t px-3 py-2.5">
          <pre className="m-0 whitespace-pre-wrap font-sans text-[12.5px] leading-relaxed text-muted-foreground">
            {content}
          </pre>
        </div>
      ) : null}
    </details>
  );
}
