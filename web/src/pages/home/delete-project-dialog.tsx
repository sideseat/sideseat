import type { Project } from "@/api/projects";
import { useDeleteProject } from "@/api/projects";
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

interface DeleteProjectDialogProps {
  project: Project | null;
  onOpenChange: (open: boolean) => void;
}

export function DeleteProjectDialog({ project, onOpenChange }: DeleteProjectDialogProps) {
  const { mutate, isPending } = useDeleteProject();

  const handleDelete = () => {
    if (!project) return;
    mutate(project.id, {
      onSuccess: () => onOpenChange(false),
    });
  };

  return (
    <AlertDialog open={!!project} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete Project</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div>
              <span>Are you sure you want to delete </span>
              <span className="font-semibold break-all">"{project?.name}"</span>
              <span>
                ? This will permanently delete all traces, spans, and data associated with this
                project.
              </span>
            </div>
          </AlertDialogDescription>
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
