import { useQuery } from "@tanstack/react-query";
import { useApiKeysClient } from "@/lib/app-context";
import { apiKeyKeys } from "../keys";

export function useApiKeys(orgId: string) {
  const client = useApiKeysClient();

  return useQuery({
    queryKey: apiKeyKeys.list(orgId),
    queryFn: () => client.list(orgId),
    enabled: !!orgId,
  });
}
