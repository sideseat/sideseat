import { type ReactNode, useEffect } from "react";
import { useNavigate, useLocation, useSearchParams } from "react-router";
import { useAuth } from "./context";

interface AuthGuardProps {
  children: ReactNode;
}

export function AuthGuard({ children }: AuthGuardProps) {
  const { authenticated, loading } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams] = useSearchParams();

  useEffect(() => {
    if (!loading && !authenticated) {
      // Check if there's a token in the URL - pass it to auth page
      const token = searchParams.get("token");

      // Build redirect_uri from pathname + query string, excluding the token param
      // (token is passed separately and shouldn't persist in the URL after login)
      const cleanParams = new URLSearchParams(searchParams);
      cleanParams.delete("token");
      const queryString = cleanParams.toString();
      const redirectUri = encodeURIComponent(
        location.pathname + (queryString ? `?${queryString}` : ""),
      );

      const authUrl = token
        ? `/auth?token=${token}&redirect_uri=${redirectUri}`
        : `/auth?redirect_uri=${redirectUri}`;
      navigate(authUrl, { replace: true });
    }
  }, [authenticated, loading, navigate, location, searchParams]);

  // Show loading state while checking authentication
  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  // If not authenticated, don't render children (redirect will happen)
  if (!authenticated) {
    return null;
  }

  return <>{children}</>;
}
