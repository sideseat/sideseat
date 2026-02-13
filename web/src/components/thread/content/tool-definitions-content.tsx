import { JsonContent } from "./json-content";

interface FunctionDef {
  name: string;
  description?: string;
  parameters?: {
    properties?: Record<string, unknown>;
    required?: string[];
  };
}

interface ToolDefinitionsContentProps {
  tools: unknown[];
  toolChoice?: unknown;
}

// Unwrap OpenAI format: {type: "function", function: {...}} -> {...}
function unwrapTool(tool: unknown): FunctionDef | null {
  if (!tool || typeof tool !== "object") return null;
  const t = tool as Record<string, unknown>;
  // OpenAI wrapped format
  if (t.function && typeof t.function === "object") {
    return t.function as FunctionDef;
  }
  // Already unwrapped or other format
  if (t.name && typeof t.name === "string") {
    return t as unknown as FunctionDef;
  }
  return null;
}

export function ToolDefinitionsContent({ tools, toolChoice }: ToolDefinitionsContentProps) {
  const functionDefs = tools.map(unwrapTool).filter((f): f is FunctionDef => f !== null);

  return (
    <div className="space-y-3">
      <table className="w-full text-xs">
        <thead>
          <tr className="border-b border-border/50 bg-muted/50">
            <th className="px-3 py-1.5 text-left font-medium">Name</th>
            <th className="px-3 py-1.5 text-left font-medium">Description</th>
            <th className="px-3 py-1.5 text-left font-medium">Parameters</th>
            <th className="px-3 py-1.5 text-left font-medium">Required</th>
          </tr>
        </thead>
        <tbody>
          {functionDefs.map((fn, i) => (
            <tr key={i} className="border-b border-border/30 last:border-0">
              <td className="px-3 py-1.5 font-mono font-medium whitespace-nowrap">{fn.name}</td>
              <td className="px-3 py-1.5 text-muted-foreground">{fn.description}</td>
              <td className="px-3 py-1.5 text-muted-foreground whitespace-nowrap">
                {Object.keys(fn.parameters?.properties || {}).join(", ")}
              </td>
              <td className="px-3 py-1.5 text-muted-foreground whitespace-nowrap">
                {fn.parameters?.required?.join(", ")}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {toolChoice !== undefined && (
        <div className="pt-2 border-t border-border/50">
          <span className="text-xs text-muted-foreground">Tool choice: </span>
          <JsonContent data={toolChoice} />
        </div>
      )}
    </div>
  );
}
