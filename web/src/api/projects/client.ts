import type { ApiClient } from "../api-client";
import type {
  CreateProjectRequest,
  ListProjectsParams,
  PaginatedResponse,
  Project,
  UpdateProjectRequest,
} from "./types";

export class ProjectsClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  async listProjects(params?: ListProjectsParams): Promise<PaginatedResponse<Project>> {
    return this.client.get<PaginatedResponse<Project>>(
      "/projects",
      params as Record<string, unknown>,
    );
  }

  async getProject(id: string): Promise<Project> {
    return this.client.get<Project>(`/projects/${id}`);
  }

  async createProject(data: CreateProjectRequest): Promise<Project> {
    return this.client.post<Project>("/projects", data);
  }

  async updateProject(id: string, data: UpdateProjectRequest): Promise<Project> {
    return this.client.put<Project>(`/projects/${id}`, data);
  }

  async deleteProject(id: string): Promise<void> {
    await this.client.delete(`/projects/${id}`);
  }
}
