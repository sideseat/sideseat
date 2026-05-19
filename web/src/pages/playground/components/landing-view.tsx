import { Skeleton } from "@/components/ui/skeleton";
import type { RegistrationEntry } from "@/api/registrations/types";
import { AgentCard } from "./agent-card";

interface Props {
  entries: RegistrationEntry[];
  selected: string | null;
  onSelect: (name: string) => void;
  loading: boolean;
}

export function LandingView({ entries, selected, onSelect, loading }: Props) {
  return (
    <section className="flex w-full flex-col gap-4">
      <div className="flex items-baseline justify-between gap-3">
        <h1 className="text-2xl font-semibold tracking-tight">Pick an agent to chat</h1>
        <p className="text-sm text-muted-foreground">
          {loading
            ? "Looking…"
            : entries.length === 1
              ? "1 registration"
              : `${entries.length} registrations`}
        </p>
      </div>

      {/* Uniform grid: `auto-fit` gives every column the same width (1fr);
          `auto-rows-fr` makes every row equal in height. Combined with the
          card's `h-full`, every card ends up the same size. */}
      <div
        className="grid auto-rows-fr gap-3"
        style={{ gridTemplateColumns: "repeat(auto-fit, minmax(min(280px, 100%), 1fr))" }}
        role="list"
      >
        {loading
          ? Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className="h-28 w-full rounded-xl" />
            ))
          : entries.map((entry) => (
              <div role="listitem" className="h-full" key={`${entry.kind}:${entry.name}`}>
                <AgentCard agent={entry} selected={selected === entry.name} onSelect={onSelect} />
              </div>
            ))}
      </div>
    </section>
  );
}
