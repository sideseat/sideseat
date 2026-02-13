import type { CustomNoRowsOverlayProps } from "ag-grid-react";
import { Link } from "react-router";
import { Button } from "@/components/ui/button";
import { WaitingIndicator } from "@/components/waiting-indicator";

type GridEmptyOverlayProps = CustomNoRowsOverlayProps & {
  entityName?: "traces" | "sessions" | "spans";
  projectId?: string;
  realtimeEnabled?: boolean;
};

export function GridEmptyOverlay(props: GridEmptyOverlayProps) {
  const entityName = props.entityName ?? "traces";
  const projectId = props.projectId ?? "default";
  const realtimeEnabled = props.realtimeEnabled ?? false;

  if (realtimeEnabled) {
    return (
      <WaitingIndicator
        entityName={entityName}
        projectId={projectId}
        className="pointer-events-auto"
      />
    );
  }

  const telemetryUrl = `/organizations/default/configuration/telemetry?project=${projectId}`;

  return (
    <div className="flex flex-col items-center justify-center gap-2 p-8 pointer-events-auto">
      <p className="text-sm text-muted-foreground">No {entityName} found</p>
      <p className="text-xs text-muted-foreground/70">Try adjusting your filters or time range</p>
      <Button variant="link" size="sm" className="h-auto p-0 text-xs mt-1" asChild>
        <Link to={telemetryUrl}>View setup instructions</Link>
      </Button>
    </div>
  );
}
