import { useEffect, useMemo, useState, type ComponentType, type SVGProps } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  AlertTriangle,
  CheckCircle2,
  Loader2,
  Radar,
  Workflow,
  Layers,
  Map,
  PlayCircle,
  Lock,
} from "lucide-react";
import { cn } from "@/lib/utils";

type HealthStatus = "loading" | "healthy" | "error";

type FeatureStatus = "planned" | "in-progress" | "ready";

type Feature = {
  title: string;
  description: string;
  status: FeatureStatus;
  meta?: string;
  icon: ComponentType<SVGProps<SVGSVGElement>>;
};

export default function HomePage() {
  const [healthStatus, setHealthStatus] = useState<HealthStatus>("loading");
  const [healthMessage, setHealthMessage] = useState<string>("Pinging backend…");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const checkHealth = async () => {
      try {
        const response = await fetch("/api/v1/health");
        if (response.ok) {
          const text = await response.text();
          setHealthStatus("healthy");
          setHealthMessage(text || "OK");
          setError(null);
        } else {
          setHealthStatus("error");
          setError(`Status: ${response.status}`);
          setHealthMessage("The API returned an unexpected response.");
        }
      } catch (err) {
        setHealthStatus("error");
        setHealthMessage("The API is not reachable right now.");
        setError(err instanceof Error ? err.message : "Unknown error");
      }
    };

    checkHealth();
    // Check health every 30 seconds
    const interval = setInterval(checkHealth, 30000);
    return () => clearInterval(interval);
  }, []);

  const features: Feature[] = useMemo(
    () => [
      {
        title: "Dashboard Overview",
        description: "Unified insight into latency, costs, and model success rates.",
        status: "in-progress",
        meta: "Target: Sprint 3",
        icon: Layers,
      },
      {
        title: "Trace Explorer",
        description: "Filter, replay, and compare LLM traces from MCP/A2A sessions.",
        status: "planned",
        meta: "Design review pending",
        icon: Radar,
      },
      {
        title: "Prompt Library",
        description: "Versioned prompt catalog with evaluation runs and approvals.",
        status: "planned",
        meta: "Needs schema definition",
        icon: Workflow,
      },
      {
        title: "Proxy Playground",
        description: "Route requests through configurable guardrails and rate limits.",
        status: "planned",
        meta: "Requires infra spike",
        icon: Map,
      },
      {
        title: "Debugger Suite",
        description: "Step through MCP interactions and inspect cached tool responses.",
        status: "planned",
        meta: "Brings MCP & A2A debuggers together",
        icon: PlayCircle,
      },
      {
        title: "Team Policies",
        description: "Centralize environment keys, ACLs, and audit trails.",
        status: "planned",
        meta: "Security review in progress",
        icon: Lock,
      },
    ],
    [],
  );

  const statusConfig: Record<
    HealthStatus,
    {
      label: string;
      badgeClass: string;
      icon: ComponentType<SVGProps<SVGSVGElement>>;
      title: string;
      description: string;
    }
  > = {
    loading: {
      label: "Checking",
      badgeClass:
        "border border-dashed border-border text-muted-foreground bg-muted/40 dark:bg-muted/20",
      icon: Loader2,
      title: "Checking API health",
      description: "Awaiting a response from the SideSeat API.",
    },
    healthy: {
      label: "Operational",
      badgeClass: "border-transparent bg-emerald-500/15 text-emerald-600 dark:text-emerald-200",
      icon: CheckCircle2,
      title: "API connected",
      description: "Responses are flowing from the SideSeat API.",
    },
    error: {
      label: "Unavailable",
      badgeClass: "border-transparent bg-destructive/15 text-destructive dark:text-destructive",
      icon: AlertTriangle,
      title: "API unreachable",
      description: "We couldn't get a healthy response from the API.",
    },
  };

  const featureBadgeClasses: Record<FeatureStatus, string> = {
    ready: "border-transparent bg-emerald-500/20 text-emerald-700 dark:text-emerald-200",
    "in-progress": "border-transparent bg-amber-500/20 text-amber-700 dark:text-amber-200",
    planned: "border border-dashed border-border text-muted-foreground bg-muted/40",
  };

  const { label, badgeClass, icon: StatusIcon, title, description } = statusConfig[healthStatus];

  return (
    <div className="mx-auto flex w-full max-w-8xl flex-col gap-12 px-4 pb-4 sm:px-6 sm:pb-6">
      <section className="space-y-3">
        <Badge className="w-fit border border-dashed border-border bg-muted/40 text-xs text-muted-foreground">
          Early preview
        </Badge>
        <h1 className="text-3xl font-semibold tracking-tight">Welcome to SideSeat</h1>
        <p className="text-muted-foreground max-w-5xl text-base">
          SideSeat centralizes your AI development workflow—observe traces, iterate on prompts, and
          roll out guardrails with confidence. Here’s the pulse of the platform while we build.
        </p>
      </section>

      <section className="grid gap-6 lg:grid-cols-[1.2fr_minmax(0,1fr)]">
        <Card className="h-full">
          <CardHeader className="flex flex-row items-start justify-between gap-4">
            <div className="space-y-1">
              <CardTitle>Platform status</CardTitle>
              <CardDescription>Live connectivity checks for the SideSeat API.</CardDescription>
            </div>
            <Badge className={cn("capitalize", badgeClass)}>{label}</Badge>
          </CardHeader>
          <CardContent className="space-y-4">
            <Alert>
              <StatusIcon
                className={cn(
                  "size-4",
                  healthStatus === "loading" && "animate-spin text-muted-foreground",
                  healthStatus === "healthy" && "text-emerald-500",
                  healthStatus === "error" && "text-destructive",
                )}
              />
              <AlertTitle>{title}</AlertTitle>
              <AlertDescription>
                <span className="font-medium text-foreground">{healthMessage}</span>
                <span className="text-xs text-muted-foreground">{description}</span>
                {error && (
                  <span className="text-muted-foreground block text-sm">
                    Error details: {error}
                  </span>
                )}
              </AlertDescription>
            </Alert>

            <div className="rounded-lg border border-dashed border-border bg-muted/40 p-4 text-sm">
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium text-foreground">Endpoint</span>
                <code className="rounded-md bg-background px-2 py-1 text-xs font-semibold">
                  /api/v1/health
                </code>
              </div>
              <div className="mt-3 space-y-2 text-xs text-muted-foreground">
                <p>We re-check availability every 30 seconds.</p>
                {healthStatus === "loading" ? (
                  <Skeleton className="h-2 w-full max-w-[180px]" />
                ) : (
                  <p>
                    Last response:{" "}
                    <span className="text-foreground font-medium">{label.toLowerCase()}</span>
                  </p>
                )}
              </div>
            </div>
          </CardContent>
        </Card>

        <Card className="h-full">
          <CardHeader>
            <CardTitle>What’s shipping next?</CardTitle>
            <CardDescription>Keep an eye on the modules rolling out soon.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex items-start gap-3 rounded-lg border border-dashed border-border/70 bg-muted/30 p-4 text-sm">
              <Workflow className="mt-0.5 size-4 text-muted-foreground" />
              <div className="space-y-1">
                <p className="font-medium text-foreground">Dashboard insights</p>
                <p className="text-muted-foreground">
                  Surface live LLM usage, error funnels, and cost projections for every environment.
                </p>
              </div>
            </div>
            <div className="flex items-start gap-3 rounded-lg border border-dashed border-border/70 bg-muted/30 p-4 text-sm">
              <PlayCircle className="mt-0.5 size-4 text-muted-foreground" />
              <div className="space-y-1">
                <p className="font-medium text-foreground">Debugger suite</p>
                <p className="text-muted-foreground">
                  Drill into multi-step MCP sessions with request/response timelines and tool cache
                  inspection.
                </p>
              </div>
            </div>
          </CardContent>
        </Card>
      </section>

      <section className="space-y-6">
        <div className="space-y-1">
          <h2 className="text-xl font-semibold tracking-tight">Feature roadmap</h2>
          <p className="text-muted-foreground text-sm">
            A snapshot of what’s ready, what’s in motion, and what’s still on the drawing board.
          </p>
        </div>
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {features.map((feature) => {
            const FeatureIcon = feature.icon;

            return (
              <Card key={feature.title} className="h-full">
                <CardHeader className="gap-3">
                  <Badge className={cn("w-fit", featureBadgeClasses[feature.status])}>
                    {feature.status === "ready"
                      ? "Ready"
                      : feature.status === "in-progress"
                        ? "In progress"
                        : "Planned"}
                  </Badge>
                  <div className="flex items-start gap-2">
                    <FeatureIcon className="mt-1 size-4 text-muted-foreground" />
                    <CardTitle className="text-base leading-tight">{feature.title}</CardTitle>
                  </div>
                  <CardDescription>{feature.description}</CardDescription>
                </CardHeader>
                {feature.meta && (
                  <CardContent className="pt-0">
                    <p className="text-sm text-muted-foreground">{feature.meta}</p>
                  </CardContent>
                )}
              </Card>
            );
          })}
        </div>
      </section>
    </div>
  );
}
