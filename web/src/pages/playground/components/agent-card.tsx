import { Bot, Sparkles, Users, Workflow, type LucideIcon } from "lucide-react";
import type { RegistrationEntry } from "@/api/registrations/types";
import { cn } from "@/lib/utils";

interface Props {
  agent: RegistrationEntry;
  selected: boolean;
  onSelect: (name: string) => void;
}

const FRAMEWORK_ICONS: Record<string, LucideIcon> = {
  strands: Bot,
  "strands-python": Bot,
  langgraph: Workflow,
  langchain: Workflow,
  crewai: Users,
  autogen: Users,
  openai: Sparkles,
  anthropic: Sparkles,
  pydantic_ai: Sparkles,
};

const PY_REPR = /^<[\w.]+ object at 0x[0-9a-f]+>$/;

function displayModel(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const trimmed = raw.trim();
  if (!trimmed) return null;
  if (PY_REPR.test(trimmed)) {
    const inner = trimmed.slice(1, trimmed.indexOf(" object at "));
    return inner.split(".").pop() ?? null;
  }
  return trimmed;
}

export function AgentCard({ agent, selected, onSelect }: Props) {
  const fw = (agent.manifest.framework ?? "").toLowerCase();
  const Icon = FRAMEWORK_ICONS[fw] ?? Bot;
  const model = displayModel(agent.manifest.model);
  const fwLabel = agent.manifest.framework ?? "agent";
  const promptHint =
    typeof agent.manifest.system_prompt === "string" ? agent.manifest.system_prompt.trim() : "";

  return (
    <button
      type="button"
      onClick={() => onSelect(agent.name)}
      className={cn(
        "rounded-lg border bg-card p-3 text-left transition-colors",
        "hover:bg-accent/40 focus:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        selected && "border-primary/60 bg-accent/30",
      )}
    >
      <div className="flex items-center gap-2">
        <Icon className="size-4 shrink-0 text-muted-foreground" />
        <span className="truncate text-sm font-medium">{agent.name}</span>
        <span
          className="ml-auto size-1.5 shrink-0 rounded-full bg-green-500"
          aria-label="online"
          title="online"
        />
      </div>
      <div className="mt-1 truncate text-xs text-muted-foreground">
        {fwLabel}
        {model && <span> · {model}</span>}
      </div>
      {promptHint && (
        <div className="mt-2 line-clamp-2 text-xs text-muted-foreground">{promptHint}</div>
      )}
    </button>
  );
}
