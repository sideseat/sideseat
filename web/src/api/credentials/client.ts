import type { ApiClient } from "../api-client";
import type {
  Credential,
  CreateCredentialRequest,
  UpdateCredentialRequest,
  TestResult,
  CredentialPermission,
  CreatePermissionRequest,
} from "./types";

export class CredentialsClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  async list(orgId: string, projectId?: string): Promise<Credential[]> {
    const params = projectId ? { project_id: projectId } : undefined;
    return this.client.get<Credential[]>(`/organizations/${orgId}/credentials`, params);
  }

  async create(orgId: string, req: CreateCredentialRequest): Promise<Credential> {
    return this.client.post<Credential>(`/organizations/${orgId}/credentials`, req);
  }

  async update(orgId: string, id: string, req: UpdateCredentialRequest): Promise<Credential> {
    return this.client.patch<Credential>(
      `/organizations/${orgId}/credentials/${encodeURIComponent(id)}`,
      req,
    );
  }

  async delete(orgId: string, id: string): Promise<void> {
    await this.client.delete(`/organizations/${orgId}/credentials/${encodeURIComponent(id)}`);
  }

  async test(orgId: string, id: string): Promise<TestResult> {
    return this.client.post<TestResult>(
      `/organizations/${orgId}/credentials/${encodeURIComponent(id)}/test`,
    );
  }

  async listPermissions(orgId: string, credentialId: string): Promise<CredentialPermission[]> {
    return this.client.get<CredentialPermission[]>(
      `/organizations/${orgId}/credentials/${encodeURIComponent(credentialId)}/permissions`,
    );
  }

  async createPermission(
    orgId: string,
    credentialId: string,
    req: CreatePermissionRequest,
  ): Promise<CredentialPermission> {
    return this.client.post<CredentialPermission>(
      `/organizations/${orgId}/credentials/${encodeURIComponent(credentialId)}/permissions`,
      req,
    );
  }

  async deletePermission(
    orgId: string,
    credentialId: string,
    permId: string,
  ): Promise<void> {
    await this.client.delete(
      `/organizations/${orgId}/credentials/${encodeURIComponent(credentialId)}/permissions/${encodeURIComponent(permId)}`,
    );
  }
}
