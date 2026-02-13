import { useState } from "react";
import { Link } from "react-router";
import type { ICellRendererParams } from "ag-grid-community";
import { MoreVertical, Maximize2, ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DeleteEntityDialog } from "./delete-entity-dialog";
import { getEntityId, type GridContext } from "./index";

type ActionsCellParams = ICellRendererParams & {
  context?: GridContext;
};

export function ActionsCellRenderer(params: ActionsCellParams) {
  const { entityType = "trace", projectId, trackDeletedIds } = params.context ?? {};
  const entityId = getEntityId(entityType, params.data);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);

  const disabled = !entityId || !projectId;

  // Build entity URL based on type
  let entityUrl: string;
  if (entityType === "span") {
    // For spans, use trace_id and span_id from composite entityId
    const [traceId, spanId] = entityId?.split(":") ?? [];
    entityUrl = `/projects/${projectId}/observability/spans/${traceId}/${spanId}`;
  } else if (entityType === "session") {
    entityUrl = `/projects/${projectId}/observability/sessions/${entityId}`;
  } else {
    entityUrl = `/projects/${projectId}/observability/traces/${entityId}`;
  }

  return (
    <div
      className="flex items-center gap-0.5"
      data-actions-cell
      onClick={(e) => e.stopPropagation()}
    >
      <Button
        variant="ghost"
        size="icon-sm"
        className="h-7 w-7"
        disabled={disabled}
        asChild={!disabled}
        aria-label="Open full page"
      >
        {disabled ? (
          <span>
            <Maximize2 className="h-3.5 w-3.5" />
          </span>
        ) : (
          <Link to={entityUrl}>
            <Maximize2 className="h-3.5 w-3.5" />
          </Link>
        )}
      </Button>

      <Button
        variant="ghost"
        size="icon-sm"
        className="h-7 w-7"
        disabled={disabled}
        asChild={!disabled}
        aria-label="Open in new window"
      >
        {disabled ? (
          <span>
            <ExternalLink className="h-3.5 w-3.5" />
          </span>
        ) : (
          <a href={`/ui${entityUrl}`} target="_blank" rel="noopener noreferrer">
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        )}
      </Button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            className="h-7 w-7 cursor-pointer"
            disabled={disabled}
            aria-label="More actions"
          >
            <MoreVertical className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-32">
          <DropdownMenuItem
            onClick={() => setDeleteDialogOpen(true)}
            disabled={disabled}
            className="cursor-pointer"
          >
            Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {projectId && entityId && (
        <DeleteEntityDialog
          entityType={entityType}
          entityIds={[entityId]}
          projectId={projectId}
          open={deleteDialogOpen}
          onOpenChange={setDeleteDialogOpen}
          onSuccess={() => trackDeletedIds?.([entityId])}
        />
      )}
    </div>
  );
}
