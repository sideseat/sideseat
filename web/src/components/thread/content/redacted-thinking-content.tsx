import { EyeOff } from "lucide-react";

export function RedactedThinkingContent() {
  return (
    <div className="rounded-md border border-border/50 bg-muted/30 px-3 py-2">
      <div className="flex items-center gap-2">
        <EyeOff className="h-4 w-4 text-pink-600 dark:text-pink-400" />
        <span className="text-sm text-muted-foreground italic">
          Thinking content not available for this request
        </span>
      </div>
    </div>
  );
}
