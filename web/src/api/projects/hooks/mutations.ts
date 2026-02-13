import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useProjectsClient } from "@/lib/app-context";
import { projectKeys } from "../keys";
import type { CreateProjectRequest, UpdateProjectRequest } from "../types";

export function useCreateProject() {
  const queryClient = useQueryClient();
  const projectsClient = useProjectsClient();

  return useMutation({
    mutationFn: (data: CreateProjectRequest) => projectsClient.createProject(data),
    onSuccess: (project) => {
      queryClient.invalidateQueries({ queryKey: projectKeys.lists() });
      toast.success(`Project "${project.name}" created`);
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to create project");
    },
  });
}

export function useUpdateProject() {
  const queryClient = useQueryClient();
  const projectsClient = useProjectsClient();

  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: UpdateProjectRequest }) =>
      projectsClient.updateProject(id, data),
    onSuccess: (project) => {
      queryClient.invalidateQueries({ queryKey: projectKeys.lists() });
      queryClient.invalidateQueries({ queryKey: projectKeys.detail(project.id) });
      toast.success(`Project renamed to "${project.name}"`);
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to update project");
    },
  });
}

export function useDeleteProject() {
  const queryClient = useQueryClient();
  const projectsClient = useProjectsClient();

  return useMutation({
    mutationFn: (id: string) => projectsClient.deleteProject(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: projectKeys.lists() });
      toast.success("Project deleted");
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : "Failed to delete project");
    },
  });
}
