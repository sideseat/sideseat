import { Wrench } from "lucide-react";
import { JsonContent } from "./json-content";

interface ToolUseContentProps {
  id?: string;
  name?: string;
  input: Record<string, unknown>;
  /** Show inline header with tool name (for ContentRenderer use) */
  showInlineHeader?: boolean;
}

export function ToolUseContent({ id, name, input, showInlineHeader = false }: ToolUseContentProps) {
  return (
    <div className="space-y-2">
      {showInlineHeader && name && (
        <div className="flex items-center gap-2">
          <Wrench className="h-4 w-4 text-orange-600 dark:text-orange-400" />
          <span className="font-mono text-sm font-semibold text-orange-700 dark:text-orange-300">
            {name}
          </span>
        </div>
      )}
      {id && <div className="text-xs text-muted-foreground font-mono">tool_call_id: {id}</div>}
      {name && <div className="text-xs text-muted-foreground font-mono">name: {name}</div>}
      <JsonContent data={input} />
    </div>
  );
}
