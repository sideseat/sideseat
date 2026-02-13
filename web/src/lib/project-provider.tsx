import type { ReactNode } from "react";
import { useParams } from "react-router";
import { useProject } from "@/api/projects/hooks/queries";
import { ProjectContext } from "./project-context";

export function ProjectProvider({ children }: { children: ReactNode }) {
  const { projectId } = useParams<{ projectId: string }>();
  const { data: project, isLoading, error } = useProject(projectId!);

  return (
    <ProjectContext.Provider
      value={{
        projectId: projectId!,
        project,
        isLoading,
        error: error as Error | null,
      }}
    >
      {children}
    </ProjectContext.Provider>
  );
}
