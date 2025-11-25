/**
 * API Client
 *
 * Centralized API client for all backend communication.
 * All API calls should go through this module.
 */

import { AuthClient } from "./auth-client";

export const API_BASE_URL = import.meta.env.PROD ? "/api/v1" : "http://localhost:5001/api/v1";

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
 * Error thrown for API errors with status and response details
 */
export class ApiError extends Error {
  status: number;
  statusText: string;

  constructor(status: number, statusText: string, message?: string) {
    super(message || `API error: ${statusText}`);
    this.name = "ApiError";
    this.status = status;
    this.statusText = statusText;
  }
}

/**
 * API Client class for making requests to the backend
 */
export class ApiClient {
  private baseUrl: string;

  /** Authentication client */
  readonly auth: AuthClient;

  constructor(baseUrl: string = API_BASE_URL) {
    this.baseUrl = baseUrl;
    this.auth = new AuthClient(baseUrl);
  }

  /**
   * Make a fetch request with default options
   */
  private async request<T>(endpoint: string, options?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${endpoint}`, {
      ...options,
      credentials: "include",
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
      throw new ApiError(response.status, response.statusText);
    }

    return response.json();
  }

  /**
   * Make a GET request
   */
  async get<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint);
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
  async delete<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, {
      method: "DELETE",
    });
  }
}

// === Default client instance ===

export const apiClient = new ApiClient();
