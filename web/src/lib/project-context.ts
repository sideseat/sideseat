import { createContext } from "react";
import type { Project } from "@/api/projects";

export interface ProjectContextValue {
  projectId: string;
  project: Project | undefined;
  isLoading: boolean;
  error: Error | null;
}

export const ProjectContext = createContext<ProjectContextValue | null>(null);
