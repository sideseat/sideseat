/* eslint-disable react-refresh/only-export-components */
import { createContext, useContext, useMemo, type ReactNode } from "react";
import { apiClient, type ApiClient } from "@/api/api-client";
import type { ApiKeysClient } from "@/api/api-keys/client";
import type { FavoritesClient } from "@/api/favorites/client";
import type { FilesClient } from "@/api/files/client";
import type { OrganizationsClient } from "@/api/organizations/client";
import type { OtelClient } from "@/api/otel/client";
import type { ProjectsClient } from "@/api/projects/client";

export interface AppContext {
  /** API client (includes auth, otel, etc.) */
  api: ApiClient;
}

const AppContextValue = createContext<AppContext | null>(null);

export interface AppProviderProps {
  /** Override API client (for testing) */
  api?: ApiClient;
  children: ReactNode;
}

export function AppProvider({ api = apiClient, children }: AppProviderProps) {
  const value = useMemo<AppContext>(() => ({ api }), [api]);
  return <AppContextValue.Provider value={value}>{children}</AppContextValue.Provider>;
}

export function useAppContext(): AppContext {
  const context = useContext(AppContextValue);
  if (!context) {
    throw new Error("useAppContext must be used within AppProvider");
  }
  return context;
}

/** Convenience hook for otel client (apiClient.otel) */
export function useOtelClient(): OtelClient {
  return useAppContext().api.otel;
}

/** Convenience hook for projects client (apiClient.projects) */
export function useProjectsClient(): ProjectsClient {
  return useAppContext().api.projects;
}

/** Convenience hook for api client */
export function useApiClient(): ApiClient {
  return useAppContext().api;
}

/** Convenience hook for favorites client (apiClient.favorites) */
export function useFavoritesClient(): FavoritesClient {
  return useAppContext().api.favorites;
}

/** Convenience hook for files client (apiClient.files) */
export function useFilesClient(): FilesClient {
  return useAppContext().api.files;
}

/** Convenience hook for organizations client (apiClient.organizations) */
export function useOrganizationsClient(): OrganizationsClient {
  return useAppContext().api.organizations;
}

/** Convenience hook for API keys client (apiClient.apiKeys) */
export function useApiKeysClient(): ApiKeysClient {
  return useAppContext().api.apiKeys;
}
