// Re-export PaginatedResponse from otel (shared type)
export type { PaginatedResponse } from "@/api/otel/types";

export interface Project {
  id: string;
  organization_id: string;
  name: string;
  created_at: string;
  updated_at: string;
}

export interface CreateProjectRequest {
  name: string;
  organization_id: string;
}

export interface UpdateProjectRequest {
  name: string;
}

export interface ListProjectsParams {
  page?: number;
  limit?: number;
  org_id?: string;
}
