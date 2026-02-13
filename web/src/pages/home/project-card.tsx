import type { MouseEvent } from "react";
import { Link } from "react-router";
import { Copy, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";

import type { Project } from "@/api/projects";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

interface ProjectCardProps {
  project: Project;
  onEdit: () => void;
  onDelete: () => void;
}

export function ProjectCard({ project, onEdit, onDelete }: ProjectCardProps) {
  const formatDate = (dateStr: string) => {
    const date = new Date(dateStr);
    return date.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
      year: "numeric",
    });
  };

  const isDefault = project.id === "default";

  const handleEdit = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    onEdit();
  };

  const handleDelete = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    onDelete();
  };

  const handleCopyId = async (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(project.id);
      toast.success("Project ID copied to clipboard");
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  };

  return (
    <Link to={`/projects/${project.id}/home`} className="block">
      <Card
        className={`border-border bg-card shadow-sm transition-colors hover:border-primary/50 hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/60 ${
          isDefault ? "border-primary/60 ring-1 ring-primary/40" : ""
        }`}
      >
        <CardHeader className="flex flex-row items-start justify-between gap-3">
          <div className="min-w-0 flex-1 space-y-2">
            <CardTitle className="text-xl leading-tight truncate">{project.name}</CardTitle>
            <button
              type="button"
              onClick={handleCopyId}
              className="group flex items-center gap-1.5 font-mono text-sm text-muted-foreground hover:text-foreground"
              aria-label="Copy project ID"
            >
              {project.id}
              <Copy className="h-3.5 w-3.5" />
            </button>
          </div>
          {!isDefault && (
            <div className="flex gap-1">
              <button
                type="button"
                onClick={handleEdit}
                className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-muted"
                aria-label="Edit project"
              >
                <Pencil className="h-4 w-4" />
              </button>
              <button
                type="button"
                onClick={handleDelete}
                className="rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-muted"
                aria-label="Delete project"
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          )}
        </CardHeader>
        <CardContent>
          <div className="flex items-center justify-between rounded-md border border-dashed border-border/70 px-3 py-2 text-sm text-muted-foreground">
            <span>Created</span>
            <span className="font-medium text-foreground">{formatDate(project.created_at)}</span>
          </div>
        </CardContent>
      </Card>
    </Link>
  );
}
