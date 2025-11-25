import { createContext, useContext, useEffect, useState, useCallback, type ReactNode } from "react";
import { apiClient } from "@/api/client";

interface AuthState {
  authenticated: boolean;
  loading: boolean;
  authMethod?: string;
  expiresAt?: Date;
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
    const data = await apiClient.auth.getStatus();

    if (data) {
      setState({
        authenticated: data.authenticated,
        loading: false,
        authMethod: data.auth_method,
        expiresAt: data.expires_at ? new Date(data.expires_at) : undefined,
      });
    } else {
      setState({ authenticated: false, loading: false });
    }
  }, []);

  const login = useCallback(
    async (token: string): Promise<boolean> => {
      const success = await apiClient.auth.exchangeToken(token);

      if (success) {
        await checkAuth();
        return true;
      }
      return false;
    },
    [checkAuth],
  );

  const logout = useCallback(async () => {
    await apiClient.auth.logout();
    setState({ authenticated: false, loading: false });
  }, []);

  // Check auth status on mount and when focus returns to window
  useEffect(() => {
    checkAuth();

    const handleFocus = () => checkAuth();
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [checkAuth]);

  // Listen for auth:required events from API client
  useEffect(() => {
    const handleAuthRequired = () => {
      setState((prev) => ({ ...prev, authenticated: false }));
    };

    window.addEventListener("auth:required", handleAuthRequired);
    return () => window.removeEventListener("auth:required", handleAuthRequired);
  }, []);

  return (
    <AuthContext.Provider value={{ ...state, login, logout, checkAuth }}>
      {children}
    </AuthContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
}
