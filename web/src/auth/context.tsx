import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useMemo,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { apiClient } from "@/api/api-client";
import type { AuthUser } from "@/api/types";

interface AuthState {
  authenticated: boolean;
  loading: boolean;
  version?: string;
  authMethod?: string;
  expiresAt?: Date;
  user?: AuthUser;
}

interface AuthContextValue extends AuthState {
  login: (token: string) => Promise<boolean>;
  logout: () => Promise<void>;
  checkAuth: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>({
    authenticated: false,
    loading: true,
  });

  const checkAuth = useCallback(async () => {
    const result = await apiClient.auth.getStatus();

    if (result.status === "authenticated") {
      setState({
        authenticated: result.data.authenticated,
        loading: false,
        version: result.data.version,
        authMethod: result.data.auth_method,
        expiresAt: result.data.expires_at ? new Date(result.data.expires_at) : undefined,
        user: result.data.user,
      });
    } else if (result.status === "unauthenticated") {
      setState({ authenticated: false, loading: false });
    } else {
      toast.error("Failed to check authentication status");
      setState({ authenticated: false, loading: false });
    }
  }, []);

  const login = useCallback(
    async (token: string): Promise<boolean> => {
      const result = await apiClient.auth.exchangeToken(token);

      if (result.success) {
        await checkAuth();
        return true;
      }

      if (result.reason === "network_error") {
        toast.error("Network error. Please check your connection.");
      } else if (result.reason === "server_error") {
        toast.error("Server error. Please try again later.");
      }
      return false;
    },
    [checkAuth],
  );

  const logout = useCallback(async () => {
    await apiClient.auth.logout();
    setState({ authenticated: false, loading: false });
  }, []);

  useEffect(() => {
    checkAuth();

    const handleFocus = () => checkAuth();
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [checkAuth]);

  useEffect(() => {
    const handleAuthRequired = () => {
      setState((prev) => ({ ...prev, authenticated: false }));
    };

    window.addEventListener("auth:required", handleAuthRequired);
    return () => window.removeEventListener("auth:required", handleAuthRequired);
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({ ...state, login, logout, checkAuth }),
    [state, login, logout, checkAuth],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

// eslint-disable-next-line react-refresh/only-export-components
export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
}
