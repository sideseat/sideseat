import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Activity, BarChart3, Gauge, Workflow } from "lucide-react";
import { toast } from "sonner";

export default function DashboardPage() {
  const snapshotCards = [
    {
      title: "Latency (P95)",
      description: "Track how requests perform across environments.",
      icon: Gauge,
    },
    {
      title: "Trace Volume",
      description: "Monitor LLM traffic spikes before they become incidents.",
      icon: Activity,
    },
    {
      title: "Cost per Token",
      description: "Understand spend trends with per-model rollups.",
      icon: BarChart3,
    },
  ];

  const onLaunchBuilder = () => {
    toast.success("Launching Builder...");
  };

  return (
    <div className="mx-auto flex w-full max-w-8xl flex-col gap-10 px-2 sm:px-4">
      <section className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h1 className="text-3xl font-semibold tracking-tight">Operational dashboard</h1>
            <p className="text-muted-foreground max-w-4xl text-base">
              Consolidate model metrics, trace analytics, and guardrail performance in one view.
              These tiles will light up as data sources land.
            </p>
          </div>
          <Button size="lg" className="gap-2" onClick={onLaunchBuilder}>
            Launch Builder
            <Workflow className="size-4" />
          </Button>
        </div>
      </section>

      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        {snapshotCards.map((card) => {
          const Icon = card.icon;
          return (
            <Card key={card.title} className="h-full">
              <CardHeader className="gap-3">
                <Icon className="size-4 text-muted-foreground" />
                <CardTitle className="text-base">{card.title}</CardTitle>
                <CardDescription>{card.description}</CardDescription>
              </CardHeader>
              <CardContent className="space-y-3">
                <Skeleton className="h-3 w-3/4 rounded-full" />
                <Skeleton className="h-32 rounded-lg" />
              </CardContent>
            </Card>
          );
        })}
      </section>

      <Card>
        <CardHeader>
          <CardTitle>Activation checklist</CardTitle>
          <CardDescription>Plug in data and ship tailored insights to your team.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center gap-3">
            <div className="size-2 rounded-full bg-muted" />
            <span className="text-sm text-muted-foreground">
              Connect SideSeat to your MCP gateway
            </span>
          </div>
          <div className="flex items-center gap-3">
            <div className="size-2 rounded-full bg-muted" />
            <span className="text-sm text-muted-foreground">
              Configure prompt evaluation runs in the Prompts workspace
            </span>
          </div>
          <div className="flex items-center gap-3">
            <div className="size-2 rounded-full bg-muted" />
            <span className="text-sm text-muted-foreground">
              Invite teammates and assign guardrail permissions
            </span>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
