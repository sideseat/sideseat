import { useDeleteTraces, useDeleteSessions, useDeleteSpans } from "@/api/otel/hooks/mutations";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Spinner } from "@/components/ui/spinner";
import type { EntityType } from "./index";

interface DeleteEntityDialogProps {
  entityType: EntityType;
  entityIds: string[];
  projectId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSuccess?: () => void;
}

export function DeleteEntityDialog({
  entityType,
  entityIds,
  projectId,
  open,
  onOpenChange,
  onSuccess,
}: DeleteEntityDialogProps) {
  const traceMutation = useDeleteTraces();
  const sessionMutation = useDeleteSessions();
  const spanMutation = useDeleteSpans();

  const isPending =
    entityType === "trace"
      ? traceMutation.isPending
      : entityType === "session"
        ? sessionMutation.isPending
        : spanMutation.isPending;

  const count = entityIds.length;

  const getEntityLabels = () => {
    switch (entityType) {
      case "trace":
        return { singular: "Trace", plural: "Traces" };
      case "session":
        return { singular: "Session", plural: "Sessions" };
      case "span":
        return { singular: "Span", plural: "Spans" };
    }
  };

  const { singular: entityLabel, plural: entityLabelPlural } = getEntityLabels();

  const getDescription = () => {
    switch (entityType) {
      case "trace":
        return `Are you sure you want to delete ${count === 1 ? "this trace" : `${count} traces`}? This will permanently delete all spans and data associated with ${count === 1 ? "this trace" : "these traces"}.`;
      case "session":
        return `Are you sure you want to delete ${count === 1 ? "this session" : `${count} sessions`}? This will permanently delete all traces, spans, and data within ${count === 1 ? "this session" : "these sessions"}.`;
      case "span":
        return `Are you sure you want to delete ${count === 1 ? "this span" : `${count} spans`}? This action cannot be undone.`;
    }
  };

  const description = getDescription();

  const handleDelete = () => {
    if (count === 0) return;

    const onMutationSuccess = () => {
      onOpenChange(false);
      onSuccess?.();
    };

    switch (entityType) {
      case "trace":
        traceMutation.mutate({ projectId, traceIds: entityIds }, { onSuccess: onMutationSuccess });
        break;
      case "session":
        sessionMutation.mutate(
          { projectId, sessionIds: entityIds },
          { onSuccess: onMutationSuccess },
        );
        break;
      case "span":
        spanMutation.mutate({ projectId, spanIds: entityIds }, { onSuccess: onMutationSuccess });
        break;
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Delete {count === 1 ? entityLabel : entityLabelPlural}
          </AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={handleDelete} disabled={isPending}>
            {isPending && <Spinner className="mr-2 h-4 w-4" />}
            Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
