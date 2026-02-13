import { useMemo } from "react";
import { useLocation, useNavigate } from "react-router";
import { Check, ChevronDown, Plus } from "lucide-react";
import { useProjects } from "@/api/projects/hooks/queries";
import { useCurrentProject } from "@/hooks/use-project";
import { sortProjectsWithDefaultFirst } from "@/lib/utils";
import { CreateProjectDialog } from "@/pages/home/create-project-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import type { Project } from "@/api/projects";

interface ProjectSwitcherProps {
  createDialogOpen: boolean;
  onCreateDialogOpenChange: (open: boolean) => void;
}

export function ProjectSwitcher({
  createDialogOpen,
  onCreateDialogOpenChange,
}: ProjectSwitcherProps) {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { projectId, project } = useCurrentProject();
  const { data: projectsData, isLoading: projectsLoading } = useProjects();

  const projects = useMemo(
    () => sortProjectsWithDefaultFirst(projectsData?.data ?? []),
    [projectsData?.data],
  );

  // Extract the subpath after /projects/:projectId/
  const getSubpath = () => {
    const match = pathname.match(/^\/projects\/[^/]+\/(.*)$/);
    return match ? match[1] : "home";
  };

  const handleProjectSelect = (selectedProject: Project) => {
    if (selectedProject.id === projectId) return;
    const subpath = getSubpath();
    navigate(`/projects/${selectedProject.id}/${subpath}`);
  };

  const handleNewProjectCreated = (newProject: Project) => {
    onCreateDialogOpenChange(false);
    navigate(`/projects/${newProject.id}/home`);
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger className="flex items-center gap-1 rounded-md px-2 py-1 text-sm font-medium hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          {project ? (
            <span className="max-w-[200px] truncate">{project.name}</span>
          ) : (
            <Skeleton className="h-4 w-24" />
          )}
          <ChevronDown className="h-4 w-4 opacity-50" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          {projectsLoading ? (
            <div className="p-2 space-y-2">
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
            </div>
          ) : (
            <>
              {projects.map((p) => (
                <DropdownMenuItem
                  key={p.id}
                  onClick={() => handleProjectSelect(p)}
                  className="flex items-center justify-between"
                >
                  <span className="truncate">{p.name}</span>
                  {p.id === projectId && <Check className="h-4 w-4 shrink-0" />}
                </DropdownMenuItem>
              ))}
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => onCreateDialogOpenChange(true)}>
                <Plus className="mr-2 h-4 w-4" />
                New Project
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <CreateProjectDialog
        open={createDialogOpen}
        onOpenChange={onCreateDialogOpenChange}
        onCreated={handleNewProjectCreated}
      />
    </>
  );
}
