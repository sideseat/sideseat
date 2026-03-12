import { useMemo } from "react";
import { Link } from "react-router";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

interface WaitingIndicatorProps {
  entityName: string;
  projectId: string;
  className?: string;
}

export function WaitingIndicator({ entityName, projectId, className }: WaitingIndicatorProps) {
  const telemetryUrl = useMemo(
    () => `/organizations/default/configuration/telemetry?project=${projectId}`,
    [projectId],
  );

  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center gap-3 p-4 sm:p-8 max-w-full",
        className,
      )}
    >
      <Spinner className="size-6 text-muted-foreground" />
      <div className="text-center space-y-1">
        <p className="text-sm text-muted-foreground">Waiting for {entityName}...</p>
        <p className="text-xs text-muted-foreground/70">Configure your app to send traces here:</p>
      </div>
      <Button variant="outline" size="sm" className="mt-1" asChild>
        <Link to={telemetryUrl}>View setup instructions</Link>
      </Button>
    </div>
  );
}
