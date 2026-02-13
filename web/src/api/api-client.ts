/**
 * API Client
 *
 * Centralized API client for all backend communication.
 * All API calls should go through this module.
 */

import { ApiKeysClient } from "./api-keys/client";
import { AuthClient } from "./auth-client";
import { FavoritesClient } from "./favorites/client";
import { FilesClient } from "./files/client";
import { OrganizationsClient } from "./organizations/client";
import { OtelClient } from "./otel/client";
import { ProjectsClient } from "./projects/client";
import type { ApiErrorResponse } from "./types";
import type { SseSpanEvent, SSEHandlers } from "./otel/types";

function getApiBaseUrl(): string {
  if (import.meta.env.VITE_API_URL) {
    return import.meta.env.VITE_API_URL;
  }
  return import.meta.env.PROD ? "/api/v1" : "http://localhost:5388/api/v1";
}

export const API_BASE_URL = getApiBaseUrl();

const DEFAULT_TIMEOUT = 30000;
const SSE_MAX_RECONNECT_ATTEMPTS = 10;
const SSE_MAX_BACKOFF = 30000;
// EventSource can't detect SSE keep-alive comments (spec limitation), so we
// reconnect after inactivity. Trade-off: idle healthy connections reconnect too.
const SSE_INACTIVITY_TIMEOUT = 45000;

/**
 * Error thrown when authentication is required
 */
export class AuthError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AuthError";
  }
}

/**
 * Error thrown for network failures
 */
export class NetworkError extends Error {
  readonly isOffline: boolean;

  constructor(isOffline: boolean) {
    super(isOffline ? "No internet connection" : "Network request failed");
    this.name = "NetworkError";
    this.isOffline = isOffline;
  }
}

/**
 * Error thrown for API errors with status and response details
 */
export class ApiError extends Error {
  readonly status: number;
  readonly statusText: string;
  readonly code: string;
  readonly errorType: string;

  constructor(status: number, statusText: string, code: string, errorType: string) {
    super(`API error: ${statusText}`);
    this.name = "ApiError";
    this.status = status;
    this.statusText = statusText;
    this.code = code;
    this.errorType = errorType;
  }

  static async fromResponse(response: Response): Promise<ApiError> {
    let code = "UNKNOWN";
    let errorType = "error";
    let message = response.statusText;

    try {
      const body: ApiErrorResponse = await response.json();
      code = body.code ?? "UNKNOWN";
      errorType = body.error ?? "error";
      message = body.message ?? response.statusText;
    } catch {
      // Use defaults if body parsing fails
    }

    const error = new ApiError(response.status, response.statusText, code, errorType);
    error.message = message;
    return error;
  }
}

interface RequestOptions extends RequestInit {
  timeout?: number;
}

/**
 * API Client class for making requests to the backend
 */
export class ApiClient {
  private baseUrl: string;
  private fetchFn: typeof fetch;

  /** API keys client */
  readonly apiKeys: ApiKeysClient;
  /** Authentication client */
  readonly auth: AuthClient;
  /** Favorites client */
  readonly favorites: FavoritesClient;
  /** Files client for content-addressed storage */
  readonly files: FilesClient;
  /** OpenTelemetry client */
  readonly otel: OtelClient;
  /** Organizations client */
  readonly organizations: OrganizationsClient;
  /** Projects client */
  readonly projects: ProjectsClient;

  constructor(baseUrl: string = API_BASE_URL, fetchFn: typeof fetch = fetch) {
    this.baseUrl = baseUrl;
    this.fetchFn = fetchFn.bind(globalThis);
    this.apiKeys = new ApiKeysClient(this);
    this.auth = new AuthClient(this);
    this.favorites = new FavoritesClient(this);
    this.files = new FilesClient(baseUrl);
    this.organizations = new OrganizationsClient(this);
    this.otel = new OtelClient(this);
    this.projects = new ProjectsClient(this);
  }

  /**
   * Build query string from params object
   */
  buildQueryString(params: Record<string, unknown>): string {
    const searchParams = new URLSearchParams();

    for (const [key, value] of Object.entries(params)) {
      if (value === undefined || value === null) continue;

      if (key === "filters") {
        searchParams.set(key, JSON.stringify(value));
      } else if (Array.isArray(value)) {
        value.forEach((v) => searchParams.append(key, String(v)));
      } else {
        searchParams.set(key, String(value));
      }
    }

    return searchParams.toString();
  }

