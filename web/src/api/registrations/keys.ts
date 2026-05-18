export const registrationsKeys = {
  all: (projectId: string) => ["registrations", projectId] as const,
  list: (projectId: string) => [...registrationsKeys.all(projectId), "list"] as const,
};
