import { Bot, Network, Sparkles, Users, Workflow, type LucideIcon } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type { RegistrationEntry, RegistrationKind } from "@/api/registrations/types";
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

const KIND_ICON: Partial<Record<RegistrationKind, LucideIcon>> = {
  graph: Network,
  swarm: Users,
};

const FRAMEWORK_LABELS: Record<string, string> = {
  strands: "Strands",
  "strands-python": "Strands Python",
  langgraph: "LangGraph",
  langchain: "LangChain",
  crewai: "CrewAI",
  autogen: "AutoGen",
  openai: "OpenAI",
  anthropic: "Anthropic",
  pydantic_ai: "PydanticAI",
};

function frameworkLabel(raw: string | null | undefined, fallback: string): string {
  if (!raw) return fallback;
  const known = FRAMEWORK_LABELS[raw.toLowerCase()];
  if (known) return known;
  return raw
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

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

interface NodeSummary {
  node_id?: string;
  name?: string;
  type?: string;
}

function isNodeSummary(item: unknown): item is NodeSummary {
  return typeof item === "object" && item !== null && ("node_id" in item || "name" in item);
}

function nodeSummary(kind: RegistrationKind, tools: unknown[] | undefined): string | null {
  if (kind !== "graph" && kind !== "swarm") return null;
  if (!Array.isArray(tools) || tools.length === 0) return null;
  const names: string[] = [];
  for (const item of tools) {
    if (!isNodeSummary(item)) continue;
    const label = item.name ?? item.node_id;
    if (typeof label === "string" && label) names.push(label);
  }
  if (names.length === 0) return null;
  const sep = kind === "graph" ? " → " : ", ";
  return `${names.length} ${names.length === 1 ? "node" : "nodes"} · ${names.join(sep)}`;
}

export function AgentCard({ agent, selected, onSelect }: Props) {
  const fw = (agent.manifest.framework ?? "").toLowerCase();
  const Icon = KIND_ICON[agent.kind] ?? FRAMEWORK_ICONS[fw] ?? Bot;
  const model = displayModel(agent.manifest.model);
  const fwLabel = frameworkLabel(agent.manifest.framework, agent.kind);
  const promptHint =
    typeof agent.manifest.system_prompt === "string" ? agent.manifest.system_prompt.trim() : "";
  const subtitle = nodeSummary(agent.kind, agent.manifest.tools) ?? promptHint;

  return (
    <button
      type="button"
      onClick={() => onSelect(agent.name)}
      title={agent.name}
      className="block h-full w-full text-left focus-visible:outline-none"
    >
      <Card
        className={cn(
          "h-full gap-3 border-border bg-card py-4 shadow-sm transition-colors",
          "hover:border-primary/50 hover:bg-accent/50",
          selected && "border-primary/60 ring-1 ring-primary/40",
        )}
      >
        <CardHeader className="flex flex-row items-start gap-3 px-5">
          <div className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted/40 text-muted-foreground">
            <Icon className="size-4" />
          </div>
          <div className="min-w-0 flex-1">
            <CardTitle className="flex items-center gap-2 text-base leading-tight">
              <span className="min-w-0 flex-1 truncate">{agent.name}</span>
              <Badge variant="outline" className="shrink-0 font-mono text-[10px] uppercase">
                {agent.kind}
              </Badge>
            </CardTitle>
            <p className="mt-1 truncate text-xs text-muted-foreground">
              {fwLabel}
              {model && <span> · {model}</span>}
            </p>
          </div>
        </CardHeader>
        {/* Body always renders so every card reserves the same height,
            even when a registration has no system_prompt or node summary. */}
        <CardContent className="px-5 pb-0">
          <p className="line-clamp-2 min-h-[2.6em] text-sm leading-relaxed text-muted-foreground">
            {subtitle || <span className="opacity-0">placeholder</span>}
          </p>
        </CardContent>
      </Card>
    </button>
  );
}
