import type { ListOrgsParams } from "./types";

export const organizationKeys = {
  all: ["organizations"] as const,
  lists: () => [...organizationKeys.all, "list"] as const,
  list: (params?: ListOrgsParams) => [...organizationKeys.lists(), params] as const,
};
