import type { ApiClient } from "../api-client";
import type { ListOrgsParams, OrgWithRole, PaginatedResponse } from "./types";

export class OrganizationsClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  async list(params?: ListOrgsParams): Promise<PaginatedResponse<OrgWithRole>> {
    return this.client.get<PaginatedResponse<OrgWithRole>>(
      "/organizations",
      params as Record<string, unknown>,
    );
  }
}
