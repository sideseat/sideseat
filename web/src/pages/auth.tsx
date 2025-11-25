import { useState, useEffect, useCallback } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useAuth } from "@/auth/context";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldLabel, FieldDescription, FieldError } from "@/components/ui/field";

export default function AuthPage() {
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const { login, authenticated, loading } = useAuth();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const redirectUri = searchParams.get("redirect_uri") || "/";
  const tokenFromUrl = searchParams.get("token");

  // If already authenticated, redirect
  useEffect(() => {
    if (!loading && authenticated) {
      navigate(decodeURIComponent(redirectUri), { replace: true });
    }
  }, [authenticated, loading, navigate, redirectUri]);

  const handleLogin = useCallback(
    async (tokenToUse: string) => {
      setIsLoading(true);
      setError(null);

      const success = await login(tokenToUse);

      if (success) {
        navigate(decodeURIComponent(redirectUri), { replace: true });
      } else {
        setError("Invalid token. Please check the token and try again.");
        setToken("");
      }

      setIsLoading(false);
    },
    [login, navigate, redirectUri],
  );

  // Auto-exchange token from URL (always try, even if already authenticated)
  useEffect(() => {
    if (tokenFromUrl && !loading) {
      handleLogin(tokenFromUrl);
    }
  }, [tokenFromUrl, loading, handleLogin]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!token.trim()) return;
    await handleLogin(token.trim());
  }

  // Show loading while checking initial auth state
  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  return (
    <div className="flex items-center justify-center min-h-screen bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="space-y-1">
          <CardTitle className="text-2xl font-bold">Authentication</CardTitle>
          <CardDescription>
            Enter the authentication token from your terminal to continue.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {tokenFromUrl && isLoading ? (
            <div className="text-center py-4 text-muted-foreground">Authenticating...</div>
          ) : (
            <form onSubmit={handleSubmit} className="space-y-4">
              <Field data-invalid={!!error}>
                <FieldLabel htmlFor="token">Token</FieldLabel>
                <Input
                  id="token"
                  type="text"
                  placeholder="Paste your token here"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                  disabled={isLoading}
                  autoComplete="off"
                  aria-invalid={!!error}
                  autoFocus
                />
                <FieldDescription>
                  Copy the token from the terminal URL when you started the server.
                </FieldDescription>
                {error && <FieldError>{error}</FieldError>}
              </Field>

              <Button type="submit" className="w-full" disabled={isLoading || !token.trim()}>
                {isLoading ? "Authenticating..." : "Authenticate"}
              </Button>
            </form>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
