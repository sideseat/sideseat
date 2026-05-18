import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

interface Props {
  payload: unknown;
}

export function StateCard({ payload }: Props) {
  const [open, setOpen] = useState(false);
  const pretty = prettyJson(payload);
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="flex w-full items-center gap-1 rounded-md border bg-card px-3 py-2 text-xs text-muted-foreground hover:text-foreground">
        <ChevronRight className={cn("size-3 transition-transform", open && "rotate-90")} />
        <span>Latest state snapshot</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="mt-1 max-h-64 overflow-auto rounded bg-muted px-2 py-1 text-xs">
          {pretty || "—"}
        </pre>
      </CollapsibleContent>
    </Collapsible>
  );
}

function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value ?? "");
  }
}
