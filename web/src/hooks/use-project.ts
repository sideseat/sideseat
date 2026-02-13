import { useContext } from "react";
import { ProjectContext, type ProjectContextValue } from "@/lib/project-context";
import type { Project } from "@/api/projects";

export function useCurrentProject(): ProjectContextValue {
  const context = useContext(ProjectContext);
  if (!context) {
    throw new Error("useCurrentProject must be used within a ProjectProvider");
  }
  return context;
}

interface UseRequiredProjectResult {
  projectId: string;
  project: Project;
}

export function useRequiredProject(): UseRequiredProjectResult {
  const context = useContext(ProjectContext);
  if (!context) {
    throw new Error("useRequiredProject must be used within a ProjectProvider");
  }
  if (!context.project) {
    throw new Error(
      "useRequiredProject called before project loaded. Use within loaded ProjectLayout.",
    );
  }
  return { projectId: context.projectId, project: context.project };
}
