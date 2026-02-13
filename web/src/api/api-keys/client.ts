import type { ApiClient } from "../api-client";
import type { ApiKey, CreateApiKeyRequest, CreateApiKeyResponse } from "./types";

export class ApiKeysClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  async list(orgId: string): Promise<ApiKey[]> {
    return this.client.get<ApiKey[]>(`/organizations/${orgId}/api-keys`);
  }

  async create(orgId: string, data: CreateApiKeyRequest): Promise<CreateApiKeyResponse> {
    return this.client.post<CreateApiKeyResponse>(`/organizations/${orgId}/api-keys`, data);
  }

  async delete(orgId: string, keyId: string): Promise<void> {
    await this.client.delete(`/organizations/${orgId}/api-keys/${keyId}`);
  }
}