  /**
   * Make a fetch request with default options
   */
  private async request<T>(endpoint: string, options?: RequestOptions): Promise<T> {
    const controller = new AbortController();
    const timeout = options?.timeout ?? DEFAULT_TIMEOUT;
    const timeoutId = setTimeout(() => controller.abort(), timeout);

    try {
      const response = await this.fetchFn(`${this.baseUrl}${endpoint}`, {
        ...options,
        credentials: "include",
        signal: controller.signal,
        headers: {
          "Content-Type": "application/json",
          ...options?.headers,
        },
      });

      if (response.status === 401) {
        window.dispatchEvent(new CustomEvent("auth:required"));
        throw new AuthError("Authentication required");
      }

      if (!response.ok) {
        throw await ApiError.fromResponse(response);
      }

      if (response.status === 204) {
        return undefined as T;
      }

      const contentLength = response.headers?.get("content-length");
      if (contentLength === "0") {
        return undefined as T;
      }

      const text = await response.text();
      if (!text) {
        return undefined as T;
      }
      return JSON.parse(text);
    } catch (error) {
      if (error instanceof AuthError || error instanceof ApiError) {
        throw error;
      }

      if (error instanceof TypeError && error.message.includes("fetch")) {
        throw new NetworkError(!navigator.onLine);
      }

      if (error instanceof DOMException && error.name === "AbortError") {
        throw new Error("Request timed out");
      }

      throw error;
    } finally {
      clearTimeout(timeoutId);
    }
  }

  /**
   * Make a GET request
   */
  async get<T>(endpoint: string, params?: Record<string, unknown>): Promise<T> {
    const url = params ? `${endpoint}?${this.buildQueryString(params)}` : endpoint;
    return this.request<T>(url);
  }

  /**
   * Make a POST request
   */
  async post<T>(endpoint: string, body?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    });
  }

  /**
   * Make a PUT request
   */
  async put<T>(endpoint: string, body?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "PUT",
      body: body ? JSON.stringify(body) : undefined,
    });
  }

  /**
   * Make a DELETE request
   */
  async delete<T>(endpoint: string, body?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "DELETE",
      body: body ? JSON.stringify(body) : undefined,
    });
  }

  /**
   * Check server health
   */
  async checkHealth(): Promise<{ status: string }> {
    const response = await this.fetchFn(`${this.baseUrl}/health`, {
      credentials: "include",
    });

    if (!response.ok) {
      throw new Error("Health check failed");
    }

    const text = await response.text();
    return text ? JSON.parse(text) : { status: "ok" };
  }

  /**
   * Connect to SSE endpoint with automatic reconnection
   */
  connectSSE(endpoint: string, handlers: SSEHandlers): () => void {
    let intentionallyClosed = false;
    let eventSource: EventSource | null = null;
    let reconnectAttempts = 0;
    let reconnectTimeout: ReturnType<typeof setTimeout> | null = null;
    let inactivityTimeout: ReturnType<typeof setTimeout> | null = null;

    const getBackoff = () => Math.min(1000 * Math.pow(2, reconnectAttempts), SSE_MAX_BACKOFF);

    const clearInactivityTimeout = () => {
      if (inactivityTimeout) {
        clearTimeout(inactivityTimeout);
        inactivityTimeout = null;
      }
    };

    const resetInactivityTimeout = () => {
      clearInactivityTimeout();
      if (intentionallyClosed) return;

      inactivityTimeout = setTimeout(() => {
        if (eventSource && eventSource.readyState !== EventSource.CLOSED) {
          // Null before close() prevents double-reconnect if onerror fires synchronously
          const es = eventSource;
          eventSource = null;
          es.close();
          scheduleReconnect();
        }
      }, SSE_INACTIVITY_TIMEOUT);
    };

    const scheduleReconnect = () => {
      if (intentionallyClosed) {
        handlers.onClose?.();
        return;
      }

      if (reconnectAttempts >= SSE_MAX_RECONNECT_ATTEMPTS) {
        handlers.onError?.(new Error("Max reconnection attempts reached"));
        handlers.onClose?.();
        return;
      }

      const backoff = getBackoff();
      reconnectAttempts++;
      reconnectTimeout = setTimeout(connect, backoff);
    };

    const connect = () => {
      if (intentionallyClosed) return;

      const url = `${this.baseUrl}${endpoint}`;
      eventSource = new EventSource(url, { withCredentials: true });

      eventSource.onopen = () => {
        reconnectAttempts = 0;
        resetInactivityTimeout();
        handlers.onOpen?.();
      };

      eventSource.onmessage = () => {
        resetInactivityTimeout();
      };

      eventSource.addEventListener("span", (event) => {
        resetInactivityTimeout();
        try {
          const data: SseSpanEvent = JSON.parse(event.data);
          handlers.onSpan(data);
        } catch (error) {
          handlers.onError?.(error instanceof Error ? error : new Error(String(error)));
        }
      });

      eventSource.addEventListener("terminate", () => {
        clearInactivityTimeout();
        eventSource?.close();
        eventSource = null;
        if (intentionallyClosed) return;
        if (reconnectTimeout) clearTimeout(reconnectTimeout);
        reconnectAttempts = 0;
        reconnectTimeout = setTimeout(connect, 1000);
      });

      eventSource.onerror = () => {
        clearInactivityTimeout();
        eventSource?.close();
        eventSource = null;
        scheduleReconnect();
      };
    };

    connect();

    return () => {
      intentionallyClosed = true;
      clearInactivityTimeout();
      if (reconnectTimeout) {
        clearTimeout(reconnectTimeout);
        reconnectTimeout = null;
      }
      eventSource?.close();
      eventSource = null;
    };
  }
}

// === Default client instance ===

export const apiClient = new ApiClient();
