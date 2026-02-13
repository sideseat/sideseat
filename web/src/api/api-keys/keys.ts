export const apiKeyKeys = {
  all: ["api-keys"] as const,
  lists: () => [...apiKeyKeys.all, "list"] as const,
  list: (orgId: string) => [...apiKeyKeys.lists(), orgId] as const,
};
