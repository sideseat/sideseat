import { JsonContent } from "./json-content";

interface ContextContentProps {
  data: unknown;
  contextType?: string;
}

export function ContextContent({ data, contextType }: ContextContentProps) {
  const label = contextType || "context";
  const itemCount = Array.isArray(data)
    ? `${data.length} items`
    : typeof data === "object" && data
      ? `${Object.keys(data).length} keys`
      : "";

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="font-medium text-amber-600 dark:text-amber-400">{label}</span>
        {itemCount && <span>({itemCount})</span>}
      </div>
      <div className="max-h-96 overflow-auto">
        <JsonContent data={data} />
      </div>
    </div>
  );
}
