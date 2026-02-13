export type { PaginatedResponse } from "@/api/otel/types";

export interface Organization {
  id: string;
  name: string;
  slug: string;
  created_at: string;
  updated_at: string;
}

export interface OrgWithRole extends Organization {
  role: string;
}

export interface ListOrgsParams {
  page?: number;
  limit?: number;
}
