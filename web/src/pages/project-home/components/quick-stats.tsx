import { Link } from "react-router";
import { GitBranch, Users, Layers, User } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCompact } from "@/lib/format";
import { cn } from "@/lib/utils";

interface QuickStatsProps {
  projectId: string;
  traces: number;
  sessions: number;
  spans: number;
  uniqueUsers: number;
  isLoading?: boolean;
}

interface StatCardProps {
  label: string;
  value: number;
  icon: React.ReactNode;
  href?: string;
}

function StatCard({ label, value, icon, href }: StatCardProps) {
  const displayValue = formatCompact(value);

  const content = (
    <div className="flex items-center gap-3">
      <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-muted/60 text-muted-foreground">
        {icon}
      </div>
      <div>
        <div className="text-2xl font-semibold tabular-nums">{displayValue}</div>
        <div className="text-xs uppercase tracking-wide text-muted-foreground">{label}</div>
      </div>
    </div>
  );

  if (href) {
    return (
      <Link to={href} className="block h-full">
        <Card
          className={cn(
            "h-full py-4 border-border/60 bg-card/80 shadow-sm transition-all",
            "hover:border-primary/30 hover:shadow-md hover:bg-background/80",
          )}
        >
          <CardContent className="h-full py-0 flex items-center">{content}</CardContent>
        </Card>
      </Link>
    );
  }

  return (
    <Card className="h-full py-4 border-border/60 bg-card/80 shadow-sm">
      <CardContent className="h-full py-0 flex items-center">{content}</CardContent>
    </Card>
  );
}

function StatCardSkeleton() {
  return (
    <Card className="h-full py-4 border-border/60 bg-card/80 shadow-sm">
      <CardContent className="h-full py-0 flex items-center">
        <div className="flex items-center gap-3">
          <Skeleton className="h-9 w-9 rounded-lg" />
          <div>
            <Skeleton className="h-7 w-16 mb-1" />
            <Skeleton className="h-3 w-12" />
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

export function QuickStats({
  projectId,
  traces,
  sessions,
  spans,
  uniqueUsers,
  isLoading,
}: QuickStatsProps) {
  if (isLoading) {
    return (
      <div className="grid h-full grid-cols-2 gap-3 sm:grid-cols-4 lg:grid-cols-2 lg:auto-rows-fr">
        <StatCardSkeleton />
        <StatCardSkeleton />
        <StatCardSkeleton />
        <StatCardSkeleton />
      </div>
    );
  }

  return (
    <div className="grid h-full grid-cols-2 gap-3 sm:grid-cols-4 lg:grid-cols-2 lg:auto-rows-fr">
      <StatCard
        label="Traces"
        value={traces}
        icon={<GitBranch className="h-4 w-4" />}
        href={`/projects/${projectId}/observability/traces`}
      />
      <StatCard
        label="Sessions"
        value={sessions}
        icon={<Users className="h-4 w-4" />}
        href={`/projects/${projectId}/observability/sessions`}
      />
      <StatCard
        label="Spans"
        value={spans}
        icon={<Layers className="h-4 w-4" />}
        href={`/projects/${projectId}/observability/spans`}
      />
      <StatCard label="Users" value={uniqueUsers} icon={<User className="h-4 w-4" />} />
    </div>
  );
}
