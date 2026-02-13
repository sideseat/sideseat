import { HelpCircle } from "lucide-react";
import { JsonContent } from "./json-content";

interface UnknownContentProps {
  data: unknown;
}

export function UnknownContent({ data }: UnknownContentProps) {
  return (
    <div className="rounded-md border border-border/50 bg-muted/30 px-3 py-2">
      <div className="flex items-center gap-2 mb-2">
        <HelpCircle className="h-4 w-4 text-muted-foreground" />
        <span className="text-sm font-medium text-muted-foreground">Unknown content type</span>
      </div>
      <JsonContent data={data} />
    </div>
  );
}
