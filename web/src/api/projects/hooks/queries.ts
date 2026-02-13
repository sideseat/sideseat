import { useQuery } from "@tanstack/react-query";
import { useProjectsClient } from "@/lib/app-context";
import { projectKeys } from "../keys";
import type { ListProjectsParams } from "../types";

export function useProjects(params?: ListProjectsParams) {
  const projectsClient = useProjectsClient();
  // Load projects with max allowed limit (server limit is 100)
  const effectiveParams = { page: 1, limit: 100, ...params };
  return useQuery({
    queryKey: projectKeys.list(effectiveParams),
    queryFn: () => projectsClient.listProjects(effectiveParams),
  });
}

export function useProject(id: string) {
  const projectsClient = useProjectsClient();
  return useQuery({
    queryKey: projectKeys.detail(id),
    queryFn: () => projectsClient.getProject(id),
    enabled: !!id,
  });
}
