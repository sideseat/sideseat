/**
 * Auth Client
 *
 * Handles authentication-related API calls.
 * These methods don't throw on errors - they return null/false instead.
 */

export interface AuthStatusResponse {
  authenticated: boolean;
  auth_method?: string;
  expires_at?: string;
}

export class AuthClient {
  private baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  /**
   * Check authentication status
   * Returns null on error instead of throwing
   */
  async getStatus(): Promise<AuthStatusResponse | null> {
    try {
      const response = await fetch(`${this.baseUrl}/auth/status`, {
        credentials: "include",
      });

      if (response.ok) {
        return response.json();
      }
      return null;
    } catch {
      return null;
    }
  }

  /**
   * Exchange bootstrap token for session
   * Returns success boolean instead of throwing
   */
  async exchangeToken(token: string): Promise<boolean> {
    try {
      const response = await fetch(`${this.baseUrl}/auth/exchange`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({ token }),
      });

      return response.ok;
    } catch {
      return false;
    }
  }

  /**
   * Logout and clear session
   */
  async logout(): Promise<void> {
    try {
      await fetch(`${this.baseUrl}/auth/logout`, {
        method: "POST",
        credentials: "include",
      });
    } catch {
      // Ignore errors on logout
    }
  }
}
