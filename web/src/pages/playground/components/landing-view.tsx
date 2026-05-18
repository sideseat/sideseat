import { Skeleton } from "@/components/ui/skeleton";
import type { RegistrationEntry } from "@/api/registrations/types";
import { cn } from "@/lib/utils";
import { AgentCard } from "./agent-card";

interface Props {
  agents: RegistrationEntry[];
  selected: string | null;
  onSelect: (name: string) => void;
  loading: boolean;
}

export function LandingView({ agents, selected, onSelect, loading }: Props) {
  const cols = Math.min(Math.max(agents.length, 1), 3);
  const gridCols =
    cols === 1
      ? "grid-cols-1"
      : cols === 2
        ? "grid-cols-1 sm:grid-cols-2"
        : "grid-cols-1 sm:grid-cols-2 lg:grid-cols-3";

  return (
    <div className="w-full">
      <div className="text-center">
        <h1 className="text-xl font-semibold">Pick an agent to chat</h1>
        <p className="mt-1 text-xs text-muted-foreground">
          {loading
            ? "Looking for online agents…"
            : agents.length === 1
              ? "One agent is online in this project."
              : `${agents.length} agents online in this project.`}
        </p>
      </div>
      <div className={cn("mt-4 grid gap-2", gridCols)}>
        {loading
          ? Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className="h-20 w-full rounded-lg" />
            ))
          : agents.map((agent) => (
              <AgentCard
                key={agent.name}
                agent={agent}
                selected={selected === agent.name}
                onSelect={onSelect}
              />
            ))}
      </div>
    </div>
  );
}
