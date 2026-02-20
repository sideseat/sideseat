import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { Plus } from "lucide-react";

import { useProjects, type Project } from "@/api/projects";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { sortProjectsWithDefaultFirst } from "@/lib/utils";

import { ProjectCard } from "./project-card";
import { CreateProjectDialog } from "./create-project-dialog";
import { EditProjectDialog } from "./edit-project-dialog";
import { DeleteProjectDialog } from "./delete-project-dialog";

export default function HomePage() {
  const navigate = useNavigate();
  const { data, isLoading } = useProjects();
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [editProject, setEditProject] = useState<Project | null>(null);
  const [deleteProject, setDeleteProject] = useState<Project | null>(null);

  const projects = useMemo(() => sortProjectsWithDefaultFirst(data?.data ?? []), [data?.data]);

  const handleProjectCreated = (project: Project) => {
    setCreateDialogOpen(false);
    navigate(`/projects/${project.id}/home`);
  };

  return (
    <div className="min-h-screen bg-background">
      <PageHeader />

      <div className="mx-auto flex w-full max-w-400 flex-col gap-8 px-4 py-6 sm:px-6">
        <section className="relative overflow-hidden rounded-3xl border border-border/60 bg-linear-to-br from-sky-500/10 via-emerald-500/10 to-amber-500/10 p-6 shadow-xl dark:from-violet-500/15 dark:via-indigo-500/20 dark:to-cyan-500/15 sm:p-10">
          <div className="absolute -right-10 -top-10 h-44 w-44 rounded-full bg-sky-500/10 blur-3xl dark:bg-violet-500/30" />
          <div className="absolute bottom-0 left-4 h-24 w-24 rounded-full bg-emerald-500/15 blur-2xl dark:bg-cyan-500/30" />
          <div className="relative flex flex-col gap-8 lg:flex-row lg:items-center lg:justify-between">
            <div className="max-w-2xl space-y-5">
              <div>
                <h1 className="text-4xl font-semibold tracking-tight sm:text-5xl">
                  Welcome to{" "}
                  <span className="bg-linear-to-r from-primary via-primary/80 to-primary/60 bg-clip-text text-transparent">
                    SideSeat
                  </span>
                </h1>
                <p className="mt-3 text-base text-muted-foreground sm:text-lg">
                  AI Development Workbench
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-3">
                <Button size="lg" onClick={() => setCreateDialogOpen(true)}>
                  <Plus className="mr-2 h-4 w-4" />
                  New Project
                </Button>
              </div>
            </div>
            <div className="flex w-full min-w-70 flex-col items-center rounded-2xl border border-border/60 bg-card/80 px-12 py-8 shadow-lg backdrop-blur sm:w-auto">
              <p className="text-sm font-medium uppercase tracking-wider text-muted-foreground">
                Projects
              </p>
              <p className="mt-2 text-6xl font-bold tabular-nums">
                {projects.length || (isLoading ? "—" : "0")}
              </p>
              <p className="mt-3 text-sm text-muted-foreground">Workspaces managed</p>
            </div>
          </div>
        </section>

        {/* Loading state with Spinner */}
        {isLoading && (
          <div className="flex justify-center py-12">
            <Spinner className="h-8 w-8" />
          </div>
        )}

        {/* Project grid/list */}
        {!isLoading && (
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <div>
                <h2 className="text-lg font-semibold tracking-tight">Projects</h2>
                <p className="text-sm text-muted-foreground">
                  Separate customer workspaces to keep data clean.
                </p>
              </div>
              <Button variant="outline" size="sm" onClick={() => setCreateDialogOpen(true)}>
                <Plus className="mr-2 h-4 w-4" />
                New Project
              </Button>
            </div>
            {projects.length > 0 ? (
              <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                {projects.map((project) => (
                  <ProjectCard
                    key={project.id}
                    project={project}
                    onEdit={() => setEditProject(project)}
                    onDelete={() => setDeleteProject(project)}
                  />
                ))}
              </div>
            ) : (
              <div className="rounded-2xl border border-dashed border-border/70 bg-muted/40 p-6 text-sm text-muted-foreground">
                No projects yet. Spin up a dedicated environment for each customer or team.
              </div>
            )}
          </div>
        )}

        {/* Create Project Dialog */}
        <CreateProjectDialog
          open={createDialogOpen}
          onOpenChange={setCreateDialogOpen}
          onCreated={handleProjectCreated}
        />

        {/* Edit Project Dialog */}
        <EditProjectDialog
          project={editProject}
          onOpenChange={(open) => !open && setEditProject(null)}
        />

        {/* Delete Confirmation AlertDialog */}
        <DeleteProjectDialog
          project={deleteProject}
          onOpenChange={(open) => !open && setDeleteProject(null)}
        />
      </div>
    </div>
  );
}
