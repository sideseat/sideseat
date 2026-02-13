import { useCallback, useMemo } from "react";
import { Link } from "react-router";
import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

interface WaitingIndicatorProps {
  entityName: string;
  projectId: string;
  className?: string;
}

export function WaitingIndicator({ entityName, projectId, className }: WaitingIndicatorProps) {
  const envVar = useMemo(
    () => `OTEL_EXPORTER_OTLP_ENDPOINT=${window.location.origin}/otel/${projectId}`,
    [projectId],
  );

  const telemetryUrl = useMemo(
    () => `/organizations/default/configuration/telemetry?project=${projectId}`,
    [projectId],
  );

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(envVar);
      toast.success("Copied to clipboard");
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  }, [envVar]);

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
        <p className="text-xs text-muted-foreground/70">Configure your app to send traces:</p>
      </div>
      <div className="flex items-center gap-1.5 mt-1 max-w-full px-2">
        <code className="text-xs bg-muted px-2 py-1 rounded font-mono overflow-x-auto max-w-[calc(100vw-120px)] sm:max-w-none whitespace-nowrap select-all cursor-text">
          {envVar}
        </code>
        <Button variant="ghost" size="sm" className="h-7 w-7 p-0 shrink-0" onClick={handleCopy}>
          <Copy className="h-3.5 w-3.5" />
        </Button>
      </div>
      <Button variant="link" size="sm" className="h-auto p-0 text-xs" asChild>
        <Link to={telemetryUrl}>View setup instructions</Link>
      </Button>
    </div>
  );
}
