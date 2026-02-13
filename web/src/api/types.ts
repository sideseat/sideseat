/**
 * API Types
 *
 * Shared TypeScript interfaces for API communication.
 */

// === Auth ===

export interface AuthUser {
  id: string;
  email?: string;
  display_name?: string;
}

export interface AuthStatusResponse {
  authenticated: boolean;
  version: string;
  auth_method?: string;
  expires_at?: string;
  user?: AuthUser;
}

// === API Error Response ===

export interface ApiErrorResponse {
  error: string;
  code: string;
  message: string;
}
