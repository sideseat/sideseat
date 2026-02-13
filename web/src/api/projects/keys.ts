import type { ListProjectsParams } from "./types";

export const projectKeys = {
  all: ["projects"] as const,
  lists: () => [...projectKeys.all, "list"] as const,
  list: (params?: ListProjectsParams) => [...projectKeys.lists(), params] as const,
  detail: (id: string) => [...projectKeys.all, "detail", id] as const,
};
