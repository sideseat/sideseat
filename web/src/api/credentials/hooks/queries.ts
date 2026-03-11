import { useQuery } from "@tanstack/react-query";
import { useCredentialsClient } from "@/lib/app-context";
import { credentialKeys } from "../keys";

export function useCredentials(orgId: string, projectId?: string | null) {
  const client = useCredentialsClient();

  return useQuery({
    queryKey: credentialKeys.list(orgId, projectId),
    queryFn: () => client.list(orgId, projectId ?? undefined),
    enabled: !!orgId,
  });
}

export function useCredentialPermissions(orgId: string, credentialId: string) {
  const client = useCredentialsClient();

  return useQuery({
    queryKey: credentialKeys.permissions(orgId, credentialId),
    queryFn: () => client.listPermissions(orgId, credentialId),
    enabled: !!orgId && !!credentialId,
  });
}
