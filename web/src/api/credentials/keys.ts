export const credentialKeys = {
  all: ["credentials"] as const,
  lists: () => [...credentialKeys.all, "list"] as const,
  list: (orgId: string, projectId?: string | null) =>
    [...credentialKeys.lists(), orgId, projectId ?? null] as const,
  permissions: (orgId: string, credentialId: string) =>
    [...credentialKeys.all, orgId, credentialId, "permissions"] as const,
};
