import { AuthError, NetworkError, type ApiClient } from "./api-client";
import type { AuthStatusResponse } from "./types";

export type AuthStatusResult =
  | { status: "authenticated"; data: AuthStatusResponse }
  | { status: "unauthenticated" }
  | { status: "error"; error: Error };

export type TokenExchangeResult =
  | { success: true }
  | { success: false; reason: "invalid_token" | "network_error" | "server_error"; error?: Error };

export class AuthClient {
  private client: ApiClient;

  constructor(client: ApiClient) {
    this.client = client;
  }

  private basePath(): string {
    return "/auth";
  }

  async getStatus(): Promise<AuthStatusResult> {
    try {
      const data = await this.client.get<AuthStatusResponse>(`${this.basePath()}/status`);
      return { status: "authenticated", data };
    } catch (error) {
      if (error instanceof AuthError) {
        return { status: "unauthenticated" };
      }
      if (error instanceof NetworkError) {
        return { status: "error", error };
      }
      return { status: "error", error: error instanceof Error ? error : new Error(String(error)) };
    }
  }

  async exchangeToken(token: string): Promise<TokenExchangeResult> {
    try {
      await this.client.post(`${this.basePath()}/exchange`, { token });
      return { success: true };
    } catch (error) {
      if (error instanceof AuthError) {
        return { success: false, reason: "invalid_token" };
      }
      if (error instanceof NetworkError) {
        return { success: false, reason: "network_error", error };
      }
      return {
        success: false,
        reason: "server_error",
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  }

  async logout(): Promise<void> {
    try {
      await this.client.post(`${this.basePath()}/logout`);
    } catch {
      // Ignore errors on logout - best effort
    }
  }
}
