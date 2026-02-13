import { useQuery } from "@tanstack/react-query";
import { useOrganizationsClient } from "@/lib/app-context";
import { organizationKeys } from "../keys";
import type { ListOrgsParams } from "../types";

export function useOrganizations(params?: ListOrgsParams) {
  const client = useOrganizationsClient();
  const effectiveParams = { page: 1, limit: 100, ...params };
  return useQuery({
    queryKey: organizationKeys.list(effectiveParams),
    queryFn: () => client.list(effectiveParams),
  });
}
