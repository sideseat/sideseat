import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useMemo,
  useRef,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { apiClient } from "@/api/api-client";
import type { AuthStatusResult } from "@/api/auth-client";
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

interface AuthSnapshot {
  state: AuthState;
  error?: string;
}

function snapshotFrom(result: AuthStatusResult): AuthSnapshot {
  if (result.status === "authenticated") {
    return {
      state: {
        authenticated: result.data.authenticated,
        loading: false,
        version: result.data.version,
        authMethod: result.data.auth_method,
        expiresAt: result.data.expires_at ? new Date(result.data.expires_at) : undefined,
        user: result.data.user,
      },
    };
  }

  if (result.status === "unauthenticated") {
    return { state: { authenticated: false, loading: false } };
  }

  return {
    state: { authenticated: false, loading: false },
    error: "Failed to check authentication status",
  };
}

async function loadAuthSnapshot(): Promise<AuthSnapshot> {
  return snapshotFrom(await apiClient.auth.getStatus());
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>({
    authenticated: false,
    loading: true,
  });
  const authRequestVersion = useRef(0);

  const applySnapshot = useCallback((version: number, snapshot: AuthSnapshot) => {
    if (version !== authRequestVersion.current) {
      return;
    }

    if (snapshot.error) {
      toast.error(snapshot.error);
    }
    setState(snapshot.state);
  }, []);

  const invalidateAuthRequests = useCallback(() => {
    ++authRequestVersion.current;
  }, []);

  const checkAuth = useCallback(async () => {
    const version = ++authRequestVersion.current;
    applySnapshot(version, await loadAuthSnapshot());
  }, [applySnapshot]);

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
    invalidateAuthRequests();
    await apiClient.auth.logout();
    setState({ authenticated: false, loading: false });
  }, [invalidateAuthRequests]);

  useEffect(() => {
    const version = ++authRequestVersion.current;
    void loadAuthSnapshot().then((snapshot) => applySnapshot(version, snapshot));

    const handleFocus = () => void checkAuth();
    window.addEventListener("focus", handleFocus);
    return () => {
      invalidateAuthRequests();
      window.removeEventListener("focus", handleFocus);
    };
  }, [applySnapshot, checkAuth, invalidateAuthRequests]);

  useEffect(() => {
    const handleAuthRequired = () => {
      invalidateAuthRequests();
      setState({ authenticated: false, loading: false });
    };

    window.addEventListener("auth:required", handleAuthRequired);
    return () => window.removeEventListener("auth:required", handleAuthRequired);
  }, [invalidateAuthRequests]);

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
